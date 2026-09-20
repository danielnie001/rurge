# 阶段 2 / M2b「VMess / AnyTLS 与按指纹复用出站」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `[Proxy]` 里的 `vmess`（AEAD 握手，可叠 `tls` / `ws`）与 `anytls`（会话复用、padding 方案）策略真正可用（TCP），并兑现"重载时按指纹复用出站"：与某条策略无关的重载不打断它的连接池，被复用的出站经 `ResolverCell` 看到新一代解析器。

**Architecture:** 协议代码全部在 `rurge-proto`：`vmess`（KDF / 请求头密封 / 分块流，请求头经 M2a 的 `LazyHead` 与首段负载合并）与 `anytls`（帧、padding、"一条会话一个任务"的会话层、空闲池）。两者都叠在 M2a 的传输阶梯 `transport::Stack` 上。`rurge-policy` 的注册表给每个出站条目记指纹，构建新一代时名字相同且指纹相等就沿用上一代的同一个 `Arc`；`rurge-engine` 的 `ResolverCell` 与注册表在同一个发布点（`publish_generation`）切换。

**Tech Stack:** Rust 1.89 / edition 2024；`ring`（AES-128-GCM、ChaCha20-Poly1305，已在 `Cargo.lock`）、`aes`（AuthID 的单块 AES，已在 `Cargo.lock`）、`md-5`、`sha3`（SHAKE128）、`crc32fast`、`tokio-util`（`PollSender`）；互操作用 sing-box 1.14.1 与 xray v26.3.27（只测 vmess），都只在 CI 上安装。

**Spec:** `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`（第 1.4 节的 M2b 行、第 4、6.3、6.4、7、8、10 – 13 节、第 15 节 V3 – V6、第 17 节）；总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准，并由最后一个任务写回设计文档第 17 节之后的新一节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-policy` 不依赖任何协议实现；`rurge-engine` / `rurge-api` / `rurge-policy` 不依赖 `rurge-platform`；平台代码只在 `rurge-platform`（AR-02）。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间）。需要推进时钟的用例：**先把真实 socket 上的往返做完，再 `tokio::time::pause()` + `advance`**——从一开始就暂停的时钟会在等真实 I/O 时自动快进，把所有超时都提前触发（计划期已踩过）。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 sing-box、不装 xray、不 `rustup target add`、不 `cargo install`）。`cargo` 为本计划点名的新依赖拉取 crate 源码是允许的。互操作由首次推送后的 CI 证明。
- 互操作夹具渲染出的 sing-box / xray 配置里绝不出现 `set_system_proxy` / `tun` / `auto_route`；只监听 `127.0.0.1`；所有目标是回环 IP 字面量。
- **凭据及其派生物永不外泄**：口令、UUID、`cmdKey`、SHA-256 哈希、每连接的 body key / IV、自定义头的值、`ws-path`，以及对端发来的原始文本——永不出现在错误文本、诊断、日志、API 输出与 `Debug` 输出里。对端文本一律先过 `untrusted_text`（去控制字符、有界）。持有凭据派生物的出站与流对象不实现 `Debug`。
- 长度先校验后分配：vmess 应答头 ≤ 4 + 255 字节、分块 ≥ 16 字节（tag）且 ≤ 65535（格式所限）；anytls 帧 ≤ 65535（格式所限），padding 方案见 P5。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && cargo test --workspace --no-fail-fast
  ```

  测试二进制异常退出而没有失败用例时，重跑一次并保留两次的日志。开工前看一眼磁盘：`target/` 在 M2a 期间曾涨到 129 GB 把 D: 写满。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。

## 计划期决定

写计划时对照参考实现与本仓库源码核对后定下的事；与设计文档文字不同的，最后一个任务写回设计文档。**计划里 Task 2 – 5 的协议代码与用例不是凭空写的**：它们先在一个临时工程里按生产调用方的方式（转发循环：`read` → `write_all`、不 flush、两个方向经 `tokio::io::split` 在同一个任务里交错轮询）编译、对参考向量跑通（33 个用例，连续多轮无抖动，clippy / rustfmt 干净），再搬进计划；搬运时只改了 `use` 路径，假服务端多了 TLS / WebSocket 接入。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V3：VMess AEAD 的逐字节格式与向量 | 对照 v2fly/v2ray-core（master，2026-09-20）`proxy/vmess/aead/{kdf,consts,authid,encrypt}.go`、`proxy/vmess/encoding/{client,auth}.go`、`common/crypto/auth.go`、`common/protocol/{address,headers}.go` 核对。**向量出处**：把上述函数原样搬进一个只用标准库的 Go 程序（本机已装 Go 1.24.3；时钟与 `crypto/rand` 换成固定输入），另写一份**不照搬**、按协议描述独立推导的 Python 程序——两者在 15 个向量上完全一致；ChaCha20-Poly1305 的分块向量只来自 Python 那份（Go 标准库没有该算法）。要点：KDF 是逐层嵌套的 HMAC（最内层 HMAC-SHA256 以 `"VMess AEAD KDF"` 为密钥，路径上每个元素给外面再包一层 HMAC，**最外层的密钥是路径的最后一个元素**）；`AuthID = AES-128(KDF16(cmdKey,"AES Auth ID Encryption"))( 时间 i64 BE ‖ 随机 4 ‖ CRC32-IEEE 前 12 字节 )`；线上 `AuthID(16) ‖ 密封长度(18) ‖ nonce(8) ‖ 密封头`，两次密封的 AAD 都是 AuthID；头明文 `版本 1 ‖ bodyIV(16) ‖ bodyKey(16) ‖ V ‖ 选项 ‖ (填充长度<<4 \| 加密方式) ‖ 0 ‖ 命令 ‖ 端口 ‖ 地址类型(1 / 2 / 3) ‖ 地址 ‖ 填充 ‖ FNV1a-32`——**端口在地址之前**，地址类型与 SOCKS 不同；应答的 key / IV 是请求的 SHA-256 前 16 字节；分块 nonce = `计数(u16 BE，回绕) ‖ iv[2..12]`、无 AAD；长度掩码取 SHAKE128(iv) 的下两个字节 |
| P2 | 设计第 3 节的新依赖表 | **AEAD 改用 `ring`**（rustls 已经把它带进 `Cargo.lock`，零新增条目，汇编实现）而不是 `aes-gcm` + `chacha20poly1305`；AuthID 的单块 AES 用 `aes = "0.8"`（`Cargo.lock` 已有 0.8.4）；`crc32fast`（已有 1.5.1）；新增的只有 `md-5 = "0.10"`、`sha3 = "0.10"` 及其依赖 `keccak`——**`Cargo.lock` 预计新增 3 个条目**（`md-5` `sha3` `keccak`），多出来就先停下报告。不需要 `hmac`：`hmac` crate 表达不了"HMAC 套 HMAC"，嵌套 HMAC 手写（20 行，向量钉住）。`tokio-util`（工作区已有）加进 `rurge-proto` 取 `PollSender` |
| P3 | VMess 分块的大小与流的边界 | 写：负载 ≤ 16368 字节（密封后 ≤ 2^14：协议文档的上限，sing-vmess 用 16384 字节的缓冲读一个分块）；读：接受 16 ..= 65535 的任何长度。传输层在分块边界上的 EOF 视为流结束（对端没发空块）；在分块中间的 EOF 是 `UnexpectedEof`。**应答头之前就 EOF** 是 `vmess: the server closed the connection without answering`（UUID 错、时钟偏差超过约 120 秒都表现为这个，服务端从不说明原因）。选项固定 `0x05`（ChunkStream + ChunkMasking） |
| P4 | 设计 4.2 留给 M2b 的那句 | `vmess` 的 `username`、`anytls` 的 `password` 都只接受命名写法（手册两页都只有命名写法）；位置值不读，按多余的位置参数报 `W0001`。`encrypt-method` 用现成的 `ParamReader::choice`（报错时回显取值——它不是凭据） |
| P5 | V4：AnyTLS | 对照 anytls-go（main，2026-09-20）`docs/protocol.md`、`proxy/session/{session,client,frame,stream}.go`、`proxy/padding/padding.go`、`cmd/client/myclient.go`、`cmd/server/inbound_tcp.go`。要点：① **鉴权必须一次写出**（参考服务端用一次 `Read` 取完 `hash ‖ 长度 ‖ padding0`，拆成两条 TLS 记录就会被当成鉴权失败）；② 包计数按"逻辑上的一次写"算，鉴权是第 0 个，**第 1 个必须是 `cmdSettings ‖ cmdSYN ‖ cmdPSH(目标地址)` 合成的一次写**，第 2 个才是用户的首段负载——默认方案就是围着这个划分设计的，所以 **anytls 不用 `LazyHead`**；③ 切分算法见 `padding::shape`（与 `Session.writeConn` 逐分支对应）；④ `padding-md5` 是方案原文的 MD5（小写十六进制），默认方案的是 `75cff2ad89aadf5e257059ee571ebe11`；⑤ `min-max` 的上界不含。**`cmdUpdatePaddingScheme` 的有界校验**：原文 ≤ 8192 字节且是 UTF-8、`stop` ≤ 256、每个包 ≤ 64 项、每项是 `c` 或 `a-b`（1 ≤ 值 ≤ 16384，一条 TLS 记录的上限）；不是 `stop` 也不是包序号的键忽略；任何一条不满足 → 保留旧方案 + 一条 WARN |
| P6 | 设计 6.3 的"双向 FIN" | **AnyTLS 没有半关闭**：`cmdFIN` 结束整条流，收到对端的 FIN 不需要回 FIN（协议文档 2025-09 的澄清）。`poll_shutdown` = 发 `cmdFIN` 并让本端的读立刻返回 EOF——sing-box 的 `CopyConn` 对没有 `CloseWrite` 的连接就是这么做的。登记进清单 |
| P7 | AnyTLS 会话层的结构 | 一条会话一个任务，独占 TLS 流（`tokio::io::split`，读循环与写循环在同一个 `select!` 里）；流句柄经两条有界队列（各 8）与任务通信，写用 `tokio_util::sync::PollSender`。任务每批写完自己 `flush`，所以"入队即在途"，不依赖转发循环从不调用的 `flush`。多个任务共享一个 `AsyncWrite` 会互相顶掉唤醒者，所以不这么做。空闲时任务照常读：心跳有人答、对端关连接能被发现。被拒的流（带文本的 `cmdSYNACK`）不连累会话。设计第 10 节的"panic 由任务边界隔离并记 ERROR"：会话任务与回收任务里没有可达的 panic 点（不索引、不 `unwrap` I/O 结果；`expect` 只在锁中毒时触发，而那已是另一个 panic 的后果），不另加监督任务，tokio 的任务边界照常隔离。**不实现参考客户端的 3 秒 SYNACK 看门狗**（复用到一条"半死"的空闲会话时由转发阶段的空闲超时兜底）：登记进清单与「延后事项」 |
| P8 | AnyTLS 的后台任务 | 回收任务在第一次 `connect_tcp` 时才启动（干构建不留任何任务）；流句柄只弱引用池（出站退役后，在途的流结束时会话直接关闭，不回池） |
| P9 | V5：xray | 固定 **v26.3.27**（GitHub 上最新的非预发布版，2026-03-27）。包名与 SHA256（取自 GitHub Releases API 的 `digest` 字段）：`Xray-linux-64.zip` `23cd9af937744d97776ee35ecad4972cf4b2109d1e0fe6be9930467608f7c8ae`；`Xray-windows-64.zip` `d004c39288ce9ada487c6f398c7c545f7d749e44bdfdd59dbc9f865afba4e1ad`；`Xray-macos-arm64-v8a.zip` `2e93a67e8aa1936ecefb307e120830fcbd4c643ab9b1c46a2d0838d5f8409eaf`。最小配置：一个 `127.0.0.1` 上的 `vmess` 入站（可带 `ws`）+ 一个 `freedom` 出站，`xray run -c <文件>` |
| P10 | V6：`environment()` | `EngineFactory` 按值捕获、又会随配置代际变化的只有 `[General] ipv6`（进每个 `DirectConnector` 的 `SocketOpts.v6_first`）→ `environment()` 返回 `"ipv6=<bool>"`。根证书库与 socket hook 是进程级常量；解析器经 `ResolverCell` 动态读取；Keystore 单独算。指纹里的 Keystore 分量直接比较被引用条目的 `(类型, base64, 口令)`，不算摘要：`rurge-policy` 没有哈希依赖，而 `Config` 本来就整代持有同样的材料；`Fingerprint` 不实现 `Debug` |
| P11 | `previous` 从哪来 | `Runtime::build` 里取 `opts.shared.cell.load()`（首次构建时为空）——bin 与测试的调用方一行都不用改 |
| P12 | 提交的原子性 | `ProtoSpec::{Vmess, AnyTls}` 两个变体、`to_spec` 的两个分支、`EngineFactory::build` 的两个分支必须在**同一个提交**里落地（`EngineFactory::build` 对 `ProtoSpec` 是穷尽匹配，注册表按"有没有 spec"决定走哪条路）。所以 Task 1 只交付公开的读取函数与 spec 类型，出站的构造函数直接吃协议自己的 spec（`VmessSpec` / `AnyTlsSpec`，与 M2a 的 `TrojanOutbound::new` 一致），装配在 Task 9 |
| P13 | 设计 4.3 的落点 | `to_spec` 给 `SpecOutcome` 加 `legacy_vmess: bool`，加载器像 `inert` 那样**每次加载只报一条** `W0007`（50 个节点的订阅不该刷 50 条）。运行期文本：成功加载的配置里，没有 spec 的 `vmess` 策略只可能是没写 `vmess-aead=true` 的（有错误的配置根本加载不了），所以注册表对这种条目的说明文本取 `vmess (legacy handshake)` |
| P14 | 承接：凭据的包装类型 | `rurge_config::spec::Secret<T>`（`Debug` 恒为 `Secret(***)`）包住 http / socks5 的 `username` `password`、trojan / anytls 的 `password`、vmess 的 `uuid`。`ws-path`、`ws-headers`、`headers` 不包（不是凭据类型的字段；生产代码从不格式化 spec）——进「延后事项」 |
| P15 | 承接：IDN 的代理服务器名 | 核对结论：解析器的线上编码只接受 ASCII（`hickory_proto::rr::Name::from_ascii`），应答校验又按原名比较——以 Unicode 写的服务器名在**任何**协议上都不可用，TLS 层的构建错误（`E0022`，加载期）反而是最早、最清楚的信号。M2b 不改行为：清单 4.2 登记"代理服务器主机名须写成 ASCII（punycode）"，全面的 IDN 支持（规则、`[Host]`、解析器、TLS）进「延后事项」 |
| P16 | 承接：转发循环不 flush | 这是真缺陷而不只是风格问题：`tokio-rustls` 的 `poll_write` 在 socket 写不动时会在"明文已收下、密文还留在自己缓冲里"的状态下返回成功，之后若没有下一次写，这段尾巴就一直留着（大请求体的最后一段）。修法：`copy_half` 在 `write_all` 之后 `flush`，同样与 `stop` 竞争。**回归用例的 RED 已在旧循环上真的跑过**：一个"收下即成功、flush 才发出"的写端，旧循环下尾巴 500 ms 内到不了，加 `flush` 后立刻到。同一个任务里顺带修一个相邻的缺口：**一个方向以错误结束时，另一个方向不会跟着结束**（`tokio::join!` 等两边），客户端要等到自己超时或空闲超时才知道隧道坏了——vmess 的"没应答就关"与 anytls 的被拒 / 会话死亡都以读错误的形式出现，没有这一条它们只会表现为卡住 |
| P17 | `vmess` 没开 `tls` 却写了 TLS 参数 | 仍是 `W0028`，但不复用 `refuse_tls` 的文本（"does not apply to `vmess` policies"会误导）：`` `<key>` has no effect without `tls=true`; ignored `` |

## 承接事项

M2a 计划「延后事项」表里标给 M2b 的四条，落点如下：

| 事项 | 落点 |
| ---- | ---- |
| 转发循环在 `write_all` 之后不 flush | Task 6（P16） |
| 以 IDN 写的代理服务器主机名在 TLS 层是构建错误 | 只登记：Task 12 写进清单与「延后事项」（P15） |
| 给 spec 的凭据字段包一层不打印的类型 | Task 1（P14） |
| "需要 server 与 port"的前置检查收进 proto | Task 9：`rurge_proto::build::server_of` |

设计第 9 节第 5 条（`publish_registry` 的 `assert!` 保留、扩成 `publish_generation`）在 Task 7；第 7.3 节最后一条（链式会话进行中重载，M2b 复核）在 Task 9。

## File Structure

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/secret.rs`（新） | `Secret<T>`：可比较、可克隆、`Debug` 不显示 | 1 |
| `crates/rurge-config/src/spec/vmess.rs`（新） | `VmessCipher` `VmessSpec` `read_vmess`（含 `vmess-aead` 的读取） | 1 |
| `crates/rurge-config/src/spec/anytls.rs`（新） | `AnyTlsSpec` `read_anytls` | 1 |
| `crates/rurge-config/src/spec/{mod,tls,http,socks5,trojan}.rs` | 导出、`idle_tls`、凭据字段改型、`ProtoSpec::tls()`；Task 9 加两个变体与 `to_spec` 分支、`legacy_vmess` | 1 / 9 |
| `crates/rurge-config/src/config.rs` | 每次加载一条的旧握手 `W0007` | 9 |
| `crates/rurge-proto/src/vmess/{kdf,header,chunk,vectors}.rs`（新） | KDF、请求头 / 应答头、分块编解码、参考向量 | 2 |
| `crates/rurge-proto/src/addr.rs` | `vmess_addr`（端口在前、类型 1 / 2 / 3） | 2 |
| `crates/rurge-proto/src/vmess/{mod,stream}.rs`（新） | `VmessOutbound`、`VmessStream` | 3 |
| `crates/rurge-proto/src/testing/vmess.rs`（新） | `FakeVmess` | 3 |
| `crates/rurge-proto/src/anytls/{frame,padding}.rs`（新） | 帧、padding 方案与切分 | 4 |
| `crates/rurge-proto/src/task.rs`（新） | `AbortOnDrop`（从 `testing` 挪到生产代码） | 5 |
| `crates/rurge-proto/src/anytls/{mod,session,pool}.rs`（新） | `AnyTlsOutbound`、会话任务与流句柄、空闲池与回收 | 5 |
| `crates/rurge-proto/src/testing/anytls.rs`（新） | `FakeAnyTls` | 5 |
| `crates/rurge-engine/src/shared.rs`、`engine.rs`、`reload.rs`、`runtime.rs` | `ResolverCell`、`publish_generation`、工厂拿 cell | 7 |
| `crates/rurge-policy/src/{factory,registry,testing}.rs` | `environment()`、`Fingerprint`、`build(.., previous)` | 8 |
| `crates/rurge-engine/src/outbounds.rs`、`crates/rurge-proto/src/build.rs` | 两个新分支、`server_of`、`environment()` 的实现 | 8 / 9 |
| `crates/rurge-engine/tests/outbounds.rs` | 两种协议的端到端、链、7.3 的四条复用用例 | 9 |
| `crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs` | 能力表翻转与 CLI 用例 | 10 |
| `crates/rurge-engine/src/relay.rs` | `write_all` 之后 `flush`；一个方向出错就结束另一个方向 | 6 |
| `tests/interop/{src/lib.rs,src/xray.rs,tests/sing_box.rs,tests/xray.rs,README.md}`、`.github/workflows/ci.yml` | vmess / anytls 入站、xray 夹具、CI 安装并校验 xray | 11 |
| 文档（清单、API 参考、README、CLAUDE.md、手工验收、设计第 18 节、本计划收尾两表） | | 12 |

---

### Task 1: 配置层——`Secret<T>`、`VmessSpec`、`AnyTlsSpec` 与两个公开的读取函数

**Files:**
- Create: `crates/rurge-config/src/spec/secret.rs`
- Create: `crates/rurge-config/src/spec/vmess.rs`
- Create: `crates/rurge-config/src/spec/anytls.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（导出、`read_credentials`、socks5 的长度检查、`ProtoSpec::tls()`）
- Modify: `crates/rurge-config/src/spec/tls.rs`（`idle_tls`）
- Modify: `crates/rurge-config/src/spec/{http,socks5,trojan}.rs`（凭据字段改型）
- Modify: `crates/rurge-proto/src/{http,socks5,trojan}.rs`（跟着改型）
- Modify: `crates/rurge-engine/src/outbounds.rs`（`tls_of` 改用 `ProtoSpec::tls()`）
- Test: 各文件自己的 `mod tests`；`crates/rurge-config/tests/policy_spec.rs` 跟着改型

**Interfaces:**
- Consumes: `ParamReader`（`str` `bool` `choice` `error` `warn` `has` `touch` `policy`）、`read_tls(r, keystore) -> TlsOpts`、`read_ws(r) -> Option<WsOpts>`、`codes::{E_INVALID_POLICY_PARAM, W_PARAM_NOT_APPLICABLE}`。
- Produces（后面的任务按这些名字用）：
  - `rurge_config::spec::Secret<T>`：`Secret::new(T)`、`expose(&self) -> &T`、`From<&str>` / `From<String>` for `Secret<String>`；派生 `Clone` `Default` `PartialEq` `Eq`，`Debug` 恒为 `Secret(***)`。
  - `rurge_config::spec::{VmessCipher, VmessSpec}`；`rurge_config::spec::vmess::{read_vmess, VmessRead}`：`read_vmess(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> VmessRead { spec: VmessSpec, aead: bool }`。
  - `rurge_config::spec::AnyTlsSpec`；`rurge_config::spec::anytls::read_anytls(r, keystore) -> AnyTlsSpec`。
  - `ProtoSpec::tls(&self) -> Option<&TlsOpts>`。
  - **本任务不加 `ProtoSpec` 的变体、不动 `to_spec` 的分发**（P12：那要和引擎工厂的分支同一个提交，Task 9）。

- [ ] **Step 1: 写 `Secret<T>` 与它的用例**

`crates/rurge-config/src/spec/secret.rs`：

```rust
//! Credentials inside a spec: compared and cloned like any other field, never
//! shown by `Debug`.

use std::fmt;

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(value: T) -> Secret<T> {
        Secret(value)
    }

    /// The value itself. Every caller is a place a credential can leave from.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl From<&str> for Secret<String> {
    fn from(value: &str) -> Secret<String> {
        Secret(value.to_string())
    }
}

impl From<String> for Secret<String> {
    fn from(value: String) -> Secret<String> {
        Secret(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_compares_and_clones_but_never_prints() {
        let a: Secret<String> = "hunter2".into();
        assert_eq!(a, Secret::new("hunter2".to_string()));
        assert_ne!(a, Secret::from("hunter3"));
        assert_eq!(a.clone().expose(), "hunter2");
        assert_eq!(format!("{a:?}"), "Secret(***)");
        assert_eq!(format!("{:?}", Some(Secret::new([7u8; 16]))), "Some(Secret(***))");
        assert_eq!(format!("{:#?}", Secret::new(7u8)), "Secret(***)");
    }
}
```

`spec/mod.rs`：在模块列表里按字母序加 `pub mod anytls;`、`pub mod secret;`、`pub mod vmess;`，导出里加

```rust
pub use anytls::AnyTlsSpec;
pub use secret::Secret;
pub use vmess::{VmessCipher, VmessSpec};
```

（`anytls` / `vmess` 两个文件在 Step 4、5 才有内容；这一步先建空文件让它编得过，或把这三行留到那两步再加——任选其一，提交时三个模块都在。）

- [ ] **Step 2: 跑用例确认 `Secret` 成立**

Run: `cargo test -p rurge-config secret`
Expected: `a_secret_compares_and_clones_but_never_prints ... ok`

- [ ] **Step 3: 把现有三种 spec 的凭据字段改型，然后跟着编译器改使用处**

字段（`crates/rurge-config/src/spec/`）：

```rust
// http.rs — HttpSpec
    pub username: Option<Secret<String>>,
    pub password: Option<Secret<String>>,
// socks5.rs — Socks5Spec
    pub username: Option<Secret<String>>,
    pub password: Option<Secret<String>>,
// trojan.rs — TrojanSpec
    pub password: Secret<String>,
```

三个文件各加 `use super::secret::Secret;`。`trojan.rs` 的 `read_trojan`：

```rust
    let password = r.str("password").unwrap_or_default();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let ws = read_ws(r);
    TrojanSpec {
        tls,
        password: password.into(),
        ws,
    }
```

`spec/mod.rs`：

```rust
/// Named `username=` / `password=` win over the positional pair.
fn read_credentials(
    r: &mut ParamReader<'_>,
) -> (Option<Secret<String>>, Option<Secret<String>>) {
    let positional = (r.positional(0), r.positional(1));
    let username = r.str("username").or(positional.0).map(Secret::from);
    let password = r.str("password").or(positional.1).map(Secret::from);
    (username, password)
}
```

同一个文件里 socks5 的长度检查把 `.is_some_and(|v| v.len() > socks5::MAX_CREDENTIAL)` 改成 `.is_some_and(|v| v.expose().len() > socks5::MAX_CREDENTIAL)`。

`crates/rurge-proto/src/http.rs`（`from_spec` 里）：

```rust
        let authorization = http.username.as_ref().map(|user| {
            let password = http.password.as_ref().map_or("", |p| p.expose().as_str());
            let pair = format!("{}:{password}", user.expose());
            format!("Basic {}", STANDARD.encode(pair))
        });
```

`crates/rurge-proto/src/socks5.rs`（`from_spec` 里）：

```rust
        if socks
            .username
            .as_ref()
            .is_some_and(|u| u.expose().len() > MAX_CREDENTIAL)
            || socks
                .password
                .as_ref()
                .is_some_and(|p| p.expose().len() > MAX_CREDENTIAL)
        {
```

```rust
        let credentials = socks.username.as_ref().map(|user| {
            let password = socks.password.as_ref().map(|p| p.expose().clone());
            (user.expose().clone(), password.unwrap_or_default())
        });
```

`crates/rurge-proto/src/trojan.rs`：`spec.password.is_empty()` → `spec.password.expose().is_empty()`；`wire_hash(&spec.password)` → `wire_hash(spec.password.expose())`。

其余使用处（各文件的 `mod tests`、`crates/rurge-config/tests/policy_spec.rs`）由编译器逐个指出，每一处都是下面三种改法之一，不要发明第四种：

| 原来 | 改成 |
| ---- | ---- |
| 构造：`password: "pw".to_string()` / `Some("pw".to_string())` / `String::new()` | `password: "pw".into()` / `Some("pw".into())` / `Secret::default()` |
| 断言：`assert_eq!(spec.password, "pwd")` / `spec.username.as_deref() == Some("u")` | `assert_eq!(spec.password.expose(), "pwd")` / `spec.username.as_ref().map(|u| u.expose().as_str()) == Some("u")` |
| 赋值：`socks.username = Some(long.clone())` | `socks.username = Some(long.clone().into())` |

Run: `cargo test -p rurge-config && cargo test -p rurge-proto`
Expected: 全部通过（数量与改型前相同，多一条 Step 1 的用例）。

- [ ] **Step 4: `vmess` 的 spec 与读取函数（先写用例）**

`crates/rurge-config/src/spec/tls.rs`：把 `refuse_tls` 的遍历抽出来，加 `idle_tls`：

```rust
fn warn_tls_keys(r: &mut ParamReader<'_>, text: impl Fn(&str) -> String) {
    let mut present: Vec<&str> = TLS_KEYS.iter().copied().filter(|k| r.has(k)).collect();
    present.sort_unstable();
    for key in present {
        r.touch(key);
        r.warn(codes::W_PARAM_NOT_APPLICABLE, text(key));
    }
}

/// For protocols that do not run over TLS: every TLS parameter present is `W0028`.
pub(crate) fn refuse_tls(r: &mut ParamReader<'_>) {
    let kind = r.policy().kind.keyword();
    warn_tls_keys(r, |key| {
        format!("`{key}` does not apply to `{kind}` policies; ignored")
    });
}

/// For `vmess` without `tls=true`: the TLS parameters are there but idle (`W0028`).
pub(crate) fn idle_tls(r: &mut ParamReader<'_>) {
    warn_tls_keys(r, |key| {
        format!("`{key}` has no effect without `tls=true`; ignored")
    });
}
```

`crates/rurge-config/src/spec/vmess.rs`：

```rust
//! `vmess` policy parameters (manual: Policies › VMess).

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, idle_tls, read_tls};
use super::ws::{WsOpts, read_ws};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

/// `encrypt-method`: how the body is sealed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VmessCipher {
    #[default]
    Aes128Gcm,
    ChaCha20Poly1305,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmessSpec {
    /// `username`: the user id.
    pub uuid: Secret<[u8; 16]>,
    pub cipher: VmessCipher,
    /// `Some` with `tls=true`.
    pub tls: Option<TlsOpts>,
    pub ws: Option<WsOpts>,
}

/// What a `vmess` line says. Without `vmess-aead=true` it asks for the legacy
/// handshake, which is not implemented: the caller makes no spec of it
/// (M2 design 4.3).
pub struct VmessRead {
    pub spec: VmessSpec,
    pub aead: bool,
}

/// The usual 8-4-4-4-12 form, or the same 32 digits without hyphens.
fn parse_uuid(text: &str) -> Option<[u8; 16]> {
    let text = text.trim();
    let digits: Vec<u8> = match text.len() {
        36 => {
            let bytes = text.as_bytes();
            if [8, 13, 18, 23].iter().any(|i| bytes[*i] != b'-') {
                return None;
            }
            bytes.iter().copied().filter(|b| *b != b'-').collect()
        }
        32 => text.bytes().collect(),
        _ => return None,
    };
    if digits.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, pair) in digits.chunks(2).enumerate() {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        out[i] = (high * 16 + low) as u8;
    }
    Some(out)
}

/// Everything `vmess`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The id is named-only (`username=`), as the manual writes it, and is never
/// quoted in a diagnostic: it is the credential.
pub fn read_vmess(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> VmessRead {
    let uuid = match r.str("username").map(parse_uuid) {
        Some(Some(uuid)) => uuid,
        found => {
            let text = match found {
                None => "`username` is required",
                Some(_) => "`username` is not a valid UUID",
            };
            r.error(codes::E_INVALID_POLICY_PARAM, text.to_string());
            [0; 16]
        }
    };
    let cipher = r
        .choice(
            "encrypt-method",
            &[
                ("aes-128-gcm", VmessCipher::Aes128Gcm),
                ("chacha20-ietf-poly1305", VmessCipher::ChaCha20Poly1305),
            ],
        )
        .unwrap_or_default();
    let aead = r.bool("vmess-aead").unwrap_or(false);
    let tls = if r.bool("tls").unwrap_or(false) {
        Some(read_tls(r, keystore))
    } else {
        idle_tls(r);
        None
    };
    let ws = read_ws(r);
    VmessRead {
        spec: VmessSpec {
            uuid: Secret::new(uuid),
            cipher,
            tls,
            ws,
        },
        aead,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use std::path::Path;
    use std::sync::Arc;

    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
    const BYTES: [u8; 16] = [
        0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1,
        0x19,
    ];

    fn read(def: &str) -> (VmessRead, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let read = read_vmess(&mut r, &[]);
        let failed = r.has_errors();
        (read, failed, r.finish())
    }

    #[test]
    fn the_manuals_example_is_a_legacy_line() {
        let (read, failed, diags) = read(&format!("vmess, 1.2.3.4, 8000, username={ID}"));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!read.aead, "`vmess-aead` defaults to false");
        assert_eq!(read.spec.uuid.expose(), &BYTES);
        assert_eq!(read.spec.cipher, VmessCipher::Aes128Gcm);
        assert!(read.spec.tls.is_none() && read.spec.ws.is_none());
    }

    #[test]
    fn every_parameter_of_the_manual() {
        let (read, failed, diags) = read(&format!(
            "vmess, h.test, 443, username={}, vmess-aead=true, encrypt-method=chacha20-ietf-poly1305, tls=true, sni=edge.test, ws=true, ws-path=/v2, ws-headers=Host:example.com|X-Token:abc",
            ID.replace('-', "").to_uppercase()
        ));
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(read.aead);
        assert_eq!(read.spec.uuid.expose(), &BYTES, "hyphens and case do not matter");
        assert_eq!(read.spec.cipher, VmessCipher::ChaCha20Poly1305);
        assert_eq!(read.spec.tls.unwrap().sni, Sni::Name("edge.test".into()));
        let ws = read.spec.ws.unwrap();
        assert_eq!(ws.path, "/v2");
        assert_eq!(ws.headers.len(), 2);
    }

    #[test]
    fn the_id_is_required_named_and_never_quoted() {
        let (_, failed, diags) = read("vmess, h.test, 443, vmess-aead=true");
        assert!(failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (codes::E_INVALID_POLICY_PARAM, "policy `P`: `username` is required")
        );
        for bad in [
            "not-a-uuid",
            "0233d11c-15a4-47d3-ade3-48ffca0ce11",   // one digit short
            "0233d11c15a4-47d3-ade3-48ffca0ce119-",  // hyphens in the wrong places
            "0233d11c-15a4-47d3-ade3-48ffca0ce11g",  // not hex
        ] {
            let (_, failed, diags) = read(&format!("vmess, h.test, 443, username={bad}"));
            assert!(failed, "{bad}");
            assert_eq!(diags[0].message, "policy `P`: `username` is not a valid UUID");
            assert!(diags.iter().all(|d| !d.message.contains(bad)), "{diags:?}");
        }
        // a positional value is not read as the id
        let (_, failed, diags) = read(&format!("vmess, h.test, 443, {ID}"));
        assert!(failed);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `username` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(messages.iter().all(|m| !m.contains("0233")));
    }

    #[test]
    fn an_unknown_cipher_is_an_error() {
        let (_, failed, diags) =
            read(&format!("vmess, h.test, 443, username={ID}, encrypt-method=rc4"));
        assert!(failed);
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `rc4` for `encrypt-method` (expected aes-128-gcm / chacha20-ietf-poly1305)"
        );
    }

    #[test]
    fn tls_parameters_without_tls_are_idle() {
        let (read, failed, diags) = read(&format!(
            "vmess, h.test, 443, username={ID}, vmess-aead=true, sni=edge.test, skip-cert-verify=true"
        ));
        assert!(!failed);
        assert!(read.spec.tls.is_none());
        let found: Vec<(&str, &str)> = diags.iter().map(|d| (d.code, d.message.as_str())).collect();
        assert_eq!(
            found,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `skip-cert-verify` has no effect without `tls=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `sni` has no effect without `tls=true`; ignored"
                ),
            ]
        );
    }

    #[test]
    fn the_spec_does_not_print_the_id() {
        let (read, _, _) = read(&format!("vmess, h.test, 443, username={ID}"));
        let printed = format!("{:?}", read.spec);
        assert!(printed.contains("Secret(***)"), "{printed}");
        // 0xd1, the third byte, as `Debug` would print it
        assert!(!printed.contains("209"), "{printed}");
    }
}
```

Run: `cargo test -p rurge-config vmess`
Expected: 6 个用例通过。（先只贴 `mod tests` 与类型、让 `read_vmess` 返回 `todo!()`，确认用例因 `not yet implemented` 而失败，再贴实现。）

- [ ] **Step 5: `anytls` 的 spec 与读取函数（先写用例）**

`crates/rurge-config/src/spec/anytls.rs`：

```rust
//! `anytls` policy parameters (manual: Policies › AnyTLS).

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, read_tls};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnyTlsSpec {
    /// AnyTLS always runs over TLS.
    pub tls: TlsOpts,
    pub password: Secret<String>,
    /// `reuse`: keep a session for the next stream (the protocol's default).
    pub reuse: bool,
}

/// Everything `anytls`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The password is named-only (`password=`), as the manual writes it: a
/// positional value stays unread and is reported as an extra positional
/// value, never quoted.
pub fn read_anytls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> AnyTlsSpec {
    let tls = read_tls(r, keystore);
    let password = r.str("password").unwrap_or_default();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let reuse = r.bool("reuse").unwrap_or(true);
    AnyTlsSpec {
        tls,
        password: password.into(),
        reuse,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (AnyTlsSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let spec = read_anytls(&mut r, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    #[test]
    fn the_manuals_example_and_reuse() {
        let (spec, failed, diags) = read("anytls, 192.168.20.6, 443, password=pwd");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.password.expose(), "pwd");
        assert!(spec.reuse, "reuse is on unless turned off");
        let (spec, failed, _) =
            read("anytls, h.test, 443, password=p, reuse=false, sni=edge.test");
        assert!(!failed);
        assert!(!spec.reuse);
        assert_eq!(spec.tls.sni, Sni::Name("edge.test".into()));
    }

    #[test]
    fn a_missing_password_is_an_error_and_a_positional_one_is_not_read() {
        let (_, failed, diags) = read("anytls, h.test, 443");
        assert!(failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `password` is required"
            )
        );
        let (_, failed, diags) = read("anytls, h.test, 443, hunter2");
        assert!(failed);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `password` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(messages.iter().all(|m| !m.contains("hunter2")));
    }

    #[test]
    fn a_bad_boolean_is_an_error_and_the_spec_does_not_print_the_password() {
        let (_, failed, _) = read("anytls, h.test, 443, password=p, reuse=maybe");
        assert!(failed);
        let (spec, _, _) = read("anytls, h.test, 443, password=hunter2");
        let printed = format!("{spec:?}");
        assert!(printed.contains("Secret(***)") && !printed.contains("hunter2"), "{printed}");
    }
}
```

Run: `cargo test -p rurge-config anytls`
Expected: 3 个用例通过。

- [ ] **Step 6: `ProtoSpec::tls()`，引擎改用它**

`crates/rurge-config/src/spec/mod.rs`（`ProtoSpec` 定义之后）：

```rust
impl ProtoSpec {
    /// The TLS options of the protocol, when it runs over TLS.
    pub fn tls(&self) -> Option<&TlsOpts> {
        match self {
            ProtoSpec::Http(http) => http.tls.as_ref(),
            ProtoSpec::Socks5(socks) => socks.tls.as_ref(),
            ProtoSpec::Trojan(trojan) => Some(&trojan.tls),
            ProtoSpec::Direct | ProtoSpec::Reject(_) => None,
        }
    }
}
```

`crates/rurge-engine/src/outbounds.rs`：删掉 `fn tls_of`，`skips_verification` 改为

```rust
fn skips_verification(spec: &PolicySpec) -> bool {
    spec.proto
        .tls()
        .is_some_and(|tls| tls.skip_cert_verify && tls.fingerprint_sha256.is_none())
}
```

并去掉不再用到的 `TlsOpts` 导入。（穷尽匹配，没有 `_` 分支：Task 9 加变体时编译器会逼着补上这里。）

Run: `cargo test -p rurge-engine outbounds`
Expected: 全部通过。

- [ ] **Step 7: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat(config): Secret<T> 包住凭据字段；VmessSpec / AnyTlsSpec 与公开的读取函数；ProtoSpec::tls()"
```

---

### Task 2: VMess AEAD 的编解码——KDF、请求头 / 应答头、分块，对参考向量

纯函数，不碰网络：时间与随机数都是参数，参考向量逐字节适用。

**Files:**
- Modify: `Cargo.toml`（工作区依赖）、`crates/rurge-proto/Cargo.toml`
- Modify: `crates/rurge-proto/src/lib.rs`（`pub mod vmess;`）
- Create: `crates/rurge-proto/src/vmess/mod.rs`（本任务只有模块声明）
- Create: `crates/rurge-proto/src/vmess/{kdf,header,chunk,vectors}.rs`

**Interfaces:**
- Consumes: 无（只用新依赖与 `sha2`）。
- Produces（Task 3 与 `testing::vmess` 按这些名字用，全部 `pub(crate)`）：
  - `vmess::kdf::{kdf(key: &[u8], path: &[&[u8]]) -> [u8; 32], kdf16(..) -> [u8; 16]}` 与九个标签常量（`AUTH_ID_KEY` `RESPONSE_LEN_KEY` `RESPONSE_LEN_IV` `RESPONSE_KEY` `RESPONSE_IV` `HEADER_KEY` `HEADER_NONCE` `HEADER_LEN_KEY` `HEADER_LEN_NONCE`）。
  - `vmess::header::{Security::{Aes128Gcm = 3, ChaCha20Poly1305 = 4}, Session { body_iv, body_key, response_v }, OPTIONS, TAG, cmd_key(&[u8;16]) -> [u8;16], auth_id(&cmd_key, unix_time: i64, random: [u8;4]) -> [u8;16], request_plain(&Session, Security, address: &[u8], padding: &[u8]) -> Vec<u8>, seal_request(&cmd_key, &auth_id, &nonce8, plain) -> Vec<u8>, response_secrets(&Session) -> ([u8;16],[u8;16]), open_response_len(&key, &iv, [u8;18]) -> Result<usize, ResponseError>, open_response(&key, &iv, &Session, &mut [u8]) -> Result<(), ResponseError>, ResponseError::text()}`。
  - `vmess::chunk::{ChunkCipher::new(Security, &key, &iv), seal(&mut self, payload, &mut Vec<u8>), open_len(&mut self, [u8;2]) -> usize, open(&mut self, &mut [u8]) -> Option<usize>, MAX_PAYLOAD}`。
  - `vmess::vectors`（`#[cfg(test)]`）：`UUID`、`session()`、`address()`、`hex()` 与七个向量常量。

- [ ] **Step 1: 依赖**

根 `Cargo.toml` 的 `[workspace.dependencies]` 末尾加：

```toml
ring = "0.17"
aes = "0.8"
md-5 = "0.10"
sha3 = "0.10"
crc32fast = "1"
```

`crates/rurge-proto/Cargo.toml` 的 `[dependencies]` 加：

```toml
ring.workspace = true
aes.workspace = true
md-5.workspace = true
sha3.workspace = true
crc32fast.workspace = true
```

Run: `cargo check -p rurge-proto && git diff --stat Cargo.lock`
Expected: 编译通过；`Cargo.lock` 里**新增的 `[[package]]` 只有 `md-5`、`sha3`、`keccak` 三个**（`ring` `aes` `crc32fast` 早已在里面；`rurge-proto` 自己的依赖列表多五行不算）。用 `git diff Cargo.lock | grep '^+name'` 核对；多出别的条目就停下来，把差异写进报告，不要继续（P2）。

- [ ] **Step 2: 模块骨架与参考向量**

`crates/rurge-proto/src/lib.rs`：在 `pub mod trojan;` 之后加 `pub mod vmess;`。

`crates/rurge-proto/src/vmess/mod.rs`（出站在 Task 3 才来，先只有声明；三个 `allow` 是**临时的**，Task 3 的第一步就删掉——在那之前这些 `pub(crate)` 项只有测试在用，不加会被 `-D warnings` 拦下）：

```rust
//! `vmess` outbound (manual: Policies › VMess). The codec lives here; the
//! outbound and its stream join it in the next commit.

#[allow(dead_code)] // until the outbound uses it (next commit)
pub(crate) mod chunk;
#[allow(dead_code)] // until the outbound uses it (next commit)
pub(crate) mod header;
#[allow(dead_code)] // until the outbound uses it (next commit)
pub(crate) mod kdf;
#[cfg(test)]
pub(crate) mod vectors;
```

`crates/rurge-proto/src/vmess/vectors.rs`（向量的出处写在文件头；**不要改动任何一个十六进制串**）：

```rust
//! Reference vectors for the VMess AEAD codec.
//!
//! Source: the functions of v2fly/v2ray-core (master, 2026-09-20)
//! `proxy/vmess/aead/{kdf,authid,encrypt}.go`,
//! `proxy/vmess/encoding/{client,auth}.go` and `common/crypto/auth.go`, copied
//! into a standard-library-only Go program and run with the fixed inputs
//! below (time 1700000000, AuthID random `01020304`, connection nonce
//! `1011121314151617`, three padding bytes `a1a2a3`). An independent Python
//! derivation written from the protocol description agrees on every value;
//! the ChaCha20-Poly1305 chunks come from that derivation alone (the Go
//! standard library has no ChaCha20-Poly1305).

use super::header::Session;

/// The manual's example id, `0233d11c-15a4-47d3-ade3-48ffca0ce119`.
pub(crate) const UUID: [u8; 16] = [
    0x02, 0x33, 0xd1, 0x1c, 0x15, 0xa4, 0x47, 0xd3, 0xad, 0xe3, 0x48, 0xff, 0xca, 0x0c, 0xe1, 0x19,
];

/// body IV `20..2f`, body key `30..3f`, response check byte `5a`.
pub(crate) fn session() -> Session {
    let mut s = Session {
        body_iv: [0; 16],
        body_key: [0; 16],
        response_v: 0x5a,
    };
    for i in 0..16u8 {
        s.body_iv[usize::from(i)] = 0x20 + i;
        s.body_key[usize::from(i)] = 0x30 + i;
    }
    s
}

/// `example.com:443` as VMess writes it: port, type 2, length, name.
pub(crate) fn address() -> Vec<u8> {
    let mut a = vec![0x01, 0xbb, 2, 11];
    a.extend_from_slice(b"example.com");
    a
}

pub(crate) fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

pub(crate) const REQUEST_PLAIN: &str = "01202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f5a0533000101bb020b6578616d706c652e636f6da1a2a398a86391";
pub(crate) const REQUEST_SEALED: &str = "c65fe1f5eada535d481be2645132f215c1763ad5769a196e356ef10a09c3ffa14c57101112131415161725e6876107f5484048fdf2af4becc615e40c4263f45ffbb7f6fe2f8548076dd61eb6ebb6fedc4e422159241ad68b210e1cfe4356670d31fd44789ae62010ff33185ef9eb2a5324e114b87557";
/// `hello`, the bytes `00..27`, then the empty end-of-stream chunk.
pub(crate) const REQUEST_CHUNKS_AES: &str = "e49d4ba7ab09e5ade26eae75b50e9586a717ba52df4054220d2681b355c66ccfc7373b0e8dc50d7bc5693c83a37ab32c2d43f7a241bc94243bfed46e7722c6b84dd4339e454490a32477b51a30e553329075922c7461a107ed105859d0ac6eb92fde07";
/// The head `5a 00 00 00`.
pub(crate) const RESPONSE_SEALED: &str =
    "6797df2b7410a67de894084db988ae97c4774a80a14980ca327785428f407bc7a23a31c0db4d";
/// `world`, then the end-of-stream chunk.
pub(crate) const RESPONSE_CHUNKS_AES: &str =
    "a9ecd094dd7369ac2392c6a3e89ac65273e9e112684569a16583ef27af34548e1075794f199626737a";
pub(crate) const REQUEST_CHUNKS_CHACHA: &str = "e49d405260ed5fa743f035fe5f23d41a83f4843220f605220d973e3fb6ed8f1a62ef8a5e79446b65264e8b7479020654adb9d454ee487929a747b091f8503472fa3cebd4735e303e3a50ac967a2477c021759218c5acf6c41666e376adb23368dacedc";
pub(crate) const RESPONSE_CHUNKS_CHACHA: &str =
    "a9ecb0ed84766f4153619d12efb66748999fbb05c2f7f6a16528534029f2c272e6938a53b3663196e5";
```

- [ ] **Step 3: KDF（先让用例失败）**

`crates/rurge-proto/src/vmess/kdf.rs`。先贴整个文件但把 `fn hash` 的函数体换成 `todo!()`：

Run: `cargo test -p rurge-proto vmess::kdf`
Expected: FAIL，`not yet implemented`。

再换回真正的函数体：

```rust
//! The VMess AEAD key derivation: HMAC-SHA256 nested once per path element.
//!
//! The reference builds it as `hmac.New(parent.Create, element)`: the hash
//! *inside* each HMAC is the HMAC one level down, and the bottom one is
//! HMAC-SHA256 keyed with a fixed label. The `hmac` crate cannot express an
//! HMAC over an HMAC, so the construction (RFC 2104, 64-byte block, 32-byte
//! output at every level) is written out here and pinned by vectors taken
//! from the reference implementation.

use sha2::{Digest, Sha256};

const ROOT: &[u8] = b"VMess AEAD KDF";
const BLOCK: usize = 64;

pub(crate) const AUTH_ID_KEY: &[u8] = b"AES Auth ID Encryption";
pub(crate) const RESPONSE_LEN_KEY: &[u8] = b"AEAD Resp Header Len Key";
pub(crate) const RESPONSE_LEN_IV: &[u8] = b"AEAD Resp Header Len IV";
pub(crate) const RESPONSE_KEY: &[u8] = b"AEAD Resp Header Key";
pub(crate) const RESPONSE_IV: &[u8] = b"AEAD Resp Header IV";
pub(crate) const HEADER_KEY: &[u8] = b"VMess Header AEAD Key";
pub(crate) const HEADER_NONCE: &[u8] = b"VMess Header AEAD Nonce";
pub(crate) const HEADER_LEN_KEY: &[u8] = b"VMess Header AEAD Key_Length";
pub(crate) const HEADER_LEN_NONCE: &[u8] = b"VMess Header AEAD Nonce_Length";

fn hmac(hash: &dyn Fn(&[u8]) -> [u8; 32], key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        block[..32].copy_from_slice(&hash(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    inner.extend(block.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(message);
    let mut outer = Vec::with_capacity(BLOCK + 32);
    outer.extend(block.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&hash(&inner));
    hash(&outer)
}

/// The hash `path` names: HMAC keyed with its last element over the hash the
/// rest of it names; the empty path is HMAC-SHA256 keyed with `ROOT`.
fn hash(path: &[&[u8]], message: &[u8]) -> [u8; 32] {
    match path.split_last() {
        None => hmac(&|m| Sha256::digest(m).into(), ROOT, message),
        Some((key, rest)) => hmac(&|m| hash(rest, m), key, message),
    }
}

pub(crate) fn kdf(key: &[u8], path: &[&[u8]]) -> [u8; 32] {
    hash(path, key)
}

pub(crate) fn kdf16(key: &[u8], path: &[&[u8]]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&kdf(key, path)[..16]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::hex;

    /// v2fly/v2ray-core `proxy/vmess/aead/kdf.go`, run with these inputs.
    #[test]
    fn the_nesting_matches_the_reference() {
        assert_eq!(
            kdf(b"key", &[]).to_vec(),
            hex("385e28ac08671660f62ac976f5e64a31827aea172eff77cb2b046c52de9c08b0")
        );
        assert_eq!(
            kdf(b"key", &[b"a"]).to_vec(),
            hex("70ec70c19ef671319ed7b5493552fa2d77a53b57dba8aeb405536e79b5b50b6e")
        );
        assert_eq!(
            kdf(b"key", &[b"a", b"b"]).to_vec(),
            hex("721bea6cc9f16fac53b2afd131a33b9dc08e884e5b997a8ffac94a12cfd639ec")
        );
        assert_eq!(
            kdf(b"key", &[b"a", b"b", b"c"]).to_vec(),
            hex("7bc9030cc29018ba2c4a5bf0e32df68140fc20235fe0a1282f39e1d222279e18")
        );
    }

    #[test]
    fn a_key_longer_than_the_block_is_hashed_first() {
        // RFC 2104: only reachable with a path element above 64 bytes, which
        // VMess never uses; pinned so the branch is not dead weight
        let long = [7u8; 100];
        assert_ne!(kdf(b"key", &[&long]), kdf(b"key", &[&long[..64]]));
        assert_eq!(
            kdf16(b"key", &[b"a"]).to_vec(),
            hex("70ec70c19ef671319ed7b5493552fa2d")
        );
    }
}
```

Run: `cargo test -p rurge-proto vmess::kdf`
Expected: 2 个用例通过。嵌套的方向最容易写反（最外层的密钥是路径的**最后**一个元素）：`the_nesting_matches_the_reference` 的四个值就是用来抓这个的。

- [ ] **Step 4: 请求头与应答头**

`crates/rurge-proto/src/vmess/header.rs`：

```rust
//! The VMess AEAD request head and the response head (M2 design 6.4;
//! byte layout checked against v2fly/v2ray-core `proxy/vmess/aead` and
//! `proxy/vmess/encoding/client.go`).
//!
//! Every function here is pure: time and randomness are arguments, so the
//! reference vectors apply byte for byte.

use super::kdf::{self, kdf, kdf16};
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockEncrypt, KeyInit};
use md5::{Digest, Md5};
use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};

const VERSION: u8 = 1;
const COMMAND_TCP: u8 = 1;
/// ChunkStream | ChunkMasking: what every server accepts.
pub(crate) const OPTIONS: u8 = 0x01 | 0x04;
pub(crate) const TAG: usize = 16;
/// A sealed response head is `V Opt Cmd Len` plus at most 255 bytes of command.
const MAX_RESPONSE_HEAD: usize = 4 + 255;

const ID_MAGIC: &[u8] = b"c48619fe-8f02-49e0-b9e9-edf763e17e21";

/// The `security` nibble of the request head.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Security {
    Aes128Gcm = 3,
    ChaCha20Poly1305 = 4,
}

pub(crate) fn cmd_key(uuid: &[u8; 16]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(uuid);
    h.update(ID_MAGIC);
    h.finalize().into()
}

/// `time(8) ‖ random(4) ‖ crc32(4)` under one AES-128 block.
pub(crate) fn auth_id(cmd_key: &[u8; 16], unix_time: i64, random: [u8; 4]) -> [u8; 16] {
    let mut block = [0u8; 16];
    block[..8].copy_from_slice(&unix_time.to_be_bytes());
    block[8..12].copy_from_slice(&random);
    let crc = crc32fast::hash(&block[..12]);
    block[12..].copy_from_slice(&crc.to_be_bytes());
    let key = kdf16(cmd_key, &[kdf::AUTH_ID_KEY]);
    let mut block = GenericArray::from(block);
    Aes128::new(&GenericArray::from(key)).encrypt_block(&mut block);
    block.into()
}

/// What one connection draws at random.
pub(crate) struct Session {
    pub body_iv: [u8; 16],
    pub body_key: [u8; 16],
    pub response_v: u8,
}

fn fnv1a(data: &[u8]) -> u32 {
    data.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    })
}

/// The head before sealing. `address` is `port ‖ type ‖ address`
/// (`addr::vmess_addr`); `padding` is 0 – 15 random bytes.
pub(crate) fn request_plain(
    session: &Session,
    security: Security,
    address: &[u8],
    padding: &[u8],
) -> Vec<u8> {
    debug_assert!(padding.len() < 16);
    let mut out = Vec::with_capacity(38 + address.len() + padding.len() + 4);
    out.push(VERSION);
    out.extend_from_slice(&session.body_iv);
    out.extend_from_slice(&session.body_key);
    out.push(session.response_v);
    out.push(OPTIONS);
    out.push(((padding.len() as u8) << 4) | security as u8);
    out.push(0);
    out.push(COMMAND_TCP);
    out.extend_from_slice(address);
    out.extend_from_slice(padding);
    let check = fnv1a(&out);
    out.extend_from_slice(&check.to_be_bytes());
    out
}

fn gcm(key: [u8; 16]) -> LessSafeKey {
    // a 16-byte key is what AES-128-GCM takes: this cannot fail
    LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &key).expect("a 16-byte key"))
}

fn nonce(bytes: &[u8; 32]) -> Nonce {
    let mut n = [0u8; 12];
    n.copy_from_slice(&bytes[..12]);
    Nonce::assume_unique_for_key(n)
}

fn seal(key: [u8; 16], iv: &[u8; 32], aad: &[u8], plain: &[u8], out: &mut Vec<u8>) {
    let start = out.len();
    out.extend_from_slice(plain);
    let tag = gcm(key)
        .seal_in_place_separate_tag(nonce(iv), Aad::from(aad), &mut out[start..])
        // fails only above 2^36 bytes
        .expect("a request head is a few dozen bytes");
    out.extend_from_slice(tag.as_ref());
}

fn open(key: [u8; 16], iv: &[u8; 32], aad: &[u8], sealed: &mut [u8]) -> Option<usize> {
    gcm(key)
        .open_in_place(nonce(iv), Aad::from(aad), sealed)
        .ok()
        .map(|plain| plain.len())
}

/// `AuthID(16) ‖ sealed length(18) ‖ nonce(8) ‖ sealed head`.
pub(crate) fn seal_request(
    cmd_key: &[u8; 16],
    auth_id: &[u8; 16],
    connection_nonce: &[u8; 8],
    plain: &[u8],
) -> Vec<u8> {
    let path = |label: &'static [u8]| [label, &auth_id[..], &connection_nonce[..]];
    let mut out = Vec::with_capacity(16 + 18 + 8 + plain.len() + TAG);
    out.extend_from_slice(auth_id);
    seal(
        kdf16(cmd_key, &path(kdf::HEADER_LEN_KEY)),
        &kdf(cmd_key, &path(kdf::HEADER_LEN_NONCE)),
        auth_id,
        &(plain.len() as u16).to_be_bytes(),
        &mut out,
    );
    out.extend_from_slice(connection_nonce);
    seal(
        kdf16(cmd_key, &path(kdf::HEADER_KEY)),
        &kdf(cmd_key, &path(kdf::HEADER_NONCE)),
        auth_id,
        plain,
        &mut out,
    );
    out
}

/// The response direction's body key and IV: the first half of the SHA-256
/// of the request's.
pub(crate) fn response_secrets(session: &Session) -> ([u8; 16], [u8; 16]) {
    use sha2::Sha256;
    let half = |input: &[u8; 16]| {
        let mut out = [0u8; 16];
        out.copy_from_slice(&Sha256::digest(input)[..16]);
        out
    };
    (half(&session.body_key), half(&session.body_iv))
}

/// Why a response head was refused. The texts go to the session log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseError {
    /// Wrong id on our side, or not a VMess AEAD server at all.
    NotAuthentic,
    TooLong,
    /// The head opened but does not answer this request.
    Mismatch,
}

impl ResponseError {
    pub(crate) fn text(self) -> &'static str {
        match self {
            ResponseError::NotAuthentic => "vmess: the response cannot be authenticated",
            ResponseError::TooLong => "vmess: the response head is longer than the protocol allows",
            ResponseError::Mismatch => "vmess: the response does not answer this request",
        }
    }
}

/// Opens the 18 sealed bytes that carry the head's length; returns how many
/// sealed bytes follow (the head plus its tag).
pub(crate) fn open_response_len(
    key: &[u8; 16],
    iv: &[u8; 16],
    mut sealed: [u8; 18],
) -> Result<usize, ResponseError> {
    let n = open(
        kdf16(key, &[kdf::RESPONSE_LEN_KEY]),
        &kdf(iv, &[kdf::RESPONSE_LEN_IV]),
        &[],
        &mut sealed,
    )
    .ok_or(ResponseError::NotAuthentic)?;
    debug_assert_eq!(n, 2);
    let len = usize::from(u16::from_be_bytes([sealed[0], sealed[1]]));
    if len > MAX_RESPONSE_HEAD {
        return Err(ResponseError::TooLong);
    }
    Ok(len + TAG)
}

/// Opens the head itself and checks that it answers `session`. A command in
/// it (the dynamic-port instruction) is ignored, as current clients do.
pub(crate) fn open_response(
    key: &[u8; 16],
    iv: &[u8; 16],
    session: &Session,
    sealed: &mut [u8],
) -> Result<(), ResponseError> {
    let n = open(
        kdf16(key, &[kdf::RESPONSE_KEY]),
        &kdf(iv, &[kdf::RESPONSE_IV]),
        &[],
        sealed,
    )
    .ok_or(ResponseError::NotAuthentic)?;
    if n < 4 || sealed[0] != session.response_v {
        return Err(ResponseError::Mismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::vectors::{self, hex};

    #[test]
    fn the_command_key_and_the_auth_id_match_the_reference() {
        let key = cmd_key(&vectors::UUID);
        assert_eq!(key.to_vec(), hex("1f449ead3205fb33e019c7af624da5b4"));
        assert_eq!(
            auth_id(&key, 1_700_000_000, [1, 2, 3, 4]).to_vec(),
            hex("c65fe1f5eada535d481be2645132f215")
        );
    }

    #[test]
    fn the_request_head_matches_the_reference_byte_for_byte() {
        let plain = request_plain(
            &vectors::session(),
            Security::Aes128Gcm,
            &vectors::address(),
            &[0xa1, 0xa2, 0xa3],
        );
        assert_eq!(plain, hex(vectors::REQUEST_PLAIN));
        let key = cmd_key(&vectors::UUID);
        let id = auth_id(&key, 1_700_000_000, [1, 2, 3, 4]);
        let sealed = seal_request(
            &key,
            &id,
            &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17],
            &plain,
        );
        assert_eq!(sealed, hex(vectors::REQUEST_SEALED));
    }

    #[test]
    fn the_security_nibble_follows_the_cipher() {
        let plain = request_plain(
            &vectors::session(),
            Security::ChaCha20Poly1305,
            &vectors::address(),
            &[],
        );
        assert_eq!(plain[35], 0x04, "no padding, chacha20-poly1305");
        assert_eq!(plain[34], OPTIONS);
    }

    #[test]
    fn the_response_head_opens_and_is_checked() {
        let session = vectors::session();
        let (key, iv) = response_secrets(&session);
        assert_eq!(key.to_vec(), hex("816b9e7c25d559c5766755b3bbb36654"));
        assert_eq!(iv.to_vec(), hex("36db1adc807ac50e4c85bd86a174b4aa"));
        let wire = hex(vectors::RESPONSE_SEALED);
        let mut len = [0u8; 18];
        len.copy_from_slice(&wire[..18]);
        let rest = open_response_len(&key, &iv, len).unwrap();
        assert_eq!(rest, 4 + TAG);
        let mut head = wire[18..].to_vec();
        assert_eq!(open_response(&key, &iv, &session, &mut head), Ok(()));
        // the same head does not answer a request that drew another V
        let other = Session {
            response_v: 0x5b,
            ..vectors::session()
        };
        let mut head = wire[18..].to_vec();
        assert_eq!(
            open_response(&key, &iv, &other, &mut head),
            Err(ResponseError::Mismatch)
        );
        // one flipped bit anywhere is an authentication failure
        let mut bad = len;
        bad[3] ^= 1;
        assert_eq!(
            open_response_len(&key, &iv, bad),
            Err(ResponseError::NotAuthentic)
        );
        let mut head = wire[18..].to_vec();
        head[0] ^= 1;
        assert_eq!(
            open_response(&key, &iv, &session, &mut head),
            Err(ResponseError::NotAuthentic)
        );
    }

    #[test]
    fn an_absurd_response_length_is_refused_before_anything_is_allocated() {
        let session = vectors::session();
        let (key, iv) = response_secrets(&session);
        let mut sealed = Vec::new();
        seal(
            kdf16(&key, &[kdf::RESPONSE_LEN_KEY]),
            &kdf(&iv, &[kdf::RESPONSE_LEN_IV]),
            &[],
            &60000u16.to_be_bytes(),
            &mut sealed,
        );
        let mut len = [0u8; 18];
        len.copy_from_slice(&sealed);
        assert_eq!(
            open_response_len(&key, &iv, len),
            Err(ResponseError::TooLong)
        );
    }
}
```

Run: `cargo test -p rurge-proto vmess::header`
Expected: 5 个用例通过。

- [ ] **Step 5: 分块**

`crates/rurge-proto/src/vmess/chunk.rs`：

```rust
//! One direction of a VMess body (options ChunkStream + ChunkMasking): every
//! chunk is `length(2) ‖ AEAD(payload)`, the length XORed with the next two
//! bytes of SHAKE128(iv), the AEAD nonce `count(2, BE) ‖ iv[2..12]`, no
//! associated data. An empty payload ends the stream.

use super::header::{Security, TAG};
use md5::{Digest, Md5};
use ring::aead::{AES_128_GCM, Aad, CHACHA20_POLY1305, LessSafeKey, Nonce, UnboundKey};
use sha3::Shake128;
use sha3::digest::{ExtendableOutput, Update, XofReader};

/// A sealed chunk never exceeds 2^14 bytes (the limit in the protocol
/// description; sing-box reads a chunk into a 16384-byte buffer).
pub(crate) const MAX_PAYLOAD: usize = 16384 - TAG;

/// ChaCha20-Poly1305 wants 32 bytes: `MD5(key) ‖ MD5(MD5(key))`.
fn chacha_key(key: &[u8; 16]) -> [u8; 32] {
    let first: [u8; 16] = Md5::digest(key).into();
    let second: [u8; 16] = Md5::digest(first).into();
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&first);
    out[16..].copy_from_slice(&second);
    out
}

pub(crate) struct ChunkCipher {
    key: LessSafeKey,
    iv: [u8; 16],
    count: u16,
    mask: sha3::Shake128Reader,
}

impl ChunkCipher {
    pub(crate) fn new(security: Security, key: &[u8; 16], iv: &[u8; 16]) -> ChunkCipher {
        // both key lengths are fixed by the types: these cannot fail
        let key = match security {
            Security::Aes128Gcm => UnboundKey::new(&AES_128_GCM, key).expect("a 16-byte key"),
            Security::ChaCha20Poly1305 => {
                UnboundKey::new(&CHACHA20_POLY1305, &chacha_key(key)).expect("a 32-byte key")
            }
        };
        let mut shake = Shake128::default();
        shake.update(iv);
        ChunkCipher {
            key: LessSafeKey::new(key),
            iv: *iv,
            count: 0,
            mask: shake.finalize_xof(),
        }
    }

    fn next_mask(&mut self) -> u16 {
        let mut two = [0u8; 2];
        self.mask.read(&mut two);
        u16::from_be_bytes(two)
    }

    /// The counter wraps after 65536 chunks, as the reference's does.
    fn next_nonce(&mut self) -> Nonce {
        let mut nonce = [0u8; 12];
        nonce[..2].copy_from_slice(&self.count.to_be_bytes());
        nonce[2..].copy_from_slice(&self.iv[2..12]);
        self.count = self.count.wrapping_add(1);
        Nonce::assume_unique_for_key(nonce)
    }

    /// Appends one chunk carrying `payload` (at most `MAX_PAYLOAD` bytes;
    /// empty = end of stream) to `out`.
    pub(crate) fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) {
        debug_assert!(payload.len() <= MAX_PAYLOAD);
        let sealed_len = (payload.len() + TAG) as u16;
        out.extend_from_slice(&(self.next_mask() ^ sealed_len).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(payload);
        let nonce = self.next_nonce();
        let tag = self
            .key
            .seal_in_place_separate_tag(nonce, Aad::empty(), &mut out[start..])
            // fails only above 2^36 bytes
            .expect("a chunk is at most 16 KiB");
        out.extend_from_slice(tag.as_ref());
    }

    /// How many sealed bytes follow these two length bytes.
    pub(crate) fn open_len(&mut self, masked: [u8; 2]) -> usize {
        usize::from(self.next_mask() ^ u16::from_be_bytes(masked))
    }

    /// Opens a sealed chunk in place; the payload is `sealed[..n]`.
    pub(crate) fn open(&mut self, sealed: &mut [u8]) -> Option<usize> {
        let nonce = self.next_nonce();
        self.key
            .open_in_place(nonce, Aad::empty(), sealed)
            .ok()
            .map(|plain| plain.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vmess::header::response_secrets;
    use crate::vmess::vectors::{self, hex};

    fn sealed(cipher: &mut ChunkCipher, payloads: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in payloads {
            cipher.seal(p, &mut out);
        }
        out
    }

    /// Splits `wire` back into payloads with a fresh cipher.
    fn opened(mut cipher: ChunkCipher, wire: &[u8]) -> Vec<Vec<u8>> {
        let (mut at, mut out) = (0, Vec::new());
        while at < wire.len() {
            let len = cipher.open_len([wire[at], wire[at + 1]]);
            let mut chunk = wire[at + 2..at + 2 + len].to_vec();
            let n = cipher.open(&mut chunk).expect("authentic");
            out.push(chunk[..n].to_vec());
            at += 2 + len;
        }
        out
    }

    #[test]
    fn aes_chunks_match_the_reference_in_both_directions() {
        let s = vectors::session();
        let long: Vec<u8> = (0u8..40).collect();
        let mut up = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        assert_eq!(
            sealed(&mut up, &[b"hello", &long, b""]),
            hex(vectors::REQUEST_CHUNKS_AES)
        );
        let (key, iv) = response_secrets(&s);
        let mut down = ChunkCipher::new(Security::Aes128Gcm, &key, &iv);
        assert_eq!(
            sealed(&mut down, &[b"world", b""]),
            hex(vectors::RESPONSE_CHUNKS_AES)
        );
    }

    #[test]
    fn chacha_chunks_match_the_independent_derivation() {
        let s = vectors::session();
        assert_eq!(
            chacha_key(&s.body_key).to_vec(),
            hex("bdf2930f973f722e24a3773d61889501c3c2e371e23677b71c00a97d736d5e0f")
        );
        let long: Vec<u8> = (0u8..40).collect();
        let mut up = ChunkCipher::new(Security::ChaCha20Poly1305, &s.body_key, &s.body_iv);
        assert_eq!(
            sealed(&mut up, &[b"hello", &long, b""]),
            hex(vectors::REQUEST_CHUNKS_CHACHA)
        );
        let (key, iv) = response_secrets(&s);
        let mut down = ChunkCipher::new(Security::ChaCha20Poly1305, &key, &iv);
        assert_eq!(
            sealed(&mut down, &[b"world", b""]),
            hex(vectors::RESPONSE_CHUNKS_CHACHA)
        );
    }

    #[test]
    fn the_length_masks_are_the_shake128_stream_of_the_iv() {
        let s = vectors::session();
        let mut c = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        let masks: Vec<u8> = (0..8).flat_map(|_| c.next_mask().to_be_bytes()).collect();
        assert_eq!(masks, hex("e48822357582b1941d2dc3ca28b23952"));
    }

    #[test]
    fn what_was_sealed_opens_and_a_flipped_bit_does_not() {
        let s = vectors::session();
        let wire = hex(vectors::REQUEST_CHUNKS_AES);
        let long: Vec<u8> = (0u8..40).collect();
        let cipher = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        assert_eq!(opened(cipher, &wire), [b"hello".to_vec(), long, Vec::new()]);
        let mut cipher = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        let len = cipher.open_len([wire[0], wire[1]]);
        assert_eq!(len, 5 + TAG);
        let mut chunk = wire[2..2 + len].to_vec();
        chunk[0] ^= 1;
        assert_eq!(cipher.open(&mut chunk), None);
    }

    #[test]
    fn the_largest_chunk_fits_the_length_field() {
        let s = vectors::session();
        let mut c = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
        let mut out = Vec::new();
        c.seal(&vec![7u8; MAX_PAYLOAD], &mut out);
        assert_eq!(out.len(), 2 + 16384);
    }
}
```

Run: `cargo test -p rurge-proto vmess::chunk`
Expected: 5 个用例通过。

- [ ] **Step 6: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat(proto): VMess AEAD 的编解码——嵌套 HMAC 的 KDF、AuthID、请求头 / 应答头密封、分块（AES-128-GCM / ChaCha20-Poly1305 + SHAKE128 掩码），对参考实现的向量"
```

---

### Task 3: VMess 的流、出站与回环假服务端

**Files:**
- Modify: `crates/rurge-proto/src/addr.rs`（`vmess_addr`）
- Modify: `crates/rurge-proto/src/vmess/mod.rs`（整个替换：出站）
- Create: `crates/rurge-proto/src/vmess/stream.rs`
- Create: `crates/rurge-proto/src/testing/vmess.rs`
- Modify: `crates/rurge-proto/src/testing/mod.rs`（`mod vmess;` 与导出）

**Interfaces:**
- Consumes: Task 1 的 `VmessSpec` / `VmessCipher` / `read_vmess`；Task 2 的编解码；M2a 的 `transport::Stack::{new, open}`、`transport::lazy_head::LazyHead::new(inner, head)`、`transport::ws::WsClient::new(&WsOpts, &Target, tls: bool)`、`build::tls_client(Option<&TlsOpts>, &HostName, &[], keystore, roots)`、`transport::prefixed::boxed(prefix, inner)`、`testing::{TlsFixture, ws::{RecordedWs, accept_bytes}, AbortOnDrop, echo_server}`。
- Produces:
  - `rurge_proto::vmess::VmessOutbound::new(name: &str, server: Target, spec: &VmessSpec, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<VmessOutbound, BuildError>`，实现 `Outbound`。
  - `rurge_proto::testing::{FakeVmess, VmessScript, RecordedVmess}`：`VmessScript::new(id: &str)`（字段 `uuid` `ws` `connect_to`）；`FakeVmess::spawn(script, tls: Option<Arc<TlsFixture>>)`；`addr()` `requests()` `ws_seen()` `connections()` `rejected()`；`RecordedVmess { command, options, security, padding, atyp, host, port, skew, early }`。
  - `crate::addr::vmess_addr(&Target) -> Result<Vec<u8>, AddrError>`。

- [ ] **Step 1: 删掉 Task 2 的三个临时 `allow`，加地址编码（先写用例）**

`crates/rurge-proto/src/addr.rs`：文件头注释改为

```rust
//! `ATYP ADDR PORT` as SOCKS5 writes it (RFC 1928 §5; Trojan and AnyTLS use
//! the same encoding), and VMess's own order and type numbers.
```

在 `socks_addr` 之后加：

```rust
/// `PORT TYPE ADDR` as VMess writes it: the port first, and the types are
/// 1 = IPv4, 2 = domain, 3 = IPv6 (not SOCKS5's 1 / 3 / 4).
pub(crate) fn vmess_addr(target: &Target) -> Result<Vec<u8>, AddrError> {
    let mut out = target.port.to_be_bytes().to_vec();
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(3);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name).ok_or(AddrError::Unsendable)?;
            let len = u8::try_from(name.len()).map_err(|_| AddrError::TooLong)?;
            out.push(2);
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    Ok(out)
}
```

`mod tests` 里加：

```rust
    #[test]
    fn vmess_puts_the_port_first_and_numbers_the_types_its_own_way() {
        let t = |host: &str| Target::new(HostName::parse(host), 0x01bb);
        assert_eq!(
            vmess_addr(&t("10.1.2.3")).unwrap(),
            [0x01, 0xbb, 1, 10, 1, 2, 3]
        );
        let v6 = vmess_addr(&t("2001:db8::1")).unwrap();
        assert_eq!((&v6[..3], v6.len()), (&[0x01, 0xbb, 3][..], 2 + 1 + 16));
        let mut expected = vec![0x01, 0xbb, 2, 11];
        expected.extend_from_slice(b"example.com");
        assert_eq!(vmess_addr(&t("example.com")).unwrap(), expected);
        // the same bytes the reference vectors were made with
        assert_eq!(expected, crate::vmess::vectors::address());
        assert_eq!(
            vmess_addr(&Target::new(HostName::Domain("a@b.test".into()), 1)),
            Err(AddrError::Unsendable)
        );
        assert_eq!(
            vmess_addr(&Target::new(HostName::Domain("a".repeat(256)), 1)),
            Err(AddrError::TooLong)
        );
    }
```

Run: `cargo test -p rurge-proto addr`
Expected: 两个用例通过。

- [ ] **Step 2: 分块流 `VmessStream`**

`crates/rurge-proto/src/vmess/stream.rs`：

```rust
//! The byte stream a VMess connection becomes once the request head is
//! queued: writes are sealed into chunks, reads open the response head and
//! then chunks.
//!
//! A write reports success only after its whole chunk has been handed to the
//! layer below, so nothing of ours waits for a `flush` the relay never calls.
//! A chunk that could not be written in one go stays parked (it is already
//! sealed with its nonce) and is finished by the next write, flush or
//! shutdown.

use super::chunk::{ChunkCipher, MAX_PAYLOAD};
use super::header::{self, ResponseError, Security, Session, TAG};
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

enum Reading {
    /// The 18 sealed bytes holding the response head's length.
    HeadLen {
        buf: [u8; 18],
        filled: usize,
    },
    Head {
        buf: Vec<u8>,
        filled: usize,
    },
    Len {
        buf: [u8; 2],
        filled: usize,
    },
    Body {
        buf: Vec<u8>,
        filled: usize,
    },
    Payload {
        buf: Vec<u8>,
        pos: usize,
        end: usize,
    },
    Eof,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Open,
    /// The end-of-stream chunk is sealed into `out`.
    Sealed,
    Done,
}

/// No `Debug`: it holds the connection's keys.
pub(crate) struct VmessStream {
    inner: BoxedStream,
    session: Session,
    response_key: [u8; 16],
    response_iv: [u8; 16],
    up: ChunkCipher,
    down: ChunkCipher,
    /// The chunk being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
    closing: Closing,
    reading: Reading,
}

fn protocol(e: ResponseError) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.text())
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "vmess: the connection ended in the middle of a chunk",
    )
}

/// Fills `buf[*filled..]`. `Ok(false)`: the peer closed before the first byte.
fn poll_fill(
    inner: &mut BoxedStream,
    cx: &mut Context<'_>,
    buf: &mut [u8],
    filled: &mut usize,
) -> Poll<io::Result<bool>> {
    while *filled < buf.len() {
        let mut space = ReadBuf::new(&mut buf[*filled..]);
        ready!(Pin::new(&mut *inner).poll_read(cx, &mut space))?;
        let n = space.filled().len();
        if n == 0 {
            return Poll::Ready(if *filled == 0 {
                Ok(false)
            } else {
                Err(cut_short())
            });
        }
        *filled += n;
    }
    Poll::Ready(Ok(true))
}

impl VmessStream {
    /// `inner` already carries (or lazily owes) the sealed request head.
    pub(crate) fn new(inner: BoxedStream, session: Session, security: Security) -> VmessStream {
        let (response_key, response_iv) = header::response_secrets(&session);
        VmessStream {
            up: ChunkCipher::new(security, &session.body_key, &session.body_iv),
            down: ChunkCipher::new(security, &response_key, &response_iv),
            inner,
            session,
            response_key,
            response_iv,
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
            closing: Closing::Open,
            reading: Reading::HeadLen {
                buf: [0; 18],
                filled: 0,
            },
        }
    }

    fn poll_out(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.out_pos < self.out.len() {
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &self.out[self.out_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.out_pos += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for VmessStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            match &mut this.reading {
                Reading::HeadLen { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        // a wrong id, or clocks too far apart: the server
                        // just closes, and nothing tells the two apart
                        return Poll::Ready(Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "vmess: the server closed the connection without answering",
                        )));
                    }
                    let rest =
                        header::open_response_len(&this.response_key, &this.response_iv, *buf)
                            .map_err(protocol)?;
                    this.reading = Reading::Head {
                        buf: vec![0; rest],
                        filled: 0,
                    };
                }
                Reading::Head { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    header::open_response(
                        &this.response_key,
                        &this.response_iv,
                        &this.session,
                        buf,
                    )
                    .map_err(protocol)?;
                    this.reading = Reading::Len {
                        buf: [0; 2],
                        filled: 0,
                    };
                }
                Reading::Len { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        // closed between chunks: the end, without the courtesy chunk
                        this.reading = Reading::Eof;
                        continue;
                    }
                    let len = this.down.open_len(*buf);
                    if len < TAG {
                        return Poll::Ready(Err(invalid("vmess: a chunk shorter than its tag")));
                    }
                    this.reading = Reading::Body {
                        buf: vec![0; len],
                        filled: 0,
                    };
                }
                Reading::Body { buf, filled } => {
                    if !ready!(poll_fill(&mut this.inner, cx, buf, filled))? {
                        return Poll::Ready(Err(cut_short()));
                    }
                    let Some(end) = this.down.open(buf) else {
                        return Poll::Ready(Err(invalid("vmess: a chunk cannot be authenticated")));
                    };
                    this.reading = if end == 0 {
                        Reading::Eof
                    } else {
                        Reading::Payload {
                            buf: std::mem::take(buf),
                            pos: 0,
                            end,
                        }
                    };
                }
                Reading::Payload { buf, pos, end } => {
                    let n = out.remaining().min(*end - *pos);
                    out.put_slice(&buf[*pos..*pos + n]);
                    *pos += n;
                    if pos == end {
                        this.reading = Reading::Len {
                            buf: [0; 2],
                            filled: 0,
                        };
                    }
                    return Poll::Ready(Ok(()));
                }
                Reading::Eof => return Poll::Ready(Ok(())),
            }
        }
    }
}

impl AsyncWrite for VmessStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.closing != Closing::Open {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            // an empty chunk would tell the server the stream is over
            return Poll::Ready(Ok(0));
        }
        if this.out_pos == this.out.len() {
            let n = data.len().min(MAX_PAYLOAD);
            this.out.clear();
            this.out_pos = 0;
            this.up.seal(&data[..n], &mut this.out);
            this.accepted = n;
        }
        ready!(this.poll_out(cx))?;
        Poll::Ready(Ok(this.accepted.min(data.len())))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.closing == Closing::Open {
            // a parked chunk first: it already consumed its nonce
            ready!(this.poll_out(cx))?;
            this.out.clear();
            this.out_pos = 0;
            this.up.seal(&[], &mut this.out);
            this.closing = Closing::Sealed;
        }
        if this.closing == Closing::Sealed {
            ready!(this.poll_out(cx))?;
            this.closing = Closing::Done;
        }
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
```

要点（评审会看这几条）：写在**整个分块交给下一层之后**才报告成功；写不动时分块停在 `out` 里（它已经用掉了自己的 nonce，不能丢、不能重封），由下一次写 / flush / shutdown 接着写完；空写不封块（空块是"流结束"）；`shutdown` 先写完停着的分块，再发空块，再关下一层。

- [ ] **Step 3: 回环假服务端**

`crates/rurge-proto/src/testing/vmess.rs`：

```rust
//! A scriptable VMess AEAD server: optionally TLS, optionally a WebSocket
//! below the protocol, the sealed request head, then a chunked relay. It never
//! resolves a name.

use super::ws::{RecordedWs, accept_bytes};
use super::{AbortOnDrop, TlsFixture};
use crate::vmess::chunk::ChunkCipher;
use crate::vmess::header::{self, Security, Session, TAG};
use crate::vmess::kdf::{self, kdf, kdf16};
use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecrypt, KeyInit};
use ring::aead::{AES_128_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct VmessScript {
    pub uuid: [u8; 16],
    /// Expect a WebSocket handshake before the request head.
    pub ws: bool,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

impl VmessScript {
    /// `id` in the usual 8-4-4-4-12 form.
    pub fn new(id: &str) -> VmessScript {
        let hex: Vec<u8> = id.bytes().filter(|b| *b != b'-').collect();
        let mut uuid = [0u8; 16];
        for (i, pair) in hex.chunks(2).enumerate() {
            let text = std::str::from_utf8(pair).expect("ascii");
            uuid[i] = u8::from_str_radix(text, 16).expect("a hex id");
        }
        VmessScript {
            uuid,
            ..VmessScript::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedVmess {
    pub command: u8,
    pub options: u8,
    pub security: u8,
    pub padding: usize,
    pub atyp: u8,
    /// An IP literal, or the name exactly as it was on the wire.
    pub host: String,
    pub port: u16,
    /// The AuthID's timestamp minus the server's clock, in seconds.
    pub skew: i64,
    /// Bytes that arrived in the same read as the end of the head.
    pub early: usize,
}

pub struct FakeVmess {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedVmess>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    connections: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

struct Shared {
    script: VmessScript,
    requests: Arc<Mutex<Vec<RecordedVmess>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    rejected: Arc<AtomicUsize>,
}

fn open_gcm(key: [u8; 16], iv: &[u8; 32], aad: &[u8], sealed: &mut [u8]) -> Option<usize> {
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&iv[..12]);
    LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &key).ok()?)
        .open_in_place(Nonce::assume_unique_for_key(nonce), Aad::from(aad), sealed)
        .ok()
        .map(|p| p.len())
}

fn seal_gcm(key: [u8; 16], iv: &[u8; 32], plain: &[u8], out: &mut Vec<u8>) {
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&iv[..12]);
    let start = out.len();
    out.extend_from_slice(plain);
    let tag = LessSafeKey::new(UnboundKey::new(&AES_128_GCM, &key).expect("a 16-byte key"))
        .seal_in_place_separate_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::empty(),
            &mut out[start..],
        )
        .expect("a short head");
    out.extend_from_slice(tag.as_ref());
}

/// The AuthID's timestamp, when it is one of ours.
fn open_auth_id(cmd_key: &[u8; 16], auth_id: &[u8; 16]) -> Option<i64> {
    let key = kdf16(cmd_key, &[kdf::AUTH_ID_KEY]);
    let mut block = GenericArray::from(*auth_id);
    Aes128::new(&GenericArray::from(key)).decrypt_block(&mut block);
    let crc = u32::from_be_bytes([block[12], block[13], block[14], block[15]]);
    (crc32fast::hash(&block[..12]) == crc).then(|| {
        let mut time = [0u8; 8];
        time.copy_from_slice(&block[..8]);
        i64::from_be_bytes(time)
    })
}

struct Parsed {
    record: RecordedVmess,
    session: Session,
}

fn parse_head(plain: &[u8]) -> Option<Parsed> {
    let mut session = Session {
        body_iv: [0; 16],
        body_key: [0; 16],
        response_v: 0,
    };
    if *plain.first()? != 1 {
        return None;
    }
    session.body_iv.copy_from_slice(plain.get(1..17)?);
    session.body_key.copy_from_slice(plain.get(17..33)?);
    session.response_v = *plain.get(33)?;
    let options = *plain.get(34)?;
    let (padding, security) = (usize::from(plain.get(35)? >> 4), plain.get(35)? & 15);
    let command = *plain.get(37)?;
    let port = u16::from_be_bytes([*plain.get(38)?, *plain.get(39)?]);
    let atyp = *plain.get(40)?;
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = plain.get(41..45)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 45)
        }
        3 => {
            let b: [u8; 16] = plain.get(41..57)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 57)
        }
        2 => {
            let len = usize::from(*plain.get(41)?);
            (
                String::from_utf8_lossy(plain.get(42..42 + len)?).into_owned(),
                42 + len,
            )
        }
        _ => return None,
    };
    // padding, then the FNV-1a of everything before it
    let body = plain.get(..used + padding)?;
    let check: [u8; 4] = plain
        .get(used + padding..used + padding + 4)?
        .try_into()
        .ok()?;
    let fnv = body.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    });
    (plain.len() == used + padding + 4 && fnv == u32::from_be_bytes(check)).then_some(Parsed {
        record: RecordedVmess {
            command,
            options,
            security,
            padding,
            atyp,
            host,
            port,
            skew: 0,
            early: 0,
        },
        session,
    })
}

async fn serve(mut stream: BoxedStream, shared: Arc<Shared>) -> io::Result<()> {
    if shared.script.ws {
        stream = accept_bytes(stream, &shared.ws_seen).await?;
    }
    let cmd_key = header::cmd_key(&shared.script.uuid);
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let mut need = 16 + 18 + 8;
    let mut head_len = None;
    let (mut parsed, skew) = loop {
        if buf.len() < need {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&chunk[..n]);
            continue;
        }
        let auth_id: [u8; 16] = buf[..16].try_into().expect("16 bytes");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let reject = |shared: &Shared| shared.rejected.fetch_add(1, Ordering::SeqCst);
        let Some(time) = open_auth_id(&cmd_key, &auth_id).filter(|t| (t - now).abs() <= 120) else {
            // a real server drains and closes: nothing is ever answered
            reject(&shared);
            return Ok(());
        };
        let nonce: [u8; 8] = buf[34..42].try_into().expect("8 bytes");
        let path = |label: &'static [u8]| [label, &auth_id[..], &nonce[..]];
        let len = match head_len {
            Some(len) => len,
            None => {
                let mut sealed: [u8; 18] = buf[16..34].try_into().expect("18 bytes");
                let opened = open_gcm(
                    kdf16(&cmd_key, &path(kdf::HEADER_LEN_KEY)),
                    &kdf(&cmd_key, &path(kdf::HEADER_LEN_NONCE)),
                    &auth_id,
                    &mut sealed,
                );
                if opened != Some(2) {
                    reject(&shared);
                    return Ok(());
                }
                let len = usize::from(u16::from_be_bytes([sealed[0], sealed[1]]));
                head_len = Some(len);
                need = 42 + len + TAG;
                len
            }
        };
        if buf.len() < need {
            continue;
        }
        let mut sealed = buf[42..need].to_vec();
        let opened = open_gcm(
            kdf16(&cmd_key, &path(kdf::HEADER_KEY)),
            &kdf(&cmd_key, &path(kdf::HEADER_NONCE)),
            &auth_id,
            &mut sealed,
        );
        match opened.and_then(|n| (n == len).then(|| parse_head(&sealed[..n])).flatten()) {
            Some(parsed) => break (parsed, time - now),
            None => {
                reject(&shared);
                return Ok(());
            }
        }
    };
    parsed.record.skew = skew;
    parsed.record.early = buf.len() - need;
    shared
        .requests
        .lock()
        .expect("requests")
        .push(parsed.record.clone());
    let security = match parsed.record.security {
        3 => Security::Aes128Gcm,
        4 => Security::ChaCha20Poly1305,
        _ => return Ok(()),
    };
    let upstream_addr = match shared.script.connect_to {
        Some(addr) => addr,
        None => match parsed.record.host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, parsed.record.port),
            // never resolves: a name without `connect_to` is a dead end
            Err(_) => return stream.shutdown().await,
        },
    };
    let upstream = TcpStream::connect(upstream_addr).await?;
    let session = parsed.session;
    let (key, iv) = header::response_secrets(&session);
    let mut answer = Vec::new();
    let plain = [session.response_v, 0, 0, 0];
    seal_gcm(
        kdf16(&key, &[kdf::RESPONSE_LEN_KEY]),
        &kdf(&iv, &[kdf::RESPONSE_LEN_IV]),
        &(plain.len() as u16).to_be_bytes(),
        &mut answer,
    );
    seal_gcm(
        kdf16(&key, &[kdf::RESPONSE_KEY]),
        &kdf(&iv, &[kdf::RESPONSE_IV]),
        &plain,
        &mut answer,
    );
    let stream = crate::transport::prefixed::boxed(buf[need..].to_vec(), stream);
    let (mut from_client, mut to_client) = tokio::io::split(stream);
    let (mut from_upstream, mut to_upstream) = upstream.into_split();
    let mut up = ChunkCipher::new(security, &session.body_key, &session.body_iv);
    let mut down = ChunkCipher::new(security, &key, &iv);
    let upward = async {
        loop {
            let mut len = [0u8; 2];
            if from_client.read_exact(&mut len).await.is_err() {
                break;
            }
            let mut sealed = vec![0u8; up.open_len(len)];
            if from_client.read_exact(&mut sealed).await.is_err() {
                break;
            }
            match up.open(&mut sealed) {
                Some(0) | None => break,
                Some(n) => to_upstream.write_all(&sealed[..n]).await?,
            }
        }
        to_upstream.shutdown().await
    };
    let downward = async {
        to_client.write_all(&answer).await?;
        let mut buf = vec![0u8; 8192];
        loop {
            let n = from_upstream.read(&mut buf).await?;
            let mut out = Vec::new();
            down.seal(&buf[..n], &mut out);
            to_client.write_all(&out).await?;
            if n == 0 {
                return to_client.shutdown().await;
            }
        }
    };
    let _ = tokio::join!(upward, downward);
    Ok(())
}

impl FakeVmess {
    /// `tls`: speak TLS with this fixture's certificate first.
    pub async fn spawn(script: VmessScript, tls: Option<Arc<TlsFixture>>) -> FakeVmess {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests: Arc<Mutex<Vec<RecordedVmess>>> = Arc::default();
        let ws_seen: Arc<Mutex<Vec<RecordedWs>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            script,
            requests: requests.clone(),
            ws_seen: ws_seen.clone(),
            rejected: rejected.clone(),
        });
        let acceptor = tls.as_ref().map(|fixture| fixture.acceptor(false));
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let (shared, tls, acceptor) = (shared.clone(), tls.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match (&tls, &acceptor) {
                        (Some(fixture), Some(acceptor)) => {
                            match fixture.accept(acceptor, tcp).await {
                                Ok(stream) => stream,
                                Err(_) => return,
                            }
                        }
                        _ => Box::new(tcp),
                    };
                    let _ = serve(stream, shared).await;
                });
            }
        });
        FakeVmess {
            addr,
            requests,
            ws_seen,
            connections,
            rejected,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<RecordedVmess> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn ws_seen(&self) -> Vec<RecordedWs> {
        self.ws_seen.lock().expect("ws").clone()
    }

    /// TCP connections accepted so far (before TLS).
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Connections dropped without an answer (unknown id, stale timestamp, garbage).
    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`：模块列表加 `mod vmess;`，导出加 `pub use vmess::{FakeVmess, RecordedVmess, VmessScript};`，文件头的清单注释若列举了假服务端就补上这一个。

- [ ] **Step 4: 出站与它的用例**

`crates/rurge-proto/src/vmess/mod.rs`（整个替换）：

```rust
//! `vmess` outbound (manual: Policies › VMess): the AEAD handshake only,
//! optionally under TLS and / or a WebSocket. The sealed request head waits
//! in a `LazyHead` for the first payload; the body is chunked and sealed in
//! both directions (`stream`).
//!
//! The server never says why it refuses: a wrong id, or a clock more than
//! about two minutes off, both end as a connection closed without an answer.

pub(crate) mod chunk;
pub(crate) mod header;
pub(crate) mod kdf;
mod stream;
#[cfg(test)]
pub(crate) mod vectors;

use crate::addr::{AddrError, vmess_addr};
use crate::build::tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::ws::WsClient;
use crate::{BuildError, Outbound, OutboundError};
use header::{Security, Session};
use rurge_config::KeystoreItem;
use rurge_config::spec::{VmessCipher, VmessSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use stream::VmessStream;

/// No `Debug`: the command key is as good as the id.
pub struct VmessOutbound {
    name: String,
    stack: Stack,
    /// `MD5(id ‖ magic)`; the id itself is not kept.
    cmd_key: [u8; 16],
    security: Security,
}

fn random<const N: usize>() -> Result<[u8; N], OutboundError> {
    let mut out = [0u8; N];
    getrandom::fill(&mut out)
        .map_err(|_| OutboundError::Proxy("vmess: no randomness available".to_string()))?;
    Ok(out)
}

impl VmessOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &VmessSpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<VmessOutbound, BuildError> {
        // error texts carry no policy name (the registry's `build_one` and
        // the dry build both prefix it). No ALPN unless the policy asks for
        // one: a WebSocket below must not be negotiated into h2 (M2 design 4.5)
        let tls = tls_client(spec.tls.as_ref(), &server.host, &[], keystore, roots)?;
        let ws = spec
            .ws
            .as_ref()
            .map(|ws| WsClient::new(ws, &server, tls.is_some()))
            .transpose()?;
        Ok(VmessOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, tls, ws),
            cmd_key: header::cmd_key(spec.uuid.expose()),
            security: match spec.cipher {
                VmessCipher::Aes128Gcm => Security::Aes128Gcm,
                VmessCipher::ChaCha20Poly1305 => Security::ChaCha20Poly1305,
            },
        })
    }

    /// The sealed request head for `target`, and the secrets it announces.
    fn head(&self, target: &Target) -> Result<(Vec<u8>, Session), OutboundError> {
        let address = vmess_addr(target).map_err(|e| {
            OutboundError::Proxy(
                match e {
                    AddrError::Unsendable => "vmess: the host name cannot be sent to the server",
                    AddrError::TooLong => "vmess: the host name is longer than 255 bytes",
                }
                .to_string(),
            )
        })?;
        let session = Session {
            body_iv: random()?,
            body_key: random()?,
            response_v: random::<1>()?[0],
        };
        // the server accepts ±120 s; the reference client spreads its own
        // timestamps over ±30 s so that they say nothing about its clock
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let jitter = i64::from(u16::from_be_bytes(random()?) % 61) - 30;
        let auth_id = header::auth_id(&self.cmd_key, now + jitter, random()?);
        let padding: [u8; 16] = random()?;
        let padding = &padding[..usize::from(padding[15] % 16)];
        let plain = header::request_plain(&session, self.security, &address, padding);
        let head = header::seal_request(&self.cmd_key, &auth_id, &random()?, &plain);
        Ok((head, session))
    }
}

impl Outbound for VmessOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let (head, session) = self.head(target)?;
            // one budget for the connection, TLS and the WebSocket handshake;
            // the server's answer comes with its first payload, in the relay
            let transport = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            let lazy: BoxedStream = Box::new(LazyHead::new(transport, head));
            Ok(Box::new(VmessStream::new(lazy, session, self.security)) as BoxedStream)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeVmess, TlsFixture, VmessScript, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::vmess::read_vmess;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

    /// The outbound for `definition` (a `vmess, host, port, ...` line).
    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> VmessOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("V", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let read = read_vmess(&mut r, &[]);
        assert!(read.aead, "the line asks for the legacy handshake");
        assert!(!r.has_errors(), "{:?}", r.finish());
        VmessOutbound::new(
            "V",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            &[],
            roots,
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn roundtrip(stream: &mut BoxedStream, text: &[u8]) {
        // no explicit `flush()`: `write_all` alone must deliver
        stream.write_all(text).await.unwrap();
        let mut buf = vec![0u8; text.len()];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert_eq!(buf, text);
    }

    #[tokio::test]
    async fn plain_vmess_carries_the_head_with_the_first_payload() {
        let echo = echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"hello through vmess").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].command, seen[0].options, seen[0].security),
            (1, 0x05, 3),
            "TCP, ChunkStream + ChunkMasking, aes-128-gcm"
        );
        assert_eq!((seen[0].atyp, seen[0].host.as_str()), (1, "127.0.0.1"));
        assert_eq!(seen[0].port, echo.port());
        assert!(seen[0].early > 0, "the head went out alone");
        assert!(seen[0].skew.abs() <= 31, "{}", seen[0].skew);
        assert!(seen[0].padding < 16);
    }

    #[tokio::test]
    async fn chacha20_a_domain_target_tls_and_a_websocket() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeVmess::spawn(
            VmessScript {
                ws: true,
                connect_to: Some(echo),
                ..VmessScript::new(ID)
            },
            Some(fixture.clone()),
        )
        .await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, encrypt-method=chacha20-ietf-poly1305, tls=true, ws=true, ws-path=/v",
                fake.addr().port()
            ),
            fixture.roots(),
        );
        let name = Target::new(HostName::Domain("bücher.example".into()), 443);
        let mut stream = out
            .connect_tcp(&name, &ConnectOpts::default())
            .await
            .unwrap();
        roundtrip(&mut stream, b"over tls and a websocket").await;
        let seen = fake.requests();
        assert_eq!(seen[0].security, 4);
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (2, "xn--bcher-kva.example", 443),
            "a name travels as its A-labels, and the server resolves it"
        );
        assert_eq!(fake.ws_seen()[0].path, "/v");
        // the fixture offers h2 first: an ALPN of ours would have picked it
        assert_eq!(fixture.seen()[0].alpn, None);
    }

    #[tokio::test]
    async fn a_wrong_id_shows_as_a_connection_closed_without_an_answer() {
        let echo = echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        // the connection itself succeeds: the server only ever answers a request it accepts
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        let err = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut buf))
            .await
            .expect("the server closes within the bound")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "vmess: the server closed the connection without answering"
        );
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
        // nothing derived from the id is in the text
        assert!(!err.to_string().contains("0233"));
    }

    #[tokio::test]
    async fn a_server_that_speaks_first_is_heard() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let banner = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            s.write_all(b"220 ready\r\n").await.unwrap();
            let mut rest = Vec::new();
            let _ = s.read_to_end(&mut rest).await;
        });
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(banner), &ConnectOpts::default())
            .await
            .unwrap();
        let mut buf = [0u8; 11];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the banner arrives within the bound")
            .unwrap();
        assert_eq!(&buf, b"220 ready\r\n");
        assert_eq!(fake.requests()[0].early, 0, "nothing was written: the head went alone");
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let bad = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = out
            .connect_tcp(&bad, &ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(
            err.to_string(),
            "vmess: the host name cannot be sent to the server"
        );
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn a_silent_server_is_a_timeout_of_the_whole_ladder() {
        // accepts and never speaks TLS
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = listener.accept().await {
                held.push(s);
            }
        });
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, tls=true",
                addr.port()
            ),
            fixture.roots(),
        );
        let opts = ConnectOpts {
            timeout: Duration::from_millis(300),
        };
        let err = out
            .connect_tcp(&target(addr), &opts)
            .await
            .err()
            .expect("times out");
        assert!(matches!(err, OutboundError::Timeout), "{err}");
    }
}
```

Run: `cargo test -p rurge-proto vmess`
Expected: Task 2 的 12 个用例 + 本任务 6 个出站用例全部通过。`plain_vmess_carries_the_head_with_the_first_payload` 里 `early > 0` 是确定的（用例先写后读，第一次 `poll_write` 早于任何 `poll_read`）；经引擎的端到端用例里它取决于时序，那里不断言它。

- [ ] **Step 5: 按转发循环的方式再压一遍**

在 `vmess/mod.rs` 的 `mod tests` 里再加五条：按转发循环的方式压两条、空写一条、失败路径两条（分块被篡改 / 被截断 / 长度装不下 tag / 应答头是垃圾都是**错误**而不是流结束；只有在分块边界上断开才算流结束）。它们就是计划期在临时工程里跑过的那几条，搬进来是为了让以后的改动继续被它们看着：

```rust
    /// The engine's relay, reduced to what matters to a stream under test:
    /// `read` → `write_all` with no flush, `shutdown` at EOF, both directions
    /// polled from one task through `tokio::io::split`.
    async fn copy_half<R, W>(mut reader: R, mut writer: W) -> std::io::Result<u64>
    where
        R: tokio::io::AsyncRead + Unpin,
        W: tokio::io::AsyncWrite + Unpin,
    {
        let mut buf = vec![0u8; 8 * 1024];
        let mut total = 0;
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                let _ = writer.shutdown().await;
                return Ok(total);
            }
            writer.write_all(&buf[..n]).await?;
            total += n as u64;
        }
    }

    #[tokio::test]
    async fn a_relay_moves_a_megabyte_each_way_with_either_cipher() {
        tokio::time::timeout(Duration::from_secs(120), async {
            for cipher in ["aes-128-gcm", "chacha20-ietf-poly1305"] {
                let echo = echo_server().await;
                let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
                let out = outbound(
                    &format!(
                        "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true, encrypt-method={cipher}",
                        fake.addr().port()
                    ),
                    no_roots(),
                );
                let upstream = out
                    .connect_tcp(&target(echo), &ConnectOpts::default())
                    .await
                    .unwrap();
                let (mut app, near) = tokio::io::duplex(64 * 1024);
                let relay = tokio::spawn(async move {
                    let (cr, cw) = tokio::io::split(near);
                    let (ur, uw) = tokio::io::split(upstream);
                    tokio::join!(copy_half(cr, uw), copy_half(ur, cw))
                });
                let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
                let (mut app_r, mut app_w) = tokio::io::split(&mut app);
                let mut back = vec![0u8; payload.len()];
                tokio::join!(
                    async { app_w.write_all(&payload).await.unwrap() },
                    async { app_r.read_exact(&mut back).await.unwrap() }
                );
                assert!(back == payload, "{cipher}: the echo differs");
                drop((app_r, app_w));
                drop(app); // the client goes away: the relay must wind down by itself
                let (up, down) = relay.await.unwrap();
                assert_eq!((up.unwrap(), down.unwrap()), (1_000_000, 1_000_000), "{cipher}");
            }
        })
        .await
        .expect("bounded");
    }

    /// A transport that takes seven bytes at a time: every write parks again
    /// and again, with the read half polled in between from the same task.
    #[tokio::test]
    async fn parked_writes_are_finished_exactly_once() {
        use crate::vmess::chunk::ChunkCipher;
        let s = vectors::session();
        let (near, far) = tokio::io::duplex(7);
        let stream: BoxedStream = Box::new(VmessStream::new(
            Box::new(near),
            vectors::session(),
            Security::Aes128Gcm,
        ));
        let (mut far_r, mut far_w) = tokio::io::split(far);
        let server = tokio::spawn(async move {
            // answer first, then read what the client sealed
            let (key, iv) = header::response_secrets(&s);
            far_w
                .write_all(&vectors::hex(vectors::RESPONSE_SEALED))
                .await
                .unwrap();
            let mut down = ChunkCipher::new(Security::Aes128Gcm, &key, &iv);
            let mut out = Vec::new();
            down.seal(b"pong", &mut out);
            far_w.write_all(&out).await.unwrap();
            let mut up = ChunkCipher::new(Security::Aes128Gcm, &s.body_key, &s.body_iv);
            let mut got = Vec::new();
            loop {
                let mut len = [0u8; 2];
                far_r.read_exact(&mut len).await.unwrap();
                let mut sealed = vec![0u8; up.open_len(len)];
                far_r.read_exact(&mut sealed).await.unwrap();
                let n = up
                    .open(&mut sealed)
                    .expect("authentic: nothing was sent twice or out of order");
                if n == 0 {
                    return got;
                }
                got.extend_from_slice(&sealed[..n]);
            }
        });
        let (mut r, mut w) = tokio::io::split(stream);
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 253) as u8).collect();
        let write = async {
            for piece in payload.chunks(3000) {
                w.write_all(piece).await.unwrap();
            }
            w.shutdown().await.unwrap();
        };
        let read = async {
            let mut buf = [0u8; 4];
            r.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"pong");
        };
        tokio::time::timeout(Duration::from_secs(30), async { tokio::join!(write, read) })
            .await
            .expect("bounded");
        assert!(server.await.unwrap() == payload);
    }

    #[tokio::test]
    async fn an_empty_write_does_not_end_the_stream() {
        let echo = echo_server().await;
        let fake = FakeVmess::spawn(VmessScript::new(ID), None).await;
        let out = outbound(
            &format!(
                "vmess, 127.0.0.1, {}, username={ID}, vmess-aead=true",
                fake.addr().port()
            ),
            no_roots(),
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(stream.write(&[]).await.unwrap(), 0);
        roundtrip(&mut stream, b"after").await;
    }

    /// What the client reads when the server's bytes are `wire`.
    async fn read_error(wire: Vec<u8>) -> std::io::Error {
        let (near, mut far) = tokio::io::duplex(4096);
        let mut stream = VmessStream::new(Box::new(near), vectors::session(), Security::Aes128Gcm);
        far.write_all(&wire).await.unwrap();
        drop(far); // nothing more is coming
        let mut sink = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut sink))
            .await
            .expect("bounded")
            .expect_err("the stream must not end cleanly")
    }

    #[tokio::test]
    async fn a_damaged_or_truncated_response_is_an_error_not_an_end_of_stream() {
        let head = vectors::hex(vectors::RESPONSE_SEALED);
        let chunks = vectors::hex(vectors::RESPONSE_CHUNKS_AES);
        let with = |tail: &[u8]| [&head[..], tail].concat();
        // not a VMess answer at all (or our id is wrong and this is someone else's)
        let garbage = read_error(vec![0x55; 64]).await;
        assert_eq!(
            garbage.to_string(),
            "vmess: the response cannot be authenticated"
        );
        // one flipped bit inside the first chunk
        let mut flipped = chunks.clone();
        flipped[5] ^= 1;
        let err = read_error(with(&flipped)).await;
        assert_eq!(err.to_string(), "vmess: a chunk cannot be authenticated");
        // the connection ends inside a chunk
        let err = read_error(with(&chunks[..10])).await;
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert_eq!(
            err.to_string(),
            "vmess: the connection ended in the middle of a chunk"
        );
        // and inside the response head
        let err = read_error(head[..30].to_vec()).await;
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        // a length that cannot even hold the tag: the vector's first two
        // bytes are `mask ^ 21` (`world` plus its tag), so `^ 21 ^ 15` makes
        // the same mask announce fifteen bytes
        let announced = u16::from_be_bytes([chunks[0], chunks[1]]) ^ 21 ^ 15;
        let short = announced.to_be_bytes();
        let err = read_error(with(&short)).await;
        assert_eq!(err.to_string(), "vmess: a chunk shorter than its tag");
    }

    #[tokio::test]
    async fn a_close_between_chunks_ends_the_stream_cleanly() {
        let head = vectors::hex(vectors::RESPONSE_SEALED);
        let chunks = vectors::hex(vectors::RESPONSE_CHUNKS_AES);
        // `world` alone, without the courtesy end-of-stream chunk
        let first = &chunks[..2 + 5 + 16];
        let (near, mut far) = tokio::io::duplex(4096);
        let mut stream = VmessStream::new(Box::new(near), vectors::session(), Security::Aes128Gcm);
        far.write_all(&[&head[..], first].concat()).await.unwrap();
        drop(far);
        let mut got = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut got))
            .await
            .expect("bounded")
            .unwrap();
        assert_eq!(got, b"world");
    }
```

Run: `cargo test -p rurge-proto vmess -- --test-threads=4`，连跑 5 遍。
Expected: 每遍都全部通过（计划期在临时工程里连跑 15 遍无抖动）。某一遍失败就是真缺陷：不要加重试、不要放宽上界，把失败的输出写进报告。

- [ ] **Step 6: 门禁与提交**

跑「Global Constraints」里的门禁（确认 Task 2 的三个 `#[allow(dead_code)]` 已经不在了：`grep -n 'allow(dead_code)' crates/rurge-proto/src/vmess/mod.rs` 无输出）。然后：

```bash
git add -A
git commit -m "feat(proto): vmess 出站——分块流（写穿、挂起的分块只写一次）、LazyHead 合并请求头、可叠 tls / ws；回环假服务端 FakeVmess"
```

---

### Task 4: AnyTLS 的帧与 padding 方案

纯函数，不碰网络。

**Files:**
- Modify: `crates/rurge-proto/src/lib.rs`（`pub mod anytls;`）
- Create: `crates/rurge-proto/src/anytls/mod.rs`（本任务只有模块声明）
- Create: `crates/rurge-proto/src/anytls/{frame,padding}.rs`

**Interfaces:**
- Consumes: `md-5`（Task 2 已加）。
- Produces（全部 `pub(crate)`）：
  - `anytls::frame::{WASTE, SYN, PSH, FIN, SETTINGS, ALERT, UPDATE_PADDING_SCHEME, SYNACK, HEART_REQUEST, HEART_RESPONSE, SERVER_SETTINGS, HEADER, MAX_DATA, push(&mut Vec<u8>, command, stream, data), frame(command, stream, data) -> Vec<u8>, parse_header(&[u8; 7]) -> (u8, u32, usize)}`。
  - `anytls::padding::{DEFAULT_SCHEME, Scheme::{parse(&[u8]) -> Option<Scheme>, default_scheme(), md5(), stop(), pieces(packet, &mut dyn FnMut(u32) -> u32) -> Vec<Piece>, auth_padding(..) -> usize}, Piece::{Size(usize), Check}, shape(&[Piece], &[u8]) -> Vec<Vec<u8>>}`。

- [ ] **Step 1: 模块骨架**

`crates/rurge-proto/src/lib.rs`：在 `mod addr;` 之后加 `pub mod anytls;`（按字母序）。

`crates/rurge-proto/src/anytls/mod.rs`（两个 `allow` 是**临时的**，Task 5 的第一步就删掉，理由同 Task 2）：

```rust
//! `anytls` outbound (manual: Policies › AnyTLS). Frames and padding live
//! here; the session layer and the outbound join them in the next commit.

#[allow(dead_code)] // until the session layer uses it (next commit)
pub(crate) mod frame;
#[allow(dead_code)] // until the session layer uses it (next commit)
pub(crate) mod padding;
```

- [ ] **Step 2: 帧**

`crates/rurge-proto/src/anytls/frame.rs`：

```rust
//! The AnyTLS session layer's frame: `command(1) ‖ stream id(4, BE) ‖
//! length(2, BE) ‖ data` (anytls-go `docs/protocol.md`).

pub(crate) const WASTE: u8 = 0;
pub(crate) const SYN: u8 = 1;
pub(crate) const PSH: u8 = 2;
pub(crate) const FIN: u8 = 3;
pub(crate) const SETTINGS: u8 = 4;
pub(crate) const ALERT: u8 = 5;
pub(crate) const UPDATE_PADDING_SCHEME: u8 = 6;
// since protocol version 2
pub(crate) const SYNACK: u8 = 7;
pub(crate) const HEART_REQUEST: u8 = 8;
pub(crate) const HEART_RESPONSE: u8 = 9;
pub(crate) const SERVER_SETTINGS: u8 = 10;

pub(crate) const HEADER: usize = 7;
/// What the two-byte length can say.
pub(crate) const MAX_DATA: usize = 65535;

/// Appends one frame. `data` is at most `MAX_DATA` bytes.
pub(crate) fn push(out: &mut Vec<u8>, command: u8, stream: u32, data: &[u8]) {
    debug_assert!(data.len() <= MAX_DATA);
    out.push(command);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

pub(crate) fn frame(command: u8, stream: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + data.len());
    push(&mut out, command, stream, data);
    out
}

/// `(command, stream id, data length)`.
pub(crate) fn parse_header(header: &[u8; HEADER]) -> (u8, u32, usize) {
    (
        header[0],
        u32::from_be_bytes([header[1], header[2], header[3], header[4]]),
        usize::from(u16::from_be_bytes([header[5], header[6]])),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_seven_bytes_of_header_and_the_data() {
        let f = frame(PSH, 0x0102_0304, b"hi");
        assert_eq!(f, [2, 1, 2, 3, 4, 0, 2, b'h', b'i']);
        let header: [u8; HEADER] = f[..HEADER].try_into().unwrap();
        assert_eq!(parse_header(&header), (PSH, 0x0102_0304, 2));
        assert_eq!(frame(SYN, 1, &[]), [1, 0, 0, 0, 1, 0, 0]);
        let big = frame(PSH, 1, &vec![0u8; MAX_DATA]);
        assert_eq!(&big[5..7], [0xff, 0xff]);
    }
}
```

Run: `cargo test -p rurge-proto anytls::frame`
Expected: 1 个用例通过。

- [ ] **Step 3: padding 方案（先让用例失败）**

`crates/rurge-proto/src/anytls/padding.rs`。先贴整个文件但把 `pub(crate) fn shape` 的函数体换成 `todo!()`：

Run: `cargo test -p rurge-proto anytls::padding`
Expected: `a_write_is_cut_and_padded_the_way_the_reference_does_it` 与 `what_is_cut_still_carries_every_payload_byte_in_order` 失败（`not yet implemented`），另外两个通过。

再换回真正的函数体：

```rust
//! AnyTLS padding schemes (anytls-go `proxy/padding/padding.go` and
//! `Session.writeConn`): for the first `stop` writes of a session, each write
//! is cut into TLS records of the sizes the scheme lists, and padded with
//! `cmdWaste` frames where the payload runs out.

use super::frame::{self, HEADER};
use md5::{Digest, Md5};
use std::collections::HashMap;

pub(crate) const DEFAULT_SCHEME: &str = "stop=8\n0=30-30\n1=100-400\n2=400-500,c,500-1000,c,500-1000,c,500-1000,c,500-1000\n3=9-9,500-1000\n4=500-1000\n5=500-1000\n6=500-1000\n7=500-1000";

/// Bounds on a scheme a server may push (M2 design 6.3): the text, how far
/// padding may reach into a session, how many records one write may become,
/// and the size of one record (a TLS record holds 2^14 bytes).
const MAX_TEXT: usize = 8192;
const MAX_STOP: u32 = 256;
const MAX_ENTRIES: usize = 64;
const MAX_SIZE: u32 = 16384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A record of `min ..= max - 1` bytes (`min` when the two are equal).
    Size { min: u32, max: u32 },
    /// `c`: stop here when the payload is used up.
    Check,
}

/// One drawn size, or the check mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Piece {
    Size(usize),
    Check,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Scheme {
    md5: String,
    stop: u32,
    packets: HashMap<u32, Vec<Entry>>,
}

fn parse_entry(text: &str) -> Option<Entry> {
    if text == "c" {
        return Some(Entry::Check);
    }
    let (a, b) = text.split_once('-')?;
    let (a, b): (u32, u32) = (a.parse().ok()?, b.parse().ok()?);
    let (min, max) = (a.min(b), a.max(b));
    (min >= 1 && max <= MAX_SIZE).then_some(Entry::Size { min, max })
}

impl Scheme {
    /// `None` unless every line that matters is well-formed and within the
    /// bounds. Keys that are neither `stop` nor a packet number are ignored.
    pub(crate) fn parse(raw: &[u8]) -> Option<Scheme> {
        if raw.len() > MAX_TEXT {
            return None;
        }
        let text = std::str::from_utf8(raw).ok()?;
        let mut stop = None;
        let mut packets = HashMap::new();
        for line in text.split('\n') {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            if key == "stop" {
                stop = Some(value.parse::<u32>().ok().filter(|s| *s <= MAX_STOP)?);
            } else if let Ok(packet) = key.parse::<u32>() {
                let entries: Vec<Entry> =
                    value.split(',').map(parse_entry).collect::<Option<_>>()?;
                if entries.len() > MAX_ENTRIES {
                    return None;
                }
                packets.insert(packet, entries);
            }
        }
        Some(Scheme {
            md5: Md5::digest(raw)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            stop: stop?,
            packets,
        })
    }

    pub(crate) fn default_scheme() -> Scheme {
        Scheme::parse(DEFAULT_SCHEME.as_bytes()).expect("the built-in scheme is valid")
    }

    /// Lower-case hex of the MD5 of the scheme's text, as `cmdSettings` reports it.
    pub(crate) fn md5(&self) -> &str {
        &self.md5
    }

    pub(crate) fn stop(&self) -> u32 {
        self.stop
    }

    /// The sizes for the `packet`-th write, drawn with `pick(n)` ∈ `0..n`.
    pub(crate) fn pieces(&self, packet: u32, pick: &mut dyn FnMut(u32) -> u32) -> Vec<Piece> {
        let Some(entries) = self.packets.get(&packet) else {
            return Vec::new();
        };
        entries
            .iter()
            .map(|e| match *e {
                Entry::Check => Piece::Check,
                Entry::Size { min, max } if min == max => Piece::Size(min as usize),
                Entry::Size { min, max } => Piece::Size((min + pick(max - min)) as usize),
            })
            .collect()
    }

    /// The padding that rides with the authentication (packet 0).
    pub(crate) fn auth_padding(&self, pick: &mut dyn FnMut(u32) -> u32) -> usize {
        match self.pieces(0, pick).first() {
            Some(Piece::Size(n)) => *n,
            _ => 0,
        }
    }
}

fn waste(len: usize) -> Vec<u8> {
    frame::frame(frame::WASTE, 0, &vec![0u8; len])
}

/// The records one write becomes: `payload` cut to the listed sizes, a
/// `cmdWaste` frame filling what the payload leaves of a size, and whatever
/// payload is left after the list in one last record.
pub(crate) fn shape(pieces: &[Piece], mut payload: &[u8]) -> Vec<Vec<u8>> {
    let mut records = Vec::new();
    for piece in pieces {
        let size = match piece {
            Piece::Check if payload.is_empty() => break,
            Piece::Check => continue,
            Piece::Size(size) => *size,
        };
        if payload.len() > size {
            records.push(payload[..size].to_vec());
            payload = &payload[size..];
        } else if !payload.is_empty() {
            let mut record = payload.to_vec();
            // the waste frame's own header counts towards the size
            if let Some(fill) = size.checked_sub(payload.len() + HEADER).filter(|n| *n > 0) {
                record.extend_from_slice(&waste(fill));
            }
            records.push(record);
            payload = &[];
        } else {
            records.push(waste(size));
        }
    }
    if !payload.is_empty() {
        records.push(payload.to_vec());
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    fn low(_: u32) -> u32 {
        0
    }

    #[test]
    fn the_default_scheme_and_its_md5() {
        let s = Scheme::default_scheme();
        assert_eq!(s.md5(), "75cff2ad89aadf5e257059ee571ebe11");
        assert_eq!(s.stop(), 8);
        assert_eq!(s.auth_padding(&mut low), 30);
        assert_eq!(s.pieces(3, &mut low), [Piece::Size(9), Piece::Size(500)]);
        assert_eq!(s.pieces(2, &mut low).len(), 9);
        assert_eq!(s.pieces(9, &mut low), []);
        // the upper bound is exclusive, as in the reference
        assert_eq!(s.pieces(1, &mut |n| n - 1), [Piece::Size(399)]);
    }

    #[test]
    fn a_pushed_scheme_is_validated_within_bounds() {
        assert!(Scheme::parse(b"stop=2\n0=10-20\n1=5-5,c,7-9\nfuture-key=1").is_some());
        for bad in [
            &b"0=10-20"[..],      // no stop
            b"stop=x",            // not a number
            b"stop=257",          // reaches too far
            b"stop=2\n1=0-5",     // a size below one
            b"stop=2\n1=1-16385", // more than a TLS record
            b"stop=2\n1=5",       // not a range
            b"stop=2\n1=a-b",
            b"stop=2\n1=\xff", // not UTF-8
        ] {
            assert_eq!(Scheme::parse(bad), None, "{}", String::from_utf8_lossy(bad));
        }
        let many = format!("stop=2\n1={}", vec!["1-2"; 65].join(","));
        assert_eq!(Scheme::parse(many.as_bytes()), None);
        assert_eq!(Scheme::parse(&vec![b'a'; 8193]), None);
        // reversed bounds are put in order, as the reference does
        let s = Scheme::parse(b"stop=2\n1=9-3").unwrap();
        assert_eq!(s.pieces(1, &mut low), [Piece::Size(3)]);
    }

    #[test]
    fn a_write_is_cut_and_padded_the_way_the_reference_does_it() {
        let payload = vec![1u8; 100];
        // more payload than the size: a record of exactly that size, the rest follows
        assert_eq!(
            shape(&[Piece::Size(30)], &payload)
                .iter()
                .map(Vec::len)
                .collect::<Vec<_>>(),
            [30, 70]
        );
        // the payload ends inside a size: padded up to it with a waste frame
        let records = shape(&[Piece::Size(30), Piece::Size(200)], &payload);
        assert_eq!(records.iter().map(Vec::len).collect::<Vec<_>>(), [30, 200]);
        assert_eq!(&records[1][..70], &payload[30..]);
        assert_eq!(&records[1][70..77], [frame::WASTE, 0, 0, 0, 0, 0, 123]);
        // too little room for a waste header: the payload goes as it is
        assert_eq!(shape(&[Piece::Size(104)], &payload)[0].len(), 100);
        // nothing left: a record of pure padding, its data as long as the size
        let records = shape(&[Piece::Size(100), Piece::Size(50)], &payload);
        assert_eq!(records[1].len(), HEADER + 50);
        assert_eq!(&records[1][..HEADER], [frame::WASTE, 0, 0, 0, 0, 0, 50]);
        // the check mark stops the padding once the payload is out …
        let records = shape(&[Piece::Size(100), Piece::Check, Piece::Size(50)], &payload);
        assert_eq!(records.len(), 1);
        // … and is skipped while there is payload left
        let records = shape(&[Piece::Size(60), Piece::Check, Piece::Size(60)], &payload);
        assert_eq!(records.iter().map(Vec::len).collect::<Vec<_>>(), [60, 60]);
        assert_eq!(shape(&[], &payload), std::slice::from_ref(&payload));
        assert!(shape(&[Piece::Check], &[]).is_empty());
    }

    #[test]
    fn what_is_cut_still_carries_every_payload_byte_in_order() {
        let payload: Vec<u8> = (0..=255u8).cycle().take(3000).collect();
        let s = Scheme::default_scheme();
        for packet in 0..8 {
            let records = shape(&s.pieces(packet, &mut |n| n / 2), &payload);
            // drop the waste frames: what is left is the payload
            let mut seen = Vec::new();
            for r in &records {
                let cut = r.len().min(payload.len() - seen.len());
                seen.extend_from_slice(&r[..cut]);
            }
            assert!(seen == payload, "packet {packet}");
        }
    }
}
```

Run: `cargo test -p rurge-proto anytls::padding`
Expected: 4 个用例通过。`shape` 的四个分支与参考实现 `Session.writeConn` 逐一对应（负载比这一档长 → 恰好切下这么多；负载在这一档里用完 → 用一个 `cmdWaste` 帧补到这一档，**帧头的 7 字节算在档位里**，补不下帧头就原样发；负载已经没了 → 整条都是 padding，**数据长度等于档位**，所以这条记录是档位 + 7 字节；`c` → 负载用完就停）。不要"顺手"统一这两种算法：那就和参考实现的字节数对不上了。

- [ ] **Step 4: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat(proto): AnyTLS 的会话层帧与 padding 方案（解析、有界校验、md5、按参考实现切分 / 补足）"
```

---

### Task 5: AnyTLS 的会话层、空闲池、出站与回环假服务端

**Files:**
- Modify: `crates/rurge-proto/Cargo.toml`（`tokio-util`）
- Create: `crates/rurge-proto/src/task.rs`；Modify: `crates/rurge-proto/src/lib.rs`（`mod task;`）、`crates/rurge-proto/src/testing/mod.rs`（`AbortOnDrop` 改为再导出）
- Create: `crates/rurge-proto/src/anytls/{session,pool}.rs`
- Modify: `crates/rurge-proto/src/anytls/mod.rs`（整个替换：出站）
- Create: `crates/rurge-proto/src/testing/anytls.rs`；Modify: `crates/rurge-proto/src/testing/mod.rs`（`mod anytls;` 与导出）

**Interfaces:**
- Consumes: Task 1 的 `AnyTlsSpec` / `read_anytls` / `Secret`；Task 4 的帧与 padding；M2a 的 `transport::Stack`、`build::tls_client`、`addr::socks_addr`、`outbound::untrusted_text`、`testing::TlsFixture`。
- Produces:
  - `rurge_proto::anytls::AnyTlsOutbound::new(name: &str, server: Target, spec: &AnyTlsSpec, keystore, roots, connector) -> Result<AnyTlsOutbound, BuildError>`，实现 `Outbound`。
  - `rurge_proto::testing::{FakeAnyTls, AnyTlsScript, RecordedStream}`：`AnyTlsScript { password, connect_to, scheme, alert, refuse, heartbeat, v1 }`（`Default`）；`FakeAnyTls::spawn(script, fixture: Arc<TlsFixture>)`；`addr()` `kick()` `sessions()` `rejected()` `streams()` `settings()` `heart_responses()` `fins()` `waste()`；`RecordedStream { session, sid, atyp, host, port }`。
  - `crate::task::AbortOnDrop`（`pub(crate)`）。

- [ ] **Step 1: 删掉 Task 4 的两个临时 `allow`；把 `AbortOnDrop` 挪进生产代码；加 `tokio-util`**

`crates/rurge-proto/Cargo.toml` 的 `[dependencies]` 加 `tokio-util.workspace = true`（工作区已有 0.7，`PollSender` 在 `tokio_util::sync` 里，不需要额外 feature；`Cargo.lock` 不会多出条目）。

`crates/rurge-proto/src/task.rs`：

```rust
//! A background task that ends with its owner.

/// Aborts the task it holds when dropped.
pub(crate) struct AbortOnDrop(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}
```

`crates/rurge-proto/src/lib.rs`：在 `pub mod socks5;` 之后加 `mod task;`。

`crates/rurge-proto/src/testing/mod.rs`：删掉那里的 `AbortOnDrop` 结构体与它的 `impl Drop`，换成一行 `pub(crate) use crate::task::AbortOnDrop;`（假服务端们继续写 `super::AbortOnDrop`）。

Run: `cargo test -p rurge-proto testing`
Expected: 全部通过。

- [ ] **Step 2: 空闲池**

`crates/rurge-proto/src/anytls/pool.rs`：

```rust
//! Idle AnyTLS sessions (anytls-go `proxy/session/client.go`): the newest one
//! is reused first, and one that has idled for a minute is closed.

use super::session::Session;
use crate::task::AbortOnDrop;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// The protocol document's suggestion: look every 30 s, close after 60 s.
pub(crate) const REAP_EVERY: Duration = Duration::from_secs(30);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
pub(crate) struct Pool {
    idle: Mutex<Vec<(Session, Instant)>>,
}

impl Pool {
    pub(crate) fn put(&self, session: Session) {
        self.idle
            .lock()
            .expect("pool")
            .push((session, Instant::now()));
    }

    /// The live session with the highest sequence number; dead ones found on
    /// the way are dropped.
    pub(crate) fn take(&self) -> Option<Session> {
        let mut idle = self.idle.lock().expect("pool");
        idle.retain(|(s, _)| !s.is_closed());
        let newest = (0..idle.len()).max_by_key(|i| idle[*i].0.seq())?;
        Some(idle.swap_remove(newest).0)
    }

    pub(crate) fn reap(&self, now: Instant) {
        self.idle
            .lock()
            .expect("pool")
            .retain(|(s, since)| !s.is_closed() && now.duration_since(*since) < IDLE_TIMEOUT);
    }

    pub(crate) fn len(&self) -> usize {
        self.idle.lock().expect("pool").len()
    }
}

/// Reaps `pool` until it is gone. Holds it weakly: the pool dies with its outbound.
pub(crate) fn spawn_reaper(pool: &Arc<Pool>) -> AbortOnDrop {
    let pool = Arc::downgrade(pool);
    AbortOnDrop(tokio::spawn(async move {
        let mut tick = tokio::time::interval(REAP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(pool) = pool.upgrade() else {
                return;
            };
            pool.reap(Instant::now());
        }
    }))
}
```

- [ ] **Step 3: 会话任务与流句柄**

`crates/rurge-proto/src/anytls/session.rs`：

```rust
//! One AnyTLS session: a task that owns the TLS connection, and the stream
//! handle that talks to it over two bounded queues.
//!
//! As in the reference client, a session carries one stream at a time; when
//! the stream is over the session goes back to the pool. The task keeps
//! reading while the session idles, so a heartbeat is answered and a closed
//! connection is noticed before anyone tries to reuse it.
//!
//! The task flushes after every batch of writes, so a stream's write is on
//! its way once it is queued: nothing here waits for a `flush` the relay
//! never calls. AnyTLS has no half-close: `shutdown` sends `cmdFIN`, which
//! ends the stream in both directions (sing-box does the same).

use super::frame::{self, HEADER, MAX_DATA};
use super::padding::{Scheme, shape};
use super::pool::Pool;
use crate::outbound::untrusted_text;
use crate::task::AbortOnDrop;
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker, ready};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf, ReadHalf, WriteHalf};
use tokio::sync::mpsc;
use tokio_util::sync::PollSender;

/// Writes waiting for the task, and frames waiting for the stream's reader.
const QUEUE: usize = 8;
const CLOSED: &str = "anytls: the session is closed";

/// The scheme of an outbound: shared by its sessions, replaced when the
/// server pushes a new one.
pub(crate) type SchemeCell = Arc<Mutex<Arc<Scheme>>>;

enum End {
    /// The server's `cmdFIN`.
    Fin,
    /// A refused stream (`cmdSYNACK` with a text) or a dead session.
    Error(String),
}

#[derive(Default)]
struct StreamState {
    end: Mutex<Option<End>>,
}

impl StreamState {
    fn set(&self, end: End) {
        self.end.lock().expect("stream state").get_or_insert(end);
    }

    fn finished_by_peer(&self) -> bool {
        matches!(*self.end.lock().expect("stream state"), Some(End::Fin))
    }

    fn is_over(&self) -> bool {
        self.end.lock().expect("stream state").is_some()
    }

    fn error(&self) -> Option<String> {
        match &*self.end.lock().expect("stream state") {
            Some(End::Error(text)) => Some(text.clone()),
            _ => None,
        }
    }
}

/// The stream the session is carrying right now.
struct Slot {
    sid: u32,
    incoming: mpsc::Sender<Vec<u8>>,
    state: Arc<StreamState>,
}

#[derive(Default)]
struct Shared {
    closed: AtomicBool,
    slot: Mutex<Option<Slot>>,
}

impl Shared {
    /// Data for a stream that is no longer there is dropped, as the
    /// reference does; a slow reader holds the whole session back, which is
    /// the back-pressure (there is only this one stream).
    async fn deliver(&self, sid: u32, data: Vec<u8>) {
        let incoming = match &*self.slot.lock().expect("slot") {
            Some(slot) if slot.sid == sid => slot.incoming.clone(),
            _ => return,
        };
        let _ = incoming.send(data).await;
    }

    fn finish(&self, sid: u32, end: End) {
        let mut slot = self.slot.lock().expect("slot");
        if slot.as_ref().is_some_and(|s| s.sid == sid)
            && let Some(slot) = slot.take()
        {
            slot.state.set(end);
        }
    }

    /// The stream let go of the session: late frames for it are dropped.
    fn release(&self, sid: u32) {
        let mut slot = self.slot.lock().expect("slot");
        if slot.as_ref().is_some_and(|s| s.sid == sid) {
            *slot = None;
        }
    }

    fn close(&self, why: String) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(slot) = self.slot.lock().expect("slot").take() {
            slot.state.set(End::Error(why));
        }
    }
}

/// A number in `0..n` (`n` ≥ 1); 0 when the system has no randomness to give.
pub(crate) fn pick(n: u32) -> u32 {
    let mut bytes = [0u8; 4];
    match getrandom::fill(&mut bytes) {
        Ok(()) => u32::from_be_bytes(bytes) % n,
        Err(_) => 0,
    }
}

async fn read_loop(
    mut io: ReadHalf<BoxedStream>,
    shared: &Shared,
    replies: &mpsc::Sender<Vec<u8>>,
    scheme: &SchemeCell,
) -> String {
    loop {
        let mut header = [0u8; HEADER];
        if io.read_exact(&mut header).await.is_err() {
            return CLOSED.to_string();
        }
        let (command, sid, len) = frame::parse_header(&header);
        // at most 65535 bytes: the length field cannot say more
        let mut data = vec![0u8; len];
        if io.read_exact(&mut data).await.is_err() {
            return CLOSED.to_string();
        }
        match command {
            frame::PSH => shared.deliver(sid, data).await,
            frame::FIN => shared.finish(sid, End::Fin),
            frame::SYNACK if !data.is_empty() => {
                let text = untrusted_text(&String::from_utf8_lossy(&data), 256);
                shared.finish(sid, End::Error(format!("anytls: {text}")));
            }
            frame::ALERT => {
                let text = untrusted_text(&String::from_utf8_lossy(&data), 256);
                return format!("anytls: the server sent an alert: {text}");
            }
            frame::UPDATE_PADDING_SCHEME => match Scheme::parse(&data) {
                Some(new) => *scheme.lock().expect("scheme") = Arc::new(new),
                None => tracing::warn!(
                    "anytls: the server pushed a padding scheme that is not valid; keeping the current one"
                ),
            },
            frame::HEART_REQUEST => {
                // never wait here: a full queue means the writer is busy, which is life enough
                let _ = replies.try_send(frame::frame(frame::HEART_RESPONSE, sid, &[]));
            }
            // waste, the server's settings, a heart response, anything newer: read and dropped
            _ => {}
        }
    }
}

async fn write_loop(
    mut io: WriteHalf<BoxedStream>,
    queue: &mut mpsc::Receiver<Vec<u8>>,
    scheme: &SchemeCell,
) -> String {
    // packet 0 was the authentication
    let mut packet: u32 = 0;
    while let Some(first) = queue.recv().await {
        let mut next = Some(first);
        while let Some(bytes) = next {
            packet = packet.saturating_add(1);
            let current = scheme.lock().expect("scheme").clone();
            let records = if packet < current.stop() {
                shape(&current.pieces(packet, &mut pick), &bytes)
            } else {
                vec![bytes]
            };
            for record in records {
                // one write, one TLS record: the sizes are what the scheme is about
                if io.write_all(&record).await.is_err() {
                    return CLOSED.to_string();
                }
            }
            next = queue.try_recv().ok();
        }
        if io.flush().await.is_err() {
            return CLOSED.to_string();
        }
    }
    CLOSED.to_string()
}

/// No `Debug`: the task holds the authenticated connection.
pub(crate) struct Session {
    seq: u64,
    commands: mpsc::Sender<Vec<u8>>,
    shared: Arc<Shared>,
    scheme: SchemeCell,
    next_sid: u32,
    /// `cmdSettings` has not gone out yet.
    fresh: bool,
    _task: AbortOnDrop,
}

impl Session {
    /// `io` is past the authentication. Spawns the session's task.
    pub(crate) fn start(seq: u64, io: BoxedStream, scheme: SchemeCell) -> Session {
        let (commands, mut queue) = mpsc::channel::<Vec<u8>>(QUEUE);
        let shared = Arc::new(Shared::default());
        let task = {
            let (shared, scheme, replies) = (shared.clone(), scheme.clone(), commands.clone());
            tokio::spawn(async move {
                let (reader, writer) = tokio::io::split(io);
                let why = tokio::select! {
                    // an alert that arrives together with a write error is the better story
                    biased;
                    why = read_loop(reader, &shared, &replies, &scheme) => why,
                    why = write_loop(writer, &mut queue, &scheme) => why,
                };
                if why != CLOSED {
                    // an alert: the text is already stripped and bounded
                    tracing::warn!("{why}");
                }
                shared.close(why);
            })
        };
        Session {
            seq,
            commands,
            shared,
            scheme,
            next_sid: 0,
            fresh: true,
            _task: AbortOnDrop(task),
        }
    }

    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::SeqCst)
    }

    /// Opens the next stream: `cmdSYN` and the target address leave in one
    /// write (with `cmdSettings` in front on a new session). The server's
    /// `cmdSYNACK` is not awaited; a refusal shows up on the first read.
    pub(crate) async fn open(
        mut self,
        address: &[u8],
        pool: Weak<Pool>,
    ) -> io::Result<AnyTlsStream> {
        let closed = || io::Error::new(io::ErrorKind::BrokenPipe, CLOSED);
        let sid = self.next_sid.checked_add(1).ok_or_else(closed)?;
        self.next_sid = sid;
        let (incoming, receiver) = mpsc::channel(QUEUE);
        let state = Arc::new(StreamState::default());
        *self.shared.slot.lock().expect("slot") = Some(Slot {
            sid,
            incoming,
            state: state.clone(),
        });
        let mut packet = Vec::new();
        if self.fresh {
            let md5 = self.scheme.lock().expect("scheme").md5().to_string();
            let settings = format!(
                "v=2\nclient=rurge/{}\npadding-md5={md5}",
                env!("CARGO_PKG_VERSION")
            );
            frame::push(&mut packet, frame::SETTINGS, 0, settings.as_bytes());
            self.fresh = false;
        }
        frame::push(&mut packet, frame::SYN, sid, &[]);
        frame::push(&mut packet, frame::PSH, sid, address);
        self.commands.send(packet).await.map_err(|_| closed())?;
        Ok(AnyTlsStream {
            sid,
            commands: PollSender::new(self.commands.clone()),
            session: Some(self),
            pool,
            incoming: receiver,
            state,
            chunk: Vec::new(),
            pos: 0,
            closed: false,
            reader: None,
        })
    }
}

pub(crate) struct AnyTlsStream {
    sid: u32,
    /// Back to the pool when the stream is dropped.
    session: Option<Session>,
    pool: Weak<Pool>,
    commands: PollSender<Vec<u8>>,
    incoming: mpsc::Receiver<Vec<u8>>,
    state: Arc<StreamState>,
    chunk: Vec<u8>,
    pos: usize,
    /// We ended it: reads are over and writes fail.
    closed: bool,
    /// A reader parked on the queue, to be told when we close.
    reader: Option<Waker>,
}

impl AnyTlsStream {
    fn close_locally(&mut self) {
        self.closed = true;
        if let Some(session) = &self.session {
            session.shared.release(self.sid);
        }
        self.incoming.close();
        if let Some(reader) = self.reader.take() {
            reader.wake();
        }
    }
}

impl AsyncRead for AnyTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.closed {
                return Poll::Ready(Ok(()));
            }
            if this.pos < this.chunk.len() {
                let n = out.remaining().min(this.chunk.len() - this.pos);
                out.put_slice(&this.chunk[this.pos..this.pos + n]);
                this.pos += n;
                return Poll::Ready(Ok(()));
            }
            match this.incoming.poll_recv(cx) {
                Poll::Ready(Some(data)) => {
                    this.chunk = data;
                    this.pos = 0;
                }
                Poll::Ready(None) => {
                    return Poll::Ready(match this.state.error() {
                        Some(text) => Err(io::Error::other(text)),
                        None => Ok(()),
                    });
                }
                Poll::Pending => {
                    this.reader = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
        }
    }
}

impl AsyncWrite for AnyTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.closed || this.state.is_over() {
            let text = this
                .state
                .error()
                .unwrap_or_else(|| "anytls: the stream is closed".to_string());
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, text)));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if ready!(this.commands.poll_reserve(cx)).is_err() {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, CLOSED)));
        }
        let n = data.len().min(MAX_DATA);
        if this
            .commands
            .send_item(frame::frame(frame::PSH, this.sid, &data[..n]))
            .is_err()
        {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::BrokenPipe, CLOSED)));
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        // the session's task flushes what it writes
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.closed {
            // the server's own FIN needs no answer; a dead session takes none
            if !this.state.finished_by_peer() && ready!(this.commands.poll_reserve(cx)).is_ok() {
                let _ = this
                    .commands
                    .send_item(frame::frame(frame::FIN, this.sid, &[]));
            }
            this.close_locally();
        }
        Poll::Ready(Ok(()))
    }
}

impl Drop for AnyTlsStream {
    fn drop(&mut self) {
        let Some(session) = self.session.take() else {
            return;
        };
        let mut reusable = !session.is_closed();
        if !self.closed {
            // dropped without a shutdown: the FIN is still owed, and a
            // session that cannot take it right now is not worth keeping
            if !self.state.finished_by_peer() {
                reusable &= session
                    .commands
                    .try_send(frame::frame(frame::FIN, self.sid, &[]))
                    .is_ok();
            }
            session.shared.release(self.sid);
        }
        if reusable && let Some(pool) = self.pool.upgrade() {
            pool.put(session);
        }
        // otherwise the session is dropped here: its task is aborted and the connection closes
    }
}
```

要点（评审会看这几条）：
- 读循环与写循环在**同一个任务**里（`select!` 两个分支都持续被轮询；任一个结束，会话就结束）。`read_exact` 不是取消安全的，但这里的"取消"只发生在会话死亡时，无所谓。
- 读循环回心跳用 `try_send`：在读循环里等队列，会在"双方都在等对方读"时死锁。
- `deliver` 在流的读者慢时会停住整条会话——这就是背压（一条会话只有这一个流）；锁不跨 `await`（先把发送端克隆出锁外）。
- 流句柄：本端 `shutdown` = 发 FIN + 让读立刻 EOF + 叫醒停在队列上的读者；对端的 FIN 不回；被拒的流（SYNACK 带文本）仍要发 FIN；`Drop` 里用 `try_send` 补 FIN，补不进去就不回池。
- 任何文本都先过 `untrusted_text`；对端的原始字节不进日志。

- [ ] **Step 4: 回环假服务端**

`crates/rurge-proto/src/testing/anytls.rs`：

```rust
//! A scriptable AnyTLS server behind TLS (protocol version 2, or 1 on
//! request). It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::anytls::frame::{self, HEADER};
use crate::anytls::padding::Scheme;
use rurge_net::connector::BoxedStream;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

#[derive(Clone, Debug, Default)]
pub struct AnyTlsScript {
    pub password: String,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
    /// The server's padding scheme: pushed to a client whose md5 differs.
    pub scheme: Option<String>,
    /// Answer `cmdSettings` with this alert and close.
    pub alert: Option<String>,
    /// Refuse every stream with this text in its `cmdSYNACK`.
    pub refuse: Option<String>,
    /// Send a `cmdHeartRequest` after the settings.
    pub heartbeat: bool,
    /// Speak protocol version 1: no `cmdServerSettings`, no `cmdSYNACK`.
    pub v1: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedStream {
    /// Which authenticated connection carried it (0-based).
    pub session: usize,
    pub sid: u32,
    pub atyp: u8,
    pub host: String,
    pub port: u16,
}

#[derive(Default)]
struct Seen {
    sessions: AtomicUsize,
    rejected: AtomicUsize,
    heart_responses: AtomicUsize,
    fins: AtomicUsize,
    waste: AtomicUsize,
    streams: Mutex<Vec<RecordedStream>>,
    settings: Mutex<Vec<String>>,
    /// `kick`: every connection parked on its next frame is closed.
    kick: tokio::sync::Notify,
}

pub struct FakeAnyTls {
    addr: SocketAddr,
    seen: Arc<Seen>,
    _task: AbortOnDrop,
}

/// `(atyp, host, port, bytes used)` of a SOCKS5 address.
fn parse_addr(buf: &[u8]) -> Option<(u8, String, u16, usize)> {
    let atyp = *buf.first()?;
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = buf.get(1..5)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 5)
        }
        4 => {
            let b: [u8; 16] = buf.get(1..17)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 17)
        }
        3 => {
            let len = usize::from(*buf.get(1)?);
            (
                String::from_utf8_lossy(buf.get(2..2 + len)?).into_owned(),
                2 + len,
            )
        }
        _ => return None,
    };
    let port = u16::from_be_bytes(buf.get(used..used + 2)?.try_into().ok()?);
    Some((atyp, host, port, used + 2))
}

struct Live {
    to_upstream: OwnedWriteHalf,
    _reader: AbortOnDrop,
}

/// After an alert: the frame is on the wire, and the connection is read to
/// its end rather than closed over unread bytes, which would reset it and
/// could cost the client the alert.
async fn alerted(
    out: mpsc::UnboundedSender<Vec<u8>>,
    writer: tokio::task::JoinHandle<()>,
    mut reader: tokio::io::ReadHalf<BoxedStream>,
) -> io::Result<()> {
    drop(out);
    let _ = writer.await;
    let mut sink = [0u8; 1024];
    while matches!(reader.read(&mut sink).await, Ok(n) if n > 0) {}
    Ok(())
}

async fn serve(
    mut stream: BoxedStream,
    script: Arc<AnyTlsScript>,
    seen: Arc<Seen>,
) -> io::Result<()> {
    let mut auth = [0u8; 34];
    stream.read_exact(&mut auth).await?;
    if auth[..32] != Sha256::digest(script.password.as_bytes())[..] {
        // what a real server's fallback site would say
        seen.rejected.fetch_add(1, Ordering::SeqCst);
        stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
            )
            .await?;
        return stream.shutdown().await;
    }
    let mut padding0 = vec![0u8; usize::from(u16::from_be_bytes([auth[32], auth[33]]))];
    stream.read_exact(&mut padding0).await?;
    seen.waste.fetch_add(padding0.len(), Ordering::SeqCst);
    let session = seen.sessions.fetch_add(1, Ordering::SeqCst);
    let (mut reader, mut writer) = tokio::io::split(stream);
    let (out, mut queue) = mpsc::unbounded_channel::<Vec<u8>>();
    // ends by itself once every sender is gone (this function's and the streams')
    let writer = tokio::spawn(async move {
        while let Some(bytes) = queue.recv().await {
            if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
                return;
            }
        }
    });
    let mut settled = false;
    let mut live: HashMap<u32, Option<Live>> = HashMap::new();
    loop {
        let mut header = [0u8; HEADER];
        tokio::select! {
            _ = seen.kick.notified() => return Ok(()),
            read = reader.read_exact(&mut header) => {
                if read.is_err() {
                    return Ok(());
                }
            }
        }
        let (command, sid, len) = frame::parse_header(&header);
        let mut data = vec![0u8; len];
        reader.read_exact(&mut data).await?;
        match command {
            frame::WASTE => {
                seen.waste.fetch_add(len, Ordering::SeqCst);
            }
            frame::SETTINGS => {
                settled = true;
                let text = String::from_utf8_lossy(&data).into_owned();
                let theirs = text
                    .lines()
                    .find_map(|l| l.strip_prefix("padding-md5="))
                    .map(str::to_string);
                seen.settings.lock().expect("settings").push(text);
                if let Some(alert) = &script.alert {
                    let _ = out.send(frame::frame(frame::ALERT, 0, alert.as_bytes()));
                    return alerted(out, writer, reader).await;
                }
                if let Some(scheme) = &script.scheme {
                    // a scheme the client could not parse has no md5 to compare: push it anyway
                    let ours = Scheme::parse(scheme.as_bytes()).map(|s| s.md5().to_string());
                    if ours.is_none() || ours != theirs {
                        let _ = out.send(frame::frame(
                            frame::UPDATE_PADDING_SCHEME,
                            0,
                            scheme.as_bytes(),
                        ));
                    }
                }
                if !script.v1 {
                    let _ = out.send(frame::frame(frame::SERVER_SETTINGS, 0, b"v=2"));
                }
                if script.heartbeat {
                    let _ = out.send(frame::frame(frame::HEART_REQUEST, 0, &[]));
                }
            }
            frame::SYN => {
                if !settled {
                    let _ = out.send(frame::frame(
                        frame::ALERT,
                        0,
                        b"client did not send its settings",
                    ));
                    return alerted(out, writer, reader).await;
                }
                live.insert(sid, None);
            }
            frame::PSH => match live.get_mut(&sid) {
                // the first push of a stream is its target
                Some(slot @ None) => {
                    let Some((atyp, host, port, used)) = parse_addr(&data) else {
                        return Ok(());
                    };
                    seen.streams.lock().expect("streams").push(RecordedStream {
                        session,
                        sid,
                        atyp,
                        host: host.clone(),
                        port,
                    });
                    if let Some(text) = &script.refuse {
                        let _ = out.send(frame::frame(frame::SYNACK, sid, text.as_bytes()));
                        live.remove(&sid);
                        continue;
                    }
                    let addr = match (script.connect_to, host.parse::<IpAddr>()) {
                        (Some(addr), _) => Some(addr),
                        (None, Ok(ip)) => Some(SocketAddr::new(ip, port)),
                        // never resolves: a name without `connect_to` is a dead end
                        (None, Err(_)) => None,
                    };
                    let upstream = match addr {
                        Some(addr) => TcpStream::connect(addr).await.ok(),
                        None => None,
                    };
                    let Some(upstream) = upstream else {
                        if !script.v1 {
                            let _ =
                                out.send(frame::frame(frame::SYNACK, sid, b"connection refused"));
                        }
                        let _ = out.send(frame::frame(frame::FIN, sid, &[]));
                        live.remove(&sid);
                        continue;
                    };
                    if !script.v1 {
                        let _ = out.send(frame::frame(frame::SYNACK, sid, &[]));
                    }
                    let (mut from_upstream, mut to_upstream) = upstream.into_split();
                    to_upstream.write_all(&data[used..]).await?;
                    let out = out.clone();
                    let reader = tokio::spawn(async move {
                        let mut buf = vec![0u8; 8192];
                        loop {
                            match from_upstream.read(&mut buf).await {
                                Ok(n) if n > 0 => {
                                    if out.send(frame::frame(frame::PSH, sid, &buf[..n])).is_err() {
                                        return;
                                    }
                                }
                                _ => {
                                    let _ = out.send(frame::frame(frame::FIN, sid, &[]));
                                    return;
                                }
                            }
                        }
                    });
                    *slot = Some(Live {
                        to_upstream,
                        _reader: AbortOnDrop(reader),
                    });
                }
                Some(Some(stream)) => stream.to_upstream.write_all(&data).await?,
                // a stream that is gone: dropped, as the reference does
                None => {}
            },
            frame::FIN => {
                seen.fins.fetch_add(1, Ordering::SeqCst);
                live.remove(&sid);
            }
            frame::HEART_RESPONSE => {
                seen.heart_responses.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        }
    }
}

impl FakeAnyTls {
    pub async fn spawn(script: AnyTlsScript, fixture: Arc<TlsFixture>) -> FakeAnyTls {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let seen: Arc<Seen> = Arc::default();
        let script = Arc::new(script);
        let shared = seen.clone();
        let acceptor = fixture.acceptor(false);
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, seen) = (script.clone(), shared.clone());
                let (fixture, acceptor) = (fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let _ = serve(stream, script, seen).await;
                });
            }
        });
        FakeAnyTls {
            addr,
            seen,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Closes every connection that is waiting for its next frame (an idle
    /// session, as a server's own clean-up would).
    pub fn kick(&self) {
        self.seen.kick.notify_waiters();
    }

    /// Connections that authenticated.
    pub fn sessions(&self) -> usize {
        self.seen.sessions.load(Ordering::SeqCst)
    }

    /// Connections answered like a web server because the password was wrong.
    pub fn rejected(&self) -> usize {
        self.seen.rejected.load(Ordering::SeqCst)
    }

    pub fn streams(&self) -> Vec<RecordedStream> {
        self.seen.streams.lock().expect("streams").clone()
    }

    /// Every `cmdSettings` text received.
    pub fn settings(&self) -> Vec<String> {
        self.seen.settings.lock().expect("settings").clone()
    }

    pub fn heart_responses(&self) -> usize {
        self.seen.heart_responses.load(Ordering::SeqCst)
    }

    pub fn fins(&self) -> usize {
        self.seen.fins.load(Ordering::SeqCst)
    }

    /// Padding bytes received: the authentication's and every `cmdWaste`.
    pub fn waste(&self) -> usize {
        self.seen.waste.load(Ordering::SeqCst)
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`：模块列表加 `mod anytls;`，导出加 `pub use anytls::{AnyTlsScript, FakeAnyTls, RecordedStream};`。

- [ ] **Step 5: 出站与它的用例**

`crates/rurge-proto/src/anytls/mod.rs`（整个替换）：

```rust
//! `anytls` outbound (manual: Policies › AnyTLS; protocol: anytls-go
//! `docs/protocol.md`, version 2). TLS, then `SHA256(password)` with the
//! scheme's first padding, then a session layer of frames (`frame`) whose
//! first writes are cut and padded the way the padding scheme says
//! (`padding`). A session carries one stream at a time and is reused for the
//! next one (`session`, `pool`); `reuse=false` closes it with its stream.
//!
//! A wrong password cannot be told from a server that closes: it answers
//! like a web site, and the stream ends in the relay.

pub(crate) mod frame;
pub(crate) mod padding;
mod pool;
mod session;

use crate::addr::{AddrError, socks_addr};
use crate::build::tls_client;
use crate::task::AbortOnDrop;
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
use padding::Scheme;
use pool::Pool;
use rurge_config::KeystoreItem;
use rurge_config::spec::AnyTlsSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use session::{SchemeCell, Session, pick};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tokio::io::AsyncWriteExt;

/// No `Debug`: the hash is as good as the password.
pub struct AnyTlsOutbound {
    name: String,
    stack: Stack,
    /// `SHA256(password)`, what the wire carries; the password itself is not kept.
    hash: [u8; 32],
    /// Starts as the protocol's default; the server may push another.
    scheme: SchemeCell,
    /// `None` with `reuse=false`.
    pool: Option<Arc<Pool>>,
    /// Started by the first connection: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDrop>,
    next_seq: AtomicU64,
}

impl AnyTlsOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &AnyTlsSpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<AnyTlsOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it
        if spec.password.expose().is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        // no ALPN unless the policy asks for one (M2 design 4.5)
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
        Ok(AnyTlsOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, tls, None),
            hash: Sha256::digest(spec.password.expose().as_bytes()).into(),
            scheme: Arc::new(Mutex::new(Arc::new(Scheme::default_scheme()))),
            pool: spec.reuse.then(Arc::<Pool>::default),
            reaper: OnceLock::new(),
            next_seq: AtomicU64::new(0),
        })
    }

    async fn new_session(&self, opts: &ConnectOpts) -> Result<Session, OutboundError> {
        let mut io = self.stack.open(opts).await?;
        let scheme = self.scheme.lock().expect("scheme").clone();
        let padding = scheme.auth_padding(&mut pick);
        // one write, one TLS record: the reference server takes the whole
        // authentication from a single read
        let mut auth = Vec::with_capacity(34 + padding);
        auth.extend_from_slice(&self.hash);
        auth.extend_from_slice(&(padding as u16).to_be_bytes());
        auth.resize(34 + padding, 0);
        io.write_all(&auth).await?;
        io.flush().await?;
        let seq = self.next_seq.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(Session::start(seq, io, self.scheme.clone()))
    }

    async fn open(&self, address: &[u8], opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let pool = match &self.pool {
            Some(pool) => {
                self.reaper.get_or_init(|| pool::spawn_reaper(pool));
                Arc::downgrade(pool)
            }
            None => Weak::new(),
        };
        // an idle session may have died since it was pooled: try the next one, then dial
        while let Some(session) = self.pool.as_ref().and_then(|p| p.take()) {
            if let Ok(stream) = session.open(address, pool.clone()).await {
                return Ok(Box::new(stream));
            }
        }
        let session = self.new_session(opts).await?;
        match session.open(address, pool).await {
            Ok(stream) => Ok(Box::new(stream)),
            Err(e) => Err(OutboundError::Proxy(e.to_string())),
        }
    }
}

impl Outbound for AnyTlsOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let address = socks_addr(target).map_err(|e| {
                OutboundError::Proxy(
                    match e {
                        AddrError::Unsendable => {
                            "anytls: the host name cannot be sent to the server"
                        }
                        AddrError::TooLong => "anytls: the host name is longer than 255 bytes",
                    }
                    .to_string(),
                )
            })?;
            // one budget for the connection, TLS, the authentication and the
            // stream's first write; the server's SYNACK is not awaited
            match tokio::time::timeout(opts.timeout, self.open(&address, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{AnyTlsScript, FakeAnyTls, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::anytls::read_anytls;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

    /// The outbound for `definition` (an `anytls, host, port, ...` line).
    fn outbound(definition: &str, fixture: &Arc<TlsFixture>) -> AnyTlsOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("A", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_anytls(&mut r, &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        AnyTlsOutbound::new(
            "A",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn script(password: &str) -> AnyTlsScript {
        AnyTlsScript {
            password: password.into(),
            ..AnyTlsScript::default()
        }
    }

    async fn server(script: AnyTlsScript) -> (Arc<TlsFixture>, FakeAnyTls) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeAnyTls::spawn(script, fixture.clone()).await;
        (fixture, fake)
    }

    fn line(fake: &FakeAnyTls, rest: &str) -> String {
        format!("anytls, 127.0.0.1, {}, password=pw{rest}", fake.addr().port())
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn connect(out: &AnyTlsOutbound, to: SocketAddr) -> BoxedStream {
        out.connect_tcp(&target(to), &ConnectOpts::default())
            .await
            .unwrap()
    }

    /// Polls `what` until it holds; bounded, and no fixed sleep decides anything.
    async fn eventually(what: impl Fn() -> bool, why: &str) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !what() {
            assert!(tokio::time::Instant::now() < deadline, "{why}");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    async fn ping(stream: &mut BoxedStream, text: &[u8]) {
        // no explicit `flush()`: `write_all` alone must deliver
        stream.write_all(text).await.unwrap();
        let mut buf = vec![0u8; text.len()];
        tokio::time::timeout(Duration::from_secs(10), stream.read_exact(&mut buf))
            .await
            .expect("the echo arrives within the bound")
            .unwrap();
        assert_eq!(buf, text);
    }

    /// The engine's relay, reduced to what matters to a stream under test:
    /// `read` → `write_all` with no flush, `shutdown` at EOF, both directions
    /// polled from one task through `tokio::io::split`.
    async fn copy_half<R, W>(mut reader: R, mut writer: W) -> std::io::Result<u64>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut buf = vec![0u8; 8 * 1024];
        let mut total = 0;
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                let _ = writer.shutdown().await;
                return Ok(total);
            }
            writer.write_all(&buf[..n]).await?;
            total += n as u64;
        }
    }

    #[tokio::test]
    async fn a_relay_moves_a_megabyte_each_way_and_the_session_comes_back() {
        tokio::time::timeout(Duration::from_secs(60), async {
            let echo = echo_server().await;
            let (fixture, fake) = server(script("pw")).await;
            let out = outbound(&line(&fake, ""), &fixture);
            let upstream = connect(&out, echo).await;
            let (mut app, near) = tokio::io::duplex(64 * 1024);
            let relay = tokio::spawn(async move {
                let (cr, cw) = tokio::io::split(near);
                let (ur, uw) = tokio::io::split(upstream);
                tokio::join!(copy_half(cr, uw), copy_half(ur, cw))
            });
            let payload: Vec<u8> = (0..1_000_000u32).map(|i| (i % 251) as u8).collect();
            let (mut app_r, mut app_w) = tokio::io::split(&mut app);
            let mut back = vec![0u8; payload.len()];
            tokio::join!(
                async { app_w.write_all(&payload).await.unwrap() },
                async { app_r.read_exact(&mut back).await.unwrap() }
            );
            assert!(back == payload, "the echo differs");
            drop((app_r, app_w));
            drop(app); // the client goes away: FIN, and the relay winds down by itself
            let (up, down) = relay.await.unwrap();
            assert_eq!((up.unwrap(), down.unwrap()), (1_000_000, 1_000_000));
            let pool = out.pool.as_ref().unwrap();
            eventually(|| pool.len() == 1, "the session never came back").await;
            eventually(|| fake.fins() == 1, "no FIN").await;
            // the authentication's 30 bytes at least, and whatever the first writes were padded with
            assert!(fake.waste() >= 30, "{}", fake.waste());
            let settings = fake.settings();
            assert_eq!(settings.len(), 1);
            assert_eq!(
                settings[0],
                format!(
                    "v=2\nclient=rurge/{}\npadding-md5=75cff2ad89aadf5e257059ee571ebe11",
                    env!("CARGO_PKG_VERSION")
                )
            );
            // TLS as every other outbound does it: no ALPN unless asked for
            assert_eq!(fixture.seen()[0].alpn, None);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn the_newest_idle_session_is_reused_and_reuse_can_be_turned_off() {
        let echo = echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        for round in 0..3u8 {
            let mut s = connect(&out, echo).await;
            ping(&mut s, &[round; 8]).await;
            s.shutdown().await.unwrap();
            drop(s);
        }
        assert_eq!(fake.sessions(), 1);
        let streams = fake.streams();
        assert_eq!(
            streams.iter().map(|s| (s.session, s.sid)).collect::<Vec<_>>(),
            [(0, 1), (0, 2), (0, 3)]
        );
        assert_eq!(fake.settings().len(), 1, "settings go out once per session");
        // two at once need two sessions; the newer one is the one reused
        let mut a = connect(&out, echo).await;
        let mut b = connect(&out, echo).await;
        ping(&mut a, b"a").await;
        ping(&mut b, b"b").await;
        assert_eq!(fake.sessions(), 2);
        drop(a);
        drop(b);
        let pool = out.pool.as_ref().unwrap();
        eventually(|| pool.len() == 2, "both come back").await;
        let mut c = connect(&out, echo).await;
        ping(&mut c, b"c").await;
        assert_eq!(fake.streams().last().unwrap().session, 1, "the newest session first");
        // reuse=false: every stream dials, and nothing is kept
        let (fixture, fake) = server(script("pw")).await;
        let once = outbound(&line(&fake, ", reuse=false"), &fixture);
        for _ in 0..2 {
            let mut s = connect(&once, echo).await;
            ping(&mut s, b"x").await;
        }
        assert_eq!(fake.sessions(), 2);
        assert!(once.pool.is_none() && once.reaper.get().is_none());
    }

    #[tokio::test]
    async fn a_refused_stream_fails_on_its_first_read_and_the_session_survives() {
        let (fixture, fake) = server(AnyTlsScript {
            refuse: Some("dial tcp: connection\r\nrefused\x1b[0m".into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, "127.0.0.1:9".parse().unwrap()).await;
        let mut buf = [0u8; 1];
        let err = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        // the server's text, stripped of control characters
        assert_eq!(err.to_string(), "anytls: dial tcp: connectionrefused[0m");
        drop(s);
        let pool = out.pool.as_ref().unwrap();
        eventually(|| pool.len() == 1, "a refused stream does not cost the session").await;
        eventually(|| fake.fins() == 1, "the refused stream is still closed with a FIN").await;
    }

    #[tokio::test]
    async fn an_alert_closes_the_session_and_a_wrong_password_shows_in_the_relay() {
        let (fixture, fake) = server(AnyTlsScript {
            alert: Some("upgrade your\nclient".into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, "127.0.0.1:9".parse().unwrap()).await;
        let mut buf = [0u8; 1];
        let err = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "anytls: the server sent an alert: upgrade yourclient"
        );
        drop(s);
        assert_eq!(out.pool.as_ref().unwrap().len(), 0, "a dead session is not pooled");
        // a wrong password: the server answers like a web site and closes
        let wrong = outbound(
            &format!("anytls, 127.0.0.1, {}, password=other", fake.addr().port()),
            &fixture,
        );
        let mut s = connect(&wrong, "127.0.0.1:9".parse().unwrap()).await;
        let err = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
            .await
            .expect("bounded")
            .unwrap_err();
        assert_eq!(err.to_string(), "anytls: the session is closed");
        assert_eq!(fake.rejected(), 1);
        assert!(!err.to_string().contains("other"));
    }

    #[tokio::test]
    async fn a_pushed_scheme_is_used_by_the_next_session_and_a_bad_one_is_ignored() {
        let echo = echo_server().await;
        let pushed = "stop=3\n0=11-11\n1=50-50\n2=60-60";
        let (fixture, fake) = server(AnyTlsScript {
            scheme: Some(pushed.into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ", reuse=false"), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"one").await;
        drop(s);
        let expected = Scheme::parse(pushed.as_bytes()).unwrap().md5().to_string();
        eventually(
            || out.scheme.lock().unwrap().md5() == expected,
            "the pushed scheme never took effect",
        )
        .await;
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"two").await;
        let settings = fake.settings();
        assert!(
            settings[1].ends_with(&format!("padding-md5={expected}")),
            "{}",
            settings[1]
        );
        // a scheme out of bounds is refused, and the current one stays
        let (fixture, bad) = server(AnyTlsScript {
            scheme: Some("stop=9999\n1=5-5".into()),
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&bad, ", reuse=false"), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"three").await;
        assert_eq!(
            out.scheme.lock().unwrap().md5(),
            "75cff2ad89aadf5e257059ee571ebe11"
        );
    }

    #[tokio::test]
    async fn a_heartbeat_is_answered_and_a_version_one_server_works() {
        let echo = echo_server().await;
        let (fixture, fake) = server(AnyTlsScript {
            heartbeat: true,
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"beat").await;
        eventually(|| fake.heart_responses() == 1, "the heartbeat went unanswered").await;
        let (fixture, v1) = server(AnyTlsScript {
            v1: true,
            ..script("pw")
        })
        .await;
        let out = outbound(&line(&v1, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"version one").await;
    }

    #[tokio::test]
    async fn a_session_the_server_closed_while_idle_is_not_reused() {
        let echo = echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"first").await;
        drop(s);
        let pool = out.pool.clone().unwrap();
        eventually(|| pool.len() == 1, "pooled").await;
        fake.kick();
        // the session's task notices on its own, with nobody using the session
        eventually(
            || {
                pool.reap(tokio::time::Instant::now());
                pool.len() == 0
            },
            "the dead session stayed in the pool",
        )
        .await;
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"second").await;
        assert_eq!(fake.sessions(), 2);
    }

    #[tokio::test]
    async fn an_idle_session_is_closed_after_a_minute_and_the_pool_dies_with_the_outbound() {
        let echo = echo_server().await;
        let (fixture, fake) = server(script("pw")).await;
        let out = outbound(&line(&fake, ""), &fixture);
        let mut s = connect(&out, echo).await;
        ping(&mut s, b"once").await;
        drop(s);
        let pool = out.pool.clone().unwrap();
        eventually(|| pool.len() == 1, "pooled").await;
        // real sockets are done: from here on the clock is ours. (A paused
        // clock from the start would auto-advance past every timeout while
        // the test waits for real I/O.)
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(45)).await;
        tokio::task::yield_now().await;
        assert_eq!(pool.len(), 1, "not yet");
        tokio::time::advance(Duration::from_secs(50)).await;
        eventually(|| pool.len() == 0, "never reaped").await;
        let weak = Arc::downgrade(&pool);
        drop(pool);
        drop(out);
        assert!(weak.upgrade().is_none(), "the outbound owned the pool");
    }

    #[tokio::test]
    async fn an_empty_password_is_a_build_error_and_a_bad_name_never_dials() {
        let (fixture, fake) = server(script("pw")).await;
        let spec = AnyTlsSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            password: rurge_config::spec::Secret::new(String::new()),
            reuse: true,
        };
        let err = AnyTlsOutbound::new(
            "A",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "`password` is empty");
        let out = outbound(&line(&fake, ""), &fixture);
        let bad = Target::new(HostName::Domain("a@b.test".into()), 80);
        let err = out
            .connect_tcp(&bad, &ConnectOpts::default())
            .await
            .err()
            .expect("refused");
        assert_eq!(
            err.to_string(),
            "anytls: the host name cannot be sent to the server"
        );
        assert_eq!(fake.sessions(), 0);
    }
}
```

Run: `cargo test -p rurge-proto anytls`
Expected: Task 4 的 5 个用例 + 本任务 9 个出站用例全部通过。连跑 5 遍（`for i in 1 2 3 4 5; do cargo test -p rurge-proto anytls || break; done`）：计划期的临时工程连跑 15 遍无抖动；某一遍失败就是真缺陷，不要加重试、不要放宽上界，把输出写进报告。

两个容易踩的地方（计划期都踩过）：
- 暂停时钟的用例**先做完真实 socket 上的往返再 `pause()`**。`#[tokio::test(start_paused = true)]` 会让 `ping` 里的 10 秒上界在等真实 I/O 时被自动快进触发。
- 假服务端发完 alert 之后**把连接读到头**再返回（`alerted`）：带着未读字节关连接会变成 RST，客户端可能还没读到 alert 就先拿到连接错误。

- [ ] **Step 6: 门禁与提交**

跑「Global Constraints」里的门禁（确认 Task 4 的两个 `#[allow(dead_code)]` 已经不在了：`grep -n 'allow(dead_code)' crates/rurge-proto/src/anytls/mod.rs` 无输出）。然后：

```bash
git add -A
git commit -m "feat(proto): anytls 出站——一条会话一个任务、流句柄与有界队列、空闲池与 30 s / 60 s 回收、padding 方案的推送；回环假服务端 FakeAnyTls"
```

---

### Task 6: 转发循环——`write_all` 之后 `flush`；一个方向出错就结束另一个方向

承接 M2a 的延后事项（P16）。两处都在 `crates/rurge-engine/src/relay.rs`，都先有在旧代码上变红的用例。

**Files:**
- Modify: `crates/rurge-engine/src/relay.rs`

**Interfaces:**
- Consumes / Produces: `pump` 与 `copy_half` 的签名不变；行为变化只有两条——写完即刷出；任一方向以 `Err` 结束时取消 `stop`（另一个方向随之以 `Ok(())` 结束，会话记为 `Failed(<那个错误>)`）。

- [ ] **Step 1: 先量一下现在的吞吐（留作对照）**

`relay.rs` 的 `mod tests` 里加一个默认不跑的测量（不是断言，只打印）：

```rust
    /// Not a test of anything: prints how fast `pump` moves bytes over
    /// loopback TCP. Run before and after a change to the copy loop:
    /// `cargo test -p rurge-engine --release relay_throughput -- --ignored --nocapture`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn relay_throughput() {
        use tokio::net::{TcpListener, TcpStream};
        const TOTAL: usize = 512 * 1024 * 1024;
        async fn pair() -> (TcpStream, TcpStream) {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let (a, b) = tokio::join!(TcpStream::connect(addr), listener.accept());
            (a.unwrap(), b.unwrap().0)
        }
        let (mut app, client) = pair().await;
        let (upstream, mut origin) = pair().await;
        let relay = tokio::spawn(pump(
            Box::new(client),
            Box::new(upstream),
            handle(),
            Duration::from_secs(600),
        ));
        let started = std::time::Instant::now();
        let send = tokio::spawn(async move {
            let block = vec![7u8; 64 * 1024];
            for _ in 0..TOTAL / block.len() {
                app.write_all(&block).await.unwrap();
            }
            app.shutdown().await.unwrap();
            app
        });
        let mut got = 0;
        let mut buf = vec![0u8; 64 * 1024];
        while got < TOTAL {
            let n = origin.read(&mut buf).await.unwrap();
            assert!(n > 0, "the relay stopped at {got} bytes");
            got += n;
        }
        let secs = started.elapsed().as_secs_f64();
        println!("relay: {:.0} MiB/s", (TOTAL as f64 / (1024.0 * 1024.0)) / secs);
        drop(origin);
        drop(send.await.unwrap());
        relay.await.unwrap();
    }
```

Run: `cargo test -p rurge-engine --release relay_throughput -- --ignored --nocapture`，跑 3 次，把三个数字记进报告（"改动前"）。

- [ ] **Step 2: 写两条失败的用例**

同一个 `mod tests`（`std::pin::Pin`、`std::task::{Context, Poll, ready}`、`tokio::io::ReadBuf` 按需导入；该模块里已有一个手写 `AsyncWrite` 的先例 `kill_interrupts_a_stalled_write`，导入方式照它）：

```rust
    /// Accepts every write at once but only passes it on when flushed: what
    /// `AsyncWrite` allows, and what tokio-rustls does with its last record
    /// once the socket below it is full.
    struct HoldsUntilFlushed {
        inner: tokio::io::DuplexStream,
        held: Vec<u8>,
    }

    impl AsyncRead for HoldsUntilFlushed {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }

    impl AsyncWrite for HoldsUntilFlushed {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            self.held.extend_from_slice(data);
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            let this = &mut *self;
            while !this.held.is_empty() {
                let n = ready!(Pin::new(&mut this.inner).poll_write(cx, &this.held))?;
                this.held.drain(..n);
            }
            Pin::new(&mut this.inner).poll_flush(cx)
        }

        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            ready!(self.as_mut().poll_flush(cx))?;
            Pin::new(&mut self.inner).poll_shutdown(cx)
        }
    }

    #[tokio::test]
    async fn what_was_written_is_flushed_without_waiting_for_more() {
        let (mut app, client) = tokio::io::duplex(4096);
        let (near, mut far) = tokio::io::duplex(4096);
        let upstream = HoldsUntilFlushed {
            inner: near,
            held: Vec::new(),
        };
        let relay = tokio::spawn(pump(
            Box::new(client),
            Box::new(upstream),
            handle(),
            Duration::from_secs(600),
        ));
        // the last bytes of a request: nothing follows them, and the answer
        // only comes once they have arrived
        app.write_all(b"the last bytes of a request").await.unwrap();
        let mut got = [0u8; 27];
        tokio::time::timeout(Duration::from_secs(5), far.read_exact(&mut got))
            .await
            .expect("the tail stayed behind in the writer")
            .unwrap();
        assert_eq!(&got, b"the last bytes of a request");
        relay.abort();
    }

    /// Reads fail at once; writes vanish.
    struct FailsOnRead;

    impl AsyncRead for FailsOnRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("boom")))
        }
    }

    impl AsyncWrite for FailsOnRead {
        fn poll_write(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            data: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(data.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn an_error_in_one_direction_ends_the_other() {
        // the client stays connected and silent: only the upstream is broken
        let (_app, client) = tokio::io::duplex(1024);
        let h = handle();
        tokio::time::timeout(
            Duration::from_secs(5),
            pump(
                Box::new(client),
                Box::new(FailsOnRead),
                h.clone(),
                Duration::from_secs(600),
            ),
        )
        .await
        .expect("the healthy direction kept the broken session open");
        assert_eq!(h.outcome(), Some(SessionOutcome::Failed("boom".into())));
    }
```

Run: `cargo test -p rurge-engine --lib relay`
Expected: 这两条都 FAIL——第一条 `the tail stayed behind in the writer`，第二条 `the healthy direction kept the broken session open`（各在 5 秒上界处）。**把这两次 RED 的输出贴进报告**：计划期已在旧循环的等价物上真的跑出过第一条的 RED；第二条是读代码得出的（`tokio::join!` 等两个方向），要在这里亲眼看到。

- [ ] **Step 3: 修**

`copy_half` 里写的那一段：

```rust
        tokio::select! {
            biased;
            _ = &mut cancelled => return Ok(()),
            w = async {
                writer.write_all(&buf[..n]).await?;
                // `write_all` only says the writer took the bytes. A TLS
                // writer whose socket is full keeps its last record to itself
                // until the next write or flush — and after the final bytes of
                // a request there is no next write.
                writer.flush().await
            } => w?,
        }
```

`pump` 里 `halves` 的那个 `async move` 块：

```rust
        async move {
            // A direction that fails ends the other one too: the tunnel is
            // broken, and the side still waiting for bytes would otherwise sit
            // there until its own peer gives up or the idle timer fires.
            let up = async {
                let r = copy_half(
                    cr,
                    uw,
                    stop.clone(),
                    a_up,
                    started,
                    move |n| h_up.add_up(n),
                    sniff_first,
                )
                .await;
                if r.is_err() {
                    stop.cancel();
                }
                r
            };
            let down = async {
                let r = copy_half(
                    ur,
                    cw,
                    stop.clone(),
                    a_down,
                    started,
                    move |n| h_down.add_down(n),
                    None,
                )
                .await;
                if r.is_err() {
                    stop.cancel();
                }
                r
            };
            let r = tokio::join!(up, down);
            stop.cancel(); // both directions done: release the watchdog
            r
        }
```

会话结果的判定不用动：被取消的是 `stop`（会话令牌的子令牌），`handle.token()` 没被取消，所以走到 `r_up.and(r_down)` 的 `Err` 分支，记为 `Failed(<错误文本>)`。文件头的模块注释补一句这两条行为。

Run: `cargo test -p rurge-engine --lib relay`
Expected: 全部通过（含 Step 2 的两条）。

- [ ] **Step 4: 再量一次吞吐**

Run: `cargo test -p rurge-engine --release relay_throughput -- --ignored --nocapture`，跑 3 次。
Expected: 与 Step 1 的数字同一量级（TCP 的 `flush` 是空操作；TLS 的 `flush` 只是把已经加密的记录写进 socket）。三个数字记进报告（"改动后"）；**若中位数下降超过 10%，停下来报告**，不要自行调整——那需要另外的设计（例如只在读端暂时无数据时才 flush）。

- [ ] **Step 5: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "fix(engine): 转发循环写完即 flush（TLS 写端在 socket 写满时会留下最后一条记录）；一个方向出错就结束另一个方向"
```

---

### Task 7: `ResolverCell` 与 `publish_generation`

每次重载都会重建整个解析器；一个跨代存活的出站（Task 8）不能还攥着上一代的。解法：工厂造的连接器都拿一个跨代稳定的 cell，它与注册表在同一个发布点切换（设计 7.2）。

**Files:**
- Modify: `crates/rurge-engine/src/shared.rs`（`ResolverCell`、`EngineShared.resolver`）
- Modify: `crates/rurge-engine/src/lib.rs`（导出 `ResolverCell`）
- Modify: `crates/rurge-engine/src/runtime.rs`（工厂拿 cell）
- Modify: `crates/rurge-engine/src/engine.rs`（`Engine::new` 发布第一代的解析器；`publish_registry` → `publish_generation`）
- Modify: `crates/rurge-engine/src/reload.rs`（调用处改名）
- Test: `crates/rurge-engine/src/shared.rs` 的 `mod tests`；`crates/rurge-engine/tests/outbounds.rs`

**Interfaces:**
- Consumes: `rurge_net::connector::Resolve`（`fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>>`）、`arc_swap::ArcSwapOption`（`rurge-engine` 已依赖 `arc-swap`）。
- Produces: `rurge_engine::ResolverCell`（`new() -> Arc<ResolverCell>`、`store(Arc<dyn Resolve>)`、`impl Resolve`）；`EngineShared { cell, selections, resolver: Arc<ResolverCell> }`；`Engine::publish_generation(&self, next: &Runtime)`（`pub(crate)`）。`EngineFactory::new` / `with_roots` 的签名**不变**。

- [ ] **Step 1: 写失败的用例**

`crates/rurge-engine/src/shared.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::BoxFuture;
    use rurge_net::connector::Resolve;
    use std::io;
    use std::net::IpAddr;

    struct Fixed(IpAddr);

    impl Resolve for Fixed {
        fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(std::future::ready(Ok(vec![self.0])))
        }
    }

    #[tokio::test]
    async fn the_cell_answers_with_whatever_generation_is_published() {
        let cell = ResolverCell::new();
        let err = cell.resolve("a.test").await.unwrap_err();
        assert_eq!(err.to_string(), "no resolver is active");
        cell.store(Arc::new(Fixed("192.0.2.1".parse().unwrap())));
        assert_eq!(
            cell.resolve("a.test").await.unwrap(),
            ["192.0.2.1".parse::<IpAddr>().unwrap()]
        );
        // the next generation: the same cell, another resolver behind it
        cell.store(Arc::new(Fixed("192.0.2.2".parse().unwrap())));
        assert_eq!(
            cell.resolve("a.test").await.unwrap(),
            ["192.0.2.2".parse::<IpAddr>().unwrap()]
        );
    }
}
```

Run: `cargo test -p rurge-engine --lib shared`
Expected: FAIL（`ResolverCell` 不存在，编译错误）。

- [ ] **Step 2: `ResolverCell` 与 `EngineShared.resolver`**

`crates/rurge-engine/src/shared.rs`（文件头的 `use` 与 `EngineShared` 改成下面这样；`Default` 实现不变）：

```rust
//! What outlives a config generation (M1 design 6.2, 6.3; M2 design 7.2).

use arc_swap::ArcSwapOption;
use rurge_net::BoxFuture;
use rurge_net::connector::Resolve;
use rurge_policy::{GroupSelections, RegistryCell, SelectionTable};
use std::io;
use std::net::IpAddr;
use std::sync::Arc;

/// Where an outbound's connectors find the current generation's resolver.
/// Every reload builds a new resolver (`[Host]`, the upstreams and the cache
/// policy may all have changed), while an outbound that a reload left alone
/// keeps the connectors it was built with: they hold this cell, not any one
/// generation's resolver. Switched together with the registry
/// (`Engine::publish_generation`).
#[derive(Default)]
pub struct ResolverCell(ArcSwapOption<Arc<dyn Resolve>>);

impl ResolverCell {
    pub fn new() -> Arc<ResolverCell> {
        Arc::new(ResolverCell::default())
    }

    pub fn store(&self, resolver: Arc<dyn Resolve>) {
        self.0.store(Some(Arc::new(resolver)));
    }
}

impl Resolve for ResolverCell {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            // nothing dials before the first generation is published
            let Some(current) = self.0.load_full() else {
                return Err(io::Error::other("no resolver is active"));
            };
            current.resolve(host).await
        })
    }
}

/// Created once per engine — before the first `Runtime::build`, because the
/// registry built there already needs all three — and handed to every later
/// `Runtime::build` of the same engine (`Engine::shared`).
#[derive(Clone)]
pub struct EngineShared {
    /// Where chain connectors find the current generation's registry.
    pub cell: Arc<RegistryCell>,
    /// The live `select` choices of the running profile.
    pub selections: Arc<SelectionTable>,
    /// Where direct connectors find the current generation's resolver.
    pub resolver: Arc<ResolverCell>,
}

impl EngineShared {
    pub fn new(initial: GroupSelections) -> EngineShared {
        EngineShared {
            cell: RegistryCell::new(),
            selections: Arc::new(SelectionTable::new(initial)),
            resolver: ResolverCell::new(),
        }
    }
}
```

`crates/rurge-engine/src/lib.rs`：`EngineShared` 的那条 `pub use` 改成同时导出 `ResolverCell`（`pub use shared::{EngineShared, ResolverCell};`，保持该文件现有的写法）。

Run: `cargo test -p rurge-engine --lib shared`
Expected: PASS。

- [ ] **Step 3: 工厂拿 cell；引擎在同一个发布点切换两者**

`crates/rurge-engine/src/runtime.rs`，`Runtime::build` 里造工厂的那一段：

```rust
        // The cell, not this generation's resolver: an outbound may outlive
        // the generation it was built in (M2 design 7.2).
        let factory = crate::outbounds::EngineFactory::new(
            &config,
            opts.shared.resolver.clone(),
            opts.stack.socket_hook.clone(),
        );
```

`crates/rurge-engine/src/engine.rs`，`Engine::new` 里紧跟 `shared.cell.store(runtime.policies.clone());` 加一行：

```rust
        shared.resolver.store(runtime.stack.resolver.clone());
```

同一个文件里把 `publish_registry` 换成：

```rust
    /// Makes `next` the generation that outlives-a-reload objects see: chain
    /// connectors resolve against its registry, direct connectors through
    /// its resolver.
    pub(crate) fn publish_generation(&self, next: &Runtime) {
        assert!(
            Arc::ptr_eq(&self.shared.cell, &next.shared.cell)
                && Arc::ptr_eq(&self.shared.resolver, &next.shared.resolver),
            "the next generation must be built with `Engine::shared()`"
        );
        self.shared.cell.store(next.policies.clone());
        self.shared.resolver.store(next.stack.resolver.clone());
    }
```

`crates/rurge-engine/src/reload.rs`：`self.publish_registry(&next);` → `self.publish_generation(&next);`。

`grep -rn publish_registry crates/` 应当无输出（注释里提到它的地方一并改名）。

- [ ] **Step 4: 端到端用例——上一代造的连接器经新一代解析**

`crates/rurge-engine/tests/outbounds.rs` 末尾：

```rust
#[tokio::test]
async fn a_connector_built_before_a_reload_resolves_through_the_new_generation() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("S = socks5, proxy.test, {}", upstream.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    // the outbound of the first generation, connectors and all
    let old = h
        .engine
        .runtime()
        .policies
        .resolve(&rurge_config::rule::PolicyRef::parse("S"))
        .outbound;

    // the next generation asks another server
    let other = MockDns::spawn().await;
    other.set("proxy.test", &["127.0.0.1"], &[], 60);
    let next = Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    }
    .text(other.addr());
    let next = runtime(h.dir.path(), &next, h.engine.shared()).await;
    h.engine.swap_runtime(next);

    let target = rurge_net::connector::Target::new(
        rurge_config::HostName::Ip(echo.ip()),
        echo.port(),
    );
    let mut stream = old
        .connect_tcp(&target, &rurge_net::connector::ConnectOpts::default())
        .await
        .expect("the old outbound still dials");
    stream.write_all(b"ping").await.unwrap();
    let mut buf = [0u8; 4];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"ping");
    let asked = |dns: &MockDns| dns.queries().iter().any(|(q, _)| q.name == "proxy.test");
    assert!(asked(&other), "the new generation's resolver was asked");
    assert!(!asked(&h.dns), "the old generation's resolver was not");
}
```

Run: `cargo test -p rurge-engine --test outbounds a_connector_built_before`
Expected: PASS。要确认它真的在看 cell：临时把 `runtime.rs` 改回 `stack.resolver.clone()`，这条用例应当在 `the new generation's resolver was asked` 上失败；确认后改回来。把这次 RED 的输出贴进报告。

- [ ] **Step 5: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat(engine): ResolverCell——工厂造的连接器经 cell 取当前一代的解析器；publish_registry 扩成 publish_generation"
```

---

### Task 8: 重载时按指纹复用出站

**Files:**
- Modify: `crates/rurge-policy/src/factory.rs`（`environment()`）
- Modify: `crates/rurge-policy/src/registry.rs`（`Fingerprint`、`build(.., previous)`）
- Modify: `crates/rurge-policy/src/testing.rs`（`FakeFactory.environment`）、`crates/rurge-policy/src/cell.rs`（测试里的调用处）
- Modify: `crates/rurge-engine/src/outbounds.rs`（`environment()` 的实现）、`crates/rurge-engine/src/runtime.rs`（传 `previous`）
- Test: `crates/rurge-policy/src/registry.rs` 的 `mod tests`；`crates/rurge-engine/src/outbounds.rs` 的 `mod tests`

**Interfaces:**
- Consumes: `PolicySpec: Clone + PartialEq`、`ProtoSpec::tls()`（Task 1）、`Config::{specs, keystore, spec(name)}`、`RegistryCell::load()`。
- Produces:
  - `OutboundFactory::environment(&self) -> String`（必须实现，没有默认体）。
  - `PolicyRegistry::build(cfg, factory, cell, selections, previous: Option<&PolicyRegistry>) -> Result<PolicyRegistry, BuildError>`。
  - 干构建不经过注册表（`dry_build` 直接调 `factory.build`），天然不复用。

- [ ] **Step 1: 写失败的用例**

`crates/rurge-policy/src/testing.rs`：`FakeFactory` 加一个字段并实现新方法——

```rust
pub(crate) struct FakeFactory {
    pub connector: Arc<RecordingConnector>,
    /// The policy whose build fails.
    pub broken: Option<&'static str>,
    /// What `environment()` reports.
    pub environment: &'static str,
}
```

`FakeFactory::new()` 里 `environment: "env"`；`impl OutboundFactory for FakeFactory` 里加

```rust
    fn environment(&self) -> String {
        self.environment.to_string()
    }
```

`crates/rurge-policy/src/registry.rs` 的 `mod tests` 里加（`PolicyRef`、`from_text`、`LoadOptions`、`FakeFactory`、`RegistryCell`、`SelectionTable`、`GroupSelections` 都已导入）：

```rust
    fn generation(
        text: &str,
        factory: &FakeFactory,
        previous: Option<&PolicyRegistry>,
    ) -> PolicyRegistry {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded
                .diagnostics
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        );
        PolicyRegistry::build(
            &loaded.config,
            factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::new(GroupSelections::new())),
            previous,
        )
        .expect("builds")
    }

    fn outbound_of(registry: &PolicyRegistry, name: &str) -> OutboundRef {
        registry.resolve(&PolicyRef::parse(name)).outbound
    }

    const REUSE: &str = "[Proxy]\nA = socks5, a.example, 1080, username=u, password=p\n\
B = https, b.example, 443, client-cert=cert1\nCorp = direct, interface=eth9\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=x\n[Rule]\nFINAL,DIRECT\n";

    #[test]
    fn an_untouched_policy_keeps_its_outbound_across_a_reload() {
        let factory = FakeFactory::new();
        let first = generation(REUSE, &factory, None);
        // an unrelated line is added above: every span moves, nothing else does
        let moved = REUSE.replace("[Proxy]\n", "[Proxy]\nNew = http, n.example, 80\n");
        let second = generation(&moved, &factory, Some(&first));
        for name in ["A", "B", "Corp"] {
            assert!(
                Arc::ptr_eq(&outbound_of(&first, name), &outbound_of(&second, name)),
                "{name} was rebuilt"
            );
        }
        // without a previous generation everything is new
        let alone = generation(&moved, &factory, None);
        assert!(!Arc::ptr_eq(&outbound_of(&first, "A"), &outbound_of(&alone, "A")));
    }

    #[test]
    fn a_changed_parameter_keystore_item_or_environment_rebuilds() {
        let factory = FakeFactory::new();
        let first = generation(REUSE, &factory, None);
        // A's own parameter
        let second = generation(&REUSE.replace("password=p", "password=q"), &factory, Some(&first));
        assert!(!Arc::ptr_eq(&outbound_of(&first, "A"), &outbound_of(&second, "A")));
        assert!(Arc::ptr_eq(&outbound_of(&first, "B"), &outbound_of(&second, "B")));
        // the content of the keystore item B refers to
        let second = generation(&REUSE.replace("base64=QUJD", "base64=QUJE"), &factory, Some(&first));
        assert!(!Arc::ptr_eq(&outbound_of(&first, "B"), &outbound_of(&second, "B")));
        assert!(Arc::ptr_eq(&outbound_of(&first, "A"), &outbound_of(&second, "A")));
        // what the factory captured by value
        let other = FakeFactory {
            environment: "other",
            ..FakeFactory::new()
        };
        let second = generation(REUSE, &other, Some(&first));
        for name in ["A", "B", "Corp"] {
            assert!(
                !Arc::ptr_eq(&outbound_of(&first, name), &outbound_of(&second, name)),
                "{name} survived a change of environment"
            );
        }
    }
```

同一个文件与 `cell.rs` 里现有的 `PolicyRegistry::build(..)` 调用各补一个 `None` 实参（共 3 处）。

Run: `cargo test -p rurge-policy`
Expected: FAIL（`environment` 不是 trait 的成员、`build` 多了一个实参：编译错误）。

- [ ] **Step 2: trait 方法**

`crates/rurge-policy/src/factory.rs`，`OutboundFactory` 里 `build` 之后加：

```rust
    /// Whatever the factory captures **by value** that may differ between
    /// config generations: an outbound built under another environment is
    /// never reused. What is read through a cell at dial time (the resolver,
    /// the registry) does not belong here.
    fn environment(&self) -> String;
```

- [ ] **Step 3: 指纹与复用**

`crates/rurge-policy/src/registry.rs`：

`use` 里补 `rurge_config::{KeystoreType, Span}`（与现有的 `rurge_config::{Builtin, Config, GroupKind, PolicyKind}` 合并）和 `std::path::Path`。

`Entry::Outbound` 加一个字段：

```rust
    /// A built proxy, or a `direct` alias with socket options.
    Outbound {
        outbound: OutboundRef,
        proxy: bool,
        fingerprint: Fingerprint,
    },
```

（`named()` 里匹配 `Entry::Outbound { outbound, proxy: true }` / `proxy: false` 的两个分支各补 `..`。）

在 `build_one` 之前加：

```rust
/// Everything an outbound was built from. Two generations that agree on it
/// may share the outbound (M2 design 7.1). No `Debug`: it holds the policy's
/// credentials and the keystore item's.
#[derive(PartialEq)]
struct Fingerprint {
    /// Without its span: an unrelated edit above the line moves it.
    spec: PolicySpec,
    /// `client-cert`'s keystore item, by content.
    keystore: Option<(KeystoreType, String, Option<String>)>,
    environment: String,
}

fn fingerprint(spec: &PolicySpec, cfg: &Config, environment: &str) -> Fingerprint {
    let mut spec = spec.clone();
    spec.span = Span::new(Arc::from(Path::new("")), 0);
    let keystore = spec
        .proto
        .tls()
        .and_then(|tls| tls.client_cert.as_ref())
        .and_then(|name| cfg.keystore.iter().find(|item| &item.name == name))
        .map(|item| (item.kind, item.base64.clone(), item.password.clone()));
    Fingerprint {
        spec,
        keystore,
        environment: environment.to_string(),
    }
}
```

（`KeystoreType` 若没有派生 `Copy`，把 `item.kind` 写成 `item.kind.clone()`；它已经派生了 `PartialEq`。）

`PolicyRegistry::build`：签名加 `previous: Option<&PolicyRegistry>`，文档注释补一句；循环之前取一次环境，两个 `build_one(spec, factory, cell)?` 的调用处改成经过 `reuse_or_build`：

```rust
    /// `cell` is where the chain connectors built here will look the
    /// registry up at dial time; the caller stores the result into it.
    /// `previous` is the generation being replaced: a policy whose
    /// fingerprint did not change keeps the outbound it had there, pools and
    /// all (M2 design 7.1).
    pub fn build(
        cfg: &Config,
        factory: &dyn OutboundFactory,
        cell: &Arc<RegistryCell>,
        selections: Arc<SelectionTable>,
        previous: Option<&PolicyRegistry>,
    ) -> Result<PolicyRegistry, BuildError> {
        let environment = factory.environment();
        let outbound_entry = |spec: &PolicySpec, proxy: bool| -> Result<Entry, BuildError> {
            let fingerprint = fingerprint(spec, cfg, &environment);
            let kept = match previous.and_then(|p| p.entries.get(&spec.name)) {
                Some(Entry::Outbound {
                    outbound,
                    fingerprint: before,
                    ..
                }) if *before == fingerprint => Some(outbound.clone()),
                _ => None,
            };
            let outbound = match kept {
                Some(outbound) => outbound,
                None => build_one(spec, factory, cell)?,
            };
            Ok(Entry::Outbound {
                outbound,
                proxy,
                fingerprint,
            })
        };
```

循环体里的两个分支相应改成：

```rust
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    outbound_entry(spec, false)?
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => outbound_entry(spec, true)?,
```

连接器的指纹不用另算：直连的 socket 选项与 `underlying-proxy` 的名字都在 `PolicySpec.common` 里；链式连接器经跨代稳定的 cell 按名字解析。

Run: `cargo test -p rurge-policy`
Expected: 全部通过（含 Step 1 的两条）。

- [ ] **Step 4: 引擎一侧**

`crates/rurge-engine/src/outbounds.rs`，`impl OutboundFactory for EngineFactory` 里加：

```rust
    fn environment(&self) -> String {
        // the one `[General]` item captured by value: it goes into every
        // direct connector's `SocketOpts`. The roots and the socket hook are
        // per-process; the resolver is read through its cell.
        format!("ipv6={}", self.v6_first)
    }
```

`mod tests` 里加：

```rust
    #[test]
    fn the_environment_follows_what_the_factory_captures_by_value() {
        let v4 = config("[General]\nipv6 = false\n[Rule]\nFINAL,DIRECT\n");
        let v6 = config("[General]\nipv6 = true\n[Rule]\nFINAL,DIRECT\n");
        assert_eq!(factory(&v4).environment(), "ipv6=false");
        assert_eq!(factory(&v6).environment(), "ipv6=true");
    }
```

`crates/rurge-engine/src/runtime.rs`，`PolicyRegistry::build` 的调用处：

```rust
        // The generation being replaced (none on the first build): whatever
        // it built from the same fingerprint is kept, connection pools and all.
        let previous = opts.shared.cell.load();
        let policies = Arc::new(
            PolicyRegistry::build(
                &config,
                &factory,
                &opts.shared.cell,
                opts.shared.selections.clone(),
                previous.as_deref(),
            )
            .map_err(|e| anyhow::anyhow!("cannot build the policies: {e}"))?,
        );
```

Run: `cargo test -p rurge-engine`
Expected: 全部通过。`a_reload_leaves_a_chained_session_alone_and_moves_the_next_one`（M2a）在复用之后仍然成立：`Exit` 的 `underlying-proxy` 变了所以重建，`EntryA` / `EntryB` 被沿用——这正是设计 7.3 最后一条要 M2b 复核的。

- [ ] **Step 5: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat(policy): 重载时按指纹复用出站——去掉 span 的 PolicySpec + 被引用的 Keystore 条目 + 工厂的 environment()；引擎把被替换的那一代传给注册表"
```

---

### Task 9: 引擎装配、旧握手的处理与端到端用例

**Files:**
- Modify: `crates/rurge-config/src/spec/mod.rs`（两个变体、`to_spec` 的两个分支、`legacy_vmess`、`ProtoSpec::tls()` 的两个分支）
- Modify: `crates/rurge-config/src/config.rs`（每次加载一条的旧握手 `W0007`）
- Modify: `crates/rurge-proto/src/build.rs`（`server_of`）
- Modify: `crates/rurge-engine/src/outbounds.rs`（两个新分支、trojan 改用 `server_of`）
- Modify: `crates/rurge-policy/src/registry.rs`（旧握手的说明文本）
- Test: `crates/rurge-config/src/spec/mod.rs`、`crates/rurge-config/src/config.rs`、`crates/rurge-engine/src/outbounds.rs` 的 `mod tests`；`crates/rurge-engine/tests/outbounds.rs`
- **变体、`to_spec` 的分支、工厂的分支必须在同一个提交里**（P12）。

**Interfaces:**
- Consumes: Task 1（`read_vmess` `read_anytls` `VmessSpec` `AnyTlsSpec`）、Task 3（`VmessOutbound::new`）、Task 5（`AnyTlsOutbound::new`）、Task 6（出错的方向结束整条会话）、Task 7 / 8。
- Produces: `ProtoSpec::{Vmess(VmessSpec), AnyTls(AnyTlsSpec)}`；`SpecOutcome.legacy_vmess: bool`；`rurge_proto::build::server_of(&PolicySpec) -> Result<Target, BuildError>`。

- [ ] **Step 1: 写失败的用例（配置层）**

`crates/rurge-config/src/spec/mod.rs` 的 `mod tests`（用现有的 `outcome(name, def)` 助手）：

```rust
    #[test]
    fn vmess_and_anytls_lines_become_specs() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let o = outcome("V", &format!("vmess, h.test, 443, username={id}, vmess-aead=true, tls=true"));
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert!(!o.legacy_vmess);
        let spec = o.spec.expect("a spec");
        let ProtoSpec::Vmess(vmess) = &spec.proto else {
            panic!("not vmess: {:?}", spec.proto);
        };
        assert!(vmess.tls.is_some() && spec.proto.tls().is_some());
        let o = outcome("A", "anytls, h.test, 443, password=pw, reuse=false");
        let spec = o.spec.expect("a spec");
        let ProtoSpec::AnyTls(anytls) = &spec.proto else {
            panic!("not anytls: {:?}", spec.proto);
        };
        assert!(!anytls.reuse && spec.proto.tls().is_some());
    }

    #[test]
    fn a_vmess_line_without_the_aead_handshake_has_no_spec() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let o = outcome("V", &format!("vmess, h.test, 443, username={id}, ws=true"));
        assert!(o.spec.is_none());
        assert!(o.legacy_vmess);
        assert!(o.diagnostics.is_empty(), "the loader reports it, once: {:?}", o.diagnostics);
        // a broken legacy line is an error like any other, and not "legacy"
        let o = outcome("V", "vmess, h.test, 443, username=nope");
        assert!(o.spec.is_none() && !o.legacy_vmess);
        assert_eq!(o.diagnostics.len(), 1);
    }
```

`crates/rurge-config/src/config.rs` 的 `mod tests`（沿用该模块里现成的加载助手；下面用 `from_text`）：

```rust
    #[test]
    fn legacy_vmess_lines_are_reported_once_per_load() {
        let id = "0233d11c-15a4-47d3-ade3-48ffca0ce119";
        let text = format!(
            "[Proxy]\nOld1 = vmess, a.test, 443, username={id}\nOld2 = vmess, b.test, 443, username={id}\n\
New = vmess, c.test, 443, username={id}, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n"
        );
        let loaded = from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests());
        let legacy: Vec<&Diagnostic> = loaded
            .diagnostics
            .iter()
            .filter(|d| d.code == codes::W_PROTOCOL_NOT_IMPLEMENTED)
            .collect();
        assert_eq!(legacy.len(), 1, "{legacy:?}");
        assert_eq!(
            legacy[0].message,
            "`vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet"
        );
        assert_eq!(legacy[0].span.as_ref().map(|s| s.line), Some(2));
        assert!(loaded.config.spec("New").is_some());
        assert!(loaded.config.spec("Old1").is_none() && loaded.config.spec("Old2").is_none());
    }
```

Run: `cargo test -p rurge-config vmess`
Expected: FAIL（`ProtoSpec::Vmess`、`legacy_vmess` 不存在：编译错误）。

- [ ] **Step 2: 配置层的装配**

`crates/rurge-config/src/spec/mod.rs`：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoSpec {
    Direct,
    Reject(Builtin),
    Http(HttpSpec),
    Socks5(Socks5Spec),
    Trojan(TrojanSpec),
    Vmess(VmessSpec),
    AnyTls(AnyTlsSpec),
}
```

`ProtoSpec::tls()` 补两个分支：

```rust
            ProtoSpec::Vmess(vmess) => vmess.tls.as_ref(),
            ProtoSpec::AnyTls(anytls) => Some(&anytls.tls),
```

`SpecOutcome` 加字段（带文档）：

```rust
    /// A `vmess` line without `vmess-aead=true`: valid, but it asks for the
    /// legacy handshake, so it has no spec. The caller reports it once per
    /// load (`W0007`, M2 design 4.3).
    pub legacy_vmess: bool,
```

`to_spec`：在 `let mut notes = Notes::default();` 之后加 `let mut legacy_vmess = false;`；`PolicyKind::Trojan` 分支之后加两个分支：

```rust
        PolicyKind::Vmess => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::note_shadow_tls(&mut r, &mut notes);
            let read = vmess::read_vmess(&mut r, env.keystore);
            // the rest of the line is still checked; it just has no spec
            legacy_vmess = !read.aead;
            (common, ProtoSpec::Vmess(read.spec))
        }
        PolicyKind::AnyTls => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::note_shadow_tls(&mut r, &mut notes);
            let anytls = anytls::read_anytls(&mut r, env.keystore);
            (common, ProtoSpec::AnyTls(anytls))
        }
```

函数末尾：

```rust
    let failed = r.has_errors();
    let diagnostics = r.finish();
    let spec = (!failed && !legacy_vmess).then(|| PolicySpec {
        name: policy.name.clone(),
        kind: policy.kind,
        server: policy.server.clone(),
        port: policy.port,
        common,
        proto,
        span: policy.span.clone(),
    });
    SpecOutcome {
        spec,
        diagnostics,
        inert: notes.inert,
        ios_only: notes.ios_only,
        legacy_vmess: legacy_vmess && !failed,
    }
```

`crates/rurge-config/src/config.rs`，构建 specs 的那个循环：在 `let mut ios_seen ...` 之后加 `let mut legacy_seen = false;`，在 `for name in outcome.ios_only { .. }` 之后、`specs.extend(outcome.spec);` 之前加：

```rust
            if outcome.legacy_vmess && !legacy_seen {
                legacy_seen = true;
                diags.push(
                    Diagnostic::warning(
                        codes::W_PROTOCOL_NOT_IMPLEMENTED,
                        "`vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet".to_string(),
                    )
                    .at(p.span.clone()),
                );
            }
```

Run: `cargo test -p rurge-config`
Expected: 这一步之后 `rurge-config` 自己通过；`rurge-engine` **编不过**（`EngineFactory::build` 的穷尽匹配缺两个分支）——所以不要在这里提交，接着做 Step 3。语料库快照（`cargo insta test -p rurge-config`）：`kitchen-sink.conf` 里的两行本来就写全了参数，预期**没有**快照差异；若有，先读差异再决定（只接受"这两行现在有了 spec / 多了一条确实成立的告警"这类变化），用 `cargo insta test -p rurge-config --accept` 接受，并把差异贴进报告。

- [ ] **Step 3: `server_of` 与引擎工厂的两个分支**

`crates/rurge-proto/src/build.rs`：`use` 里补 `rurge_config::spec::PolicySpec`（与 `TlsOpts` 合并）和 `rurge_net::connector::Target`，在 `tls_client` 之前加：

```rust
/// The proxy server `spec` names. Every protocol that dials a server needs
/// both halves; a spec without them never came out of `rurge-config`.
pub fn server_of(spec: &PolicySpec) -> Result<Target, BuildError> {
    match (&spec.server, spec.port) {
        (Some(host), Some(port)) => Ok(Target::new(host.clone(), port)),
        _ => Err(BuildError::new(format!(
            "a {} policy needs a server and a port",
            spec.kind.keyword()
        ))),
    }
}
```

`mod tests` 里加：

```rust
    #[test]
    fn a_spec_without_a_server_is_refused_by_name_of_its_protocol() {
        use rurge_config::config::{LoadOptions, from_text};
        let text = "[Proxy]\nT = trojan, proxy.test, 443, password=pw\n[Rule]\nFINAL,DIRECT\n";
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        let mut spec = loaded.config.spec("T").expect("a spec").clone();
        let server = server_of(&spec).unwrap();
        assert_eq!((server.host.to_string(), server.port), ("proxy.test".to_string(), 443));
        spec.port = None;
        assert_eq!(
            server_of(&spec).unwrap_err().message,
            "a trojan policy needs a server and a port"
        );
    }
```

`crates/rurge-engine/src/outbounds.rs`：`use` 里补 `rurge_proto::anytls::AnyTlsOutbound`、`rurge_proto::build::server_of`、`rurge_proto::vmess::VmessOutbound`，去掉不再用到的 `Target`（若编译器说它还在别处用就留着）。`build` 里 trojan 的分支改用 `server_of`，并加两个分支：

```rust
            ProtoSpec::Trojan(trojan) => Arc::new(TrojanOutbound::new(
                &spec.name,
                server_of(spec)?,
                trojan,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::Vmess(vmess) => Arc::new(VmessOutbound::new(
                &spec.name,
                server_of(spec)?,
                vmess,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::AnyTls(anytls) => Arc::new(AnyTlsOutbound::new(
                &spec.name,
                server_of(spec)?,
                anytls,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

同一个文件的 `mod tests`：`every_implemented_protocol_builds` 的配置里加两行、循环的名单里加两项——

```text
V = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, ws=true
A = anytls, proxy.test, 443, password=pw
```

```rust
            ("V", "V"),
            ("A", "A"),
```

再加两条：

```rust
    #[test]
    fn a_vmess_or_anytls_policy_that_cannot_be_built_is_a_load_error() {
        let cfg = config(
            "[Proxy]\nV = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, client-cert=cert1\n\
A = anytls, proxy.test, 443, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("policy `V` cannot be built: keystore item `cert1`"));
        assert!(messages[1].starts_with("policy `A` cannot be built: keystore item `cert1`"));
        for m in &messages {
            assert!(
                !m.contains("hunter2") && !m.contains("s3same0pen") && !m.contains("0233d11c"),
                "{m}"
            );
        }
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
        // no tokio runtime here: spawning anything at build time would panic
        let cfg = config("[Proxy]\nA = anytls, proxy.test, 443, password=pw\n[Rule]\nFINAL,DIRECT\n");
        assert!(dry_build(&cfg).is_empty());
    }

    #[test]
    fn skip_cert_verify_is_noticed_on_the_new_protocols_too() {
        let cfg = config(
            "[Proxy]\nA = anytls, proxy.test, 443, password=pw, skip-cert-verify=true\n\
V = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, skip-cert-verify=true\n\
Plain = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n",
        );
        let flagged: Vec<&str> = cfg
            .specs
            .iter()
            .filter(|s| skips_verification(s))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(flagged, ["A", "V"]);
    }
```

- [ ] **Step 4: 运行期的旧握手文本**

`crates/rurge-policy/src/registry.rs`：`Entry::Unsupported` 的说明文本不再总是关键字——

```rust
/// What the session log says about a policy that has no spec. Since M2b a
/// `vmess` policy of a profile that loaded is only ever without one for a
/// single reason: the line lacks `vmess-aead=true` (M2 design 4.3).
fn unsupported_text(kind: PolicyKind) -> String {
    match kind {
        PolicyKind::Vmess => "vmess (legacy handshake)".to_string(),
        other => other.keyword().to_string(),
    }
}
```

`named()` 里：

```rust
            Some(Entry::Unsupported { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                self.rejected(chain, Some(Note::Unsupported(unsupported_text(*kind))))
            }
```

`mod tests` 里加（`built()` 用的 `PROFILE` 不含 vmess，单独建一个）：

```rust
    #[test]
    fn a_legacy_vmess_policy_says_why_it_rejects() {
        let text = "[Proxy]\nOld = vmess, a.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
SS = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
        let registry = generation(text, &FakeFactory::new(), None);
        let old = registry.resolve(&PolicyRef::parse("Old"));
        assert_eq!(old.terminal, TerminalKind::Reject);
        assert_eq!(
            old.note,
            Some(Note::Unsupported("vmess (legacy handshake)".into()))
        );
        assert_eq!(chain(&old), ["Old", "!unsupported:vmess", "REJECT"]);
        let ss = registry.resolve(&PolicyRef::parse("SS"));
        assert_eq!(ss.note, Some(Note::Unsupported("ss".into())));
    }
```

（会话日志的 `policy protocol not implemented: <文本>` 由引擎从 `Note::Unsupported` 取词，不用改。）

Run: `cargo test -p rurge-config && cargo test -p rurge-proto build && cargo test -p rurge-policy && cargo test -p rurge-engine --lib`
Expected: 全部通过。

- [ ] **Step 5: 端到端用例**

`crates/rurge-engine/tests/outbounds.rs`：`use rurge_proto::testing::{..}` 的清单里补 `AnyTlsScript, FakeAnyTls, FakeVmess, VmessScript`，在 trojan 的助手之后加：

```rust
const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

fn pin_of(fixture: &TlsFixture) -> String {
    fixture
        .leaf_fingerprint()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fake VMess server relaying to `to`, and the parameters that reach it.
/// The harness trusts the OS roots, so the fixture's leaf is pinned.
async fn vmess_upstream(tls: bool, ws: bool, to: SocketAddr) -> (FakeVmess, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let mut params = format!("username={VMESS_ID}, vmess-aead=true");
    if tls {
        params += &format!(", tls=true, server-cert-fingerprint-sha256={}", pin_of(&fixture));
    }
    if ws {
        params += ", ws=true, ws-path=/v";
    }
    let script = VmessScript {
        ws,
        connect_to: Some(to),
        ..VmessScript::new(VMESS_ID)
    };
    (FakeVmess::spawn(script, tls.then_some(fixture)).await, params)
}

async fn anytls_upstream(to: SocketAddr) -> (FakeAnyTls, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let params = format!(
        "password=s3same, server-cert-fingerprint-sha256={}",
        pin_of(&fixture)
    );
    let script = AnyTlsScript {
        password: "s3same".into(),
        connect_to: Some(to),
        ..AnyTlsScript::default()
    };
    (FakeAnyTls::spawn(script, fixture).await, params)
}

#[tokio::test]
async fn a_connect_leaves_through_a_vmess_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    for (tls, ws) in [(false, false), (true, true)] {
        let (upstream, params) = vmess_upstream(tls, ws, origin_addr(&origin)).await;
        let h = harness(Profile {
            proxies: &format!("V = vmess, 127.0.0.1, {}, {params}", upstream.addr().port()),
            rules: "DOMAIN,target.test,V",
            ..Profile::default()
        })
        .await;
        let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
        let response = get(&mut tunnel, "target.test", "/hello").await;
        assert!(response.ends_with("hi there"), "tls={tls} ws={ws}: {response}");
        let seen = upstream.requests();
        let first = seen.first().expect("the upstream never saw a request");
        assert_eq!(
            (first.command, first.atyp, first.host.as_str(), first.port),
            (1, 2, "target.test", 8080),
            "the server resolves the name: rurge never looked it up"
        );
        assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
        drop(tunnel);
        let log = h.engine.request_log();
        wait_until("the session to finish", || !log.recent(10).is_empty()).await;
        let record = &log.recent(10)[0];
        assert_eq!(record.policy, ["V"]);
        assert!(record.error.is_none(), "{:?}", record.error);
    }
}

#[tokio::test]
async fn a_wrong_vmess_id_ends_the_session_with_a_text_that_says_so() {
    let origin = TestServer::spawn().await;
    let (upstream, _) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "V = vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,V",
        ..Profile::default()
    })
    .await;
    // the tunnel comes up: the server only answers a request it accepts
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    tunnel
        .write_all(b"GET / HTTP/1.1\r\nHost: target.test\r\n\r\n")
        .await
        .unwrap();
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut rest))
        .await
        .expect("the tunnel closes within the bound");
    assert_eq!(upstream.rejected(), 1);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(
        error,
        "vmess: the server closed the connection without answering"
    );
    assert!(!error.contains("0233"));
}

#[tokio::test]
async fn an_anytls_upstream_carries_two_requests_over_one_session() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!("A = anytls, 127.0.0.1, {}, {params}", upstream.addr().port()),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    for round in 1..=2 {
        let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
        let response = get(&mut tunnel, "target.test", "/hello").await;
        assert!(response.ends_with("hi there"), "{response}");
        drop(tunnel);
        // the stream is over when the server has seen its FIN: only then is
        // the session back in the pool for the next request
        wait_until("the stream to close", || upstream.fins() == round).await;
    }
    assert_eq!(upstream.sessions(), 1, "the second request reused the session");
    let streams = upstream.streams();
    assert_eq!(
        streams.iter().map(|s| (s.sid, s.host.as_str(), s.port)).collect::<Vec<_>>(),
        [(1, "target.test", 8080), (2, "target.test", 8080)]
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
}

#[tokio::test]
async fn the_new_protocols_work_at_either_end_of_a_chain() {
    let echo = rurge_proto::testing::echo_server().await;
    // entry: vmess; exit: anytls, reached by name through the entry
    let (exit, exit_params) = anytls_upstream(echo).await;
    let (entry, entry_params) = vmess_upstream(false, false, exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = vmess, 127.0.0.1, {}, {entry_params}\nExit = anytls, exit.example, 443, {exit_params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"vmess then anytls").await;
    let asked = &entry.requests()[0];
    assert_eq!(
        (asked.host.as_str(), asked.port),
        ("exit.example", 443),
        "the entry is asked for the exit by name"
    );
    assert_eq!(exit.streams()[0].host, "target.test");

    // and the other way round
    let (exit, exit_params) = vmess_upstream(false, false, echo).await;
    let (entry, entry_params) = anytls_upstream(exit.addr()).await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = anytls, 127.0.0.1, {}, {entry_params}\nExit = vmess, exit.example, 443, {exit_params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"anytls then vmess").await;
    assert_eq!(entry.streams()[0].host, "exit.example");
    assert_eq!(exit.requests()[0].host, "target.test");
}
```

设计 7.3 的复用用例（前两条在这里；"解析器"那条是 Task 7 的用例加上 Task 8 的复用，下面第三条把两者接起来；第四条是 M2a 的 `a_reload_leaves_a_chained_session_alone_and_moves_the_next_one`，Task 8 已复核）：

```rust
fn outbound_now(h: &Harness, name: &str) -> rurge_proto::OutboundRef {
    h.engine
        .runtime()
        .policies
        .resolve(&rurge_config::rule::PolicyRef::parse(name))
        .outbound
}

#[tokio::test]
async fn an_unrelated_reload_keeps_an_anytls_pool_and_a_change_of_its_own_drops_it() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = anytls_upstream(origin_addr(&origin)).await;
    let port = upstream.addr().port();
    let profile = |extra: &str, a_extra: &str| {
        format!("A = anytls, 127.0.0.1, {port}, {params}{a_extra}\n{extra}")
    };
    let h = harness(Profile {
        proxies: &profile("", ""),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    })
    .await;
    let request = |h: &Harness| {
        let http = h.http();
        async move {
            let mut tunnel = connect_via_http(http, "target.test:8080").await;
            let response = get(&mut tunnel, "target.test", "/hello").await;
            assert!(response.ends_with("hi there"), "{response}");
        }
    };
    request(&h).await;
    wait_until("the first stream to close", || upstream.fins() == 1).await;
    let before = outbound_now(&h, "A");

    // a reload that has nothing to do with A
    let next = Profile {
        proxies: &profile("Other = http, other.example, 8080", ""),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(Arc::ptr_eq(&before, &outbound_now(&h, "A")), "A was rebuilt");
    request(&h).await;
    wait_until("the second stream to close", || upstream.fins() == 2).await;
    assert_eq!(upstream.sessions(), 1, "the idle session survived the reload");

    // a reload that changes A itself
    let next = Profile {
        proxies: &profile("Other = http, other.example, 8080", ", reuse=false"),
        rules: "DOMAIN,target.test,A",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(!Arc::ptr_eq(&before, &outbound_now(&h, "A")), "A was kept");
    request(&h).await;
    assert_eq!(upstream.sessions(), 2, "the new outbound dialled for itself");
}

#[tokio::test]
async fn a_reused_outbound_resolves_through_the_new_generation() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(echo),
        ..Socks5Script::default()
    })
    .await;
    let proxies = format!("S = socks5, proxy.test, {}", upstream.addr().port());
    let h = harness(Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    })
    .await;
    h.dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let before = outbound_now(&h, "S");
    let other = MockDns::spawn().await;
    for name in ["proxy.test", "target.test"] {
        other.set(name, &["127.0.0.1"], &[], 60);
    }
    let next = Profile {
        proxies: &proxies,
        rules: "DOMAIN,target.test,S",
        ..Profile::default()
    }
    .text(other.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &next, h.engine.shared()).await);
    assert!(Arc::ptr_eq(&before, &outbound_now(&h, "S")), "only `dns-server` changed");
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"through the reused outbound").await;
    let asked = |dns: &MockDns| dns.queries().iter().any(|(q, _)| q.name == "proxy.test");
    assert!(asked(&other) && !asked(&h.dns));
}
```

Run: `cargo test -p rurge-engine --test outbounds`
Expected: 全部通过。`a_wrong_vmess_id_ends_the_session_with_a_text_that_says_so` 依赖 Task 6 的"一个方向出错就结束另一个方向"：没有它，这条用例会卡到上界（上行方向还在等客户端）。

- [ ] **Step 6: 门禁与提交（一个提交）**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat: vmess / anytls 接进配置层与引擎——ProtoSpec 两个变体、to_spec、EngineFactory、server_of；没写 vmess-aead=true 的行按 W0007 + REJECT；端到端、链与重载复用的用例"
```

---

### Task 10: 能力表翻转 `vmess` / `anytls`、CLI 用例与守卫核对

翻转是让死路径变活的时刻（M1b 的 Critical 就漏在这里）：先核对守卫，再翻。

**Files:**
- Modify: `crates/rurge/src/capabilities.rs`
- Modify: `crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 9 之后 `vmess`（写了 `vmess-aead=true`）与 `anytls` 行都有 spec、都能构建。
- Produces: `rurge check` 不再对这两个类型报通用的 `W0007`；没写 `vmess-aead=true` 的行仍报那条专门的 `W0007`。

- [ ] **Step 1: 守卫核对（只读，结论写进报告）**

逐条确认设计承诺过的守卫在代码里真的存在；每条给出 `文件:行号`。缺任何一条就**停下来报告**，不要翻转：

| 守卫 | 去哪里找 |
| ---- | -------- |
| 目标主机名经 `hostname::to_ascii`，发不出去的名字**在拨号之前**就失败 | `crates/rurge-proto/src/addr.rs` 的 `vmess_addr` / `socks_addr`；`vmess/mod.rs` 与 `anytls/mod.rs` 的 `connect_tcp` 第一步 |
| 整条阶梯 + 协议自己的握手在**一个** `timeout(opts.timeout, ..)` 里 | 两个出站的 `connect_tcp` |
| 对端文本过 `untrusted_text`，且有界 | `anytls/session.rs`（SYNACK 文本、alert 文本） |
| 长度先校验后分配 | `vmess/header.rs` 的 `MAX_RESPONSE_HEAD`、`vmess/stream.rs` 的 `len < TAG`、`anytls/padding.rs` 的四个上限 |
| 出站与流对象不实现 `Debug`；spec 的凭据是 `Secret` | `grep -n "derive(.*Debug" crates/rurge-proto/src/vmess crates/rurge-proto/src/anytls -r`：命中的只应是不含密钥的类型（`Security` `ResponseError` `Entry` `Piece` `Scheme`） |
| 干构建不留后台任务 | Task 9 的 `a_dry_build_of_an_anytls_policy_leaves_no_task_behind` |
| `skip-cert-verify` 的 WARN 覆盖新协议 | `skips_verification` 走 `ProtoSpec::tls()`；Task 9 的 `skip_cert_verify_is_noticed_on_the_new_protocols_too` |
| 防环回退与协议无关 | `Engine::dial_internal`（按"要在本机解析其服务器名的那一跳"判断）；不需要改，读一遍确认没有按协议名单写死 |

- [ ] **Step 2: 写失败的 CLI 用例**

`crates/rurge/tests/cli.rs`，紧跟 `check_knows_trojan` 之后：

```rust
const VMESS_ANYTLS: &str = "[General]\n[Proxy]\n\
V = vmess, proxy.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, ws=true, ws-path=/w\n\
A = anytls, proxy.test, 443, password=s3same\n\
Legacy1 = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
Legacy2 = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119\n\
[Rule]\nFINAL,DIRECT\n";
const VMESS_BAD_ID: &str = "[General]\n[Proxy]\nV = vmess, proxy.test, 443, username=s3cretnotauuid, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n";

#[test]
fn check_knows_vmess_and_anytls() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "m2b.conf", VMESS_ANYTLS))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // one W0007, and it is the one about the legacy handshake — once, not per line
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("uses the legacy handshake"), "{out}");
    assert!(out.contains("m2b.conf:5"), "{out}");
    assert!(!out.contains("`anytls`") && !out.contains("0233d11c"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", VMESS_BAD_ID))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains("bad.conf:3"))
        .stdout(predicate::str::contains("s3cretnotauuid").not());
}
```

Run: `cargo test -p rurge --test cli check_knows_vmess_and_anytls`
Expected: FAIL（现在有两条 `W0007`：通用的 `vmess` 那条与 `anytls` 那条，再加旧握手那条）。

- [ ] **Step 3: 翻转**

`crates/rurge/src/capabilities.rs`：`policy_kinds` 里在 `PolicyKind::Trojan,` 之后加 `PolicyKind::Vmess,` 与 `PolicyKind::AnyTls,`；文件头注释改为

```rust
//! What this build of rurge actually implements: the built-in alias
//! policies, the HTTP / SOCKS5 proxy family (phase 2 M1), `trojan`
//! (phase 2 M2a), `vmess` with the AEAD handshake and `anytls` (phase 2
//! M2b), and `select` groups.
```

Run: `cargo test -p rurge`
Expected: 全部通过。`check_knows_trojan` 里"`ss` 仍是 `W0007`"的断言不受影响。

- [ ] **Step 4: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "feat(bin): 能力表翻转 vmess（AEAD）与 anytls；CLI 用例"
```

---

### Task 11: 互操作——sing-box（vmess ± tls ± ws、anytls）与 xray（vmess ± ws），CI 安装并校验 xray

本机没有这两个二进制，也**不要安装**：本任务的用例在本机都会打印原因并跳过，由首次推送后的 CI 证明（`RURGE_INTEROP_REQUIRED=1`）。本机能验证的是渲染出的配置与夹具的安全守卫。

**Files:**
- Modify: `tests/interop/src/lib.rs`（`InboundKind::{Vmess, AnyTls}`；把"起子进程、等端口、收日志、退出时杀掉"抽成两个夹具共用的私有类型）
- Create: `tests/interop/src/xray.rs`
- Modify: `tests/interop/tests/sing_box.rs`；Create: `tests/interop/tests/xray.rs`
- Modify: `tests/interop/README.md`、`.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `EngineFactory` 能构建 `vmess` / `anytls`（Task 9）；`TlsFixture::{leaf_pem, leaf_key_pem, roots}`。
- Produces: `rurge_interop::InboundKind::{Vmess, AnyTls}`；`rurge_interop::xray::{BINARY_ENV, xray_or_skip, XrayInbound, render, Xray}`。

- [ ] **Step 1: sing-box 夹具认识两种新入站（先写用例）**

`tests/interop/src/lib.rs` 的 `mod tests`：`every_kind()` 末尾加两项——

```rust
            (
                Inbound {
                    kind: InboundKind::Vmess,
                    users: vec![("u".into(), "0233d11c-15a4-47d3-ade3-48ffca0ce119".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: None,
                    }),
                    ws_path: Some("/v".into()),
                },
                1005,
            ),
            (
                Inbound {
                    kind: InboundKind::AnyTls,
                    users: vec![("u".into(), "pw".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: None,
                    }),
                    ws_path: None,
                },
                1006,
            ),
```

`inbounds_are_rendered_as_sing_box_spells_them` 末尾加：

```rust
        let vmess = &config["inbounds"][4];
        assert_eq!(vmess["type"], "vmess");
        // `alterId: 0` is what makes sing-box expect the AEAD handshake
        assert_eq!(
            vmess["users"],
            json!([{ "name": "u", "uuid": "0233d11c-15a4-47d3-ade3-48ffca0ce119", "alterId": 0 }])
        );
        assert_eq!(vmess["transport"], json!({ "type": "ws", "path": "/v" }));
        let anytls = &config["inbounds"][5];
        assert_eq!(anytls["type"], "anytls");
        assert_eq!(anytls["users"], json!([{ "name": "u", "password": "pw" }]));
        assert_eq!(anytls["tls"]["enabled"], true);
```

（`the_configuration_never_touches_the_machine` 不用改：它遍历 `every_kind()`，新入站自动进来。）

实现：`InboundKind` 加 `Vmess`、`AnyTls`；`render` 里类型名的 `match` 补 `InboundKind::Vmess => "vmess"`、`InboundKind::AnyTls => "anytls"`；用户的渲染改为

```rust
                    .map(|(u, p)| match inbound.kind {
                        // sing-box's trojan and anytls users are `name` + `password`
                        InboundKind::Trojan | InboundKind::AnyTls => {
                            json!({ "name": u, "password": p })
                        }
                        // the second half is the id; `alterId: 0` selects the AEAD handshake
                        InboundKind::Vmess => json!({ "name": u, "uuid": p, "alterId": 0 }),
                        _ => json!({ "username": u, "password": p }),
                    })
```

`tls` 的断言放宽为 `Http | Trojan | Vmess | AnyTls`（文案同步）；`ws_path` 的断言放宽为 `Trojan | Vmess`（文案同步）。`Inbound` 两个字段的文档注释相应更新。

Run: `cargo test -p rurge-interop --lib`
Expected: 全部通过。

- [ ] **Step 2: 两个夹具共用的子进程类型**

`tests/interop/src/lib.rs`：把 `SingBox` 里"等端口 / 读日志 / 退出时杀掉"挪进一个私有类型，`SingBox` 变成它的薄包装（对外的 `spawn` `port` `log_text` 不变）：

```rust
/// A reference implementation running as a child on the loopback; killed and
/// reaped on drop.
pub(crate) struct Reference {
    what: &'static str,
    child: Child,
    ports: Vec<u16>,
    log: PathBuf,
}

impl Reference {
    /// Starts `command` with its output in `log` and waits until every port
    /// accepts connections.
    pub(crate) fn start(
        what: &'static str,
        mut command: Command,
        ports: Vec<u16>,
        log: PathBuf,
    ) -> Reference {
        let out = std::fs::File::create(&log).expect("create the log");
        let child = command
            .stdin(Stdio::null())
            .stdout(out.try_clone().expect("clone the log handle"))
            .stderr(out)
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {what}: {e}"));
        let mut running = Reference {
            what,
            child,
            ports,
            log,
        };
        running.wait_ready();
        running
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        for port in self.ports.clone() {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            loop {
                // Before the connect, not after: if the child is already dead
                // and an unrelated process happens to hold `port`, a connect
                // that comes first reads as "ready".
                if let Ok(Some(status)) = self.child.try_wait() {
                    panic!("{} exited early ({status}):\n{}", self.what, self.log_text());
                }
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "{} never listened on {addr}:\n{}",
                    self.what,
                    self.log_text()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    pub(crate) fn port(&self, index: usize) -> u16 {
        self.ports[index]
    }

    pub(crate) fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Reference {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
```

`SingBox` 改成 `pub struct SingBox(Reference);`，`spawn` 里组好 `Command`（`run -c <config> -D <dir>`）后交给 `Reference::start("sing-box", command, ports, dir.join("sing-box.log"))`，`port` / `log_text` 转调；原来的 `wait_ready` 与 `impl Drop for SingBox` 删掉。

Run: `cargo test -p rurge-interop`
Expected: 全部通过（本机：互操作用例逐条打印跳过原因）。

- [ ] **Step 3: xray 夹具**

`tests/interop/src/lib.rs` 加 `pub mod xray;`。`tests/interop/src/xray.rs`：

```rust
//! An xray child process on the loopback, for the VMess interoperability
//! tests only: VMess was defined by this family of implementations, and
//! sing-box's is a rewrite, so rurge's hand-written codec is checked against
//! both (M2 design, M2-D5). The same rules as for sing-box apply: nothing is
//! downloaded or installed here, the rendered configuration listens on
//! 127.0.0.1 only, and its single outbound is `freedom`.

use crate::{REQUIRED_ENV, Reference, free_port};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_XRAY";

/// `RURGE_TEST_XRAY`, else the first `xray` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) { "xray.exe" } else { "xray" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn xray_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no xray was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no xray ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// A VMess inbound. xray only knows the AEAD handshake.
pub struct XrayInbound {
    pub uuid: String,
    /// A WebSocket transport on this path.
    pub ws_path: Option<String>,
}

/// The whole configuration for `inbounds`, each on its loopback port.
pub fn render(inbounds: &[(XrayInbound, u16)]) -> Value {
    let rendered: Vec<Value> = inbounds
        .iter()
        .enumerate()
        .map(|(i, (inbound, port))| {
            let mut v = json!({
                "tag": format!("in-{i}"),
                "listen": "127.0.0.1",
                "port": port,
                "protocol": "vmess",
                "settings": { "clients": [{ "id": inbound.uuid }] },
            });
            if let Some(path) = &inbound.ws_path {
                v["streamSettings"] = json!({ "network": "ws", "wsSettings": { "path": path } });
            }
            v
        })
        .collect();
    json!({
        "log": { "loglevel": "warning" },
        "inbounds": rendered,
        "outbounds": [{ "protocol": "freedom", "tag": "direct" }],
    })
}

/// A running xray; killed and reaped on drop.
pub struct Xray(Reference);

impl Xray {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every inbound accepts connections.
    pub fn spawn(binary: &Path, dir: &Path, inbounds: Vec<XrayInbound>) -> Xray {
        let with_ports: Vec<(XrayInbound, u16)> =
            inbounds.into_iter().map(|i| (i, free_port())).collect();
        let ports: Vec<u16> = with_ports.iter().map(|(_, p)| *p).collect();
        let config = dir.join("xray.json");
        std::fs::write(&config, render(&with_ports).to_string()).expect("write the config");
        let mut command = Command::new(binary);
        command.arg("run").arg("-c").arg(&config).current_dir(dir);
        Xray(Reference::start("xray", command, ports, dir.join("xray.log")))
    }

    /// The loopback port of the `index`-th inbound.
    pub fn port(&self, index: usize) -> u16 {
        self.0.port(index)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn both() -> Vec<(XrayInbound, u16)> {
        let uuid = "0233d11c-15a4-47d3-ade3-48ffca0ce119".to_string();
        vec![
            (
                XrayInbound {
                    uuid: uuid.clone(),
                    ws_path: None,
                },
                2001,
            ),
            (
                XrayInbound {
                    uuid,
                    ws_path: Some("/v".into()),
                },
                2002,
            ),
        ]
    }

    #[test]
    fn the_configuration_never_touches_the_machine() {
        let config = render(&both());
        let text = config.to_string();
        for forbidden in ["set_system_proxy", "tun", "auto_route", "0.0.0.0", "::", "dokodemo", "sockopt"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["inbounds", "log", "outbounds"]);
        for inbound in config["inbounds"].as_array().unwrap() {
            assert_eq!(inbound["listen"], "127.0.0.1");
            assert_eq!(inbound["protocol"], "vmess");
        }
        assert_eq!(
            config["outbounds"],
            json!([{ "protocol": "freedom", "tag": "direct" }])
        );
    }

    #[test]
    fn inbounds_are_rendered_as_xray_spells_them() {
        let config = render(&both());
        let plain = &config["inbounds"][0];
        assert_eq!(plain["port"], 2001);
        assert_eq!(
            plain["settings"],
            json!({ "clients": [{ "id": "0233d11c-15a4-47d3-ade3-48ffca0ce119" }] })
        );
        assert!(plain.get("streamSettings").is_none());
        assert_eq!(
            config["inbounds"][1]["streamSettings"],
            json!({ "network": "ws", "wsSettings": { "path": "/v" } })
        );
    }
}
```

（`REQUIRED_ENV`、`free_port` 已是 `pub`；`Reference` 是 `pub(crate)`，同一个 crate 里可见。）

Run: `cargo test -p rurge-interop --lib`
Expected: 全部通过。

- [ ] **Step 4: 互操作用例**

`tests/interop/tests/sing_box.rs` 末尾：

```rust
const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

fn vmess_inbound(tls: Option<TlsFiles>, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Vmess,
        users: vec![("u".into(), VMESS_ID.into())],
        tls,
        ws_path: ws_path.map(str::to_string),
    }
}

#[tokio::test]
async fn vmess_with_and_without_tls_and_websocket() {
    let Some(bin) = sing_box_or_skip("vmess_with_and_without_tls_and_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            vmess_inbound(None, None),
            vmess_inbound(None, Some("/v")),
            vmess_inbound(Some(leaf_files(&fixture, dir.path())), None),
            vmess_inbound(Some(leaf_files(&fixture, dir.path())), Some("/v")),
        ],
    );
    let echo = echo_server().await;
    let line = |name: &str, index: usize, rest: &str| {
        format!(
            "{name} = vmess, 127.0.0.1, {}, username={VMESS_ID}, vmess-aead=true{rest}\n",
            sb.port(index)
        )
    };
    let profile = format!(
        "[Proxy]\n{}{}{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("Plain", 0, ""),
        line("Chacha", 0, ", encrypt-method=chacha20-ietf-poly1305"),
        line("Ws", 1, ", ws=true, ws-path=/v"),
        line("Tls", 2, ", tls=true"),
        line("TlsWs", 3, ", tls=true, ws=true, ws-path=/v"),
    );
    for name in ["Plain", "Chacha", "Ws", "Tls", "TlsWs"] {
        let out = outbound(&profile, name, Some(&fixture));
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
    }
}

/// More than one chunk each way: the length masks and the nonce counter have
/// to stay in step. Bounded like `roundtrip`.
async fn roundtrip_big(out: &OutboundRef, echo: SocketAddr) {
    let bound = std::time::Duration::from_secs(20);
    let payload: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let stream = tokio::time::timeout(
        bound,
        out.connect_tcp(&target(echo), &ConnectOpts::default()),
    )
    .await
    .expect("the tunnel is established within the bound")
    .expect("the tunnel is established");
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; payload.len()];
    let both = async {
        tokio::join!(
            async { writer.write_all(&payload).await.unwrap() },
            async { reader.read_exact(&mut back).await.unwrap() }
        )
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("100 000 bytes make it there and back within the bound");
    assert!(back == payload, "the echo differs");
}

#[tokio::test]
async fn a_wrong_vmess_id_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_vmess_id_is_not_relayed") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![vmess_inbound(None, None)]);
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWrong = vmess, 127.0.0.1, {}, username=0233d11c-15a4-47d3-ade3-48ffca0ce118, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the server only ever answers a request it accepts: connecting succeeds
    let mut stream = outbound(&profile, "Wrong", None)
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("connecting succeeds");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        stream.read_exact(&mut buf),
    )
    .await;
    // an error, or silence until the bound: never the echo
    assert!(!matches!(got, Ok(Ok(_))), "a wrong id was relayed");
}

#[tokio::test]
async fn anytls_reuses_its_session_and_can_be_told_not_to() {
    let Some(bin) = sing_box_or_skip("anytls_reuses_its_session_and_can_be_told_not_to") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![Inbound {
            kind: InboundKind::AnyTls,
            users: vec![("u".into(), "s3same".into())],
            tls: Some(leaf_files(&fixture, dir.path())),
            ws_path: None,
        }],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nA = anytls, 127.0.0.1, {0}, password=s3same\nOnce = anytls, 127.0.0.1, {0}, password=s3same, reuse=false\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the same outbound three times: the second and third stream ride the
    // session the first one left behind
    let reused = outbound(&profile, "A", Some(&fixture));
    for _ in 0..3 {
        roundtrip(&reused, echo).await;
    }
    let once = outbound(&profile, "Once", Some(&fixture));
    for _ in 0..2 {
        roundtrip(&once, echo).await;
    }
}
```

（`roundtrip` 返回时流被丢弃，`Drop` 里补发 FIN 并把会话还回池；下一轮 `connect_tcp` 取到它。）

`tests/interop/tests/xray.rs`：

```rust
//! rurge's `vmess` outbound against a real xray on the loopback: the second
//! reference for the hand-written VMess codec. Every target is a loopback IP
//! literal. Without an xray binary each test prints why it is skipped
//! (`RURGE_INTEROP_REQUIRED=1` turns that into a failure).

use rurge_config::config::{LoadOptions, from_text};
use rurge_engine::EngineFactory;
use rurge_interop::xray::{Xray, XrayInbound, xray_or_skip};
use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
use rurge_net::socket::NoopSocketHook;
use rurge_policy::OutboundFactory;
use rurge_proto::OutboundRef;
use rurge_proto::testing::echo_server;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const VMESS_ID: &str = "0233d11c-15a4-47d3-ade3-48ffca0ce119";

fn outbound(profile: &str, name: &str) -> OutboundRef {
    let loaded = from_text(profile, Path::new("interop.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let factory = EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook));
    let spec = cfg.spec(name).expect("the policy has a spec");
    factory
        .build(spec, factory.direct_connector(&spec.common))
        .expect("the policy builds")
}

/// Every step is bounded: nobody runs these locally, and on CI an unbounded
/// read turns a regression into a hung job instead of a failed one.
async fn roundtrip(out: &OutboundRef, echo: SocketAddr, payload: &[u8]) {
    let bound = Duration::from_secs(10);
    let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
    let stream = tokio::time::timeout(bound, out.connect_tcp(&target, &ConnectOpts::default()))
        .await
        .expect("the tunnel is established within the bound")
        .expect("the tunnel is established");
    // read while writing: an echo of this size does not fit the socket buffers
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut back = vec![0u8; payload.len()];
    let both = async {
        tokio::join!(
            async { writer.write_all(payload).await.unwrap() },
            async { reader.read_exact(&mut back).await.unwrap() }
        )
    };
    tokio::time::timeout(bound, both)
        .await
        .expect("the payload makes it there and back within the bound");
    assert!(back == payload, "the echo differs");
}

#[tokio::test]
async fn vmess_with_either_cipher_with_and_without_websocket() {
    let Some(bin) = xray_or_skip("vmess_with_either_cipher_with_and_without_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let xray = Xray::spawn(
        &bin,
        dir.path(),
        vec![
            XrayInbound {
                uuid: VMESS_ID.into(),
                ws_path: None,
            },
            XrayInbound {
                uuid: VMESS_ID.into(),
                ws_path: Some("/v".into()),
            },
        ],
    );
    let echo = echo_server().await;
    let line = |name: &str, index: usize, rest: &str| {
        format!(
            "{name} = vmess, 127.0.0.1, {}, username={VMESS_ID}, vmess-aead=true{rest}\n",
            xray.port(index)
        )
    };
    let profile = format!(
        "[Proxy]\n{}{}{}[Rule]\nFINAL,DIRECT\n",
        line("Plain", 0, ""),
        line("Chacha", 0, ", encrypt-method=chacha20-ietf-poly1305"),
        line("Ws", 1, ", ws=true, ws-path=/v"),
    );
    // more than one chunk each way: the masks and the nonce counter have to stay in step
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    for name in ["Plain", "Chacha", "Ws"] {
        let out = outbound(&profile, name);
        roundtrip(&out, echo, b"interop").await;
        roundtrip(&out, echo, &big).await;
    }
}
```

Run: `cargo test -p rurge-interop`
Expected: 本机：渲染与守卫的单元用例通过；互操作用例逐条打印 `skipping …: no sing-box` / `no xray` 并通过。**不要为了让它们真的跑起来而下载任何东西。**

- [ ] **Step 5: CI 安装并校验 xray**

`.github/workflows/ci.yml`：在安装 sing-box 的那一步之后、`cargo test --workspace` 之前加：

```yaml
      - name: Install xray for the VMess interoperability tests
        shell: bash
        run: |
          set -euo pipefail
          version=26.3.27
          case "$RUNNER_OS" in
            Linux)   asset="Xray-linux-64.zip";        sha=23cd9af937744d97776ee35ecad4972cf4b2109d1e0fe6be9930467608f7c8ae ;;
            Windows) asset="Xray-windows-64.zip";      sha=d004c39288ce9ada487c6f398c7c545f7d749e44bdfdd59dbc9f865afba4e1ad ;;
            macOS)   asset="Xray-macos-arm64-v8a.zip"; sha=2e93a67e8aa1936ecefb307e120830fcbd4c643ab9b1c46a2d0838d5f8409eaf ;;
            *) echo "unexpected runner OS: $RUNNER_OS"; exit 1 ;;
          esac
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/XTLS/Xray-core/releases/download/v$version/$asset"
          if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$asset" | cut -d' ' -f1)
          else
            actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
          fi
          if [ "$actual" != "$sha" ]; then
            echo "xray checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p xray
          unzip -q "$asset" -d xray
          bin=$(find "$PWD/xray" -type f \( -name xray -o -name xray.exe \) | head -n 1)
          [ -n "$bin" ] || { echo "no xray binary in $asset"; exit 1; }
          chmod +x "$bin"
          if [ "$RUNNER_OS" = "Windows" ]; then bin=$(cygpath -w "$bin"); fi
          echo "RURGE_TEST_XRAY=$bin" >> "$GITHUB_ENV"
```

（`RURGE_INTEROP_REQUIRED=1` 已由 sing-box 那一步写进环境，对两个夹具都生效。）

`tests/interop/README.md`：补一节 xray——用途（只测 vmess，理由见 M2-D5）、固定版本 v26.3.27、三个平台的包名与 SHA256（与上面一致）、环境变量 `RURGE_TEST_XRAY`、"本机不安装，CI 证明"，以及渲染出的配置的样子（一个回环 `vmess` 入站 + `freedom` 出站）。sing-box 一节补上 vmess / anytls 入站的说明。

- [ ] **Step 6: 门禁与提交**

跑「Global Constraints」里的门禁。然后：

```bash
git add -A
git commit -m "test(interop): sing-box 的 vmess（± tls ± ws）/ anytls 入站；xray v26.3.27 的 vmess 夹具与用例；CI 安装并校验 xray"
```

---

### Task 12: 文档

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`、`docs/api/phase2.md`、`README.md`、`CLAUDE.md`、`docs/acceptance/phase2-manual.md`
- Modify: `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`（第 3、6.3、15 节的订正与新的第 18 节）
- Modify: 本计划末尾两张表

素材由控制者在派发本任务时给出（`task-12-inputs.md`：各任务报告里的偏差、评审裁定延后的条目、Task 6 的吞吐数字）。**两张收尾表不要自己编。**

- [ ] **Step 1: 兼容性清单**

`docs/surge-compatibility-matrix.md`，逐条落到对应的行（用 `Grep` 找到现有的行再改，不要新开重复的行）：

| 位置 | 内容 |
| ---- | ---- |
| 4.2 `vmess`（协议一览与参数两处） | M2b 已实现 AEAD 握手（TCP）；没写 `vmess-aead=true` 的行在 M8 之前按 `W0007`（每次加载一条）+ REJECT 处理，会话日志 `policy protocol not implemented: vmess (legacy handshake)`；只开 ChunkStream + ChunkMasking（Surge 的取值未公开）；UUID 错与时钟偏差超过约 120 秒都表现为 `vmess: the server closed the connection without answering`，分辨不出；`username` 只接受命名写法；`tls=false` 时写的 TLS 参数是 `W0028`；默认不带 ALPN（未核对）；UDP 属 M5 |
| 4.2 `anytls`（两处） | M2b 已实现（TCP）；一条会话同一时刻一个流（与参考实现一致）；空闲 60 秒回收、每 30 秒检查；不等 `cmdSYNACK`，被拒的流在第一次读上以 `anytls: <服务端文本>` 失败；**没有半关闭**：客户端方向的 EOF 以 `cmdFIN` 结束整条流（与 sing-box 一致）；不实现参考客户端的 3 秒 SYNACK 看门狗；服务端推送的 padding 方案做有界校验（原文 ≤ 8192 字节、`stop` ≤ 256、每包 ≤ 64 项、每项 1 ..= 16384），不合法就保留旧方案；`password` 只接受命名写法；默认不带 ALPN（未核对）；UDP（udp-over-tcp v2）属 M5 |
| 4.2 通用 | 代理服务器主机名须写成 ASCII（punycode）：以 Unicode 写的名字在 TLS 类协议上是加载错误 `E0022`，在明文协议上是拨号期的解析失败；全面的 IDN 支持留待 M8 |
| `W0007` 那一行 | 移除 `vmess`（AEAD）与 `anytls`；补旧握手那条专门的文本 |
| 重载（AR-04 / 5.5 的相应行） | 重载时按指纹复用出站：名字相同且"去掉行号的参数 + 被引用的 Keystore 条目 + `[General] ipv6`"都没变的策略沿用上一代的出站（含连接池）；被复用的出站按新一代的解析器解析 |
| 转发（会话 / relay 的相应行，没有就加在 10.x 的会话日志一节） | 任一方向以错误结束时整条会话立即结束并记为失败；写完即刷出 |

- [ ] **Step 2: API 参考、README、CLAUDE.md、手工验收**

- `docs/api/phase2.md`："会话日志的出站错误文本"一节补 `vmess:` 与 `anytls:` 前缀下的全部固定文本（从 `vmess/{header,stream,mod}.rs` 与 `anytls/{session,mod}.rs` 里逐条抄，连同触发条件），并注明 `anytls: <文本>` 里的文本来自服务端、已去控制字符且截到 256 个字符。脱敏名单不变（`username` `password` 早已在内）。
- `README.md`（中英两段）：特性表与路线图里 M2b 标为已完成；协议清单补 vmess（AEAD）/ anytls。
- `CLAUDE.md`：「当前状态」补 M2b 一段（`Secret<T>`、`VmessSpec` / `AnyTlsSpec`、`rurge-proto` 的 `vmess` / `anytls` 模块与两个假服务端、`ResolverCell` / `publish_generation`、按指纹复用、转发循环的两处修正、能力表翻转、xray 互操作）；「先读这些文档」加入本计划；「常用命令」补 `cargo test -p rurge-proto vmess`、`cargo test -p rurge-proto anytls` 与 `RURGE_TEST_XRAY`。
- `docs/acceptance/phase2-manual.md`：加 VMess 与 AnyTLS 两节，各 6 步以内，照 Trojan 一节的体例：`rurge check`（无 `W0007`）→ `rurge run` + `curl -x` 经该策略访问一个 HTTPS 站点 → 故意写错 UUID / 口令看会话日志的文本 → （AnyTLS）连续两次请求后在服务端日志里确认只有一条 TLS 连接 → 改一条无关策略后 `rurge reload`，再请求一次，确认仍是那条连接 → （VMess）把本机时钟拨偏 3 分钟，确认表现与"UUID 错"相同（排错一节引用它）。

- [ ] **Step 3: 设计文档的订正**

`docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`：

- 第 3 节的新依赖表：VMess 一行改为 `ring`（已有）`aes`（已有）`md-5` `sha3` `crc32fast`（已有）；AnyTLS 一行 `md-5`（同上）+ `tokio-util`（已有）。
- 6.3 "会话复用"一条里的"双向 FIN"改为"任一方的 `cmdFIN`"，并补一句"没有半关闭"。
- 第 15 节 V3 – V6 各加"（已核对：M2b 计划 P1 / P5 / P9 / P10）"。
- 第 17 节之后加 **第 18 节「M2b 实施期的订正」**：一张与第 17 节同体例的表，逐条登记本计划「计划期决定」里与设计文字不同的 P2、P3、P5（有界校验的取值）、P6、P7、P13、P16、P17，以及执行期新发现的出入（来自 `task-12-inputs.md`）。

- [ ] **Step 4: 本计划的两张收尾表**

把 `task-12-inputs.md` 里的条目填进下面两张表（任务、改了什么、原因、提交；事项、去向）。

- [ ] **Step 5: 门禁与提交**

文档任务也跑一遍门禁（文档里引用的命令与文本要与代码一致：对照 `grep` 一遍错误文本）。然后：

```bash
git add -A
git commit -m "docs: M2b——兼容性清单、API 参考、README、CLAUDE.md、手工验收、M2 设计第 18 节与计划收尾表"
```

---

## 验收对照（设计第 12 节）

| 验收标准 | 落点 |
| -------- | ---- |
| 1. `vmess`（AEAD，± tls，± ws）、`anytls`（`reuse` 两种取值）对回环假服务端的成功与失败路径；对 sing-box；vmess 另对 xray | Task 3、5（假服务端）；Task 11（sing-box、xray，CI 证明） |
| 3. 六个 TLS 参数在新协议上各有用例 | TLS 层是 M1 / M2a 的同一个 `tls_client`，逐参数的用例在 `transport/tls.rs`；新协议各钉住"走的是它"：默认不带 ALPN（Task 3、5）、指纹固定（Task 9 的端到端全部用 `server-cert-fingerprint-sha256`）、`client-cert` 坏了是加载错误（Task 9）、`skip-cert-verify` 的告警（Task 9）、`tls=false` 时的 `W0028`（Task 1） |
| 4. 各能作链的入口与出口 | Task 9 `the_new_protocols_work_at_either_end_of_a_chain` |
| 5. 重载复用的四条用例（7.3） | Task 8（注册表层）；Task 9 `an_unrelated_reload_keeps_an_anytls_pool_and_a_change_of_its_own_drops_it`、`a_reused_outbound_resolves_through_the_new_generation`；链式会话进行中重载是 M2a 的用例，Task 8 复核 |
| 6. 承接事项全部落地 | 「承接事项」表 |
| 7. 凭据不外泄 | Task 1（`Secret`、诊断不回显）、Task 3 / 5（错误文本）、Task 9（`E0022` 文本、会话日志）、Task 10（CLI 输出） |
| 8. 门禁全绿；CI 首次推送后为绿 | 每个任务的最后一步；推送由项目所有者决定 |
| 9. 需要真实节点的项目进手工验收清单 | Task 12 |

（第 2 条是 M2c 的。）

## 执行期修正记录

| 任务 | 改了什么 | 原因 | 提交 |
| ---- | -------- | ---- | ---- |
| | | | |

## 延后事项

| 事项 | 去向 |
| ---- | ---- |
| （计划期）AnyTLS 不实现参考客户端的 3 秒 SYNACK 看门狗：复用到一条"半死"的空闲会话时由转发阶段的空闲超时兜底（P7） | M8 收尾时评估；清单已登记 |
| （计划期）以 Unicode 写的代理服务器主机名不可用：解析器的线上编码与应答校验都只认 ASCII，TLS 层是构建错误（P15） | M8：连同规则、`[Host]`、解析器一起做全面的 IDN 支持；清单已登记 |
| （计划期）`Secret<T>` 只包了凭据类型的字段；`WsOpts.path` / `headers`、`HttpSpec.headers` 与 `KeystoreItem` 的 `base64` / `password` 仍会被 `Debug` 打印（生产代码从不格式化它们）（P14） | M8 |
| （计划期）VMess 旧式（非 AEAD）握手 | M8（设计 M2-D4） |
