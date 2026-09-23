# 阶段 2 / M2c「Shadow TLS v2 / v3」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 三个参数在所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`）上真正生效：连接先与伪装站点做一次真实的 TLS 握手（由 Shadow TLS 服务端转发），之后的数据装在看起来像该会话 ApplicationData 的记录里；v2 与 v3 都支持，v3 在 stock rustls 上签名 ClientHello，不 fork、不 patch、不写 unsafe。

**Architecture:** 新模块 `rurge_proto::transport::shadow_tls`：记录读取器（`record`）、带密钥的摘要链（`auth`）、握手后的帧化字节流（`framed`）、v3 的两遍构造 ClientHello（`sign`，M2 设计附录 A）、自己驱动的伪装握手与"体面收尾"（`mod.rs` 的 `ShadowTlsClient`）。它作为传输阶梯 `transport::Stack` 的一层接在 connect 与 tls 之间（connect → shadow-tls → tls → ws）；M1 的 `http` / `socks5` 出站同时迁到 `Stack`。配置层新增 `rurge_config::spec::ShadowTlsOpts`，挂在 `PolicySpec` 上，因此自动进入重载复用的指纹；三个参数的 `W0029` 在同一个任务里退役。回环假服务端 `FakeShadowTls` 按参考服务端的做法逐记录转发伪装握手，伪装站点是夹具自己的 TLS 服务端 `Camouflage`。

**Tech Stack:** Rust 1.89 / edition 2024；`rustls` 0.23.43（`ClientConnection` 的 `read_tls` / `process_new_packets` / `write_tls`，自定义 `CryptoProvider`）、`ring`（HMAC-SHA1：`HMAC_SHA1_FOR_LEGACY_USE_ONLY`）、`sha2`——**不新增任何依赖**；互操作用 sing-box 1.14.1 的 `shadowtls` 入站（只在 CI 上安装）。

**Spec:** `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`（第 1.4 节的 M2c 行、第 4.1 / 4.2 / 4.4 节、第 5.1 / 5.3 节、第 8、10 – 13 节、第 15 节 V7 / V8、第 16 节、附录 A，以及第 17 / 18 节的订正）；总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。与本计划「计划期决定」表不一致处，以该表为准，并由最后一个任务写回设计文档新增的第 19 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` + 唯一一个 `#[allow(unsafe_code)]` 函数。**本计划不新增任何 unsafe**（M2-D3：Shadow TLS v3 在 stock rustls 上实现，不 fork、不 patch）。
- 依赖方向不变：`rurge-policy` 不依赖任何协议实现；`rurge-engine` / `rurge-api` / `rurge-policy` 不依赖 `rurge-platform`；平台代码只在 `rurge-platform`（AR-02）。**不新增任何第三方依赖**；`Cargo.lock` 只因 `tests/interop` 多了一条对已有 `rustls` 的 dev 依赖而多一行。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间）。伪装站点是夹具自己在回环上起的 TLS 服务端，绝不是真实网站。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（不装 sing-box、不装 xray、不 `rustup target add`、不 `cargo install`）。互操作由首次推送后的 CI 证明。
- 互操作夹具渲染出的 sing-box 配置里绝不出现 `set_system_proxy` / `tun` / `auto_route`；只监听 `127.0.0.1`；所有目标（含 `shadowtls` 入站的 `handshake.server`）是回环 IP 字面量。
- **凭据及其派生物永不外泄**：`shadow-tls-password`、由它派生的 HMAC 标签与摘要、异或密钥、ServerRandom 派生的链状态，以及对端发来的原始文本——永不出现在错误文本、诊断、日志、API 输出与 `Debug` 输出里。rustls 的错误文本可能引用服务端证书里的名字，一律先过 `untrusted_text`（去控制字符、有界）。持有口令或链状态的类型（`ShadowTlsClient`、`Chain`、`Mode`、`Framed`、`ShadowTlsScript`）不实现 `Debug`。
- 长度先校验后分配：一条记录的长度字段是 16 位，读取器的缓冲因此不超过 5 + 65535 字节（格式所限）；握手期的记录交给 rustls，由它执行 TLS 规范的上限。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && cargo test --workspace --no-fail-fast
  ```

  测试二进制异常退出而没有失败用例时（本机已知的偶发 `STATUS_ACCESS_VIOLATION`），重跑一次并保留两次的日志。开工前看一眼磁盘：`target/` 目前约 33 GB，D: 剩约 97 GB。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。
- 本机的 bash 处理不了超过约 8 KB 或引号复杂的 heredoc：新文件一律用写文件的工具落盘，不用 heredoc。

## 计划期决定

写计划时对照参考实现、Surge 手册与本仓库源码核对后定下的事；与设计文档文字不同的，最后一个任务写回设计文档第 19 节。

**本计划里的代码不是凭空写的。** Task 2 – 4 的协议代码、假服务端与用例先在一个临时工程里编译并跑通：dead-code 检查开着，三种构建形态（不带 `testing` 特性 / 带 `testing` / `--all-targets`）都过 clippy；用例按生产调用方的方式写（转发循环：`read` → `write_all` → `flush`、两个方向经 `tokio::io::split` 在同一个任务里交错轮询、源结束时 `shutdown`，1 MiB 往返，里层再叠一层真实 TLS）；整套用例连跑 550 轮无失败；对 9 处故意改坏的实现逐一确认相应用例变红（见 Task 4）。带密钥的原语另由一份只用 Python 标准库（`hmac` / `hashlib`）、逐步照搬参考实现的脚本生成向量钉住（P10）。之后全部 9 个任务的改动又在仓库的一份副本上按任务顺序真实做了一遍，每个任务后各跑一次受影响 crate 的 fmt / clippy / 测试，最后一次是全工作区门禁：**38 个测试二进制，798 通过 / 0 失败 / 1 忽略**（本计划开工前的 main 是 748）。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V7：v2 摘要覆盖的确切字节范围 | 对照 `ihciah/shadow-tls`（master，2026-09-21）`src/helper_v2.rs` 的 `HashedReadStream`、`src/client.rs` 的 `relay_v2`、`src/server.rs` 的 `copy_until_handshake_finished`，与 `SagerNet/sing-shadowtls`（main）`v2_hash.go` / `v2_client.go` / `v2_server.go`。结论：HMAC-SHA1（密钥为口令，无初始数据）喂入**握手期间从服务端收到的每一个字节**——含每条记录的 5 字节头；取摘要前 8 字节，放在客户端**第一帧**负载的最前面（同一条记录内）。rurge 一次只读一条记录，所以喂进摘要的恰好是"到服务端 Finished 为止"的全部字节，不会多读 |
| P2 | V7：v2 握手之后仍在转发的记录 | 服务端在看到带摘要的首帧之前一直在转发伪装站点（TLS 1.3 的 session ticket 就落在这段时间里）。参考客户端（`SessionFilterStream`）把收到的记录先交给伪装会话：它能解开的不是负载；第一条它解不开的记录起才是数据。rurge 照做（`Mode::V2.session`）。分类：rustls 报 `DecryptError` / `InvalidMessage` / `PeerSentOversizedRecord`（都发生在解开记录之前）→ 数据阶段开始；其它错误或 `close_notify`（记录确实来自伪装会话，而会话结束了）→ `shadow-tls: the handshake server closed the session`——**v2 口令错误就表现为这一条**（握手本身分辨不出口令对错）。设计原文没有这一段 |
| P3 | V7：v3 的链与帧 | 同上两个仓库的 `src/client.rs`（`generate_session_id`、`StreamWrapper`）、`src/server.rs`（`verified_extract_sni`、`copy_by_frame_with_modification`、`copy_by_frame_until_hmac_matches`）、`src/util.rs`（`Hmac`、`kdf`、`xor_slice`、`copy_add_appdata`、`verify_appdata`）与 `v3_client.go` / `v3_server.go` / `v3_conn.go`。确认设计 5.3 的文字，并补足细节：① ClientHello 的标签覆盖**去掉记录头之后的整条记录**（标签 4 字节置零）；② ServerRandom 取**第一条** ServerHello 记录的 `[11..43]`（与服务端一致）；③ 握手期的标签 = 链 `HMAC(口令; ServerRandom ‖ 线上负载₁ ‖ 线上负载₂ …)` 当前摘要的前 4 字节，**标签不回喂**；异或密钥 `SHA256(口令 ‖ ServerRandom)` **每条记录从头开始**；先验证（对线上负载）再异或还原；④ 数据阶段：`ServerRandom ‖ "C"` / `"S"` 起始，负载喂入、取 4 字节、**再把这 4 字节喂回**；客户端的首帧就是 C 链的第一帧；⑤ 数据阶段开始后，能在握手链下验证的记录（服务端还在转发的 ticket）跳过，直到第一条验不过的，此后握手链作废 |
| P4 | V7："体面收尾" | 参考客户端在严格模式下：握手照常完成 → 经真实会话发一个长度随机的 HTTP 请求 → 关闭写端 → 读到 EOF → 失败。sing-box 的客户端不做这一步。rurge 照参考客户端做，两种拒绝原因（不是 TLS 1.3、没通过验证）共用：请求写成格式正确的 `GET / HTTP/1.1`（`Host`、`User-Agent: curl/8.5.0`、`Accept`、长度随机的 `Cookie`、`Connection: close`，CRLF——参考实现用的是裸 LF 且头部没有结尾空行，不照抄）；之后 `close_notify`，读到对端关闭为止；整段另有 2 秒上限（`FAREWELL`），外层仍是调用方的那一个超时。伪装握手失败（证书不被信任等）时，把 rustls 排队的 alert 发出去再返回错误——TLS 客户端离开时会说原因 |
| P5 | alert 记录 | 参考服务端只在 TLS 1.2 会话上发 alert；**sing-box 的服务端对客户端的 FIN（以及任何读错误）一律回一条 31 字节的 alert 记录，而且另一方向的数据可能还在继续发**。rurge：从不发 alert 记录；**收到的 alert 记录跳过，流的结束以 TCP 连接的结束为准**。与两个参考客户端不同（它们把 alert 当作读方向的结束）——rurge 的转发循环会半关闭，照它们的做法，对 sing-box 服务端半关闭之后的下行数据会被截断。登记进清单 |
| P6 | 记录的大小 | 写：每帧负载 ≤ 16384 字节（TLS 明文记录的上限；帧不该比它模仿的记录更大）。读：**接受 16 位长度字段能表示的任何长度**——sing-box 的 `WriteBuffer` 不切分，一帧可以有一个 32 KiB 拷贝缓冲那么大；按 TLS 规范的上限去拒绝会让经 sing-box 服务端的大下载失败。设计 5.3 的"单条记录的上限按 TLS 规范"只对握手期成立（由 rustls 自己执行） |
| P7 | `shadow-tls-sni` 缺省时（手册 vs 设计） | 手册（Policies › TLS and Shadow TLS，2026-09-21 取）："The SNI sent to the server during the Shadow TLS handshake in plain text. **If not set, no SNI is sent.**" 设计原文"没写则用策略的有效 SNI 名（未核对）"按手册订正：**不写就不发 SNI**（`enable_sni = false`）。证书仍然照常校验（设计 5.3 的要求不变），校验用的名字取策略自己的 TLS 会用的那个：`sni` 写了名字就用它，否则用服务器主机名（IP 也可）。后果：服务器写成 IP、伪装站点是别人的网站时必须写 `shadow-tls-sni`，否则证书对不上——错误文本会说明。v3 必须写（手册与设计一致）。`shadow-tls-sni` 只接受 DNS 名，不接受 IP 字面量（SNI 扩展装不了地址） |
| P8 | 设计第 3 节的新依赖表（`hmac` `sha1`） | 不引入：HMAC-SHA1 用 `ring::hmac`（`HMAC_SHA1_FOR_LEGACY_USE_ONLY`，`Context` 可克隆——"取当前摘要、链继续"靠克隆实现），SHA-256 用工作区已有的 `sha2`。沿用 M2b P2 的先例，零新增条目。`Chain` 把 `ring` 的上下文装箱：它约 300 字节，一条流持有三条链（clippy `large_enum_variant`） |
| P9 | V8：附录 A 在所用 rustls 上是否仍成立 | `Cargo.lock` 里的 rustls 就是 spike 用的 0.23.43。临时工程里重新验证，并比 spike 多三点：① 伪装握手的配置是 `with_safe_default_protocol_versions`（TLS 1.2 + 1.3，spike 只开了 1.3）——两遍构造照样成立，且 TLS 1.2 的站点能把握手走完以便"体面收尾"；② 脚本化的随机源与密钥交换组是两个 `static` 单元结构体（spike 用的是 `Box::leak`）；③ 线程局部的脚本状态由一个 `Drop` 守卫复位，任何提前返回都不会把脚本留在激活状态。四条自检任何一条不满足 → `None` → `shadow-tls: cannot sign the ClientHello`；`ShadowTlsClient::build` 对 v3 先演一遍，所以假设失效会在加载期（`E0022`）暴露，而不是连接期 |
| P10 | 向量的出处 | 假服务端与客户端共用带密钥的原语（`Chain`、`xor`、`xor_key`、`hello_tag`），两边犯同一个错会互相抵消。所以这些原语由 `vectors.rs` 钉住：向量用一份 Python 脚本生成，只用标准库 `hmac` / `hashlib`，逐步照搬参考实现的函数（脚本保存在写计划的临时目录里，向量与推导方法写在 `vectors.rs` 的文件头）。假服务端的**帧处理**则刻意不复用客户端的 `Framed`，而是按参考服务端逐记录手写 |
| P11 | 凭据字段 | `ShadowTlsOpts.password` 是 `Secret<String>`（M2b 的约定）；设计 4.1 写的是 `String` |
| P12 | 配置层生效的时机 | Task 1 只交付类型、判断函数与公开的读取函数（`read_shadow_tls` 是 `pub`，没有调用者也不触发 dead-code）；`to_spec` 调用它、`PolicySpec.shadow_tls` 出现、三个参数的 `W0029` 退役，都放在 Task 5 与"出站真的会用这一层"同一个提交里。否则中间状态是：配置不再警告、spec 里有 Shadow TLS、而连接**不带** Shadow TLS 直接发出去 |
| P13 | 判断函数 | `rurge_config::spec::shadow_tls::allowed_on(PolicyKind)`：`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale` 返回 `false`（手册：与 TUIC / WireGuard / Tailscale 组合是配置错误；对其它 QUIC 类协议"没有意义"；清单 4.4 把两者都记作配置错误）。`read_shadow_tls` 自己用它，所以它是活代码；这些协议目前没有 spec（`to_spec` 提前返回），相应的 `E0018` 要等它们的 spec 出现才会真的报出来 |
| P14 | 出站拿到这一层的方式 | `Stack::new(connector, server, shadow_tls, tls, ws)`：参数顺序就是经过的顺序。`TrojanOutbound::new` / `VmessOutbound::new` / `AnyTlsOutbound::new` 在 `spec` 之后多一个参数 `shadow_tls: Option<&ShadowTlsOpts>`（7 个参数，clippy 的上限）；`HttpOutbound` / `Socks5Outbound` 的 `from_spec` 吃整个 `PolicySpec`，自己读 `spec.shadow_tls`。共用的构建函数 `rurge_proto::build::shadow_tls_client(opts, tls, server, roots)` 负责 P7 的名字选取 |
| P15 | 承接：`environment()` 复核 | 迁移之后 `EngineFactory` 按值捕获、又会随配置代际变化的仍然只有 `[General] ipv6`：Shadow TLS 一层只读 spec（已在指纹里）与根证书库（进程级）。`environment()` 不变。Task 7 顺带把"根证书库在引擎存续期间不变"从注释变成结构：它移进 `EngineShared`（见 P16） |
| P16 | 端到端夹具怎么信任伪装站点 | 设计 5.3 说"测试经已有的 `EngineFactory::with_roots` 注入自己的根"——互操作夹具直接用工厂，确实如此；但引擎的端到端夹具经 `Runtime::build` 构建，那里写死了 `EngineFactory::new`（系统根），M2a / M2b 的用例靠指纹钉扎绕过，而伪装握手按设计没有任何绕过。决定：`EngineShared` 增加 `roots: Option<Arc<RootCertStore>>`（`None` = 系统根；bin 不用改），`Runtime::build` 据此选 `with_roots`。放在 `EngineShared` 是因为它正是"随引擎存续、不随代际变化"的那组对象，重载自然沿用同一份根 |
| P17 | 夹具的两处缺陷（探针跑出来的） | ① `TlsFixture` 的 TLS echo 服务端 `write_all` 之后不 `flush`：tokio-rustls 在 socket 写不动时会把密文尾巴留在自己的缓冲里（M2b P16 同一类），300 KB 的往返约 2 – 5% 的轮次卡死；加 `flush` 后 400 轮无一卡死。② 假服务端的数据阶段起初在一个 `select!` 循环里做写，两边同时回压就互相堵死；改成两个互不等待的循环（只有 alert 让它们共用客户端的写端）。两处都是夹具的毛病，不是客户端的 |
| P18 | 夹具的时序 | ① rustls 0.23.43 的服务端把两张 session ticket 打进**一条**记录，所以假服务端"等 ticket 转发完再进数据阶段"的开关按记录计数（`late_records`）。② `TlsFixture::seen()` 是服务端在 `accept` 返回之后才记的，客户端那时已经拿到流：新增 `seen_at_least(n)`（有界等待）。③ 假服务端在**做出判断的那一刻**记会话（首帧验证通过 / 转成普通转发），不等连接结束。④ 两个 CA 的主题名相同（`rurge test CA`），"不被信任的证书"的具体错误是 `BadSignature` 而不是 `UnknownIssuer`，用例只断言前缀 |
| P19 | v2 的固有局限 | v2 的服务端靠"首帧到达时自己已转发的字节"比对摘要：首帧来得越晚，越可能有 ticket 先被转发。参考服务端记下最近 10 个摘要，sing-box 只多容忍一次写。rurge 的首帧是里层 TLS 的 ClientHello（`trojan` `anytls` `https` `socks5-tls`、`vmess` + `tls`）或协议自己的第一个包（`http` 的 CONNECT、`socks5` 的问候），握手一完就发；只有不带 TLS 的 `vmess` 会等 `LazyHead` 的 100 ms。登记进清单；互操作的 v2 用例让伪装站点不发 ticket，v3 用例发两张（v3 没有这个竞争，正好覆盖"握手链下的残留记录"） |
| P20 | 承接：拆分两个大测试文件 | 用一段脚本完成（Task 6 给出全文，用完即弃、不进仓库）：不是用例的顶层项进 `tests/common/mod.rs` 并变成 `pub`（含 `use` → `pub use`，测试文件只需 `use common::*`），用例按名单分到两个文件，其余一字不动。验证标准：拆分前后的用例名单完全一致 |

## 承接事项

M2b 计划「延后事项」表里标给 M2c 的条目，以及写本计划时发现、需要顺带处理的。

| # | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- |
| C1 | `crates/rurge-proto/src/anytls/mod.rs` 测试里的 `copy_half` 缺"特意保持不 flush"那句注释（vmess 那份已有） | 补上同一句 | 6 |
| C2 | `crates/rurge-engine/tests/outbounds.rs`（1554 行）与 `tests/interop/tests/sing_box.rs`（518 行）按协议拆分 | 各拆成"公共夹具 + 原文件 + TLS 族文件"，Shadow TLS 的用例另起新文件（P20） | 6 |
| C3 | 迁移 http / socks5 到 `Stack` 时复核 `environment()` | 结论见 P15：不变；根证书库移进 `EngineShared`（P16） | 5、7 |
| C4 | `GET /v1/profiles/current` 与 `policies/detail` 目前把 `shadow-tls-password` 的值原样给出（`SECRET_PARAMS` 里的 `password` 只在 token 边界上匹配，`shadow-tls-password` 对不上）——设计 4.4 把补这一条排在 M2c | `SECRET_PARAMS` 增加 `shadow-tls-password`，用例在旧名单上确实变红 | 1 |
| C5 | 每 8 KiB 一次 flush 在 TLS / WS / VMess 栈上的成本没测过 | 不属于 M2c；继续留给 M8（本计划末尾「延后事项」表带着它） | — |
| C6 | 首次推送后要盯的互操作项 | 追加 Shadow TLS 的几条（见末尾「延后事项」表） | 9 |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/shadow_tls.rs` | `ShadowTlsVersion`、`ShadowTlsOpts`、`allowed_on`、`read_shadow_tls` 与它们的用例 | 1 |
| `crates/rurge-proto/src/transport/shadow_tls/mod.rs` | 模块声明（Task 2 / 3）；`ShadowTlsClient`：构建、伪装握手（v2 / v3）、体面收尾，与端到端风格的用例（Task 4） | 2、3、4 |
| `crates/rurge-proto/src/transport/shadow_tls/record.rs` | TLS 记录的常量与按轮询工作的整记录读取器 | 2 |
| `crates/rurge-proto/src/transport/shadow_tls/auth.rs` | `Chain`（运行中的 HMAC-SHA1）、常数时间比较、v3 的异或密钥 | 2 |
| `crates/rurge-proto/src/transport/shadow_tls/framed.rs` | 握手之后的帧化字节流（`Mode::V2` / `Mode::V3`），`AsyncRead + AsyncWrite` | 2 |
| `crates/rurge-proto/src/transport/shadow_tls/vectors.rs` | 带密钥原语的向量（`#[cfg(test)]`） | 2、3、4 |
| `crates/rurge-proto/src/transport/shadow_tls/sign.rs` | v3：脚本化的随机源与 X25519、两遍构造 ClientHello、四条自检 | 3 |
| `crates/rurge-proto/src/testing/shadow_tls.rs` | `FakeShadowTls`（v2 / v3 服务端）与 `Camouflage`（伪装站点） | 4 |
| `crates/rurge-engine/tests/common/mod.rs` | 引擎端到端用例共用的夹具（由拆分产生） | 6 |
| `crates/rurge-engine/tests/outbounds_tls_family.rs` | trojan / vmess / anytls 的端到端用例（由拆分产生） | 6 |
| `crates/rurge-engine/tests/outbounds_shadow_tls.rs` | Shadow TLS 的端到端用例 | 7 |
| `tests/interop/tests/common/mod.rs`、`tests/interop/tests/sing_box_tls_family.rs` | 互操作用例的公共部分与 TLS 族用例（由拆分产生） | 6 |
| `tests/interop/tests/sing_box_shadow_tls.rs` | 对 sing-box `shadowtls` 入站的互操作用例 | 8 |

修改：

| 文件 | 改动 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/mod.rs` | 声明并导出新模块（1）；`PolicySpec.shadow_tls`、`to_spec` 读取、五处 `note_shadow_tls` 删除、用例（5） | 1、5 |
| `crates/rurge-config/src/spec/tls.rs` | `is_server_name` 改 `pub(crate)`（1）；删除 `SHADOW_TLS_KEYS` 与 `note_shadow_tls`（5） | 1、5 |
| `crates/rurge-config/src/redact.rs` | `SECRET_PARAMS` 增加 `shadow-tls-password` 与用例 | 1 |
| `crates/rurge-proto/src/transport/mod.rs` | `pub mod shadow_tls;` | 2 |
| `crates/rurge-proto/src/testing/{mod.rs, tls.rs}` | 导出假服务端；`TlsFixture::camouflage_acceptor`、`seen_at_least`；TLS echo 写完 `flush` | 4 |
| `crates/rurge-proto/src/transport/stack.rs` | `shadow_tls` 一层与用例 | 5 |
| `crates/rurge-proto/src/build.rs` | `shadow_tls_client` 与用例 | 5 |
| `crates/rurge-proto/src/{trojan.rs, vmess/mod.rs, anytls/mod.rs}` | 构造函数多一个参数 | 5 |
| `crates/rurge-proto/src/{http.rs, socks5.rs}` | 迁到 `Stack`，读 `spec.shadow_tls`，各一条经 Shadow TLS 的用例 | 5 |
| `crates/rurge-engine/src/outbounds.rs` | 工厂把 `spec.shadow_tls` 交给三个 M2 出站 | 5 |
| `crates/rurge-proto/src/anytls/mod.rs` | C1 的注释 | 6 |
| `crates/rurge-engine/tests/outbounds.rs`、`tests/interop/tests/sing_box.rs` | 拆分后留下的部分 | 6 |
| `crates/rurge-engine/src/{shared.rs, runtime.rs}` | `EngineShared.roots` | 7 |
| `tests/interop/{Cargo.toml, src/lib.rs}`、`Cargo.lock` | `InboundKind::ShadowTls`、渲染与用例；`rustls` dev 依赖 | 8 |
| 文档（清单、两份 API 参考、README、CLAUDE.md、手工验收清单、互操作 README、M2 设计、本计划末尾两张表） | 见 Task 9 | 9 |

---

### Task 1: 配置层——`ShadowTlsOpts`、判断函数、公开的读取函数，与脱敏名单

三个参数的类型、校验与"能不能包在这种协议外面"的判断。读取函数是公开的，**这个任务不把它接进 `to_spec`**（P12：那要等出站真的会用这一层，见 Task 5），所以三个参数此时仍然报 `W0029`，行为不变。另外补上脱敏名单里缺的一条（C4）。

**Files:**
- Create: `crates/rurge-config/src/spec/shadow_tls.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（模块声明与导出）
- Modify: `crates/rurge-config/src/spec/tls.rs`（`is_server_name` 的可见性）
- Modify: `crates/rurge-config/src/redact.rs`（`SECRET_PARAMS` 与用例）

**Interfaces:**
- Consumes: `rurge_config::spec::{ParamReader, Secret}`（`ParamReader::{has, touch, str, choice, invalid, error, warn, policy}`）、`spec::tls::is_server_name`、`crate::policy::PolicyKind`、诊断码 `codes::E_INVALID_POLICY_PARAM`（`E0018`）与 `codes::W_PARAM_NOT_APPLICABLE`（`W0028`）。
- Produces（后续任务按这些名字与类型使用）:
  - `rurge_config::spec::ShadowTlsVersion { V2 /* Default */, V3 }`：`Clone + Copy + Debug + Default + PartialEq + Eq`
  - `rurge_config::spec::ShadowTlsOpts { pub password: Secret<String>, pub sni: Option<String>, pub version: ShadowTlsVersion }`：`Clone + Debug + PartialEq + Eq`
  - `rurge_config::spec::shadow_tls::allowed_on(kind: PolicyKind) -> bool`
  - `rurge_config::spec::shadow_tls::read_shadow_tls(r: &mut ParamReader<'_>) -> Option<ShadowTlsOpts>`

- [ ] **Step 1: 先写脱敏的回归用例**

`crates/rurge-config/src/redact.rs`——把

```rust
        assert_eq!(
            redact_definition(
                "trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=Host:edge.test|X-Key:k3y"
            ),
            "trojan, t.test, 443, password=***, ws=true, ws-path=***, ws-headers=***"
        );
    }
```

换成

```rust
        assert_eq!(
            redact_definition(
                "trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=Host:edge.test|X-Key:k3y"
            ),
            "trojan, t.test, 443, password=***, ws=true, ws-path=***, ws-headers=***"
        );
        // not covered by `password`: that one only matches at a token boundary
        assert_eq!(
            redact_definition(
                "snell, 1.2.3.4, 443, psk=pwd1, shadow-tls-password=pwd2, shadow-tls-sni=example.com"
            ),
            "snell, 1.2.3.4, 443, psk=***, shadow-tls-password=***, shadow-tls-sni=example.com"
        );
    }
```

- [ ] **Step 2: 运行，确认它在旧名单上变红**

Run: `cargo test -p rurge-config --lib redact`

Expected: `a_definition_is_redacted_like_its_profile_line` 失败，左边是 `"snell, 1.2.3.4, 443, psk=***, shadow-tls-password=pwd2, shadow-tls-sni=example.com"`——口令原样出现。（写计划时在旧实现上实际跑过，输出如此。）

- [ ] **Step 3: 把 `shadow-tls-password` 加进名单**

`crates/rurge-config/src/redact.rs`——把

```rust
/// nodes behind a CDN routinely use as a shared secret. Over-redacting is the
/// safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 11] = [
```

换成

```rust
/// nodes behind a CDN routinely use as a shared secret. `shadow-tls-password`
/// needs its own entry: `password` only matches at a token boundary.
/// Over-redacting is the safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 12] = [
```

`crates/rurge-config/src/redact.rs`——把

```rust
    "ws-headers",
    "ws-path",
];
```

换成

```rust
    "ws-headers",
    "ws-path",
    "shadow-tls-password",
];
```

Run: `cargo test -p rurge-config --lib redact` → 9 passed。

- [ ] **Step 4: 新模块——先写用例，确认编译不过**

新建 `crates/rurge-config/src/spec/shadow_tls.rs`，**先只写文件头的 `use` 与文件末尾的 `#[cfg(test)] mod tests { … }`**（全文见 Step 5），并接上模块：

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod secret;
pub mod socks5;
```

换成

```rust
pub mod secret;
pub mod shadow_tls;
pub mod socks5;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub use secret::Secret;
```

换成

```rust
pub use secret::Secret;
pub use shadow_tls::{ShadowTlsOpts, ShadowTlsVersion};
```

`crates/rurge-config/src/spec/tls.rs`——把

```rust
fn is_server_name(name: &str) -> bool {
```

换成

```rust
pub(crate) fn is_server_name(name: &str) -> bool {
```

Run: `cargo test -p rurge-config --lib shadow_tls`

Expected: 编译错误——`cannot find function `read_shadow_tls``、`cannot find type `ShadowTlsOpts``、`cannot find function `allowed_on``。

- [ ] **Step 5: 写实现**

`crates/rurge-config/src/spec/shadow_tls.rs` 全文：

```rust
//! Shadow TLS parameters (manual: Policies › TLS and Shadow TLS): an
//! obfuscation layer below the policy's own protocol, for a policy that
//! reaches its server over TCP. `shadow-tls-password` switches it on.

use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::is_server_name;
use crate::diagnostic::codes;
use crate::policy::PolicyKind;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowTlsVersion {
    #[default]
    V2,
    V3,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowTlsOpts {
    pub password: Secret<String>,
    /// The name sent as SNI in the camouflage handshake. `None`: no SNI is
    /// sent at all (manual); v3 always has one.
    pub sni: Option<String>,
    pub version: ShadowTlsVersion,
}

const KEYS: [&str; 3] = [
    "shadow-tls-password",
    "shadow-tls-sni",
    "shadow-tls-version",
];

/// Whether Shadow TLS can wrap a policy of this kind. It wraps a TCP
/// connection, so the QUIC-based protocols and the two VPN-like ones are out
/// (manual: a configuration error).
pub fn allowed_on(kind: PolicyKind) -> bool {
    !matches!(
        kind,
        PolicyKind::Tuic
            | PolicyKind::TuicV5
            | PolicyKind::Hysteria2
            | PolicyKind::Masque
            | PolicyKind::WireGuard
            | PolicyKind::Tailscale
    )
}

/// The Shadow TLS layer of the policy, when it asks for one. After an error
/// was reported the returned value is meaningless: the caller checks
/// `r.has_errors()`.
pub fn read_shadow_tls(r: &mut ParamReader<'_>) -> Option<ShadowTlsOpts> {
    if !KEYS.iter().any(|key| r.has(key)) {
        return None;
    }
    let kind = r.policy().kind;
    if !allowed_on(kind) {
        for key in KEYS {
            r.touch(key);
        }
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!(
                "Shadow TLS cannot be combined with a `{}` policy",
                kind.keyword()
            ),
        );
        return None;
    }
    let Some(password) = r.str("shadow-tls-password") else {
        // no password, no Shadow TLS: the other two have nothing to act on
        for key in ["shadow-tls-sni", "shadow-tls-version"] {
            if r.has(key) {
                r.touch(key);
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect without `shadow-tls-password`; ignored"),
                );
            }
        }
        return None;
    };
    if password.is_empty() {
        // never echo the value of this one
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`shadow-tls-password` is empty".to_string(),
        );
    }
    let table = [("2", ShadowTlsVersion::V2), ("3", ShadowTlsVersion::V3)];
    let version = r.choice("shadow-tls-version", &table).unwrap_or_default();
    let mut sni = None;
    match r.str("shadow-tls-sni").map(str::trim) {
        // the SNI extension carries a DNS name, never an address
        Some(name) if is_server_name(name) && name.parse::<IpAddr>().is_err() => {
            sni = Some(name.to_string());
        }
        Some(name) => r.invalid(
            "shadow-tls-sni",
            name,
            "a host name (an IDN in its xn-- form)",
        ),
        None if version == ShadowTlsVersion::V3 => r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`shadow-tls-sni` is required when `shadow-tls-version=3`".to_string(),
        ),
        None => {}
    }
    Some(ShadowTlsOpts {
        password: password.into(),
        sni,
        version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (Option<ShadowTlsOpts>, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        // what every protocol reads for itself
        for key in ["password", "psk", "version", "reuse"] {
            r.touch(key);
        }
        let opts = read_shadow_tls(&mut r);
        let failed = r.has_errors();
        (opts, failed, r.finish())
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    #[test]
    fn the_manuals_two_examples() {
        let (opts, failed, diags) =
            read("snell, 1.2.3.4, 443, psk=pwd1, version=4, reuse=true, shadow-tls-password=pwd2");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let opts = opts.unwrap();
        assert_eq!(opts.password.expose(), "pwd2");
        assert_eq!((opts.sni, opts.version), (None, ShadowTlsVersion::V2));
        let (opts, failed, diags) = read(
            "snell, 1.2.3.4, 443, psk=pwd1, version=4, reuse=true, shadow-tls-password=pwd2, shadow-tls-version=3, shadow-tls-sni=example.com",
        );
        assert!(!failed && diags.is_empty(), "{diags:?}");
        let opts = opts.unwrap();
        assert_eq!(
            (opts.sni.as_deref(), opts.version),
            (Some("example.com"), ShadowTlsVersion::V3)
        );
        // nothing written, nothing read
        let (opts, failed, diags) = read("trojan, h.test, 443, password=p");
        assert!(opts.is_none() && !failed && diags.is_empty());
    }

    #[test]
    fn what_is_wrong_with_the_three_parameters_is_an_error() {
        let (_, failed, diags) = read("trojan, h.test, 443, password=p, shadow-tls-password=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `shadow-tls-password` is empty"
            )]
        );
        let (_, failed, diags) = read(
            "trojan, h.test, 443, password=p, shadow-tls-password=s3cret, shadow-tls-version=4",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid value `4` for `shadow-tls-version` (expected 2 / 3)"
            )]
        );
        let (_, failed, diags) = read(
            "trojan, h.test, 443, password=p, shadow-tls-password=s3cret, shadow-tls-version=3",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `shadow-tls-sni` is required when `shadow-tls-version=3`"
            )]
        );
        for bad in ["not a name", "192.0.2.1", "-x.test", ""] {
            let (_, failed, diags) = read(&format!(
                "trojan, h.test, 443, password=p, shadow-tls-password=s3cret, shadow-tls-sni={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM, "{bad}");
        }
        // the password is never quoted back
        assert!(diags.iter().all(|d| !d.message.contains("s3cret")));
    }

    #[test]
    fn the_other_two_do_nothing_without_a_password() {
        let (opts, failed, diags) = read(
            "trojan, h.test, 443, password=p, shadow-tls-sni=example.com, shadow-tls-version=3",
        );
        assert!(opts.is_none() && !failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `shadow-tls-sni` has no effect without `shadow-tls-password`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `shadow-tls-version` has no effect without `shadow-tls-password`; ignored"
                ),
            ]
        );
    }

    #[test]
    fn it_wraps_tcp_and_nothing_else() {
        use PolicyKind::*;
        for kind in [Tuic, TuicV5, Hysteria2, Masque, WireGuard, Tailscale] {
            assert!(!allowed_on(kind), "{kind:?}");
        }
        for kind in [
            Http,
            Https,
            H2Connect,
            Socks5,
            Socks5Tls,
            Shadowsocks,
            Snell,
            Vmess,
            Trojan,
            AnyTls,
            TrustTunnel,
            Ssh,
        ] {
            assert!(allowed_on(kind), "{kind:?}");
        }
        let (opts, failed, diags) =
            read("hysteria2, h.test, 443, password=p, shadow-tls-password=s3cret");
        assert!(opts.is_none() && failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: Shadow TLS cannot be combined with a `hysteria2` policy"
            )]
        );
    }
}
```

要点（评审时对照）：
- 没写任何一个参数 → `None`，不碰 `ParamReader`。
- 协议不允许（`allowed_on` 为假）→ 三个键都 `touch`（不再报"未知参数"），一条 `E0018`：`` Shadow TLS cannot be combined with a `<type>` policy ``。
- 没有 `shadow-tls-password` → 另外两个各一条 `W0028`：`` `<key>` has no effect without `shadow-tls-password`; ignored ``，返回 `None`。
- 口令为空 → `E0018` `` `shadow-tls-password` is empty ``（**永不回显口令**）；`shadow-tls-version` 用现成的 `ParamReader::choice`（报错时回显取值，它不是凭据）；`shadow-tls-sni` 必须是 DNS 名（`is_server_name` 且不是 IP 字面量）；v3 缺 `shadow-tls-sni` → `E0018`。

- [ ] **Step 6: 运行**

Run: `cargo test -p rurge-config --lib shadow_tls` → 4 passed（`the_manuals_two_examples`、`what_is_wrong_with_the_three_parameters_is_an_error`、`the_other_two_do_nothing_without_a_password`、`it_wraps_tcp_and_nothing_else`）。

- [ ] **Step 7: 门禁与提交**

跑 Global Constraints 里的门禁（三条命令）。此时 `to_spec` 尚未调用 `read_shadow_tls`，三个参数仍报 `W0029`：`spec::tests::notes_unknowns_and_limits` 保持原样通过。

```bash
git add crates/rurge-config/src/spec/shadow_tls.rs crates/rurge-config/src/spec/mod.rs crates/rurge-config/src/spec/tls.rs crates/rurge-config/src/redact.rs
git commit -m "feat(config): ShadowTlsOpts、能否包在某种协议外面的判断、公开的读取函数；脱敏名单补上 shadow-tls-password"
```

---

### Task 2: 记录读取器、带密钥的摘要链、帧化字节流

Shadow TLS 握手之后的全部线上格式。这个任务不需要 TLS：用例在 `tokio::io::duplex` 上用手工拼出的帧验证读写两个方向。握手（Task 4）还没来，所以模块声明带着 `#[allow(dead_code)]`（M2b 的 VMess 编解码任务用过同一个做法），Task 4 把它们去掉。

**Files:**
- Create: `crates/rurge-proto/src/transport/shadow_tls/mod.rs`
- Create: `crates/rurge-proto/src/transport/shadow_tls/record.rs`
- Create: `crates/rurge-proto/src/transport/shadow_tls/auth.rs`
- Create: `crates/rurge-proto/src/transport/shadow_tls/framed.rs`
- Create: `crates/rurge-proto/src/transport/shadow_tls/vectors.rs`
- Modify: `crates/rurge-proto/src/transport/mod.rs`

**Interfaces:**
- Consumes: `rurge_net::connector::BoxedStream`；`ring::hmac`；`sha2`；`rustls::ClientConnection`（只作为 `Mode::V2` 里的一个字段类型，这个任务的用例传 `None`）。
- Produces（都是 `pub(crate)`）:
  - `record::{HEADER: usize = 5, ALERT: u8 = 21, HANDSHAKE: u8 = 22, APPLICATION_DATA: u8 = 23, MAX_DATA: usize = 16384}`
  - `record::data_header(len: usize) -> [u8; 5]`；`record::cut_short() -> io::Error`
  - `record::RecordReader`（`Default`）：`poll_record(&mut self, cx, stream) -> Poll<io::Result<bool>>`、`next(&mut self, stream).await -> io::Result<bool>`（`false` = 对端在两条记录之间关闭）、`record(&mut self) -> &mut Vec<u8>`（含 5 字节头）、`consume(&mut self)`
  - `auth::{TAG: usize = 4, V2_TAG: usize = 8}`；`auth::Chain`（`Clone`，无 `Debug`）：`new(password: &[u8], seed: &[&[u8]])`、`update(&mut self, &[u8])`、`digest::<N>(&self) -> [u8; N]`、`frame_tag(&mut self, payload: &[u8]) -> [u8; 4]`
  - `auth::same(a: &[u8], b: &[u8]) -> bool`；`auth::xor_key(password: &[u8], server_random: &[u8; 32]) -> [u8; 32]`；`auth::xor(data: &mut [u8], key: &[u8; 32])`
  - `framed::Mode::{V2 { first: Option<[u8; 8]>, session: Option<Box<ClientConnection>> }, V3 { add: Chain, verify: Chain, ignore: Option<Chain> }}`（`framed` 模块是私有的，只有 `mod.rs` 用它）
  - `framed::Framed::new(inner: BoxedStream, reader: RecordReader, mode: Mode) -> Framed`：`AsyncRead + AsyncWrite + Unpin + Send`

错误文本（`io::Error`，都以 `shadow-tls:` 开头）：`the connection ended in the middle of a record`（`UnexpectedEof`）、`a record cannot be authenticated`、`unexpected record type`、`the handshake server closed the session`（后三条 `InvalidData`）。

- [ ] **Step 1: 模块声明**

`crates/rurge-proto/src/transport/mod.rs`——把

```rust
pub mod prefixed;
pub mod stack;
```

换成

```rust
pub mod prefixed;
pub mod shadow_tls;
pub mod stack;
```

新建 `crates/rurge-proto/src/transport/shadow_tls/mod.rs`：

```rust
//! Shadow TLS, client side (M2 design 5.3). The records, the keyed digests
//! and the framed stream live here; the handshake joins them in a later commit.

#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod auth;
#[allow(dead_code)] // until the handshake uses it (a later commit)
mod framed;
#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod record;
#[cfg(test)]
mod vectors;
```

- [ ] **Step 2: 先写向量与用例，确认编译不过**

新建 `crates/rurge-proto/src/transport/shadow_tls/vectors.rs`（向量的出处见文件头与 P10；**取值逐字照抄，不要重新计算**）：

```rust
//! Vectors for the keyed primitives of Shadow TLS. They were computed with
//! Python's `hmac` / `hashlib` by a script that mirrors, step by step, what
//! the reference implementation does (`ihciah/shadow-tls`: `Hmac`, `kdf`,
//! `xor_slice`, `generate_session_id`, `copy_by_frame_with_modification`,
//! `copy_add_appdata`, `verify_appdata`) and what `sing-shadowtls` does
//! (`v2_hash.go`, `v3_client.go`, `v3_server.go`, `v3_conn.go`). The fake
//! server shares these primitives with the client, so without the vectors a
//! mistake made on both sides would go unnoticed.
//!
//! Inputs: the password `vector-password`; the ServerRandom `20 21 .. 3f`.

use super::auth::{Chain, V2_TAG, xor_key};

const PASSWORD: &[u8] = b"vector-password";

fn unhex(parts: &[&str]) -> Vec<u8> {
    let text = parts.concat();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn server_random() -> [u8; 32] {
    std::array::from_fn(|i| 0x20 + i as u8)
}

#[test]
fn the_xor_key_is_sha256_of_password_and_server_random() {
    assert_eq!(
        xor_key(PASSWORD, &server_random())[..],
        unhex(&["e2eccdfee33a7db066a43dd90958affcb12c1cc10bfe4b6e0eec29e9ab936ca9"])[..]
    );
}

#[test]
fn data_tags_chain_per_direction_and_feed_themselves_back() {
    for (side, tags) in [
        (&b"C"[..], ["23155410", "352ad183", "2ab277ef"]),
        (&b"S"[..], ["aac92819", "6b406818", "571bab9d"]),
    ] {
        let mut chain = Chain::new(PASSWORD, &[&server_random(), side]);
        let third = b"third ".repeat(50);
        for (payload, tag) in [&b"first payload"[..], &b""[..], &third[..]]
            .into_iter()
            .zip(tags)
        {
            assert_eq!(chain.frame_tag(payload)[..], unhex(&[tag])[..]);
        }
    }
}

#[test]
fn the_v2_digest_is_eight_bytes_over_everything_fed() {
    let mut digest = Chain::new(PASSWORD, &[]);
    for part in [&b"ServerHello..."[..], b"ChangeCipherSpec", b"...Finished"] {
        digest.update(part);
    }
    assert_eq!(
        digest.digest::<V2_TAG>()[..],
        unhex(&["6ded756644915408"])[..]
    );
}
```

Run: `cargo test -p rurge-proto --lib shadow_tls`

Expected: 编译错误——`file not found for module `auth``（以及 `framed`、`record`）。

- [ ] **Step 3: `record.rs`**

```rust
//! TLS records as Shadow TLS sees them: a 5-byte header (type, version,
//! length) and up to 65535 bytes behind it. Nothing here decrypts anything.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, ReadBuf};

pub(crate) const HEADER: usize = 5;
pub(crate) const ALERT: u8 = 21;
pub(crate) const HANDSHAKE: u8 = 22;
pub(crate) const APPLICATION_DATA: u8 = 23;

/// What a data frame may carry. A TLS record holds at most 2^14 bytes of
/// plaintext, and a frame should not look bigger than the records it imitates.
pub(crate) const MAX_DATA: usize = 16384;

pub(crate) fn cut_short() -> io::Error {
    io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "shadow-tls: the connection ended in the middle of a record",
    )
}

/// The header of an ApplicationData record carrying `len` bytes.
pub(crate) fn data_header(len: usize) -> [u8; HEADER] {
    let len = u16::try_from(len).expect("a frame fits the 16-bit length field");
    let [hi, lo] = len.to_be_bytes();
    [APPLICATION_DATA, 3, 3, hi, lo]
}

/// Reads whole records, one at a time, across any number of polls. The length
/// field bounds the buffer: a record is never longer than 5 + 65535 bytes.
#[derive(Default)]
pub(crate) struct RecordReader {
    buf: Vec<u8>,
    filled: usize,
}

impl RecordReader {
    /// `Ok(true)`: `record()` holds a whole record, header included.
    /// `Ok(false)`: the peer closed between two records.
    pub(crate) fn poll_record<S: AsyncRead + Unpin + ?Sized>(
        &mut self,
        cx: &mut Context<'_>,
        stream: &mut S,
    ) -> Poll<io::Result<bool>> {
        if self.buf.len() < HEADER {
            // a fresh record: `filled` bytes of the previous one are gone
            self.buf.clear();
            self.buf.resize(HEADER, 0);
            self.filled = 0;
        }
        loop {
            if self.filled == self.buf.len() {
                if self.buf.len() > HEADER {
                    return Poll::Ready(Ok(true));
                }
                let len = usize::from(u16::from_be_bytes([self.buf[3], self.buf[4]]));
                if len == 0 {
                    return Poll::Ready(Ok(true));
                }
                self.buf.resize(HEADER + len, 0);
            }
            let mut space = ReadBuf::new(&mut self.buf[self.filled..]);
            ready!(Pin::new(&mut *stream).poll_read(cx, &mut space))?;
            let n = space.filled().len();
            if n == 0 {
                return Poll::Ready(if self.filled == 0 {
                    Ok(false)
                } else {
                    Err(cut_short())
                });
            }
            self.filled += n;
        }
    }

    /// The record `poll_record` just completed.
    pub(crate) fn record(&mut self) -> &mut Vec<u8> {
        &mut self.buf
    }

    /// Forgets the completed record: the next poll starts a new one.
    pub(crate) fn consume(&mut self) {
        self.buf.clear();
        self.filled = 0;
    }

    pub(crate) async fn next<S: AsyncRead + Unpin + ?Sized>(
        &mut self,
        stream: &mut S,
    ) -> io::Result<bool> {
        std::future::poll_fn(|cx| self.poll_record(cx, stream)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn records_come_out_whole_however_the_bytes_arrive() {
        let mut wire = Vec::new();
        for (kind, len) in [(HANDSHAKE, 300usize), (APPLICATION_DATA, 0), (ALERT, 2)] {
            wire.extend_from_slice(&[kind, 3, 3]);
            wire.extend_from_slice(&(len as u16).to_be_bytes());
            wire.extend((0..len).map(|i| i as u8));
        }
        for step in [1usize, 2, 3, 5, 7, 64, 4096] {
            let (mut tx, mut rx) = tokio::io::duplex(8);
            let bytes = wire.clone();
            let writer = tokio::spawn(async move {
                for piece in bytes.chunks(step) {
                    tx.write_all(piece).await.unwrap();
                }
            });
            let mut reader = RecordReader::default();
            let mut seen = Vec::new();
            while reader.next(&mut rx).await.unwrap() {
                let record = reader.record();
                seen.push((record[0], record.len() - HEADER));
                assert!(
                    record[HEADER..]
                        .iter()
                        .enumerate()
                        .all(|(i, b)| *b == i as u8)
                );
                reader.consume();
            }
            writer.await.unwrap();
            assert_eq!(
                seen,
                [(HANDSHAKE, 300), (APPLICATION_DATA, 0), (ALERT, 2)],
                "step {step}"
            );
        }
    }

    #[tokio::test]
    async fn an_end_inside_a_record_is_an_error_and_between_records_is_not() {
        for (wire, clean) in [
            (&[][..], true),
            (&[23, 3, 3][..], false),
            (&[23, 3, 3, 0, 4, 1, 2][..], false),
            (&[23, 3, 3, 0, 2, 1, 2][..], true),
        ] {
            let mut rx = wire;
            let mut reader = RecordReader::default();
            let mut result = Ok(());
            loop {
                match reader.next(&mut rx).await {
                    Ok(true) => reader.consume(),
                    Ok(false) => break,
                    Err(e) => {
                        result = Err(e);
                        break;
                    }
                }
            }
            match (result, clean) {
                (Ok(()), true) => {}
                (Err(e), false) => {
                    assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
                    assert_eq!(
                        e.to_string(),
                        "shadow-tls: the connection ended in the middle of a record"
                    );
                }
                (other, _) => panic!("{wire:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn the_data_header_is_an_application_data_record_of_tls_1_2() {
        assert_eq!(data_header(0x1234), [23, 3, 3, 0x12, 0x34]);
    }
}
```

- [ ] **Step 4: `auth.rs`**

```rust
//! The keyed digests of Shadow TLS: every one of them is HMAC-SHA1 under the
//! password, read a few bytes at a time while more data keeps going in.

use ring::hmac;
use sha2::{Digest, Sha256};

/// v3 puts 4 bytes of a digest in front of a payload; v2 sends 8, once.
pub(crate) const TAG: usize = 4;
pub(crate) const V2_TAG: usize = 8;

/// A running HMAC-SHA1. No `Debug`: it is keyed with the password. Boxed:
/// ring's context is some 300 bytes, and a stream holds three of them.
#[derive(Clone)]
pub(crate) struct Chain(Box<hmac::Context>);

impl Chain {
    pub(crate) fn new(password: &[u8], seed: &[&[u8]]) -> Chain {
        let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, password);
        let mut chain = Chain(Box::new(hmac::Context::with_key(&key)));
        for part in seed {
            chain.update(part);
        }
        chain
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    /// The first `N` bytes of the digest of everything fed so far. The chain
    /// itself goes on.
    pub(crate) fn digest<const N: usize>(&self) -> [u8; N] {
        let full = hmac::Context::clone(&self.0).sign();
        let mut out = [0u8; N];
        out.copy_from_slice(&full.as_ref()[..N]);
        out
    }

    /// A data frame of v3: the payload goes in, 4 bytes come out, and those 4
    /// bytes go in as well.
    pub(crate) fn frame_tag(&mut self, payload: &[u8]) -> [u8; TAG] {
        self.update(payload);
        let tag = self.digest::<TAG>();
        self.update(&tag);
        tag
    }
}

/// Compares every byte: no early exit on the first difference.
pub(crate) fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// v3: what a server XORs the handshake server's ApplicationData with.
pub(crate) fn xor_key(password: &[u8], server_random: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password);
    hasher.update(server_random);
    hasher.finalize().into()
}

/// The key starts over with every record.
pub(crate) fn xor(data: &mut [u8], key: &[u8; 32]) {
    for (byte, k) in data.iter_mut().zip(key.iter().cycle()) {
        *byte ^= k;
    }
}
```

- [ ] **Step 5: `framed.rs`**

```rust
//! The byte stream a Shadow TLS connection becomes after the camouflage
//! handshake: payload travels inside records that look like TLS
//! ApplicationData (`17 03 03 <len>`).
//!
//! - v2: the first frame a client writes starts with 8 bytes of the digest
//!   of the handshake; nothing else is authenticated. Until the server has
//!   seen that frame it keeps relaying the handshake server, so what arrives
//!   is offered to the camouflage session first: whatever that session can
//!   open (session tickets, as a rule) is not payload.
//! - v3: every frame is `<4-byte tag><payload>`, one HMAC chain per
//!   direction. Records the server was still relaying when the data phase
//!   began verify under the handshake's chain and are skipped.
//!
//! The same two contracts as `VmessStream`: a write that returned `Pending`
//! must be retried with the same bytes (the parked frame was built from them,
//! and in v3 its tag has already moved the chain on), and a read error is final.

use super::auth::{Chain, TAG, V2_TAG, same};
use super::record::{ALERT, APPLICATION_DATA, HEADER, MAX_DATA, RecordReader, data_header};
use rurge_net::connector::BoxedStream;
use rustls::ClientConnection;
use std::io::{self, Read};
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// No `Debug`: the chains are keyed with the password.
pub(crate) enum Mode {
    V2 {
        /// The digest of the handshake, owed to the first frame written.
        first: Option<[u8; V2_TAG]>,
        /// The camouflage session, kept until the first record it cannot open.
        session: Option<Box<ClientConnection>>,
    },
    V3 {
        add: Chain,
        verify: Chain,
        /// The handshake's chain, until the first record it does not verify.
        ignore: Option<Chain>,
    },
}

fn invalid(text: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, text)
}

enum Offered {
    /// A record of the camouflage session: not payload.
    Taken,
    /// Not something the camouflage session wrote: the data phase has begun.
    Foreign,
    /// The camouflage session ended: the server never switched to data.
    Ended,
}

fn offer(session: &mut ClientConnection, record: &[u8]) -> Offered {
    let mut rest = record;
    while !rest.is_empty() {
        if session.read_tls(&mut rest).is_err() {
            return Offered::Foreign;
        }
        match session.process_new_packets() {
            Ok(state) => {
                // nobody asked the handshake server for anything
                let mut sink = vec![0; state.plaintext_bytes_to_read()];
                let _ = session.reader().read(&mut sink);
                if state.peer_has_closed() {
                    return Offered::Ended;
                }
            }
            // what rustls says before it could open the record
            Err(
                rustls::Error::DecryptError
                | rustls::Error::InvalidMessage(_)
                | rustls::Error::PeerSentOversizedRecord,
            ) => return Offered::Foreign,
            // opened, and it was an alert or something out of place
            Err(_) => return Offered::Ended,
        }
    }
    Offered::Taken
}

impl Mode {
    /// Appends one frame carrying `payload` (at most `MAX_DATA` bytes).
    fn seal(&mut self, payload: &[u8], out: &mut Vec<u8>) {
        match self {
            Mode::V2 { first, .. } => {
                let prefix = first.take();
                let prefix = prefix.as_ref().map_or(&[][..], |p| &p[..]);
                out.extend_from_slice(&data_header(prefix.len() + payload.len()));
                out.extend_from_slice(prefix);
            }
            Mode::V3 { add, .. } => {
                out.extend_from_slice(&data_header(TAG + payload.len()));
                out.extend_from_slice(&add.frame_tag(payload));
            }
        }
        out.extend_from_slice(payload);
    }

    /// Where the payload of `record` starts; `None` for a record to skip.
    fn open(&mut self, record: &[u8]) -> io::Result<Option<usize>> {
        if let Mode::V2 { session, .. } = self
            && let Some(live) = session
        {
            match offer(live, record) {
                Offered::Taken => return Ok(None),
                Offered::Ended => {
                    return Err(invalid(
                        "shadow-tls: the handshake server closed the session",
                    ));
                }
                Offered::Foreign => *session = None,
            }
        }
        match (record[0], self) {
            // a server that answers our FIN with an alert may still be
            // sending: the end of the stream is the end of the connection
            (ALERT, _) => Ok(None),
            (APPLICATION_DATA, Mode::V2 { .. }) => Ok(Some(HEADER)),
            (APPLICATION_DATA, Mode::V3 { verify, ignore, .. }) => {
                let Some((tag, payload)) = record[HEADER..].split_at_checked(TAG) else {
                    return Err(invalid("shadow-tls: a record cannot be authenticated"));
                };
                if let Some(chain) = ignore {
                    chain.update(payload);
                    if same(&chain.digest::<TAG>(), tag) {
                        return Ok(None);
                    }
                    *ignore = None;
                }
                if !same(&verify.frame_tag(payload), tag) {
                    return Err(invalid("shadow-tls: a record cannot be authenticated"));
                }
                Ok(Some(HEADER + TAG))
            }
            _ => Err(invalid("shadow-tls: unexpected record type")),
        }
    }
}

pub(crate) struct Framed {
    inner: BoxedStream,
    reader: RecordReader,
    mode: Mode,
    /// Where the unread payload of the reader's current record starts.
    payload_at: Option<usize>,
    /// The frame being written, how much of it is out, and how many payload
    /// bytes it carries.
    out: Vec<u8>,
    out_pos: usize,
    accepted: usize,
}

impl Framed {
    /// `reader` comes from the handshake: it is between two records.
    pub(crate) fn new(inner: BoxedStream, reader: RecordReader, mode: Mode) -> Framed {
        Framed {
            inner,
            reader,
            mode,
            payload_at: None,
            out: Vec::new(),
            out_pos: 0,
            accepted: 0,
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

impl AsyncRead for Framed {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if let Some(at) = this.payload_at {
                let record = this.reader.record();
                let n = out.remaining().min(record.len() - at);
                out.put_slice(&record[at..at + n]);
                if at + n == record.len() {
                    this.payload_at = None;
                    this.reader.consume();
                } else {
                    this.payload_at = Some(at + n);
                }
                return Poll::Ready(Ok(()));
            }
            if !ready!(this.reader.poll_record(cx, &mut this.inner))? {
                return Poll::Ready(Ok(()));
            }
            let record = this.reader.record();
            match this.mode.open(record)? {
                // an empty payload is not the end of the stream
                Some(at) if at < record.len() => this.payload_at = Some(at),
                _ => this.reader.consume(),
            }
        }
    }
}

impl AsyncWrite for Framed {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        if this.out_pos == this.out.len() {
            let n = data.len().min(MAX_DATA);
            this.out.clear();
            this.out_pos = 0;
            this.mode.seal(&data[..n], &mut this.out);
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
        // a parked frame first: in v3 its tag is already part of the chain
        ready!(this.poll_out(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

    const PASSWORD: &[u8] = b"pw";
    const RANDOM: [u8; 32] = [7; 32];

    fn chain(side: &[u8]) -> Chain {
        Chain::new(PASSWORD, &[&RANDOM, side])
    }

    /// A v3 client stream over one end of an in-memory pipe, and the other end.
    fn v3(pipe: usize) -> (Framed, DuplexStream) {
        let (near, far) = tokio::io::duplex(pipe);
        let mode = Mode::V3 {
            add: chain(b"C"),
            verify: chain(b"S"),
            ignore: Some(Chain::new(PASSWORD, &[&RANDOM])),
        };
        (
            Framed::new(Box::new(near), RecordReader::default(), mode),
            far,
        )
    }

    fn v2(first: Option<[u8; V2_TAG]>) -> (Framed, DuplexStream) {
        let (near, far) = tokio::io::duplex(1 << 16);
        let mode = Mode::V2 {
            first,
            session: None,
        };
        (
            Framed::new(Box::new(near), RecordReader::default(), mode),
            far,
        )
    }

    /// What a v3 server would write for `payload`.
    fn sealed(chain: &mut Chain, payload: &[u8]) -> Vec<u8> {
        let mut out = data_header(TAG + payload.len()).to_vec();
        out.extend_from_slice(&chain.frame_tag(payload));
        out.extend_from_slice(payload);
        out
    }

    /// A record the server was still relaying: tagged by the handshake's
    /// chain, which does not take its own tags in.
    fn relayed(chain: &mut Chain, payload: &[u8]) -> Vec<u8> {
        chain.update(payload);
        let mut out = data_header(TAG + payload.len()).to_vec();
        out.extend_from_slice(&chain.digest::<TAG>());
        out.extend_from_slice(payload);
        out
    }

    #[tokio::test]
    async fn v3_frames_carry_the_client_chain_s_tags_and_split_at_the_record_limit() {
        let (mut stream, mut far) = v3(1 << 20);
        stream.write_all(b"hello").await.unwrap();
        let big = vec![0x42u8; MAX_DATA + 10];
        stream.write_all(&big).await.unwrap();
        // an empty write is not a frame
        assert_eq!(stream.write(&[]).await.unwrap(), 0);
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let mut expected = Vec::new();
        let mut c = chain(b"C");
        for payload in [&b"hello"[..], &big[..MAX_DATA], &big[MAX_DATA..]] {
            expected.extend_from_slice(&sealed(&mut c, payload));
        }
        assert!(wire == expected, "{} bytes on the wire", wire.len());
    }

    #[tokio::test]
    async fn v3_reads_skip_what_the_handshake_left_behind_empty_frames_and_alerts() {
        let (mut stream, mut far) = v3(1 << 16);
        let mut handshake = Chain::new(PASSWORD, &[&RANDOM]);
        let mut s = chain(b"S");
        let mut wire = Vec::new();
        wire.extend_from_slice(&relayed(&mut handshake, b"a session ticket"));
        wire.extend_from_slice(&relayed(&mut handshake, b"and another one"));
        wire.extend_from_slice(&sealed(&mut s, b"first "));
        wire.extend_from_slice(&sealed(&mut s, b""));
        wire.extend_from_slice(&[ALERT, 3, 3, 0, 2, 1, 0]);
        wire.extend_from_slice(&sealed(&mut s, b"second"));
        far.write_all(&wire).await.unwrap();
        far.shutdown().await.unwrap();
        // a small buffer: one payload is handed over in several reads
        let mut got = Vec::new();
        let mut buf = [0u8; 4];
        loop {
            let n = stream.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"first second");
    }

    #[tokio::test]
    async fn a_record_of_the_handshake_s_chain_is_not_accepted_once_data_has_begun() {
        let (mut stream, mut far) = v3(1 << 16);
        let mut handshake = Chain::new(PASSWORD, &[&RANDOM]);
        let mut s = chain(b"S");
        let mut wire = sealed(&mut s, b"data");
        wire.extend_from_slice(&relayed(&mut handshake, b"late"));
        far.write_all(&wire).await.unwrap();
        let mut buf = [0u8; 16];
        assert_eq!(stream.read(&mut buf).await.unwrap(), 4);
        let err = stream.read(&mut buf).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            err.to_string(),
            "shadow-tls: a record cannot be authenticated"
        );
    }

    #[tokio::test]
    async fn what_is_wrong_with_a_record_is_said_in_the_error() {
        let mut s = chain(b"S");
        let mut flipped = sealed(&mut s, b"payload");
        flipped[HEADER + TAG] ^= 1;
        let mut short = data_header(2).to_vec();
        short.extend_from_slice(&[0, 0]);
        let cases: [(Vec<u8>, io::ErrorKind, &str); 4] = [
            (
                flipped,
                io::ErrorKind::InvalidData,
                "shadow-tls: a record cannot be authenticated",
            ),
            (
                short,
                io::ErrorKind::InvalidData,
                "shadow-tls: a record cannot be authenticated",
            ),
            (
                vec![22, 3, 3, 0, 1, 0],
                io::ErrorKind::InvalidData,
                "shadow-tls: unexpected record type",
            ),
            (
                vec![23, 3, 3, 0, 9, 1, 2],
                io::ErrorKind::UnexpectedEof,
                "shadow-tls: the connection ended in the middle of a record",
            ),
        ];
        for (wire, kind, text) in cases {
            let (mut stream, mut far) = v3(1 << 16);
            far.write_all(&wire).await.unwrap();
            far.shutdown().await.unwrap();
            let mut buf = [0u8; 16];
            let err = stream.read(&mut buf).await.unwrap_err();
            assert_eq!((err.kind(), err.to_string().as_str()), (kind, text));
        }
    }

    #[tokio::test]
    async fn v2_puts_the_digest_in_front_of_the_first_frame_only() {
        let (mut stream, mut far) = v2(Some(*b"8 bytes!"));
        stream.write_all(b"one").await.unwrap();
        stream.write_all(b"two").await.unwrap();
        stream.shutdown().await.unwrap();
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        let mut expected = data_header(V2_TAG + 3).to_vec();
        expected.extend_from_slice(b"8 bytes!one");
        expected.extend_from_slice(&data_header(3));
        expected.extend_from_slice(b"two");
        assert_eq!(wire, expected);
    }

    #[tokio::test]
    async fn v2_reads_take_every_data_record_as_payload_whatever_its_length() {
        let (mut stream, mut far) = v2(None);
        // sing-box does not split what it copies: a frame may be longer than
        // any TLS record
        let long = vec![9u8; 40_000];
        let mut wire = data_header(3).to_vec();
        wire.extend_from_slice(b"abc");
        wire.extend_from_slice(&data_header(long.len()));
        wire.extend_from_slice(&long);
        let writer = tokio::spawn(async move {
            far.write_all(&wire).await.unwrap();
            far.shutdown().await.unwrap();
        });
        let mut got = Vec::new();
        stream.read_to_end(&mut got).await.unwrap();
        writer.await.unwrap();
        assert_eq!(got.len(), 3 + long.len());
        assert_eq!(&got[..3], b"abc");
    }

    #[tokio::test]
    async fn a_frame_parked_by_a_full_pipe_goes_out_once_and_whole() {
        // 64 bytes of pipe: every frame is parked half-written many times over
        let (stream, mut far) = v3(64);
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            let mut stream = stream;
            // the relay's way: `write_all`, then `flush`
            for piece in sent.chunks(8192) {
                stream.write_all(piece).await.unwrap();
                stream.flush().await.unwrap();
            }
            stream.shutdown().await.unwrap();
        });
        let mut wire = Vec::new();
        far.read_to_end(&mut wire).await.unwrap();
        writer.await.unwrap();
        // what a server does with it: every tag verifies, in order
        let mut c = chain(b"C");
        let (mut at, mut got) = (0, Vec::new());
        while at < wire.len() {
            let len = usize::from(u16::from_be_bytes([wire[at + 3], wire[at + 4]]));
            let (tag, payload) = wire[at + HEADER..at + HEADER + len].split_at(TAG);
            assert_eq!(c.frame_tag(payload), tag, "the frame at {at}");
            got.extend_from_slice(payload);
            at += HEADER + len;
        }
        assert!(got == data);
    }
}
```

要点（评审时对照）：
- 写是"写穿"的：`poll_write` 只在整帧交给下层之后才报告成功；没写完的帧留在 `out` 里（v3 的标签已经推进了链），下一次 `poll_write` / `poll_flush` / `poll_shutdown` 把它写完。调用方的约定与 `VmessStream` 相同：返回 `Pending` 的写必须用同一段字节重试。
- 读：负载为空的帧不是流的结束（跳过，绝不向调用方返回 0 字节）；alert 记录跳过（P5）；`Mode::V2` 在 `session` 还在时先把记录交给伪装会话（P2）——这一支要到 Task 4 才有用例，因为它需要一个真实的 TLS 会话。
- `poll_read` 从不写，`poll_write` 从不读：两个方向互不等待，转发循环可以在同一个任务里交错轮询两个 `split` 出来的半边。

- [ ] **Step 6: 运行**

Run: `cargo test -p rurge-proto --lib shadow_tls`

Expected: 13 passed——`record::tests` 3 条、`framed::tests` 7 条、`vectors` 3 条。

再核对两种构建形态都没有警告（dead-code 由 `mod.rs` 里的三个 `allow` 盖住，别处不应再需要）：

```bash
cargo clippy -p rurge-proto -- -D warnings
cargo clippy -p rurge-proto --features testing -- -D warnings
```

- [ ] **Step 7: 门禁与提交**

```bash
git add crates/rurge-proto/src/transport/mod.rs crates/rurge-proto/src/transport/shadow_tls
git commit -m "feat(proto): Shadow TLS 的记录读取器、带密钥的摘要链与握手后的帧化字节流（v2 的 8 字节首帧摘要、v3 的双向 HMAC 链），对参考实现的向量"
```

---

### Task 3: v3 的签名 ClientHello（M2 设计附录 A）

在 stock rustls 上让 ClientHello 的 SessionID 后 4 字节等于对这条 ClientHello 自身的 HMAC。做法、四条自检与它们为什么同时是单元用例，见设计附录 A；与 spike 的三点不同见 P9。

**Files:**
- Create: `crates/rurge-proto/src/transport/shadow_tls/sign.rs`
- Modify: `crates/rurge-proto/src/transport/shadow_tls/mod.rs`（声明）
- Modify: `crates/rurge-proto/src/transport/shadow_tls/vectors.rs`（ClientHello 标签的向量）

**Interfaces:**
- Consumes: Task 2 的 `auth::{Chain, TAG}`、`record::{HANDSHAKE, HEADER}`；`rustls::crypto::{ActiveKeyExchange, CryptoProvider, GetRandomFailed, SecureRandom, SharedSecret, SupportedKxGroup}`；`ring::rand::SystemRandom`。
- Produces（`pub(crate)`）:
  - `sign::provider() -> Arc<CryptoProvider>`：ring 的 provider，换上脚本化的随机源，`kx_groups` 只有脚本化的 X25519
  - `sign::signed_hello(config: &Arc<ClientConfig>, name: &ServerName<'static>, password: &[u8]) -> Option<(ClientConnection, Vec<u8>)>`：连接，以及**已经从连接里取出来**的那条 ClientHello 记录（调用方负责发送）；任何一条自检不满足 → `None`。`config` 必须建在 `provider()` 上且关闭了会话恢复
  - `sign::hello_tag(password: &[u8], hello: &[u8]) -> [u8; 4]`（`hello` 含 5 字节记录头）

- [ ] **Step 1: 声明模块**

`crates/rurge-proto/src/transport/shadow_tls/mod.rs` 改成：

```rust
//! Shadow TLS, client side (M2 design 5.3). The records, the keyed digests,
//! the framed stream and the signed ClientHello of v3 live here; the handshake
//! joins them in a later commit.

#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod auth;
#[allow(dead_code)] // until the handshake uses it (a later commit)
mod framed;
#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod record;
#[allow(dead_code)] // until the handshake uses it (a later commit)
pub(crate) mod sign;
#[cfg(test)]
mod vectors;
```

- [ ] **Step 2: 先加向量，确认编译不过**

`crates/rurge-proto/src/transport/shadow_tls/vectors.rs`——把

```rust
use super::auth::{Chain, V2_TAG, xor_key};
```

换成

```rust
use super::auth::{Chain, V2_TAG, xor_key};
use super::sign::hello_tag;
```

并在文件末尾追加：

```rust

const HELLO: &[&str] = &[
    "160301006f0100006b0303000102030405060708090a0b0c0d0e0f1011121314",
    "15161718191a1b1c1d1e1f20808182838485868788898a8b8c8d8e8f90919293",
    "9495969798999a9b9c9d9e9fc0c1c2c3c4c5c6c7c8c9cacbcccdcecfd0d1d2d3",
    "d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7",
];

#[test]
fn the_client_hello_tag_covers_the_hello_without_its_record_header() {
    // HMAC-SHA1(password, hello[5..72] || 00 00 00 00 || hello[76..]), 4 bytes
    assert_eq!(hello_tag(PASSWORD, &unhex(HELLO)), unhex(&["a7640deb"])[..]);
}
```

Run: `cargo test -p rurge-proto --lib shadow_tls`

Expected: 编译错误——`file not found for module `sign``。

- [ ] **Step 3: `sign.rs`**

```rust
//! Shadow TLS v3 wants the last 4 bytes of the ClientHello's session id to be
//! an HMAC over that very ClientHello — and the session id is part of the
//! handshake transcript, so it cannot be patched after rustls wrote it.
//!
//! Everything random in a rustls ClientHello comes from two replaceable
//! places: `CryptoProvider::secure_random` (the client random, the session
//! id, the seed that shuffles the extensions) and `SupportedKxGroup::start`
//! (the key share). So the hello is built twice, synchronously, on one
//! thread (M2 design, appendix A):
//!
//! 1. pass 1 records every random draw on a tape and parks the real key
//!    exchange, handing the throw-away connection the public half only;
//! 2. the HMAC over hello #1 is patched into the tape, where the session id was drawn;
//! 3. pass 2 replays the tape and takes the parked key exchange.
//!
//! Hello #2 is hello #1 but for those 4 bytes, so the HMAC holds; and it is
//! the real connection's own hello, so its transcript is consistent. Four
//! checks guard the assumptions; when one fails nothing is sent at all.

use super::auth::{Chain, TAG};
use super::record::{HANDSHAKE, HEADER};
use rustls::crypto::{
    ActiveKeyExchange, CryptoProvider, GetRandomFailed, SecureRandom, SharedSecret,
    SupportedKxGroup,
};
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, NamedGroup};
use std::cell::RefCell;
use std::sync::Arc;

const CLIENT_HELLO: u8 = 1;
const SESSION_ID_LEN: usize = 32;
/// Where the length byte of the session id sits in the record: behind the
/// record header, the handshake header (4), the version (2) and the random (32).
const SESSION_ID_LEN_AT: usize = HEADER + 4 + 2 + 32;
const SESSION_ID_AT: usize = SESSION_ID_LEN_AT + 1;
const TAG_AT: usize = SESSION_ID_AT + SESSION_ID_LEN - TAG;

enum Mode {
    Off,
    Record,
    Replay,
}

struct Script {
    mode: Mode,
    tape: Vec<u8>,
    cursor: usize,
    parked: Option<Box<dyn ActiveKeyExchange>>,
}

impl Script {
    const fn off() -> Script {
        Script {
            mode: Mode::Off,
            tape: Vec::new(),
            cursor: 0,
            parked: None,
        }
    }
}

thread_local! {
    static SCRIPT: RefCell<Script> = const { RefCell::new(Script::off()) };
}

/// Switches the script off again however `signed_hello` is left.
struct Reset;

impl Drop for Reset {
    fn drop(&mut self) {
        SCRIPT.with(|s| *s.borrow_mut() = Script::off());
    }
}

fn system_random(buf: &mut [u8]) -> Result<(), GetRandomFailed> {
    // what rustls' own ring provider draws from
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), buf)
        .map_err(|_| GetRandomFailed)
}

#[derive(Debug)]
struct ScriptedRandom;

impl SecureRandom for ScriptedRandom {
    fn fill(&self, buf: &mut [u8]) -> Result<(), GetRandomFailed> {
        SCRIPT.with(|s| {
            let mut s = s.borrow_mut();
            match s.mode {
                Mode::Off => system_random(buf),
                Mode::Record => {
                    system_random(buf)?;
                    s.tape.extend_from_slice(buf);
                    Ok(())
                }
                Mode::Replay => {
                    let end = s.cursor + buf.len();
                    let Some(drawn) = s.tape.get(s.cursor..end) else {
                        return Err(GetRandomFailed);
                    };
                    buf.copy_from_slice(drawn);
                    s.cursor = end;
                    Ok(())
                }
            }
        })
    }
}

/// What pass 1 hands to its throw-away connection.
struct PublicHalf {
    public: Vec<u8>,
    group: NamedGroup,
}

impl ActiveKeyExchange for PublicHalf {
    fn complete(self: Box<Self>, _peer: &[u8]) -> Result<SharedSecret, rustls::Error> {
        Err(rustls::Error::General(
            "a recorded key exchange is never completed".into(),
        ))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public
    }

    fn group(&self) -> NamedGroup {
        self.group
    }
}

#[derive(Debug)]
struct ScriptedX25519;

impl SupportedKxGroup for ScriptedX25519 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, rustls::Error> {
        let real = rustls::crypto::ring::kx_group::X25519;
        SCRIPT.with(|s| {
            let mut s = s.borrow_mut();
            match s.mode {
                Mode::Off => real.start(),
                Mode::Record => {
                    let started = real.start()?;
                    let public = PublicHalf {
                        public: started.pub_key().to_vec(),
                        group: started.group(),
                    };
                    s.parked = Some(started);
                    Ok(Box::new(public) as Box<dyn ActiveKeyExchange>)
                }
                Mode::Replay => s
                    .parked
                    .take()
                    .ok_or_else(|| rustls::Error::General("no recorded key exchange".into())),
            }
        })
    }

    fn name(&self) -> NamedGroup {
        rustls::crypto::ring::kx_group::X25519.name()
    }
}

static RANDOM: ScriptedRandom = ScriptedRandom;
static X25519: ScriptedX25519 = ScriptedX25519;

/// The ring provider with the two scripted pieces: only for the camouflage
/// handshake of Shadow TLS v3. While no script runs it behaves like ring's
/// own, except that X25519 is the only key exchange group on offer.
pub(crate) fn provider() -> Arc<CryptoProvider> {
    Arc::new(CryptoProvider {
        kx_groups: vec![&X25519],
        secure_random: &RANDOM,
        ..rustls::crypto::ring::default_provider()
    })
}

fn first_flight(conn: &mut ClientConnection) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    conn.write_tls(&mut out).ok()?;
    Some(out)
}

/// Exactly one handshake record holding a ClientHello with a 32-byte session id.
fn well_formed(hello: &[u8]) -> bool {
    hello.len() >= SESSION_ID_AT + SESSION_ID_LEN
        && hello[0] == HANDSHAKE
        && usize::from(u16::from_be_bytes([hello[3], hello[4]])) + HEADER == hello.len()
        && hello[HEADER] == CLIENT_HELLO
        && usize::from(hello[SESSION_ID_LEN_AT]) == SESSION_ID_LEN
}

/// HMAC-SHA1 under the password over the ClientHello without its record
/// header and with the 4 bytes of the tag as zeroes.
pub(crate) fn hello_tag(password: &[u8], hello: &[u8]) -> [u8; TAG] {
    let mut chain = Chain::new(password, &[]);
    chain.update(&hello[HEADER..TAG_AT]);
    chain.update(&[0; TAG]);
    chain.update(&hello[TAG_AT + TAG..]);
    chain.digest()
}

/// A connection whose ClientHello carries the tag, and that ClientHello
/// (already taken out of the connection: the caller sends it). `None` when
/// one of the assumptions does not hold — the caller must not connect then.
///
/// `config` must have been built on `provider()` with resumption disabled.
pub(crate) fn signed_hello(
    config: &Arc<ClientConfig>,
    name: &ServerName<'static>,
    password: &[u8],
) -> Option<(ClientConnection, Vec<u8>)> {
    let _reset = Reset;
    SCRIPT.with(|s| s.borrow_mut().mode = Mode::Record);
    let mut rehearsal = ClientConnection::new(config.clone(), name.clone()).ok()?;
    let first = first_flight(&mut rehearsal)?;
    drop(rehearsal);
    // check 1: the shape the offsets above rely on
    if !well_formed(&first) {
        return None;
    }
    let tag = hello_tag(password, &first);
    let session_id = &first[SESSION_ID_AT..SESSION_ID_AT + SESSION_ID_LEN];
    let patched = SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let mut hits = s
            .tape
            .windows(SESSION_ID_LEN)
            .enumerate()
            .filter(|(_, window)| *window == session_id)
            .map(|(at, _)| at);
        // check 2: the session id is exactly one draw
        let (Some(at), None) = (hits.next(), hits.next()) else {
            return false;
        };
        let at = at + SESSION_ID_LEN - TAG;
        s.tape[at..at + TAG].copy_from_slice(&tag);
        s.mode = Mode::Replay;
        true
    });
    if !patched {
        return None;
    }
    let mut conn = ClientConnection::new(config.clone(), name.clone()).ok()?;
    let hello = first_flight(&mut conn)?;
    // check 3: pass 2 drew exactly what pass 1 drew, and took the key exchange
    let replayed = SCRIPT.with(|s| {
        let s = s.borrow();
        s.cursor == s.tape.len() && s.parked.is_none()
    });
    // check 4: the two hellos differ in the tag and nowhere else
    let same_but_the_tag = hello.len() == first.len()
        && hello[..TAG_AT] == first[..TAG_AT]
        && hello[TAG_AT + TAG..] == first[TAG_AT + TAG..]
        && hello[TAG_AT..TAG_AT + TAG] == tag;
    (replayed && same_but_the_tag).then_some((conn, hello))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rustls::RootCertStore;

    pub(crate) fn config(roots: Arc<RootCertStore>) -> Arc<ClientConfig> {
        let mut config = ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        config.resumption = rustls::client::Resumption::disabled();
        Arc::new(config)
    }

    fn name() -> ServerName<'static> {
        ServerName::try_from("camouflage.test").unwrap()
    }

    // The four self-checks of appendix A are these tests: a rustls upgrade
    // that breaks an assumption turns them red.

    #[test]
    fn the_hello_is_one_record_with_a_32_byte_session_id_and_carries_the_tag() {
        let config = config(Arc::new(RootCertStore::empty()));
        for _ in 0..20 {
            let (_conn, hello) = signed_hello(&config, &name(), b"pw").expect("signed");
            assert!(well_formed(&hello));
            let tag = &hello[TAG_AT..TAG_AT + TAG];
            assert_eq!(tag, hello_tag(b"pw", &hello));
            assert_ne!(tag, hello_tag(b"another", &hello), "the tag is keyed");
        }
    }

    #[test]
    fn the_script_is_off_again_afterwards_and_unscripted_hellos_differ() {
        let config = config(Arc::new(RootCertStore::empty()));
        let (_a, first) = signed_hello(&config, &name(), b"pw").unwrap();
        SCRIPT.with(|s| {
            let s = s.borrow();
            assert!(matches!(s.mode, Mode::Off));
            assert!(s.tape.is_empty() && s.parked.is_none());
        });
        // no script: fresh randomness, a fresh key share
        let mut plain = ClientConnection::new(config.clone(), name()).unwrap();
        let unscripted = first_flight(&mut plain).unwrap();
        assert!(well_formed(&unscripted));
        assert_ne!(
            unscripted[HEADER + 6..HEADER + 38],
            first[HEADER + 6..HEADER + 38]
        );
        let (_b, second) = signed_hello(&config, &name(), b"pw").unwrap();
        assert_ne!(second, first, "every signed hello is fresh");
    }

    #[test]
    fn a_config_on_another_provider_is_refused_rather_than_sent_unsigned() {
        // ring's own provider never touches the tape: check 2 fails
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(RootCertStore::empty())
                .with_no_client_auth();
        assert!(signed_hello(&Arc::new(config), &name(), b"pw").is_none());
        SCRIPT.with(|s| assert!(matches!(s.borrow().mode, Mode::Off)));
    }
}
```

要点（评审时对照）：
- 两遍构造之间**没有 `await`**，整个函数是同步的：线程局部的脚本不会被别的任务看到。
- `Reset` 守卫保证任何返回路径（含 `?` 提前返回）都把脚本复位为 `Off`；用例 `a_config_on_another_provider_is_refused_rather_than_sent_unsigned` 钉住失败路径上的复位。
- 四条自检：① 恰好一条握手记录、ClientHello、SessionID 长 32；② SessionID 在带子上恰好出现一次；③ 第二遍恰好用完带子并取走寄存的密钥交换；④ 两遍的 ClientHello 只在标签的 4 个字节上不同，且那 4 个字节就是算出来的标签。
- 错误路径不 panic、不 `unwrap`：一切失败都是 `None`。

- [ ] **Step 4: 运行**

Run: `cargo test -p rurge-proto --lib shadow_tls`

Expected: 17 passed（Task 2 的 13 条 + `sign::tests` 3 条 + 1 条向量）。

```bash
cargo clippy -p rurge-proto -- -D warnings
cargo clippy -p rurge-proto --features testing -- -D warnings
```

- [ ] **Step 5: 门禁与提交**

```bash
git add crates/rurge-proto/src/transport/shadow_tls
git commit -m "feat(proto): Shadow TLS v3 在 stock rustls 上签名 ClientHello——脚本化的随机源与 X25519、两遍构造、四条自检"
```

---

### Task 4: 伪装握手（v2 / v3）、体面收尾，与回环假服务端

`ShadowTlsClient`：构建期准备好 rustls 配置，连接期自己驱动伪装握手（一次一条记录），然后交出 Task 2 的帧化流。同一个任务里交付它的对端：`FakeShadowTls`（按参考服务端逐记录转发伪装握手）与 `Camouflage`（伪装站点），否则握手路径没有用例。

**Files:**
- Modify: `crates/rurge-proto/src/transport/shadow_tls/mod.rs`（整份替换；三个 `#[allow(dead_code)]` 到此去掉）
- Modify: `crates/rurge-proto/src/transport/shadow_tls/vectors.rs`（握手期记录的向量）
- Create: `crates/rurge-proto/src/testing/shadow_tls.rs`
- Modify: `crates/rurge-proto/src/testing/mod.rs`
- Modify: `crates/rurge-proto/src/testing/tls.rs`

**Interfaces:**
- Consumes: Task 1 的 `ShadowTlsOpts` / `ShadowTlsVersion`；Task 2、3 的全部；`crate::{BuildError, OutboundError}`、`crate::outbound::untrusted_text`；`rurge_config::HostName`。
- Produces:
  - `rurge_proto::transport::shadow_tls::ShadowTlsClient`（`pub`，无 `Debug`）
    - `ShadowTlsClient::build(opts: &ShadowTlsOpts, fallback: &HostName, roots: Arc<RootCertStore>) -> Result<ShadowTlsClient, BuildError>`——`fallback` 是没写 `shadow-tls-sni` 时用来校验证书的名字（那时不发 SNI，P7）
    - `ShadowTlsClient::wrap(&self, stream: BoxedStream).await -> Result<BoxedStream, OutboundError>`——自己没有超时（调用方把整条阶梯包进一个超时），只有体面收尾另有 2 秒上限
  - `rurge_proto::testing::{Camouflage, FakeShadowTls, RecordedShadowTls, ShadowTlsFault, ShadowTlsScript}`
    - `Camouflage::spawn(fixture: &Arc<TlsFixture>, versions: &[&'static SupportedProtocolVersion], tickets: usize).await`、`.addr()`、`.received() -> Vec<u8>`（客户端在 TLS 会话里说过的全部明文）
    - `ShadowTlsScript::new(version, password: &str, camouflage: SocketAddr, connect_to: SocketAddr)`；公开字段 `residual` `late_records` `alert_on_fin` `empty_record` `fault`
    - `FakeShadowTls::spawn(script).await`、`.addr()`、`.sessions() -> Vec<RecordedShadowTls>`（`authenticated: bool`，在服务端做出判断的那一刻记下）
  - `TlsFixture::camouflage_acceptor(versions, tickets) -> TlsAcceptor`、`TlsFixture::seen_at_least(count).await -> Vec<SeenHandshake>`

错误文本（`OutboundError::Proxy`）：`shadow-tls: cannot sign the ClientHello`、`shadow-tls: the camouflage handshake failed: <rustls 的文本，经 untrusted_text，≤ 200 字符>`、`shadow-tls: the server closed the connection during the handshake`、`shadow-tls: the handshake server does not support TLS 1.3`、`shadow-tls: the server did not authenticate itself`。构建错误：`the Shadow TLS handshake has no valid server name`、`shadow-tls: cannot sign the ClientHello`。

- [ ] **Step 1: 夹具——`TlsFixture` 的两个新方法与 echo 的 `flush`**

`crates/rurge-proto/src/testing/tls.rs`：在 `impostor_acceptor` 的文档注释（`/// A server that presents the fixture's real leaf certificate but signs`）之前插入：

```rust
    /// What a Shadow TLS server relays a handshake to: the given protocol
    /// versions, no ALPN, and `tickets` session tickets after every TLS 1.3
    /// handshake (rustls' own default is 2).
    pub fn camouflage_acceptor(
        &self,
        versions: &[&'static SupportedProtocolVersion],
        tickets: usize,
    ) -> TlsAcceptor {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.clone()));
        let mut config = ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(versions)
            .expect("protocol versions")
            .with_no_client_auth()
            .with_single_cert(vec![self.leaf.clone()], key)
            .expect("server certificate");
        config.send_tls13_tickets = tickets;
        TlsAcceptor::from(Arc::new(config))
    }
```

在 `spawn_with_acceptor` 的文档注释（`/// Runs the echo accept loop behind `acceptor`; failed handshakes are`）之前插入：

```rust
    /// `seen()`, once it holds at least `count` handshakes: the server side
    /// writes one down after the client already has its stream. Panics when
    /// they do not show up within five seconds.
    pub async fn seen_at_least(&self, count: usize) -> Vec<SeenHandshake> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let seen = self.seen();
            if seen.len() >= count {
                return seen;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{} of {count} handshakes were recorded",
                seen.len()
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
```

并把 echo 循环

```rust
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
```

换成

```rust
                    while let Ok(n) = stream.read(&mut buf).await {
                        // `flush`: tokio-rustls reports a write as done while
                        // ciphertext may still sit in its own buffer, and
                        // with nothing more to echo that tail would stay there
                        if n == 0
                            || stream.write_all(&buf[..n]).await.is_err()
                            || stream.flush().await.is_err()
                        {
                            break;
                        }
                    }
```

（P17：没有这个 `flush`，Step 5 里"里层 TLS 跑在帧里"的用例约每 20 – 50 轮卡死一次。）

- [ ] **Step 2: 假服务端与伪装站点**

新建 `crates/rurge-proto/src/testing/shadow_tls.rs`：

```rust
//! A Shadow TLS server on a loopback port (v2 or v3) and the camouflage site
//! it relays handshakes to. The server side is written from the protocol
//! documents and the reference server, record by record — it shares the
//! keyed primitives with the client (vectors pin those), not its framing.
//! It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::transport::shadow_tls::auth::{Chain, TAG, V2_TAG, same, xor, xor_key};
use crate::transport::shadow_tls::record::{
    ALERT, APPLICATION_DATA, HANDSHAKE, HEADER, RecordReader, data_header,
};
use crate::transport::shadow_tls::sign::hello_tag;
use rurge_config::spec::ShadowTlsVersion;
use rustls::SupportedProtocolVersion;
use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, watch};

const CHANGE_CIPHER_SPEC: u8 = 20;
const SERVER_HELLO: u8 = 2;

/// What goes wrong at the start of the data phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowTlsFault {
    /// v3: a data record whose tag is wrong.
    BadTag,
    /// The connection ends in the middle of a record.
    CutRecord,
    /// A record that is neither data nor an alert.
    StrayHandshake,
}

/// No `Debug`: it holds the password.
#[derive(Clone)]
pub struct ShadowTlsScript {
    pub version: ShadowTlsVersion,
    pub password: String,
    /// The handshake server: a TLS server on a loopback port.
    pub camouflage: SocketAddr,
    /// Where the payload of a client that proved itself goes.
    pub connect_to: SocketAddr,
    /// v3: records sealed under the handshake's chain, sent ahead of the
    /// first data record (what a session ticket in flight looks like).
    pub residual: usize,
    /// v2: how many records of the handshake server must have gone down
    /// after the client's Finished before the data phase may start (rustls
    /// packs its session tickets into one record).
    pub late_records: usize,
    /// Answer the client's FIN with an alert record and keep sending (sing-box).
    pub alert_on_fin: bool,
    /// An empty data record ahead of everything else.
    pub empty_record: bool,
    pub fault: Option<ShadowTlsFault>,
}

impl ShadowTlsScript {
    pub fn new(
        version: ShadowTlsVersion,
        password: &str,
        camouflage: SocketAddr,
        connect_to: SocketAddr,
    ) -> ShadowTlsScript {
        ShadowTlsScript {
            version,
            password: password.to_string(),
            camouflage,
            connect_to,
            residual: 0,
            late_records: 0,
            alert_on_fin: false,
            empty_record: false,
            fault: None,
        }
    }
}

/// Written down the moment the server knows: a client that proved itself
/// when its first data record verified, any other when it is turned into a
/// plain relay (v3) or when its connection ends (v2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedShadowTls {
    pub authenticated: bool,
}

type Sessions = Arc<Mutex<Vec<RecordedShadowTls>>>;

fn note(sessions: &Sessions, authenticated: bool) {
    sessions
        .lock()
        .expect("sessions")
        .push(RecordedShadowTls { authenticated });
}

pub struct FakeShadowTls {
    addr: SocketAddr,
    sessions: Sessions,
    _task: AbortOnDrop,
}

impl FakeShadowTls {
    pub async fn spawn(script: ShadowTlsScript) -> FakeShadowTls {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let sessions = Sessions::default();
        let recorded = sessions.clone();
        let task = tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let (script, recorded) = (script.clone(), recorded.clone());
                tokio::spawn(async move {
                    // an error here is the client's doing: the tests look at
                    // what the client saw
                    let _ = match script.version {
                        ShadowTlsVersion::V2 => serve_v2(client, &script, &recorded).await,
                        ShadowTlsVersion::V3 => serve_v3(client, &script, &recorded).await,
                    };
                });
            }
        });
        FakeShadowTls {
            addr,
            sessions,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn sessions(&self) -> Vec<RecordedShadowTls> {
        self.sessions.lock().expect("sessions").clone()
    }
}

/// What a server does with a client that did not prove itself: it is a plain
/// relay in front of the handshake server.
async fn plain_relay(
    mut client: TcpStream,
    mut camouflage: TcpStream,
    sessions: &Sessions,
) -> io::Result<()> {
    note(sessions, false);
    let _ = tokio::io::copy_bidirectional(&mut client, &mut camouflage).await;
    Ok(())
}

async fn serve_v3(
    mut client: TcpStream,
    script: &ShadowTlsScript,
    sessions: &Sessions,
) -> io::Result<()> {
    let password = script.password.as_bytes();
    let mut from_client = RecordReader::default();
    if !from_client.next(&mut client).await? {
        return Ok(());
    }
    let hello = from_client.record().clone();
    from_client.consume();
    // type, ClientHello, a 32-byte session id whose last 4 bytes are the tag
    let signed = hello.len() >= 76
        && hello[0] == HANDSHAKE
        && hello[HEADER] == 1
        && hello[43] == 32
        && same(&hello_tag(password, &hello), &hello[72..76]);
    let mut camouflage = TcpStream::connect(script.camouflage).await?;
    camouflage.write_all(&hello).await?;
    if !signed {
        return plain_relay(client, camouflage, sessions).await;
    }
    let mut from_camouflage = RecordReader::default();
    if !from_camouflage.next(&mut camouflage).await? {
        return Ok(());
    }
    let first = from_camouflage.record().clone();
    from_camouflage.consume();
    client.write_all(&first).await?;
    if first.len() < 43 || first[0] != HANDSHAKE || first[HEADER] != SERVER_HELLO {
        return plain_relay(client, camouflage, sessions).await;
    }
    let random: [u8; 32] = first[11..43].try_into().expect("32 bytes");
    let key = xor_key(password, &random);
    let mut handshake_chain = Chain::new(password, &[&random]);
    let (mut client_r, mut client_w) = tokio::io::split(client);
    let (mut camouflage_r, mut camouflage_w) = tokio::io::split(camouflage);
    let stop = Notify::new();
    let mut verify = Chain::new(password, &[&random, b"C"]);
    let upwards = async {
        // the client's records go to the handshake server until one of them
        // verifies under the client's chain: that one is the first payload
        let found = loop {
            if !from_client.next(&mut client_r).await? {
                break None;
            }
            let record = from_client.record();
            if record[0] == APPLICATION_DATA && record.len() > HEADER + TAG {
                let mut attempt = verify.clone();
                let tag = attempt.frame_tag(&record[HEADER + TAG..]);
                if same(&tag, &record[HEADER..HEADER + TAG]) {
                    verify = attempt;
                    let payload = record[HEADER + TAG..].to_vec();
                    from_client.consume();
                    break Some(payload);
                }
            }
            camouflage_w.write_all(record).await?;
            from_client.consume();
        };
        stop.notify_one();
        io::Result::Ok(found)
    };
    let downwards = async {
        loop {
            // stopped between two records only: a record is never cut
            tokio::select! {
                biased;
                _ = stop.notified() => return io::Result::Ok(()),
                more = from_camouflage.next(&mut camouflage_r) => {
                    if !more? {
                        return Ok(());
                    }
                }
            }
            let record = from_camouflage.record();
            if record[0] == APPLICATION_DATA {
                xor(&mut record[HEADER..], &key);
                handshake_chain.update(&record[HEADER..]);
                let tag = handshake_chain.digest::<TAG>();
                client_w
                    .write_all(&data_header(TAG + record.len() - HEADER))
                    .await?;
                client_w.write_all(&tag).await?;
                client_w.write_all(&record[HEADER..]).await?;
            } else {
                client_w.write_all(record).await?;
            }
            from_camouflage.consume();
        }
    };
    let (found, relayed) = tokio::join!(upwards, downwards);
    relayed?;
    let Some(first_payload) = found? else {
        note(sessions, false);
        return Ok(());
    };
    note(sessions, true);
    drop((camouflage_r, camouflage_w));
    for n in 0..script.residual {
        let mut payload = vec![n as u8; 40 + n];
        xor(&mut payload, &key);
        handshake_chain.update(&payload);
        client_w
            .write_all(&data_header(TAG + payload.len()))
            .await?;
        client_w.write_all(&handshake_chain.digest::<TAG>()).await?;
        client_w.write_all(&payload).await?;
    }
    let phase = DataPhase {
        client_r,
        client_w,
        from_client,
        up: Tagging::V3(verify),
        down: Tagging::V3(Chain::new(password, &[&random, b"S"])),
        first_payload,
    };
    data_phase(phase, script).await
}

async fn serve_v2(
    client: TcpStream,
    script: &ShadowTlsScript,
    sessions: &Sessions,
) -> io::Result<()> {
    let password = script.password.as_bytes();
    let camouflage = TcpStream::connect(script.camouflage).await?;
    let (mut client_r, mut client_w) = tokio::io::split(client);
    let (mut camouflage_r, mut camouflage_w) = tokio::io::split(camouflage);
    // every byte written to the client while the handshake is relayed
    let written = Mutex::new(Chain::new(password, &[]));
    // how many records went down after the client's first ApplicationData
    // record (its Finished, in TLS 1.3)
    let (after_finished, mut progress) = watch::channel(0usize);
    let finished = std::sync::atomic::AtomicBool::new(false);
    let stop = Notify::new();
    let mut from_client = RecordReader::default();
    let upwards = async {
        let (mut seen_handshake, mut seen_ccs) = (false, false);
        // the digest as it stood at each of the client's recent records: what
        // the client saw may be older than what has been relayed since
        let mut digests: VecDeque<[u8; V2_TAG]> = VecDeque::with_capacity(10);
        let found = loop {
            if !from_client.next(&mut client_r).await? {
                break None;
            }
            let record = from_client.record();
            seen_handshake |= record[0] == HANDSHAKE;
            seen_ccs |= record[0] == CHANGE_CIPHER_SPEC;
            if record[0] == APPLICATION_DATA
                && seen_handshake
                && seen_ccs
                && record.len() >= HEADER + V2_TAG
            {
                if digests.len() == 10 {
                    digests.pop_front();
                }
                digests.push_back(written.lock().expect("digest").digest::<V2_TAG>());
                let claimed = &record[HEADER..HEADER + V2_TAG];
                if digests.iter().any(|d| same(d, claimed)) {
                    let payload = record[HEADER + V2_TAG..].to_vec();
                    from_client.consume();
                    break Some(payload);
                }
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            camouflage_w.write_all(record).await?;
            from_client.consume();
        };
        if found.is_some() {
            // what the script wants the client to have met first
            let _ = progress.wait_for(|n| *n >= script.late_records).await;
        }
        stop.notify_one();
        io::Result::Ok(found)
    };
    let downwards = async {
        let mut from_camouflage = RecordReader::default();
        loop {
            tokio::select! {
                biased;
                _ = stop.notified() => return io::Result::Ok(()),
                more = from_camouflage.next(&mut camouflage_r) => {
                    if !more? {
                        return Ok(());
                    }
                }
            }
            let record = from_camouflage.record();
            client_w.write_all(record).await?;
            written.lock().expect("digest").update(record);
            if finished.load(std::sync::atomic::Ordering::SeqCst) {
                after_finished.send_modify(|n| *n += 1);
            }
            from_camouflage.consume();
        }
    };
    let (found, relayed) = tokio::join!(upwards, downwards);
    relayed?;
    let Some(first_payload) = found? else {
        note(sessions, false);
        return Ok(());
    };
    note(sessions, true);
    drop((camouflage_r, camouflage_w));
    let phase = DataPhase {
        client_r,
        client_w,
        from_client,
        up: Tagging::V2,
        down: Tagging::V2,
        first_payload,
    };
    data_phase(phase, script).await
}

/// One direction of the data phase: a v2 frame carries nothing but payload,
/// a v3 frame starts with a tag from this direction's chain.
enum Tagging {
    V2,
    V3(Chain),
}

impl Tagging {
    fn seal(&mut self, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + TAG + payload.len());
        match self {
            Tagging::V2 => out.extend_from_slice(&data_header(payload.len())),
            Tagging::V3(chain) => {
                out.extend_from_slice(&data_header(TAG + payload.len()));
                out.extend_from_slice(&chain.frame_tag(payload));
            }
        }
        out.extend_from_slice(payload);
        out
    }

    /// The payload of a client record, or `None` when it does not verify.
    fn open<'a>(&mut self, record: &'a [u8]) -> Option<&'a [u8]> {
        if record[0] != APPLICATION_DATA {
            return None;
        }
        match self {
            Tagging::V2 => Some(&record[HEADER..]),
            Tagging::V3(chain) => {
                let (tag, payload) = record[HEADER..].split_at_checked(TAG)?;
                same(&chain.frame_tag(payload), tag).then_some(payload)
            }
        }
    }
}

struct DataPhase {
    client_r: ReadHalf<TcpStream>,
    client_w: WriteHalf<TcpStream>,
    from_client: RecordReader,
    /// Verifies what the client sends.
    up: Tagging,
    /// Seals what goes to the client.
    down: Tagging,
    first_payload: Vec<u8>,
}

async fn data_phase(phase: DataPhase, script: &ShadowTlsScript) -> io::Result<()> {
    let DataPhase {
        mut client_r,
        mut client_w,
        mut from_client,
        mut up,
        mut down,
        first_payload,
    } = phase;
    if script.empty_record {
        client_w.write_all(&down.seal(&[])).await?;
    }
    match script.fault {
        Some(ShadowTlsFault::BadTag) => {
            let mut record = down.seal(b"not what the tag says");
            record[HEADER] ^= 0x01;
            client_w.write_all(&record).await?;
        }
        Some(ShadowTlsFault::CutRecord) => {
            let record = down.seal(&[7u8; 100]);
            client_w.write_all(&record[..40]).await?;
            client_w.shutdown().await?;
            // read the client out, so the close is a FIN and not a reset
            while from_client.next(&mut client_r).await? {
                from_client.consume();
            }
            return Ok(());
        }
        Some(ShadowTlsFault::StrayHandshake) => {
            client_w.write_all(&[HANDSHAKE, 3, 3, 0, 1, 0]).await?;
        }
        None => {}
    }
    let mut target = TcpStream::connect(script.connect_to).await?;
    target.write_all(&first_payload).await?;
    let (mut target_r, mut target_w) = target.split();
    // two loops that never wait for each other: a relay that writes from
    // inside one `select!` loop deadlocks as soon as both sides push back.
    // Only the alert makes them share the client's write half.
    let client_w = tokio::sync::Mutex::new(client_w);
    let upwards = async {
        while from_client.next(&mut client_r).await? {
            let Some(payload) = up.open(from_client.record()) else {
                return Err(io::Error::other("a client record does not verify"));
            };
            target_w.write_all(payload).await?;
            from_client.consume();
        }
        target_w.shutdown().await?;
        if script.alert_on_fin {
            let mut alert = vec![ALERT, 3, 3, 0, 26];
            alert.extend_from_slice(&[0x5a; 26]);
            client_w.lock().await.write_all(&alert).await?;
        }
        Ok(())
    };
    let downwards = async {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = target_r.read(&mut buf).await?;
            if n == 0 {
                return client_w.lock().await.shutdown().await;
            }
            let record = down.seal(&buf[..n]);
            client_w.lock().await.write_all(&record).await?;
        }
    };
    let (sent, received) = tokio::join!(upwards, downwards);
    sent.and(received)
}

/// The site a Shadow TLS server borrows its handshake from: completes TLS
/// handshakes (the fixture records them), keeps what clients say, and says
/// nothing itself.
pub struct Camouflage {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<u8>>>,
    _task: AbortOnDrop,
}

impl Camouflage {
    pub async fn spawn(
        fixture: &Arc<TlsFixture>,
        versions: &[&'static SupportedProtocolVersion],
        tickets: usize,
    ) -> Camouflage {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let acceptor = fixture.camouflage_acceptor(versions, tickets);
        let received: Arc<Mutex<Vec<u8>>> = Arc::default();
        let (fixture, heard) = (fixture.clone(), received.clone());
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (fixture, acceptor, heard) = (fixture.clone(), acceptor.clone(), heard.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 {
                            break;
                        }
                        heard.lock().expect("received").extend_from_slice(&buf[..n]);
                    }
                    let _ = stream.shutdown().await;
                });
            }
        });
        Camouflage {
            addr,
            received,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Everything clients sent inside their TLS sessions, in arrival order.
    pub fn received(&self) -> Vec<u8> {
        self.received.lock().expect("received").clone()
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
mod http_proxy;
mod socks5;
```

换成

```rust
mod http_proxy;
mod shadow_tls;
mod socks5;
```

并把

```rust
pub use http_proxy::{FakeHttpProxy, HttpProxyScript, RecordedHead};
```

换成

```rust
pub use http_proxy::{FakeHttpProxy, HttpProxyScript, RecordedHead};
pub use shadow_tls::{
    Camouflage, FakeShadowTls, RecordedShadowTls, ShadowTlsFault, ShadowTlsScript,
};
```

要点（评审时对照）：
- 假服务端的握手期是两个 `join!` 在一起的循环，停在**两条记录之间**（`Notify` + `biased` 的 `select!`）：一条记录绝不会被截断。数据阶段也是两个互不等待的循环（P17）。
- v2：记下"客户端每条 ApplicationData 记录到达时"的摘要（最近 10 个），首帧的 8 字节与其中任何一个相等即通过——参考服务端的做法；并要求此前见过客户端的 Handshake 与 ChangeCipherSpec 记录，这同时钉住了"rustls 会发那条兼容用的 CCS"。
- v3：首条记录验 ClientHello 的标签，不通过就转成普通转发（SNI 代理）——口令错误的客户端因此会和伪装站点完成一次真实握手。
- 被拒 / 出错时先关写端、再把连接读到头（`CutRecord`）：带着未读字节关连接会变成 RST（M2b 的教训）。

- [ ] **Step 3: 先写客户端的用例与向量，确认编译不过**

`crates/rurge-proto/src/transport/shadow_tls/vectors.rs`——把

```rust
use super::auth::{Chain, V2_TAG, xor_key};
use super::sign::hello_tag;
```

换成

```rust
use super::ServerSide;
use super::auth::{Chain, V2_TAG, xor_key};
use super::sign::hello_tag;
```

并在文件末尾追加：

```rust

const HANDSHAKE_PLAIN_0: &[&str] = &[
    "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
    "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f",
    "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f",
    "60616263",
];
const HANDSHAKE_WIRE_0: &[&str] = &[
    "1703030068312db371e2edcffde73f7bb76ead37d20555a1f3a13d0ed21feb5d",
    "7916f533f2b78e72b6c2cdefddc71f5b974e8d17f2257581d3811d2ef23fcb7d",
    "5936d513d297ae5296a2ad8fbda77f3bf72eed77924515e1b3e17d4e925fab1d",
    "3956b573b2f7ce32f6828daf9d",
];
const HANDSHAKE_PLAIN_1: &[&str] = &[
    "c8c9cacbcccdcecfd0d1d2d3d4d5d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7",
    "e8e9eaebecedeeeff0f1f2f3f4f5f6f7f8f9c8c9cacbcccdcecfd0d1d2d3d4d5",
    "d6d7d8d9dadbdcdddedfe0e1e2e3e4e5e6e7e8e9eaebecedeeeff0f1f2f3f4f5",
    "f6f7f8f9",
];
const HANDSHAKE_WIRE_1: &[&str] = &[
    "1703030068e517c5622a2507352ff7b37fb675ef0add8d792b69f5c61ad72395",
    "b1ee0dcb0a4f768a4e0a0527150fd7935f9655cf2afdad590b49d5d408c13587",
    "a3c023f9387940b87c343b152739e1a16db87bdd38ebbb4b1957cbf428e115a7",
    "83e003d9185960985c141b3507",
];

#[test]
fn handshake_records_verify_in_order_and_come_back_as_the_site_wrote_them() {
    let mut side = ServerSide::new(PASSWORD, &server_random());
    for (wire, plain) in [
        (HANDSHAKE_WIRE_0, HANDSHAKE_PLAIN_0),
        (HANDSHAKE_WIRE_1, HANDSHAKE_PLAIN_1),
    ] {
        let mut record = unhex(wire);
        assert!(side.restore(&mut record));
        let plain = unhex(plain);
        assert_eq!(record[..3], [23, 3, 3]);
        assert_eq!(
            usize::from(u16::from_be_bytes([record[3], record[4]])),
            plain.len()
        );
        assert_eq!(record[5..], plain[..]);
    }
    // the chain ran on: the first record does not verify a second time
    let mut again = unhex(HANDSHAKE_WIRE_0);
    let before = again.clone();
    assert!(!side.restore(&mut again));
    assert_eq!(
        again, before,
        "a record that is not ours is left as it came"
    );
}

#[test]
fn a_flipped_bit_anywhere_in_a_handshake_record_is_noticed() {
    for at in [5usize, 8, 9, 60, 108] {
        let mut side = ServerSide::new(PASSWORD, &server_random());
        let mut record = unhex(HANDSHAKE_WIRE_0);
        record[at] ^= 0x10;
        assert!(!side.restore(&mut record), "byte {at}");
    }
}
```

Run: `cargo test -p rurge-proto --lib shadow_tls`

Expected: 编译错误——`unresolved import `super::ServerSide``。

- [ ] **Step 4: 客户端**

`crates/rurge-proto/src/transport/shadow_tls/mod.rs` 整份替换为（用例在文件末尾的 `mod tests` 里）：

```rust
//! Shadow TLS, client side (M2 design 5.3): a real TLS handshake with a
//! camouflage site, relayed by the server, and then frames that look like
//! the ApplicationData of that session. The layer sits between the
//! connector and the policy's own TLS.
//!
//! The handshake is driven by hand (`read_tls` / `process_new_packets` /
//! `write_tls`), one record at a time: v2 digests every byte the server sends,
//! v3 has to verify and rewrite records before rustls may see them.
//!
//! The camouflage handshake verifies the certificate like any TLS client
//! would, and none of the policy's TLS parameters applies to it: a client
//! that does not mind a bad certificate is a tell.

pub(crate) mod auth;
mod framed;
pub(crate) mod record;
pub(crate) mod sign;
#[cfg(test)]
mod vectors;

use crate::outbound::untrusted_text;
use crate::{BuildError, OutboundError};
use auth::{Chain, TAG, V2_TAG, same, xor, xor_key};
use framed::{Framed, Mode};
use record::{APPLICATION_DATA, HANDSHAKE, HEADER, RecordReader};
use rurge_config::HostName;
use rurge_config::spec::{ShadowTlsOpts, ShadowTlsVersion};
use rurge_net::connector::BoxedStream;
use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};
use std::io::{self, Read, Write};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const SERVER_HELLO: u8 = 2;
/// A ServerHello record: its random sits behind the record header, the
/// handshake header (4) and the version (2); the session id follows.
const SERVER_RANDOM_AT: usize = HEADER + 4 + 2;
const SESSION_ID_LEN_AT: usize = SERVER_RANDOM_AT + 32;
const SUPPORTED_VERSIONS: u16 = 43;
const TLS13: u16 = 0x0304;
/// How long the good-bye to a server that is not ours may take.
const FAREWELL: Duration = Duration::from_secs(2);

fn proxy(text: &str) -> OutboundError {
    OutboundError::Proxy(text.to_string())
}

/// Built once per outbound, used for every connection. No `Debug`: it holds
/// the password.
pub struct ShadowTlsClient {
    version: ShadowTlsVersion,
    password: Vec<u8>,
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
}

impl ShadowTlsClient {
    /// `fallback` is the name the certificate is checked against when the
    /// policy has no `shadow-tls-sni`; no SNI is sent then (manual).
    pub fn build(
        opts: &ShadowTlsOpts,
        fallback: &HostName,
        roots: Arc<RootCertStore>,
    ) -> Result<ShadowTlsClient, BuildError> {
        let provider = match opts.version {
            ShadowTlsVersion::V2 => Arc::new(rustls::crypto::ring::default_provider()),
            ShadowTlsVersion::V3 => sign::provider(),
        };
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| BuildError::new(format!("cannot set up the Shadow TLS handshake: {e}")))?
            .with_root_certificates(roots)
            .with_no_client_auth();
        // a resumed session would change what the ClientHello draws (appendix A)
        config.resumption = rustls::client::Resumption::disabled();
        config.enable_sni = opts.sni.is_some();
        let name = match (&opts.sni, fallback) {
            (Some(name), _) => name.clone(),
            (None, HostName::Domain(domain)) => domain.clone(),
            (None, HostName::Ip(ip)) => ip.to_string(),
        };
        let name = ServerName::try_from(name)
            .map_err(|_| BuildError::new("the Shadow TLS handshake has no valid server name"))?;
        let client = ShadowTlsClient {
            version: opts.version,
            password: opts.password.expose().as_bytes().to_vec(),
            config: Arc::new(config),
            name,
        };
        // the four assumptions of appendix A, once, before anything connects
        if client.version == ShadowTlsVersion::V3
            && sign::signed_hello(&client.config, &client.name, &client.password).is_none()
        {
            return Err(BuildError::new("shadow-tls: cannot sign the ClientHello"));
        }
        Ok(client)
    }

    /// The camouflage handshake over `stream`, and then the framed stream.
    /// No timeout of its own, but for the good-bye to a server that turned
    /// out not to be ours.
    pub async fn wrap(&self, stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        match self.version {
            ShadowTlsVersion::V2 => self.wrap_v2(stream).await,
            ShadowTlsVersion::V3 => self.wrap_v3(stream).await,
        }
    }

    async fn wrap_v2(&self, mut stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        let conn = ClientConnection::new(self.config.clone(), self.name.clone())
            .map_err(handshake_failed)?;
        let mut shake = Handshake::new(conn);
        let mut digest = Chain::new(&self.password, &[]);
        while shake.step(&mut stream).await? {
            // every byte the server sends while the handshake lasts
            digest.update(shake.reader.record());
            shake.feed(&mut stream).await?;
        }
        let mode = Mode::V2 {
            first: Some(digest.digest::<V2_TAG>()),
            session: Some(Box::new(shake.conn)),
        };
        Ok(Box::new(Framed::new(stream, shake.reader, mode)))
    }

    async fn wrap_v3(&self, mut stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        let Some((conn, hello)) = sign::signed_hello(&self.config, &self.name, &self.password)
        else {
            return Err(proxy("shadow-tls: cannot sign the ClientHello"));
        };
        stream.write_all(&hello).await?;
        let mut shake = Handshake::new(conn);
        let mut server: Option<ServerSide> = None;
        let mut tls13 = false;
        // every ApplicationData record of the handshake carried a good tag
        let mut verified = 0usize;
        let mut genuine = true;
        while shake.step(&mut stream).await? {
            let record = shake.reader.record();
            match record[0] {
                HANDSHAKE if server.is_none() => {
                    if let Some(random) = server_random(record) {
                        tls13 = is_tls13(record);
                        server = Some(ServerSide::new(&self.password, &random));
                    }
                }
                APPLICATION_DATA if genuine => {
                    if server.as_mut().is_some_and(|side| side.restore(record)) {
                        verified += 1;
                    } else {
                        // not ours: let the handshake run its course untouched
                        genuine = false;
                    }
                }
                _ => {}
            }
            shake.feed(&mut stream).await?;
        }
        let refusal = if !tls13 {
            Some("shadow-tls: the handshake server does not support TLS 1.3")
        } else if !genuine || verified == 0 {
            Some("shadow-tls: the server did not authenticate itself")
        } else {
            None
        };
        if let Some(text) = refusal {
            // what we reached is the camouflage site itself, or someone in
            // between: behave like a client that wanted a page (the
            // reference client's way out), then leave
            let _ = tokio::time::timeout(FAREWELL, shake.farewell(&mut stream, &self.name)).await;
            return Err(proxy(text));
        }
        let side = server.expect("a verified record implies a ServerHello");
        let mode = Mode::V3 {
            add: Chain::new(&self.password, &[&side.random, b"C"]),
            verify: Chain::new(&self.password, &[&side.random, b"S"]),
            ignore: Some(side.chain),
        };
        Ok(Box::new(Framed::new(stream, shake.reader, mode)))
    }
}

/// What v3 learns from the ServerHello.
struct ServerSide {
    random: [u8; 32],
    chain: Chain,
    key: [u8; 32],
}

impl ServerSide {
    fn new(password: &[u8], random: &[u8; 32]) -> ServerSide {
        ServerSide {
            random: *random,
            chain: Chain::new(password, &[random]),
            key: xor_key(password, random),
        }
    }

    /// An ApplicationData record of the handshake as a v3 server sends it:
    /// `<4-byte tag><payload XOR key>`. Verified, it becomes the record the
    /// handshake server wrote. `false`: not ours, and left as it came.
    fn restore(&mut self, record: &mut Vec<u8>) -> bool {
        if record.len() <= HEADER + TAG {
            return false;
        }
        self.chain.update(&record[HEADER + TAG..]);
        if !same(&self.chain.digest::<TAG>(), &record[HEADER..HEADER + TAG]) {
            return false;
        }
        record.drain(HEADER..HEADER + TAG);
        xor(&mut record[HEADER..], &self.key);
        let len = u16::try_from(record.len() - HEADER).expect("it was read as one record");
        record[3..HEADER].copy_from_slice(&len.to_be_bytes());
        true
    }
}

/// The random of a record that starts with a ServerHello.
fn server_random(record: &[u8]) -> Option<[u8; 32]> {
    if record.len() <= SESSION_ID_LEN_AT || record[HEADER] != SERVER_HELLO {
        return None;
    }
    record[SERVER_RANDOM_AT..SESSION_ID_LEN_AT].try_into().ok()
}

/// Whether the ServerHello in `record` selects TLS 1.3 (`supported_versions`).
fn is_tls13(record: &[u8]) -> bool {
    fn take<'a>(rest: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
        let (head, tail) = rest.split_at_checked(n)?;
        *rest = tail;
        Some(head)
    }
    fn u16_of(bytes: &[u8]) -> usize {
        usize::from(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
    let scan = || {
        let mut rest = record.get(SESSION_ID_LEN_AT..)?;
        let session_id = usize::from(take(&mut rest, 1)?[0]);
        // the session id, the cipher suite (2) and the compression method (1)
        take(&mut rest, session_id + 3)?;
        let extensions = u16_of(take(&mut rest, 2)?);
        let mut rest = rest.get(..extensions)?;
        while !rest.is_empty() {
            let kind = u16_of(take(&mut rest, 2)?);
            let len = u16_of(take(&mut rest, 2)?);
            let body = take(&mut rest, len)?;
            if kind == usize::from(SUPPORTED_VERSIONS) {
                return Some(body.len() == 2 && u16_of(body) == usize::from(TLS13));
            }
        }
        Some(false)
    };
    scan().unwrap_or(false)
}

fn handshake_failed(error: rustls::Error) -> OutboundError {
    // the text may quote names the server presented
    OutboundError::Proxy(format!(
        "shadow-tls: the camouflage handshake failed: {}",
        untrusted_text(&error.to_string(), 200)
    ))
}

/// A rustls client connection driven one record at a time.
struct Handshake {
    conn: ClientConnection,
    reader: RecordReader,
}

impl Handshake {
    fn new(conn: ClientConnection) -> Handshake {
        Handshake {
            conn,
            reader: RecordReader::default(),
        }
    }

    async fn send(&mut self, stream: &mut BoxedStream) -> io::Result<()> {
        let mut out = Vec::new();
        while self.conn.wants_write() {
            self.conn.write_tls(&mut out)?;
        }
        if !out.is_empty() {
            stream.write_all(&out).await?;
            stream.flush().await?;
        }
        Ok(())
    }

    /// Sends what rustls wants sent; then, while the handshake lasts, reads
    /// the next record into `self.reader`. `false`: the handshake is done.
    async fn step(&mut self, stream: &mut BoxedStream) -> Result<bool, OutboundError> {
        self.reader.consume();
        self.send(stream).await?;
        if !self.conn.is_handshaking() {
            return Ok(false);
        }
        if !self.reader.next(stream).await? {
            return Err(proxy(
                "shadow-tls: the server closed the connection during the handshake",
            ));
        }
        Ok(true)
    }

    /// Hands the reader's record (as the caller left it) to rustls.
    async fn feed(&mut self, stream: &mut BoxedStream) -> Result<(), OutboundError> {
        let mut rest = &self.reader.record()[..];
        let mut outcome = Ok(());
        while outcome.is_ok() && !rest.is_empty() {
            outcome = match self.conn.read_tls(&mut rest) {
                Ok(_) => self
                    .conn
                    .process_new_packets()
                    .map(drop)
                    .map_err(handshake_failed),
                Err(e) => Err(e.into()),
            };
        }
        if outcome.is_err() {
            // the alert rustls queued: a TLS client says why it leaves
            let _ = self.send(stream).await;
        }
        outcome
    }

    /// One plausible request over the finished session, then whatever comes
    /// back until the other side is done. Nothing of it is kept.
    async fn farewell(&mut self, stream: &mut BoxedStream, name: &ServerName<'static>) {
        let mut pad = [0u8; 48];
        let _ = getrandom::fill(&mut pad);
        // 16 to 47 characters: the request has no constant length
        let session: String = pad[1..17 + usize::from(pad[0] % 32)]
            .iter()
            .map(|b| char::from(b"abcdefghijklmnopqrstuvwxyz0123456789"[usize::from(b % 36)]))
            .collect();
        let host = match name {
            ServerName::DnsName(dns) => dns.as_ref().to_string(),
            ServerName::IpAddress(ip) => std::net::IpAddr::from(*ip).to_string(),
            _ => String::new(),
        };
        let request = format!(
            "GET / HTTP/1.1\r\nHost: {host}\r\nUser-Agent: curl/8.5.0\r\nAccept: */*\r\n\
             Cookie: sessionid={session}\r\nConnection: close\r\n\r\n"
        );
        if self.conn.writer().write_all(request.as_bytes()).is_err() {
            return;
        }
        self.conn.send_close_notify();
        if self.send(stream).await.is_err() {
            return;
        }
        loop {
            self.reader.consume();
            if !matches!(self.reader.next(stream).await, Ok(true)) {
                return;
            }
            let mut rest = &self.reader.record()[..];
            while !rest.is_empty() {
                if self.conn.read_tls(&mut rest).is_err() {
                    return;
                }
                let Ok(state) = self.conn.process_new_packets() else {
                    return;
                };
                let mut sink = vec![0; state.plaintext_bytes_to_read()];
                let _ = self.conn.reader().read(&mut sink);
                if state.peer_has_closed() {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        Camouflage, FakeShadowTls, ShadowTlsFault, ShadowTlsScript, TlsFixture, echo_server,
    };
    use rurge_config::spec::Secret;
    use rustls::version::{TLS12, TLS13};
    use std::net::SocketAddr;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
    use tokio::net::{TcpListener, TcpStream};

    const SITE: &str = "camouflage.test";
    const BOUND: Duration = Duration::from_secs(30);

    fn opts(version: ShadowTlsVersion, password: &str, sni: Option<&str>) -> ShadowTlsOpts {
        ShadowTlsOpts {
            password: Secret::from(password),
            sni: sni.map(str::to_string),
            version,
        }
    }

    fn client(version: ShadowTlsVersion, password: &str, fixture: &TlsFixture) -> ShadowTlsClient {
        let fallback = HostName::parse("127.0.0.1");
        ShadowTlsClient::build(
            &opts(version, password, Some(SITE)),
            &fallback,
            fixture.roots(),
        )
        .expect("the client builds")
    }

    async fn open(
        client: &ShadowTlsClient,
        server: SocketAddr,
    ) -> Result<BoxedStream, OutboundError> {
        let tcp = TcpStream::connect(server).await?;
        client.wrap(Box::new(tcp)).await
    }

    /// One direction of the engine's relay: read, `write_all`, `flush`, and
    /// `shutdown` when the source ends.
    async fn copy_half<R, W>(from: &mut R, to: &mut W) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = from.read(&mut buf).await?;
            if n == 0 {
                return to.shutdown().await;
            }
            to.write_all(&buf[..n]).await?;
            to.flush().await?;
        }
    }

    /// Puts `outbound` behind a loopback socket the way the engine does:
    /// both directions polled from ONE task, over `tokio::io::split` halves.
    async fn behind_a_relay(outbound: BoxedStream) -> TcpStream {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (inbound, _) = listener.accept().await.unwrap();
            let (mut in_r, mut in_w) = tokio::io::split(inbound);
            let (mut out_r, mut out_w) = tokio::io::split(outbound);
            let _ = tokio::join!(
                copy_half(&mut in_r, &mut out_w),
                copy_half(&mut out_r, &mut in_w)
            );
        });
        TcpStream::connect(addr).await.unwrap()
    }

    /// Sends `len` bytes while reading them back, then half-closes and
    /// expects the end of the stream.
    async fn echo_round_trip(stream: TcpStream, len: usize) {
        let data: Vec<u8> = (0..len).map(|i| (i * 31 % 251) as u8).collect();
        let (mut r, mut w) = stream.into_split();
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w
        });
        let mut back = vec![0u8; len];
        r.read_exact(&mut back).await.unwrap();
        assert!(back == data, "the echo differs");
        let mut w = writer.await.unwrap();
        w.shutdown().await.unwrap();
        let mut rest = Vec::new();
        r.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    /// The fake's record of its `n`th session, waited for within a bound.
    async fn session(fake: &FakeShadowTls, n: usize) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while fake.sessions().len() <= n {
            assert!(tokio::time::Instant::now() < deadline, "no session {n}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        fake.sessions()[n].authenticated
    }

    struct World {
        fixture: Arc<TlsFixture>,
        camouflage: Camouflage,
        fake: FakeShadowTls,
    }

    /// A camouflage site, an echo target and a fake server in front of both.
    async fn world(
        version: ShadowTlsVersion,
        versions: &[&'static rustls::SupportedProtocolVersion],
        tickets: usize,
        tune: impl FnOnce(&mut ShadowTlsScript),
    ) -> World {
        let fixture = TlsFixture::new(&[SITE]);
        let camouflage = Camouflage::spawn(&fixture, versions, tickets).await;
        let mut script =
            ShadowTlsScript::new(version, "right", camouflage.addr(), echo_server().await);
        tune(&mut script);
        let fake = FakeShadowTls::spawn(script).await;
        World {
            fixture,
            camouflage,
            fake,
        }
    }

    #[tokio::test]
    async fn v3_carries_a_megabyte_both_ways_in_the_relay_s_shape() {
        tokio::time::timeout(BOUND, async {
            // two real session tickets may still be on their way when the
            // data phase begins, and three fabricated records certainly are
            let w = world(ShadowTlsVersion::V3, &[&TLS13], 2, |s| s.residual = 3).await;
            let client = client(ShadowTlsVersion::V3, "right", &w.fixture);
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 1 << 20).await;
            let seen = w.fixture.seen_at_least(1).await;
            assert_eq!(seen[0].sni.as_deref(), Some(SITE));
            assert_eq!(seen[0].alpn, None);
            assert!(session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v2_carries_a_megabyte_both_ways_and_swallows_the_session_tickets() {
        tokio::time::timeout(BOUND, async {
            // the fake holds the data phase back until the record with the
            // site's two session tickets went down
            let w = world(ShadowTlsVersion::V2, &[&TLS13], 2, |s| s.late_records = 1).await;
            let client = client(ShadowTlsVersion::V2, "right", &w.fixture);
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 1 << 20).await;
            assert!(session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v2_works_over_a_tls_1_2_handshake_too() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V2, &[&TLS12], 0, |_| {}).await;
            let client = client(ShadowTlsVersion::V2, "right", &w.fixture);
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 100_000).await;
            assert!(session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn without_shadow_tls_sni_no_sni_is_sent_and_the_fallback_name_is_verified() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V2, &[&TLS13], 0, |_| {}).await;
            let fallback = HostName::parse(SITE);
            let client = ShadowTlsClient::build(
                &opts(ShadowTlsVersion::V2, "right", None),
                &fallback,
                w.fixture.roots(),
            )
            .unwrap();
            let stream = open(&client, w.fake.addr()).await.unwrap();
            echo_round_trip(behind_a_relay(stream).await, 1000).await;
            assert_eq!(w.fixture.seen_at_least(1).await[0].sni, None);
            // and a name the certificate does not cover is refused
            let other = HostName::parse("elsewhere.test");
            let client = ShadowTlsClient::build(
                &opts(ShadowTlsVersion::V2, "right", None),
                &other,
                w.fixture.roots(),
            )
            .unwrap();
            let err = open(&client, w.fake.addr()).await.err().expect("refused");
            assert!(
                err.to_string().starts_with(
                    "shadow-tls: the camouflage handshake failed: invalid peer certificate"
                ),
                "{err}"
            );
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn the_policy_s_own_tls_runs_inside_the_frames() {
        tokio::time::timeout(BOUND, async {
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let fixture = TlsFixture::new(&[SITE, "proxy.test"]);
                let camouflage = Camouflage::spawn(&fixture, &[&TLS13], 2).await;
                // behind the Shadow TLS server: a TLS echo server
                let target = fixture.spawn_echo(false).await;
                let fake = FakeShadowTls::spawn(ShadowTlsScript::new(
                    version,
                    "right",
                    camouflage.addr(),
                    target,
                ))
                .await;
                let client = client(version, "right", &fixture);
                let framed = open(&client, fake.addr()).await.unwrap();
                let config = ClientConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(fixture.roots())
                .with_no_client_auth();
                let inner = tokio_rustls::TlsConnector::from(Arc::new(config))
                    .connect(ServerName::try_from("proxy.test").unwrap(), framed)
                    .await
                    .expect("the inner handshake");
                echo_round_trip(behind_a_relay(Box::new(inner)).await, 300_000).await;
                // the site's handshake, then the proxy's own
                let seen = fixture.seen_at_least(2).await;
                assert_eq!(seen[0].sni.as_deref(), Some(SITE));
                assert_eq!(seen[1].sni.as_deref(), Some("proxy.test"));
            }
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v3_says_good_bye_like_a_browser_when_the_server_is_not_ours() {
        tokio::time::timeout(BOUND, async {
            // a wrong password: the server relays the site untouched
            let w = world(ShadowTlsVersion::V3, &[&TLS13], 2, |_| {}).await;
            let client = client(ShadowTlsVersion::V3, "wrong", &w.fixture);
            let err = open(&client, w.fake.addr()).await.err().expect("refused");
            assert_eq!(
                err.to_string(),
                "shadow-tls: the server did not authenticate itself"
            );
            let heard = String::from_utf8(w.camouflage.received()).unwrap();
            assert!(
                heard.starts_with("GET / HTTP/1.1\r\nHost: camouflage.test\r\n")
                    && heard.ends_with("Connection: close\r\n\r\n"),
                "{heard}"
            );
            assert!(!session(&w.fake, 0).await);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v3_needs_a_handshake_server_that_speaks_tls_1_3() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V3, &[&TLS12], 0, |_| {}).await;
            let client = client(ShadowTlsVersion::V3, "right", &w.fixture);
            let err = open(&client, w.fake.addr()).await.err().expect("refused");
            assert_eq!(
                err.to_string(),
                "shadow-tls: the handshake server does not support TLS 1.3"
            );
            // the good-bye went through the TLS 1.2 session all the same
            let heard = w.camouflage.received();
            assert!(heard.starts_with(b"GET / HTTP/1.1\r\n"), "{heard:?}");
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn v2_with_a_wrong_password_ends_when_the_site_ends_the_session() {
        tokio::time::timeout(BOUND, async {
            let w = world(ShadowTlsVersion::V2, &[&TLS13], 2, |_| {}).await;
            let client = client(ShadowTlsVersion::V2, "wrong", &w.fixture);
            // the handshake itself cannot tell
            let mut stream = open(&client, w.fake.addr()).await.unwrap();
            stream.write_all(b"this goes to the site").await.unwrap();
            let mut buf = [0u8; 16];
            let err = stream.read(&mut buf).await.expect_err("the site hangs up");
            assert_eq!(
                err.to_string(),
                "shadow-tls: the handshake server closed the session"
            );
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn a_site_with_an_unknown_certificate_is_refused() {
        tokio::time::timeout(BOUND, async {
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let w = world(version, &[&TLS13], 0, |_| {}).await;
                let stranger = TlsFixture::new(&[SITE]);
                let client = client(version, "right", &stranger);
                let err = open(&client, w.fake.addr()).await.err().expect("refused");
                // which way the chain fails is webpki's business
                assert!(
                    err.to_string().starts_with(
                        "shadow-tls: the camouflage handshake failed: invalid peer certificate: "
                    ),
                    "{err}"
                );
            }
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn a_server_that_hangs_up_during_the_handshake_is_reported_as_such() {
        tokio::time::timeout(BOUND, async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                while let Ok((mut tcp, _)) = listener.accept().await {
                    // take the ClientHello, then a FIN and not a reset
                    let mut sink = [0u8; 4096];
                    let _ = tcp.read(&mut sink).await;
                    let _ = tcp.shutdown().await;
                    while matches!(tcp.read(&mut sink).await, Ok(n) if n > 0) {}
                }
            });
            let fixture = TlsFixture::new(&[SITE]);
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let client = client(version, "right", &fixture);
                let err = open(&client, addr).await.err().expect("refused");
                assert_eq!(
                    err.to_string(),
                    "shadow-tls: the server closed the connection during the handshake"
                );
            }
        })
        .await
        .expect("bounded");
    }

    async fn first_read_error(version: ShadowTlsVersion, fault: ShadowTlsFault) -> io::Error {
        let w = world(version, &[&TLS13], 0, |s| s.fault = Some(fault)).await;
        let client = client(version, "right", &w.fixture);
        let mut stream = open(&client, w.fake.addr()).await.unwrap();
        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 64];
        loop {
            match stream.read(&mut buf).await {
                Ok(0) => panic!("the stream ended without an error"),
                Ok(_) => {}
                Err(e) => return e,
            }
        }
    }

    #[tokio::test]
    async fn what_a_broken_data_phase_looks_like() {
        tokio::time::timeout(BOUND, async {
            let e = first_read_error(ShadowTlsVersion::V3, ShadowTlsFault::BadTag).await;
            assert_eq!(e.kind(), io::ErrorKind::InvalidData);
            assert_eq!(
                e.to_string(),
                "shadow-tls: a record cannot be authenticated"
            );
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let e = first_read_error(version, ShadowTlsFault::CutRecord).await;
                assert_eq!(e.kind(), io::ErrorKind::UnexpectedEof);
                assert_eq!(
                    e.to_string(),
                    "shadow-tls: the connection ended in the middle of a record"
                );
                let e = first_read_error(version, ShadowTlsFault::StrayHandshake).await;
                assert_eq!(e.to_string(), "shadow-tls: unexpected record type");
            }
        })
        .await
        .expect("bounded");
    }

    /// Answers only once the client has finished talking.
    async fn answers_after_the_fin() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut said = Vec::new();
                    tcp.read_to_end(&mut said).await.unwrap();
                    said.reverse();
                    tcp.write_all(&said).await.unwrap();
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn an_alert_after_our_fin_and_an_empty_record_do_not_end_the_stream() {
        tokio::time::timeout(BOUND, async {
            for version in [ShadowTlsVersion::V2, ShadowTlsVersion::V3] {
                let fixture = TlsFixture::new(&[SITE]);
                let camouflage = Camouflage::spawn(&fixture, &[&TLS13], 0).await;
                let mut script = ShadowTlsScript::new(
                    version,
                    "right",
                    camouflage.addr(),
                    answers_after_the_fin().await,
                );
                script.alert_on_fin = true;
                script.empty_record = true;
                let fake = FakeShadowTls::spawn(script).await;
                let client = client(version, "right", &fixture);
                let mut stream = open(&client, fake.addr()).await.unwrap();
                stream.write_all(b"abcdef").await.unwrap();
                // sing-box answers this FIN with an alert record, and the
                // answer of the target comes after it
                stream.shutdown().await.unwrap();
                let mut answer = Vec::new();
                stream.read_to_end(&mut answer).await.unwrap();
                assert_eq!(answer, b"fedcba");
            }
        })
        .await
        .expect("bounded");
    }

    #[test]
    fn a_name_rustls_cannot_use_is_a_build_error_and_v3_checks_itself_at_build_time() {
        let roots = Arc::new(RootCertStore::empty());
        let fallback = HostName::parse("127.0.0.1");
        let bad = opts(ShadowTlsVersion::V2, "pw", Some("not a name"));
        let err = ShadowTlsClient::build(&bad, &fallback, roots.clone())
            .err()
            .expect("refused");
        assert_eq!(
            err.message,
            "the Shadow TLS handshake has no valid server name"
        );
        // v3: the two-pass ClientHello is rehearsed once
        let good = opts(ShadowTlsVersion::V3, "pw", Some(SITE));
        assert!(ShadowTlsClient::build(&good, &fallback, roots).is_ok());
    }

    #[test]
    fn the_server_hello_parsers_take_only_what_is_there() {
        // type, version, length, ServerHello, its length, version, random …
        let mut record = vec![22, 3, 3, 0, 0, 2, 0, 0, 0, 3, 3];
        record.extend(1..=32u8);
        assert_eq!(server_random(&record), None, "no session id length yet");
        // … no session id …
        record.push(0);
        assert_eq!(server_random(&record).unwrap()[..3], [1, 2, 3]);
        assert!(!is_tls13(&record), "cut short");
        // … a cipher suite, no compression, and two extensions
        record.extend([0x13, 0x01, 0]);
        record.extend([0, 10, 0, 51, 0, 0, 0, 43, 0, 2, 3, 4]);
        assert!(is_tls13(&record));
        let at = record.len() - 1;
        record[at] = 3;
        assert!(!is_tls13(&record), "supported_versions says TLS 1.2");
        record.truncate(at);
        assert!(!is_tls13(&record), "an extension longer than the record");
        let mut hello_retry = record.clone();
        hello_retry[5] = 1;
        assert_eq!(server_random(&hello_retry), None, "not a ServerHello");
    }
}
```

要点（评审时对照）：
- `Handshake::step` 每次先把 rustls 想发的字节发出去（含握手完成后那一笔 Finished），再读**一条**记录；`feed` 把（可能已被 v3 改写过的）记录交给 rustls，失败时先把 rustls 排队的 alert 发出去。
- v2：摘要喂入每条记录的全部字节；握手完成后 `ClientConnection` 不丢，进 `Mode::V2.session`（P2）。
- v3：`ServerSide::restore` 先对线上负载验证标签，通过才异或还原、去掉 4 字节、改回长度；一旦有一条 ApplicationData 验不过，此后的记录原样交给 rustls（让真实握手走完），最后体面收尾再失败。TLS 1.3 的判断读 ServerHello 的 `supported_versions`。
- 没有任何路径会把口令、标签或密钥放进错误文本；rustls 的错误文本经 `untrusted_text` 截到 200 字符。

- [ ] **Step 5: 运行**

Run: `cargo test -p rurge-proto --lib shadow_tls`

Expected: 33 passed（Task 2 / 3 的 17 条 + 2 条向量 + `tests` 里的 14 条）。

再连跑若干轮确认没有偶发失败（写计划时连跑过 550 轮）：

```bash
for i in $(seq 1 30); do cargo test -p rurge-proto --lib shadow_tls 2>&1 | grep -q "33 passed" || echo "round $i failed"; done
```

- [ ] **Step 6: 亲手取一次 RED**

写计划时对下面 9 处故意改坏的实现逐一确认了相应用例变红（基线为绿）：

| 改坏的地方 | 变红的用例 |
| ---------- | ---------- |
| v2：收到的记录从不先交给伪装会话 | `v2_carries_a_megabyte_both_ways_and_swallows_the_session_tickets` |
| v3：握手链下的残留记录不跳过 | `v3_carries_a_megabyte_both_ways_in_the_relay_s_shape` |
| alert 记录当作错误 | `an_alert_after_our_fin_and_an_empty_record_do_not_end_the_stream` |
| 空负载的帧当作流的结束 | 同上 |
| 总是发 SNI | `without_shadow_tls_sni_no_sni_is_sent_and_the_fallback_name_is_verified` |
| 不做体面收尾 | `v3_says_good_bye_like_a_browser_when_the_server_is_not_ours` |
| v3：验不过的记录也算通过 | 同上 |
| v2：摘要不含记录头 | `v2_works_over_a_tls_1_2_handshake_too` |
| v3：标签不回喂 | `vectors::data_tags_chain_per_direction_and_feed_themselves_back` |

执行者亲手复现其中一处：把 `framed.rs` 里的 `(ALERT, _) => Ok(None),` 临时改成 `(ALERT, _) => Err(invalid("shadow-tls: unexpected record type")),`，运行 `cargo test -p rurge-proto --lib an_alert_after_our_fin` 确认失败，**然后改回去**，把失败输出的最后几行记进任务报告。

- [ ] **Step 7: 三种构建形态**

```bash
cargo clippy -p rurge-proto -- -D warnings
cargo clippy -p rurge-proto --features testing -- -D warnings
cargo clippy -p rurge-proto --all-targets -- -D warnings
```

三条都必须干净：`mod.rs` 里已经没有任何 `#[allow(dead_code)]`。

- [ ] **Step 8: 门禁与提交**

```bash
git add crates/rurge-proto/src/transport/shadow_tls crates/rurge-proto/src/testing
git commit -m "feat(proto): Shadow TLS 客户端——自己驱动的伪装握手（v2 摘要、v3 逐记录验证与还原）、体面收尾；回环假服务端 FakeShadowTls 与伪装站点 Camouflage；TLS echo 夹具写完即 flush"
```

---

### Task 5: 接线——`Stack` 的 shadow-tls 一层、各出站、`http` / `socks5` 迁到 `Stack`、配置层生效

这是"死路径变活"的那个提交（P12）：`to_spec` 开始读三个参数、`PolicySpec` 带上 `shadow_tls`、`W0029` 对它们退役，**同时**每一种 TCP 出站真的把这一层接上。三个 crate 一起改，一个提交。

**Files:**
- Modify: `crates/rurge-config/src/spec/mod.rs`、`crates/rurge-config/src/spec/tls.rs`
- Modify: `crates/rurge-proto/src/transport/stack.rs`、`crates/rurge-proto/src/build.rs`
- Modify: `crates/rurge-proto/src/trojan.rs`、`crates/rurge-proto/src/vmess/mod.rs`、`crates/rurge-proto/src/anytls/mod.rs`
- Modify: `crates/rurge-proto/src/http.rs`、`crates/rurge-proto/src/socks5.rs`
- Modify: `crates/rurge-engine/src/outbounds.rs`

**Interfaces:**
- Consumes: Task 1 的 `read_shadow_tls` / `ShadowTlsOpts`；Task 4 的 `ShadowTlsClient` 与 `rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript}`、`TlsFixture::seen_at_least`。
- Produces:
  - `rurge_config::spec::PolicySpec.shadow_tls: Option<ShadowTlsOpts>`（新字段，位于 `proto` 与 `span` 之间；`rurge-policy` 的指纹克隆整个 spec，所以它自动进入指纹）
  - `rurge_proto::transport::Stack::new(connector, server, shadow_tls: Option<ShadowTlsClient>, tls: Option<TlsClient>, ws: Option<WsClient>) -> Stack`
  - `rurge_proto::build::shadow_tls_client(opts: Option<&ShadowTlsOpts>, tls: Option<&TlsOpts>, server: &HostName, roots: Arc<RootCertStore>) -> Result<Option<ShadowTlsClient>, BuildError>`
  - `TrojanOutbound::new(name, server, spec, shadow_tls: Option<&ShadowTlsOpts>, keystore, roots, connector)`；`VmessOutbound::new`、`AnyTlsOutbound::new` 同样在 `spec` 之后多这一个参数
  - `HttpOutbound::from_spec` / `Socks5Outbound::from_spec` 的签名不变（读 `spec.shadow_tls`）

- [ ] **Step 1: 配置层——先改用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            "socks5, h, 1080, udp-relay=true, shadow-tls-password=pw, mystery=1",
        );
        assert_eq!(o.inert, ["udp-relay", "shadow-tls-password"]);
```

换成

```rust
            "socks5, h, 1080, udp-relay=true, shadow-tls-password=pw, mystery=1",
        );
        // Shadow TLS took effect in M2c: no longer on the list
        assert_eq!(o.inert, ["udp-relay"]);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    #[test]
    fn socket_options_under_a_chain_are_reported() {
```

换成

```rust
    #[test]
    fn shadow_tls_is_part_of_the_spec_of_every_tcp_protocol() {
        for def in [
            "http, h.test, 80",
            "https, h.test, 443",
            "socks5, h.test, 1080",
            "socks5-tls, h.test, 1080",
            "trojan, h.test, 443, password=p",
            "vmess, h.test, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true",
            "anytls, h.test, 443, password=p",
        ] {
            let o = outcome(
                "P",
                &format!("{def}, shadow-tls-password=s3cret, shadow-tls-version=3, shadow-tls-sni=site.test"),
            );
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            let layer = o.spec.unwrap().shadow_tls.expect(def);
            assert_eq!(layer.password.expose(), "s3cret");
            assert_eq!(layer.sni.as_deref(), Some("site.test"));
            assert_eq!(layer.version, ShadowTlsVersion::V3);
            // without the parameters there is no layer
            assert!(outcome("P", def).spec.unwrap().shadow_tls.is_none(), "{def}");
        }
        // an error in the layer drops the spec like any other
        let o = outcome("P", "http, h.test, 80, shadow-tls-password=s3cret, shadow-tls-version=3");
        assert!(o.spec.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        // a policy that opens no connection to a server has no use for it
        let o = outcome("P", "direct, shadow-tls-password=s3cret");
        assert!(o.spec.unwrap().shadow_tls.is_none());
        assert_eq!(o.diagnostics[0].code, codes::W_UNKNOWN_KEY);
        assert!(!o.diagnostics[0].message.contains("s3cret"));
    }

    #[test]
    fn socket_options_under_a_chain_are_reported() {
```

Run: `cargo test -p rurge-config --lib spec::tests`

Expected: 编译错误——`no field `shadow_tls` on type `PolicySpec``。（只改第一处断言而不加新用例时，`notes_unknowns_and_limits` 是运行期变红：左边是 `["udp-relay", "shadow-tls-password"]`——写计划时实际见过。）

- [ ] **Step 2: 配置层——实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    pub common: CommonOpts,
    pub proto: ProtoSpec,
    pub span: Span,
}
```

换成

```rust
    pub common: CommonOpts,
    pub proto: ProtoSpec,
    /// Shadow TLS below the protocol (and below its TLS, when it has one).
    pub shadow_tls: Option<ShadowTlsOpts>,
    pub span: Span,
}
```

`crates/rurge-config/src/spec/mod.rs`（共 5 处，全部删除）——删除：

```rust
            tls::note_shadow_tls(&mut r, &mut notes);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        _ => return SpecOutcome::default(),
    };
    if !matches!(proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
        check_underlying(&mut r, &mut common, env);
```

换成

```rust
        _ => return SpecOutcome::default(),
    };
    let mut shadow_tls = None;
    if !matches!(proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
        shadow_tls = shadow_tls::read_shadow_tls(&mut r);
        check_underlying(&mut r, &mut common, env);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        common,
        proto,
        span: policy.span.clone(),
    });
```

换成

```rust
        common,
        proto,
        shadow_tls,
        span: policy.span.clone(),
    });
```

`crates/rurge-config/src/spec/tls.rs`——删除：

```rust
/// Shadow TLS arrives in M2; until then the parameters are known but inert.
pub(crate) const SHADOW_TLS_KEYS: [&str; 3] = [
    "shadow-tls-password",
    "shadow-tls-sni",
    "shadow-tls-version",
];
```

`crates/rurge-config/src/spec/tls.rs`——删除：

```rust
pub(crate) fn note_shadow_tls(r: &mut ParamReader<'_>, notes: &mut Notes) {
    for key in SHADOW_TLS_KEYS {
        if r.has(key) {
            r.touch(key);
            notes.inert.push(key);
        }
    }
}
```

`note_shadow_tls` 走了之后 `tls.rs` 不再用 `Notes`：

`crates/rurge-config/src/spec/tls.rs`——把

```rust
use super::common::Notes;
use super::reader::ParamReader;
```

换成

```rust
use super::reader::ParamReader;
```

Run: `cargo test -p rurge-config` → 全部通过（lib 137 条）。`direct` / `reject` 不读这三个参数：写在它们上面是普通的未知参数（`W0001`），用例的最后一段钉住这一点，也钉住口令不被回显。

- [ ] **Step 3: `rurge-proto`——先写用例**

阶梯的顺序（shadow-tls 在 tls 与 ws 之下）与"哪一层失败说哪一层"：

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    use crate::testing::{FakeWs, TlsFixture, WsScript};
    use rurge_config::HostName;
    use rurge_config::spec::{TlsOpts, WsOpts};
```

换成

```rust
    use crate::testing::{Camouflage, FakeShadowTls, FakeWs, ShadowTlsScript, TlsFixture, WsScript};
    use crate::transport::shadow_tls::ShadowTlsClient;
    use rurge_config::HostName;
    use rurge_config::spec::{Secret, ShadowTlsOpts, ShadowTlsVersion, TlsOpts, WsOpts};
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    #[tokio::test]
    async fn each_layer_says_which_one_failed() {
```

换成

```rust
    #[tokio::test]
    async fn shadow_tls_sits_below_tls_and_websocket() {
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let fixture = TlsFixture::new(&["127.0.0.1", "site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
            // behind the Shadow TLS server: TLS, and a WebSocket inside it
            let ws = FakeWs::spawn_tls(WsScript::default(), fixture.clone()).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                ShadowTlsVersion::V3,
                "pw",
                site.addr(),
                ws.addr(),
            ))
            .await;
            let server = Target::new(HostName::Ip(front.addr().ip()), front.addr().port());
            let shadow_tls = ShadowTlsClient::build(
                &ShadowTlsOpts {
                    password: Secret::from("pw"),
                    sni: Some("site.test".into()),
                    version: ShadowTlsVersion::V3,
                },
                &server.host,
                fixture.roots(),
            )
            .unwrap();
            let tls = TlsClient::build(
                &TlsOpts::default(),
                &server.host,
                &[],
                None,
                fixture.roots(),
            )
            .unwrap();
            let ws_client = WsClient::new(
                &WsOpts {
                    path: "/tunnel".into(),
                    headers: Vec::new(),
                },
                &server,
                true,
            )
            .unwrap();
            let stack = Stack::new(
                Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                server,
                Some(shadow_tls),
                Some(tls),
                Some(ws_client),
            );
            let mut stream = stack.open(&ConnectOpts::default()).await.unwrap();
            stream.write_all(b"through three layers").await.unwrap();
            let mut buf = [0u8; 20];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"through three layers");
            // the site's handshake first, then the proxy's own TLS
            let seen = fixture.seen_at_least(2).await;
            assert_eq!(seen[0].sni.as_deref(), Some("site.test"));
            assert_eq!(seen[1].sni, None, "an IP literal: no SNI");
            assert_eq!(ws.seen()[0].path, "/tunnel");
            assert!(front.sessions()[0].authenticated);
        })
        .await
        .expect("the round trip finished within the bound");
    }

    #[tokio::test]
    async fn each_layer_says_which_one_failed() {
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
        assert!(matches!(err, OutboundError::Tls(_)), "{err}");
    }
```

换成

```rust
        assert!(matches!(err, OutboundError::Tls(_)), "{err}");
        // and a Shadow TLS client in front of a server that is none
        let shadow_tls = ShadowTlsClient::build(
            &ShadowTlsOpts {
                password: Secret::from("pw"),
                sni: Some("site.test".into()),
                version: ShadowTlsVersion::V2,
            },
            &HostName::Ip(fake.addr().ip()),
            fixture.roots(),
        )
        .unwrap();
        let stack = Stack::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port()),
            Some(shadow_tls),
            None,
            None,
        );
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stack.open(&ConnectOpts::default()),
        )
        .await
        .expect("the peer answers or closes")
        .err()
        .expect("a TLS handshake against a plain server fails");
        assert!(err.to_string().starts_with("shadow-tls: "), "{err}");
    }
```

没写 `shadow-tls-sni` 时用哪个名字校验证书（P7）：

`crates/rurge-proto/src/build.rs`——把

```rust
    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }
```

换成

```rust
    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    #[tokio::test]
    async fn without_shadow_tls_sni_the_camouflage_certificate_is_checked_against_the_policy_s_name() {
        use crate::testing::{Camouflage, FakeShadowTls, ShadowTlsScript, TlsFixture, echo_server};
        use rurge_config::spec::{Secret, ShadowTlsVersion};
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let fixture = TlsFixture::new(&["site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 0).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                ShadowTlsVersion::V2,
                "pw",
                site.addr(),
                echo_server().await,
            ))
            .await;
            let opts = ShadowTlsOpts {
                password: Secret::from("pw"),
                sni: None,
                version: ShadowTlsVersion::V2,
            };
            let server = HostName::Ip(front.addr().ip());
            let addr = front.addr();
            let open = |client: ShadowTlsClient| async move {
                let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
                client.wrap(Box::new(tcp)).await.map(|_| ())
            };
            // the policy's own `sni` names the certificate
            let tls = TlsOpts {
                sni: Sni::Name("site.test".into()),
                ..TlsOpts::default()
            };
            let client = shadow_tls_client(Some(&opts), Some(&tls), &server, fixture.roots())
                .unwrap()
                .expect("a layer");
            open(client).await.expect("verified against `sni`");
            let seen = fixture.seen_at_least(1).await;
            assert_eq!(seen[0].sni, None, "and no SNI was sent (manual)");
            // no name of its own: the server's, which this certificate does not cover
            let client = shadow_tls_client(Some(&opts), None, &server, fixture.roots())
                .unwrap()
                .expect("a layer");
            let err = open(client).await.expect_err("127.0.0.1 is not in the certificate");
            assert!(
                err.to_string()
                    .starts_with("shadow-tls: the camouflage handshake failed: "),
                "{err}"
            );
            // and no options, no layer
            assert!(
                shadow_tls_client(None, Some(&tls), &server, no_roots())
                    .unwrap()
                    .is_none()
            );
        })
        .await
        .expect("bounded");
    }
```

`http` 与 `socks5-tls` 经 Shadow TLS：

`crates/rurge-proto/src/http.rs`——把

```rust
    use crate::testing::{FakeHttpProxy, HttpProxyScript, TlsFixture, echo_server};
```

换成

```rust
    use crate::testing::{
        Camouflage, FakeHttpProxy, FakeShadowTls, HttpProxyScript, ShadowTlsScript, TlsFixture,
        echo_server,
    };
```

`crates/rurge-proto/src/http.rs`——把

```rust
    #[tokio::test]
    async fn the_target_name_is_sent_as_it_is_and_ipv6_is_bracketed() {
```

换成

```rust
    #[tokio::test]
    async fn connect_tunnels_through_shadow_tls() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let echo = echo_server().await;
            let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
            let fixture = TlsFixture::new(&["site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                rurge_config::spec::ShadowTlsVersion::V2,
                "pw",
                site.addr(),
                proxy.addr(),
            ))
            .await;
            let out = outbound(
                &format!(
                    "http, 127.0.0.1, {}, shadow-tls-password=pw, shadow-tls-sni=site.test",
                    front.addr().port()
                ),
                fixture.roots(),
            );
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"through the frames").await;
            assert_eq!(
                proxy.heads()[0].request_line,
                format!("CONNECT {echo} HTTP/1.1")
            );
            assert!(front.sessions()[0].authenticated);
            // plain requests in absolute form take the same road
            let forward = out.http_forward().expect("an http proxy forwards");
            let mut stream = forward.connect(&ConnectOpts::default()).await.unwrap();
            stream
                .write_all(b"GET http://origin.test/ HTTP/1.1\r\nHost: origin.test\r\n\r\n")
                .await
                .unwrap();
            let mut answer = [0u8; 12];
            stream.read_exact(&mut answer).await.unwrap();
            assert!(answer.starts_with(b"HTTP/1.1 "), "{answer:?}");
            assert!(front.sessions()[1].authenticated);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn the_target_name_is_sent_as_it_is_and_ipv6_is_bracketed() {
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    use crate::testing::{FakeSocks5, Socks5Script, TlsFixture, echo_server};
```

换成

```rust
    use crate::testing::{
        Camouflage, FakeShadowTls, FakeSocks5, ShadowTlsScript, Socks5Script, TlsFixture,
        echo_server,
    };
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    #[tokio::test]
    async fn connects_without_and_with_credentials() {
```

换成

```rust
    #[tokio::test]
    async fn socks5_tls_runs_inside_shadow_tls() {
        tokio::time::timeout(Duration::from_secs(30), async {
            let echo = echo_server().await;
            let fixture = TlsFixture::new(&["127.0.0.1", "site.test"]);
            let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
            let proxy = FakeSocks5::spawn_tls(Socks5Script::default(), fixture.clone(), false).await;
            let front = FakeShadowTls::spawn(ShadowTlsScript::new(
                rurge_config::spec::ShadowTlsVersion::V3,
                "pw",
                site.addr(),
                proxy.addr(),
            ))
            .await;
            let out = outbound(
                &format!(
                    "socks5-tls, 127.0.0.1, {}, shadow-tls-password=pw, shadow-tls-version=3, shadow-tls-sni=site.test",
                    front.addr().port()
                ),
                fixture.roots(),
            );
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"three handshakes deep").await;
            assert_eq!(proxy.requests().len(), 1);
            assert!(front.sessions()[0].authenticated);
        })
        .await
        .expect("bounded");
    }

    #[tokio::test]
    async fn connects_without_and_with_credentials() {
```

Run: `cargo test -p rurge-proto --lib`

Expected: 编译错误——`Stack::new` 只接受 4 个参数、`cannot find function `shadow_tls_client``、`no field `shadow_tls``（取决于编译器先报哪一个）。

- [ ] **Step 4: `rurge-proto`——阶梯与共用的构建函数**

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
//! The fixed ladder between a connector and a protocol's own handshake
//! (phase 2 design §5.4): connect → tls → ws. Shadow TLS joins in M2c.
```

换成

```rust
//! The fixed ladder between a connector and a protocol's own handshake
//! (phase 2 design §5.4): connect → shadow-tls → tls → ws.
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
use crate::OutboundError;
use crate::transport::tls::TlsClient;
```

换成

```rust
use crate::OutboundError;
use crate::transport::shadow_tls::ShadowTlsClient;
use crate::transport::tls::TlsClient;
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    server: Target,
    tls: Option<TlsClient>,
    ws: Option<WsClient>,
}
```

换成

```rust
    server: Target,
    shadow_tls: Option<ShadowTlsClient>,
    tls: Option<TlsClient>,
    ws: Option<WsClient>,
}
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        tls: Option<TlsClient>,
        ws: Option<WsClient>,
    ) -> Stack {
        Stack {
            connector,
            server,
            tls,
            ws,
        }
    }
```

换成

```rust
    /// The layers in the order they are passed through.
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        shadow_tls: Option<ShadowTlsClient>,
        tls: Option<TlsClient>,
        ws: Option<WsClient>,
    ) -> Stack {
        Stack {
            connector,
            server,
            shadow_tls,
            tls,
            ws,
        }
    }
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(tls) = &self.tls {
```

换成

```rust
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(shadow_tls) = &self.shadow_tls {
            stream = shadow_tls.wrap(stream).await?;
        }
        if let Some(tls) = &self.tls {
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
                server,
                Some(tls),
                Some(ws),
            );
```

换成

```rust
                server,
                None,
                Some(tls),
                Some(ws),
            );
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
            server,
            Some(tls),
            None,
        );
```

换成

```rust
            server,
            None,
            Some(tls),
            None,
        );
```

`crates/rurge-proto/src/build.rs`——把

```rust
//! `BuildError` (why an outbound could not be built from its spec) and the
//! helpers shared by the outbounds' `from_spec` (currently `tls_client`).
```

换成

```rust
//! `BuildError` (why an outbound could not be built from its spec) and the
//! helpers shared by the outbounds' constructors.
```

`crates/rurge-proto/src/build.rs`——把

```rust
use crate::keystore::decode_p12;
use crate::transport::tls::TlsClient;
use rurge_config::spec::{PolicySpec, TlsOpts};
```

换成

```rust
use crate::keystore::decode_p12;
use crate::transport::shadow_tls::ShadowTlsClient;
use crate::transport::tls::TlsClient;
use rurge_config::spec::{PolicySpec, ShadowTlsOpts, Sni, TlsOpts};
```

`crates/rurge-proto/src/build.rs`——把

```rust
    TlsClient::build(opts, server, default_alpn, identity, roots).map(Some)
}
```

换成

```rust
    TlsClient::build(opts, server, default_alpn, identity, roots).map(Some)
}

/// The Shadow TLS layer of a policy, when it has one. Without a
/// `shadow-tls-sni` the camouflage certificate is checked against the name
/// the policy's own TLS would use: its `sni`, else the server.
pub fn shadow_tls_client(
    opts: Option<&ShadowTlsOpts>,
    tls: Option<&TlsOpts>,
    server: &HostName,
    roots: Arc<RootCertStore>,
) -> Result<Option<ShadowTlsClient>, BuildError> {
    let Some(opts) = opts else {
        return Ok(None);
    };
    let fallback = match tls.map(|tls| &tls.sni) {
        Some(Sni::Name(name)) => HostName::parse(name),
        _ => server.clone(),
    };
    ShadowTlsClient::build(opts, &fallback, roots).map(Some)
}
```

- [ ] **Step 5: `rurge-proto`——三个 M2 出站多一个参数**

`trojan.rs`：

`crates/rurge-proto/src/trojan.rs`——把

```rust
use crate::build::tls_client;
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client};
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
        spec: &TrojanSpec,
        keystore: &[KeystoreItem],
```

换成

```rust
        spec: &TrojanSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
```

换成

```rust
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
            stack: Stack::new(connector, server, tls, ws),
```

换成

```rust
            stack: Stack::new(connector, server, shadow_tls, tls, ws),
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
use rurge_config::spec::TrojanSpec;
```

换成

```rust
use rurge_config::spec::{ShadowTlsOpts, TrojanSpec};
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
    use rurge_config::spec::TlsOpts;
    use rurge_config::spec::trojan::read_trojan;
```

换成

```rust
    use rurge_config::spec::TlsOpts;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::trojan::read_trojan;
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
        let spec = read_trojan(&mut r, &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        TrojanOutbound::new(
            "T",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            &[],
```

换成

```rust
        let spec = read_trojan(&mut r, &[]);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        TrojanOutbound::new(
            "T",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            &[],
```

`crates/rurge-proto/src/trojan.rs`——把

```rust
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            &[],
```

换成

```rust
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            None,
            &[],
```

`vmess/mod.rs`：

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
use crate::build::tls_client;
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client};
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
        spec: &VmessSpec,
        keystore: &[KeystoreItem],
```

换成

```rust
        spec: &VmessSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
        let tls = tls_client(spec.tls.as_ref(), &server.host, &[], keystore, roots)?;
```

换成

```rust
        let shadow_tls =
            shadow_tls_client(shadow_tls, spec.tls.as_ref(), &server.host, roots.clone())?;
        let tls = tls_client(spec.tls.as_ref(), &server.host, &[], keystore, roots)?;
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
            stack: Stack::new(connector, server, tls, ws),
```

换成

```rust
            stack: Stack::new(connector, server, shadow_tls, tls, ws),
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
use rurge_config::spec::{VmessCipher, VmessSpec};
```

换成

```rust
use rurge_config::spec::{ShadowTlsOpts, VmessCipher, VmessSpec};
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::vmess::read_vmess;
```

换成

```rust
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::vmess::read_vmess;
```

`crates/rurge-proto/src/vmess/mod.rs`——把

```rust
        assert!(read.aead, "the line asks for the legacy handshake");
        assert!(!r.has_errors(), "{:?}", r.finish());
        VmessOutbound::new(
            "V",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            &[],
```

换成

```rust
        assert!(read.aead, "the line asks for the legacy handshake");
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        VmessOutbound::new(
            "V",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &read.spec,
            shadow_tls.as_ref(),
            &[],
```

`anytls/mod.rs`：

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
use crate::build::tls_client;
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client};
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
        spec: &AnyTlsSpec,
        keystore: &[KeystoreItem],
```

换成

```rust
        spec: &AnyTlsSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
```

换成

```rust
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
            stack: Stack::new(connector, server, tls, None),
```

换成

```rust
            stack: Stack::new(connector, server, shadow_tls, tls, None),
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
use rurge_config::spec::AnyTlsSpec;
```

换成

```rust
use rurge_config::spec::{AnyTlsSpec, ShadowTlsOpts};
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::anytls::read_anytls;
```

换成

```rust
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::anytls::read_anytls;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
        let spec = read_anytls(&mut r, &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        AnyTlsOutbound::new(
            "A",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            &[],
```

换成

```rust
        let spec = read_anytls(&mut r, &[]);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        AnyTlsOutbound::new(
            "A",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            &[],
```

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            &[],
```

换成

```rust
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            None,
            &[],
```

三个文件的测试夹具现在从同一行策略里读出 Shadow TLS 的参数（`read_shadow_tls`），其余用例不变。

- [ ] **Step 6: `rurge-proto`——`http` / `socks5` 迁到 `Stack`**

`crates/rurge-proto/src/http.rs`——把

```rust
use crate::build::tls_client;
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client};
```

`crates/rurge-proto/src/http.rs`——把

```rust
use crate::transport::prefixed;
use crate::transport::tls::TlsClient;
```

换成

```rust
use crate::transport::Stack;
use crate::transport::prefixed;
```

`crates/rurge-proto/src/http.rs`——把

```rust
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    tls: Option<TlsClient>,
    /// The `Proxy-Authorization` value, ready to send.
```

换成

```rust
    name: String,
    stack: Stack,
    /// The `Proxy-Authorization` value, ready to send.
```

`crates/rurge-proto/src/http.rs`——把

```rust
        let tls = tls_client(http.tls.as_ref(), host, &[], keystore, roots)?;
```

换成

```rust
        let shadow_tls = shadow_tls_client(
            spec.shadow_tls.as_ref(),
            http.tls.as_ref(),
            host,
            roots.clone(),
        )?;
        let tls = tls_client(http.tls.as_ref(), host, &[], keystore, roots)?;
```

`crates/rurge-proto/src/http.rs`——把

```rust
            name: spec.name.clone(),
            server: Target::new(host.clone(), port),
            connector,
            tls,
            authorization,
```

换成

```rust
            name: spec.name.clone(),
            stack: Stack::new(
                connector,
                Target::new(host.clone(), port),
                shadow_tls,
                tls,
                None,
            ),
            authorization,
```

`crates/rurge-proto/src/http.rs`——把

```rust
    /// TCP to the proxy, then TLS for `https`.
    async fn dial(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let stream = self.connector.connect(&self.server, opts).await?;
        match &self.tls {
            Some(tls) => tls.wrap(stream).await.map_err(OutboundError::tls),
            None => Ok(stream),
        }
    }
```

换成

```rust
    /// TCP to the proxy, then Shadow TLS when the policy has it, then TLS for `https`.
    async fn dial(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        self.stack.open(opts).await
    }
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
use crate::build::tls_client;
```

换成

```rust
use crate::build::{shadow_tls_client, tls_client};
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
use crate::transport::tls::TlsClient;
```

换成

```rust
use crate::transport::Stack;
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    tls: Option<TlsClient>,
```

换成

```rust
    name: String,
    stack: Stack,
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
        let tls = tls_client(socks.tls.as_ref(), host, &[], keystore, roots)?;
```

换成

```rust
        let shadow_tls = shadow_tls_client(
            spec.shadow_tls.as_ref(),
            socks.tls.as_ref(),
            host,
            roots.clone(),
        )?;
        let tls = tls_client(socks.tls.as_ref(), host, &[], keystore, roots)?;
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
            name: spec.name.clone(),
            server: Target::new(host.clone(), port),
            connector,
            tls,
            credentials,
```

换成

```rust
            name: spec.name.clone(),
            stack: Stack::new(
                connector,
                Target::new(host.clone(), port),
                shadow_tls,
                tls,
                None,
            ),
            credentials,
```

`crates/rurge-proto/src/socks5.rs`——把

```rust
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(tls) = &self.tls {
            stream = tls.wrap(stream).await.map_err(OutboundError::tls)?;
        }
```

换成

```rust
        let mut stream = self.stack.open(opts).await?;
```

迁移之后这两个出站不再自己持有 `connector` / `tls`：`dial`（http）与握手的第一步（socks5）就是 `self.stack.open(opts)`。TLS 层的错误仍由 `Stack::open` 映射成 `OutboundError::Tls`，明文 HTTP 的绝对 URI 转发（`HttpForward::connect`）经同一个 `dial`，所以同样带上 Shadow TLS——Step 3 的 http 用例后半段钉住这一点。

- [ ] **Step 7: `rurge-engine`——工厂把这一层交给三个 M2 出站**

`crates/rurge-engine/src/outbounds.rs`——把

```rust
                server_of(spec)?,
                trojan,
                &self.keystore,
```

换成

```rust
                server_of(spec)?,
                trojan,
                spec.shadow_tls.as_ref(),
                &self.keystore,
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
                server_of(spec)?,
                vmess,
                &self.keystore,
```

换成

```rust
                server_of(spec)?,
                vmess,
                spec.shadow_tls.as_ref(),
                &self.keystore,
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
                server_of(spec)?,
                anytls,
                &self.keystore,
```

换成

```rust
                server_of(spec)?,
                anytls,
                spec.shadow_tls.as_ref(),
                &self.keystore,
```

`environment()` 不改（P15）：这一层只读 spec 与进程级的根证书库。

- [ ] **Step 8: 运行**

```bash
cargo test -p rurge-config
cargo test -p rurge-proto
cargo test -p rurge-engine
```

Expected: `rurge-config` lib 137 passed；`rurge-proto` lib 160 passed（Task 4 之后的 156 + `shadow_tls_sits_below_tls_and_websocket`、`without_shadow_tls_sni_the_camouflage_certificate_is_checked_against_the_policy_s_name`、`connect_tunnels_through_shadow_tls`、`socks5_tls_runs_inside_shadow_tls`）；`rurge-engine` 全部通过（已有用例不受影响）。

- [ ] **Step 9: 门禁与提交**

```bash
git add crates/rurge-config crates/rurge-proto crates/rurge-engine
git commit -m "feat: Shadow TLS 接进阶梯与配置层——Stack 的 shadow-tls 一层、PolicySpec.shadow_tls、三个参数的 W0029 退役；http / socks5 迁到 Stack；trojan / vmess / anytls 的构造函数多一个参数"
```

---

### Task 6: 承接——两个大测试文件按协议拆分；`copy_half` 的那句注释

纯搬运，不改任何用例的内容（C1、C2、P20）。先做这一步，Task 7 / 8 的新用例才有公共夹具可用。

**Files:**
- Create: `crates/rurge-engine/tests/common/mod.rs`、`crates/rurge-engine/tests/outbounds_tls_family.rs`
- Modify: `crates/rurge-engine/tests/outbounds.rs`
- Create: `tests/interop/tests/common/mod.rs`、`tests/interop/tests/sing_box_tls_family.rs`
- Modify: `tests/interop/tests/sing_box.rs`
- Modify: `crates/rurge-proto/src/anytls/mod.rs`

**Interfaces:**
- Consumes: 两个文件现有的全部内容。
- Produces: 两个 `tests/common/mod.rs`——原文件里**不是用例的每一个顶层项**（夹具结构体、辅助函数、常量）原样搬入并变成 `pub`，原来的 `use` 变成 `pub use`；测试文件只需要 `mod common;` 与 `use common::*;`。Task 7 用到的名字：`Harness`（字段 `dir` `engine` `listeners` `dns`）、`Profile`、`harness`、`runtime`、`connect_via_http`、`get`、`plain_get`、`wait_until`、`origin_addr`、`trojan_upstream`、`outbound_now`，以及经 `pub use` 带出来的 `TestServer` `FakeHttpProxy` `HttpProxyScript` `FakeSocks5` `Socks5Script` `TlsFixture` `EngineShared` `TcpStream` `Arc` `Duration` `SocketAddr` 等。Task 8 用到的：`outbound`、`target`、`roundtrip`、`roundtrip_big`、`leaf_files`、`trojan_inbound`，以及 `Inbound` `InboundKind` `SingBox` `sing_box_or_skip` `TlsFixture` `echo_server` `ConnectOpts` `OutboundError` `Path` `Arc`。

- [ ] **Step 1: 记下拆分前的用例名单**

```bash
cargo test -p rurge-engine --test outbounds -- --list 2>/dev/null | grep ": test" | sort > /tmp/outbounds-before.txt
cargo test -p rurge-interop --test sing_box -- --list 2>/dev/null | grep ": test" | sort > /tmp/sing-box-before.txt
wc -l /tmp/outbounds-before.txt /tmp/sing-box-before.txt
```

Expected: 27 与 10。

- [ ] **Step 2: 把拆分脚本存成一个临时文件**

存到仓库**之外**（例如 `/tmp/split_tests.py`），用完即弃，不提交：

```python
"""Splits one integration-test file into `common/mod.rs` (everything that is
not a test), the original file (the tests that stay) and a second file (the
tests named in MOVED). Nothing is rewritten but visibility: what moves into
`common` becomes `pub`, because the test crates reach it through `mod common`.

usage: split_tests.py <tests dir> <file> <second file> <second file's header> <test name>...
"""
import io
import os
import re
import sys

tests_dir, first, second, second_header = sys.argv[1:5]
moved = set(sys.argv[5:])
path = os.path.join(tests_dir, first)
lines = io.open(path, encoding="utf-8").read().split("\n")

# the file's own header: `//!` lines, then the `use` block
at = 0
while lines[at].startswith("//!"):
    at += 1
header = lines[:at]
while lines[at] == "":
    at += 1
uses = []
while lines[at].startswith("use ") or (uses and not lines[at].startswith(("fn ", "async fn ", "struct ", "const ", "#[", "///", "impl ")) and lines[at] != ""):
    uses.append(lines[at])
    at += 1

# top-level items: from the first line of their doc comment / attributes to
# the line that closes them (`}` in column 0, or a `;` at depth 0)
items, current, depth = [], [], 0
for line in lines[at:]:
    if not current and line == "":
        continue
    current.append(line)
    code = re.sub(r'"(\\.|[^"\\])*"', '""', line)
    code = re.sub(r"'(\\.|[^'\\])'", "''", code)
    code = code.split("//")[0]
    depth += code.count("{") - code.count("}")
    opened = any(l.startswith(("fn ", "async fn ", "struct ", "impl ", "const ", "static ")) for l in current)
    if depth == 0 and opened and (line == "}" or line.rstrip().endswith(";")):
        items.append(current)
        current = []
assert not current, current[:3]


def name_of(item):
    for l in item:
        m = re.match(r"(?:async )?fn (\w+)", l)
        if m:
            return m.group(1)
    return None


def is_test(item):
    return any(l.strip() in ("#[tokio::test]", "#[test]") or l.startswith("#[tokio::test(") for l in item)


def publish(item):
    out, in_struct = [], False
    for l in item:
        if re.match(r"(async fn|fn|struct|const|static) ", l):
            in_struct = l.startswith("struct ") and l.rstrip().endswith("{")
            l = "pub " + l
        elif in_struct and re.match(r"    \w+: ", l):
            l = "    pub " + l[4:]
        elif re.match(r"    (async fn|fn) ", l):
            l = "    pub " + l[4:]
        if l == "}":
            in_struct = False
        out.append(l)
    return out


common, stay, go = [], [], []
for item in items:
    if not is_test(item):
        common.append(publish(item))
    elif name_of(item) in moved:
        go.append(item)
    else:
        stay.append(item)
found = {name_of(i) for i in go}
assert found == moved, moved - found


def write(rel, head, body):
    p = os.path.join(tests_dir, rel)
    os.makedirs(os.path.dirname(p), exist_ok=True)
    text = "\n".join(head) + "\n\n" + "\n\n".join("\n".join(i) for i in body) + "\n"
    io.open(p, "w", encoding="utf-8", newline="\n").write(text)


shared = [("pub " + l if l.startswith("use ") else l) for l in uses]
write("common/mod.rs",
      ["//! What the test files of this directory share: the harness, the helpers and",
       "//! the imports (re-exported, so a test file needs `use common::*` and nothing",
       "//! else). Each test file is a crate of its own and uses a part of all this",
       "//! only: hence the two `allow`s.",
       "",
       "#![allow(dead_code, unused_imports)]",
       ""] + shared, common)
glob = ["", "mod common;", "", "use common::*;"]
write(first, header + glob, stay)
write(second, [second_header] + glob, go)
print(f"{first}: {len(stay)} tests stay, {second}: {len(go)} tests, common: {len(common)} items")
```

它做的事：文件头的 `//!` 注释留给原文件；`use` 块进 `common/mod.rs` 并变成 `pub use`；其余顶层项按"有没有 `#[tokio::test]` / `#[test]`"分成用例与非用例，非用例进 `common/mod.rs` 并变成 `pub`（含结构体字段与 `impl` 块里的方法），用例按名单留下或搬走。除可见性之外一个字符都不改。

- [ ] **Step 3: 拆引擎的端到端用例**

```bash
python /tmp/split_tests.py crates/rurge-engine/tests outbounds.rs outbounds_tls_family.rs \
  "//! Sessions that leave through the TLS family (trojan, vmess, anytls): the same harness as \`outbounds.rs\`." \
  a_connect_leaves_through_a_vmess_upstream_with_the_name_unresolved \
  a_wrong_vmess_id_ends_the_session_with_a_text_that_says_so \
  an_anytls_upstream_carries_two_requests_over_one_session \
  the_new_protocols_work_at_either_end_of_a_chain \
  a_connect_leaves_through_a_trojan_upstream_with_the_name_unresolved \
  a_plain_request_is_tunnelled_through_trojan_over_websocket \
  a_trojan_exit_is_reached_through_a_socks5_entry_by_name \
  a_socks5_exit_is_reached_through_a_trojan_entry \
  an_unrelated_reload_keeps_an_anytls_pool_and_a_change_of_its_own_drops_it \
  a_reused_outbound_resolves_through_the_new_generation
```

Expected 输出：`outbounds.rs: 17 tests stay, outbounds_tls_family.rs: 10 tests, common: 21 items`。

- [ ] **Step 4: 拆互操作用例**

```bash
python /tmp/split_tests.py tests/interop/tests sing_box.rs sing_box_tls_family.rs \
  "//! The TLS family against sing-box (trojan, vmess, anytls): the same helpers as \`sing_box.rs\`." \
  trojan_with_and_without_websocket \
  a_wrong_trojan_password_is_not_relayed \
  vmess_with_and_without_tls_and_websocket \
  a_wrong_vmess_id_is_not_relayed \
  anytls_reuses_its_session_and_can_be_told_not_to
```

Expected 输出：`sing_box.rs: 5 tests stay, sing_box_tls_family.rs: 5 tests, common: 10 items`。

- [ ] **Step 5: 核对——格式、警告、名单**

```bash
RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check
cargo clippy -p rurge-engine -p rurge-interop --all-targets -- -D warnings
cargo test -p rurge-engine --test outbounds --test outbounds_tls_family -- --list 2>/dev/null | grep ": test" | sort > /tmp/outbounds-after.txt
cargo test -p rurge-interop --test sing_box --test sing_box_tls_family -- --list 2>/dev/null | grep ": test" | sort > /tmp/sing-box-after.txt
diff /tmp/outbounds-before.txt /tmp/outbounds-after.txt && diff /tmp/sing-box-before.txt /tmp/sing-box-after.txt && echo SAME
```

Expected: fmt 无差异、clippy 无警告、`SAME`。脚本产出的文件写计划时就是 rustfmt 干净的；如果 `cargo fmt --check` 报差异，跑一次 `cargo fmt --all` 并在报告里说明。`common/mod.rs` 顶上的 `#![allow(dead_code, unused_imports)]` 是必需的：每个测试文件是一个独立的 crate，只用到公共部分的一部分。脚本写出的文件是 LF 行尾，而本机工作区里的文件是 CRLF（`core.autocrlf=true`，索引里存的是 LF）：`git diff` 只会显示真实的改动，`git add` 时那句 "LF will be replaced by CRLF" 的提示可以忽略。

Run: `cargo test -p rurge-engine --test outbounds --test outbounds_tls_family` → 17 passed 与 10 passed。

- [ ] **Step 6: C1 的注释**

`crates/rurge-proto/src/anytls/mod.rs`——把

```rust
    /// `read` → `write_all` with no flush, `shutdown` at EOF, both directions
    /// polled from one task through `tokio::io::split`.
    async fn copy_half
```

换成

```rust
    /// `read` → `write_all` with no flush, `shutdown` at EOF, both directions
    /// polled from one task through `tokio::io::split`. Stays the un-flushed
    /// loop on purpose even though the engine's relay now flushes: it is the
    /// stricter caller here, so do not "fix" it to match.
    async fn copy_half
```

（与 `crates/rurge-proto/src/vmess/mod.rs` 里那份 `copy_half` 的注释一字不差。）

- [ ] **Step 7: 门禁与提交**

```bash
git add crates/rurge-engine/tests tests/interop/tests crates/rurge-proto/src/anytls/mod.rs
git commit -m "test: 引擎端到端与 sing-box 互操作的用例文件按协议拆分（公共夹具进 tests/common）；anytls 测试里 copy_half 的注释补齐"
```

---

### Task 7: 引擎端到端——经 Shadow TLS 的会话；根证书库进 `EngineShared`

伪装握手照常校验证书、没有任何绕过，所以端到端夹具必须能让引擎信任夹具自己的 CA（P16）。

**Files:**
- Modify: `crates/rurge-engine/src/shared.rs`、`crates/rurge-engine/src/runtime.rs`
- Modify: `crates/rurge-engine/tests/common/mod.rs`
- Create: `crates/rurge-engine/tests/outbounds_shadow_tls.rs`

**Interfaces:**
- Consumes: Task 5 的接线；Task 6 的 `tests/common`；`rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript}`；`EngineFactory::with_roots`（已有）。
- Produces:
  - `rurge_engine::EngineShared.roots: Option<Arc<rustls::RootCertStore>>`（`None` = 操作系统的根；`EngineShared::new` / `default` 给 `None`，bin 一行不用改）
  - `tests/common`：`harness_trusting(p: Profile<'_>, roots: Arc<rustls::RootCertStore>) -> Harness`

- [ ] **Step 1: 先写用例**

新建 `crates/rurge-engine/tests/outbounds_shadow_tls.rs`：

```rust
//! Sessions that leave through a policy wrapped in Shadow TLS: the same
//! harness as `outbounds.rs`, with the camouflage site's CA as the engine's
//! trust anchors (the camouflage handshake always verifies certificates).

mod common;

use common::*;
use rurge_config::spec::ShadowTlsVersion;
use rurge_proto::testing::{Camouflage, FakeShadowTls, ShadowTlsScript};

const SITE: &str = "site.test";

/// A camouflage site and a Shadow TLS server (password `st-pw`) in front of `behind`.
async fn shadow_front(
    version: ShadowTlsVersion,
    names: &[&str],
    behind: SocketAddr,
) -> (Arc<TlsFixture>, Camouflage, FakeShadowTls) {
    let fixture = TlsFixture::new(names);
    let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
    let front =
        FakeShadowTls::spawn(ShadowTlsScript::new(version, "st-pw", site.addr(), behind)).await;
    (fixture, site, front)
}

#[tokio::test]
async fn a_connect_leaves_through_trojan_wrapped_in_shadow_tls_v3() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, _site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "T = trojan, 127.0.0.1, {}, {params}, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni={SITE}",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,T",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    // the site's handshake came first and carried the configured name
    let seen = fixture.seen_at_least(1).await;
    assert_eq!(seen[0].sni.as_deref(), Some(SITE));
    assert!(front.sessions()[0].authenticated);
    // and the name went to the trojan server unresolved
    let request = trojan
        .requests()
        .first()
        .cloned()
        .expect("a trojan request");
    assert_eq!((request.host.as_str(), request.port), ("target.test", 8080));
    assert!(h.dns.queries().is_empty(), "{:?}", h.dns.queries());
}

#[tokio::test]
async fn an_http_upstream_behind_shadow_tls_v2_needs_no_sni() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let proxy = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    // no `shadow-tls-sni`: no SNI goes out, and the certificate is checked
    // against the policy's own server
    let (fixture, _site, front) =
        shadow_front(ShadowTlsVersion::V2, &["127.0.0.1"], proxy.addr()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "Up = http, 127.0.0.1, {}, shadow-tls-password=st-pw",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,Up",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    assert_eq!(fixture.seen_at_least(1).await[0].sni, None);
    assert!(front.sessions()[0].authenticated);
    // a plain request in absolute form takes the same road (the fake proxy
    // answers such a request itself)
    let answer = plain_get(
        h.http(),
        "http://target.test:8080/hello",
        "target.test:8080",
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    let heads = proxy.heads();
    let forwarded = &heads.last().expect("a forwarded request").request_line;
    assert!(
        forwarded.starts_with("GET http://target.test:8080/hello "),
        "{forwarded}"
    );
    assert!(front.sessions()[1].authenticated);
}

#[tokio::test]
async fn a_wrong_shadow_tls_password_fails_the_session_with_a_text_that_says_so() {
    let origin = TestServer::spawn().await;
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "T = trojan, 127.0.0.1, {}, {params}, shadow-tls-password=an0ther, shadow-tls-version=3, shadow-tls-sni={SITE}",
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,T",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    // `Connection: close`: rurge closes after the 502, so the read below ends
    s.write_all(
        b"CONNECT target.test:8080 HTTP/1.1\r\nHost: target.test:8080\r\nConnection: close\r\n\r\n",
    )
    .await
    .unwrap();
    let mut answer = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), s.read_to_end(&mut answer))
        .await
        .expect("the proxy answers within the bound");
    let answer = String::from_utf8_lossy(&answer).into_owned();
    assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(error, "shadow-tls: the server did not authenticate itself");
    assert!(!error.contains("an0ther"));
    // the server saw a stranger, and the site got a visitor that asked for a page
    assert!(!front.sessions()[0].authenticated);
    assert!(site.received().starts_with(b"GET / HTTP/1.1\r\n"));
    assert!(trojan.requests().is_empty());
}

#[tokio::test]
async fn shadow_tls_runs_through_an_underlying_proxy() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, _site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "Entry = socks5, 127.0.0.1, {}\nExit = trojan, 127.0.0.1, {}, {params}, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni={SITE}, underlying-proxy=Entry",
                entry.addr().port(),
                front.addr().port()
            ),
            rules: "DOMAIN,target.test,Exit",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    // the entry was asked for the Shadow TLS server, not for the target
    let asked = entry.requests().first().cloned().expect("a socks5 request");
    assert_eq!(asked.port, front.addr().port());
    assert!(front.sessions()[0].authenticated);
}

#[tokio::test]
async fn vmess_without_tls_and_anytls_run_inside_shadow_tls_too() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    // vmess without TLS: the protocol runs on the frames directly, and its
    // request head waits for the first payload — the late first frame that
    // v2 likes least
    let (vmess, vmess_params) = vmess_upstream(false, false, origin_addr(&origin)).await;
    let (fixture, _site, vmess_front) =
        shadow_front(ShadowTlsVersion::V2, &[SITE], vmess.addr()).await;
    // anytls: its own TLS inside, and a session that is reused
    let (anytls, anytls_params) = anytls_upstream(origin_addr(&origin)).await;
    let site = Camouflage::spawn(&fixture, &[&rustls::version::TLS13], 2).await;
    let anytls_front = FakeShadowTls::spawn(ShadowTlsScript::new(
        ShadowTlsVersion::V3,
        "st-pw",
        site.addr(),
        anytls.addr(),
    ))
    .await;
    let h = harness_trusting(
        Profile {
            proxies: &format!(
                "V = vmess, 127.0.0.1, {}, {vmess_params}, shadow-tls-password=st-pw, shadow-tls-sni={SITE}\nA = anytls, 127.0.0.1, {}, {anytls_params}, shadow-tls-password=st-pw, shadow-tls-version=3, shadow-tls-sni={SITE}",
                vmess_front.addr().port(),
                anytls_front.addr().port()
            ),
            rules: "DOMAIN,target.test,V\nDOMAIN,alt.test,A",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let answer = get(&mut tunnel, "target.test", "/hello").await;
    assert!(answer.ends_with("hi there"), "{answer}");
    assert!(vmess_front.sessions()[0].authenticated);
    assert_eq!(vmess.requests().len(), 1);
    // our half of the tunnel is still open: let the relay finish
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the vmess session to finish", || log.recent(10).len() == 1).await;
    for round in 1..=2 {
        let mut tunnel = connect_via_http(h.http(), "alt.test:8080").await;
        let answer = get(&mut tunnel, "alt.test", "/hello").await;
        assert!(answer.ends_with("hi there"), "{answer}");
        drop(tunnel);
        // one record for the vmess session above, then one per round
        wait_until("the session to finish", || {
            log.recent(10).len() == 1 + round
        })
        .await;
    }
    // both requests went over one AnyTLS session, hence one Shadow TLS connection
    assert_eq!(anytls.sessions(), 1);
    assert_eq!(anytls_front.sessions().len(), 1);
}

#[tokio::test]
async fn the_layer_is_part_of_what_a_reload_compares_and_of_what_the_api_hides() {
    let origin = TestServer::spawn().await;
    let (trojan, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let (fixture, _site, front) = shadow_front(ShadowTlsVersion::V3, &[SITE], trojan.addr()).await;
    let line = |password: &str| {
        format!(
            "T = trojan, 127.0.0.1, {}, {params}, shadow-tls-password={password}, shadow-tls-version=3, shadow-tls-sni={SITE}",
            front.addr().port()
        )
    };
    let h = harness_trusting(
        Profile {
            proxies: &line("st-pw"),
            rules: "DOMAIN,target.test,T",
            ..Profile::default()
        },
        fixture.roots(),
    )
    .await;
    let detail = h.engine.policy_detail("T").expect("a configured policy");
    assert!(
        detail.contains("shadow-tls-password=***") && !detail.contains("st-pw"),
        "{detail}"
    );
    assert!(
        detail.contains(&format!("shadow-tls-sni={SITE}")),
        "{detail}"
    );
    let before = outbound_now(&h, "T");
    // an unrelated edit: the outbound stays
    let unrelated = Profile {
        proxies: &line("st-pw"),
        rules: "DOMAIN,target.test,T\nDOMAIN,alt.test,DIRECT",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &unrelated, h.engine.shared()).await);
    assert!(Arc::ptr_eq(&before, &outbound_now(&h, "T")));
    // another Shadow TLS password: a new outbound
    let changed = Profile {
        proxies: &line("an0ther"),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    }
    .text(h.dns.addr());
    h.engine
        .swap_runtime(runtime(h.dir.path(), &changed, h.engine.shared()).await);
    assert!(!Arc::ptr_eq(&before, &outbound_now(&h, "T")));
}
```

Run: `cargo test -p rurge-engine --test outbounds_shadow_tls`

Expected: 编译错误——`cannot find function `harness_trusting``。

- [ ] **Step 2: `EngineShared.roots` 与夹具**

`crates/rurge-engine/src/shared.rs`——把

```rust
use rurge_policy::{GroupSelections, RegistryCell, SelectionTable};
```

换成

```rust
use rurge_policy::{GroupSelections, RegistryCell, SelectionTable};
use rustls::RootCertStore;
```

`crates/rurge-engine/src/shared.rs`——把

```rust
/// Created once per engine — before the first `Runtime::build`, because the
/// registry built there already needs all three — and handed to every later
/// `Runtime::build` of the same engine (`Engine::shared`).
```

换成

```rust
/// Created once per engine — before the first `Runtime::build`, because the
/// registry built there already needs them — and handed to every later
/// `Runtime::build` of the same engine (`Engine::shared`).
```

`crates/rurge-engine/src/shared.rs`——把

```rust
    /// Where direct connectors find the current generation's resolver.
    pub resolver: Arc<ResolverCell>,
}
```

换成

```rust
    /// Where direct connectors find the current generation's resolver.
    pub resolver: Arc<ResolverCell>,
    /// The trust anchors of every outbound's TLS. `None`: the operating
    /// system's. They belong here because they must not change while the
    /// engine lives: a reload reuses outbounds by a fingerprint the roots are
    /// not part of. Tests bring their own CA this way.
    pub roots: Option<Arc<RootCertStore>>,
}
```

`crates/rurge-engine/src/shared.rs`——把

```rust
            selections: Arc::new(SelectionTable::new(initial)),
            resolver: ResolverCell::new(),
        }
```

换成

```rust
            selections: Arc::new(SelectionTable::new(initial)),
            resolver: ResolverCell::new(),
            roots: None,
        }
```

`crates/rurge-engine/src/runtime.rs`——把

```rust
        let factory = crate::outbounds::EngineFactory::new(
            &config,
            opts.shared.resolver.clone(),
            opts.stack.socket_hook.clone(),
        );
```

换成

```rust
        let factory = match &opts.shared.roots {
            Some(roots) => crate::outbounds::EngineFactory::with_roots(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
                roots.clone(),
            ),
            None => crate::outbounds::EngineFactory::new(
                &config,
                opts.shared.resolver.clone(),
                opts.stack.socket_hook.clone(),
            ),
        };
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
pub async fn harness(p: Profile<'_>) -> Harness {
    let dns = MockDns::spawn().await;
```

换成

```rust
pub async fn harness(p: Profile<'_>) -> Harness {
    harness_with(p, EngineShared::default()).await
}

/// An engine whose outbounds trust `roots` instead of the operating system's.
pub async fn harness_trusting(p: Profile<'_>, roots: Arc<rustls::RootCertStore>) -> Harness {
    let shared = EngineShared {
        roots: Some(roots),
        ..EngineShared::default()
    };
    harness_with(p, shared).await
}

async fn harness_with(p: Profile<'_>, shared: EngineShared) -> Harness {
    let dns = MockDns::spawn().await;
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
```

换成

```rust
    let engine = Engine::new(runtime(dir.path(), &text, shared).await);
```

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-engine --test outbounds_shadow_tls`

Expected: 6 passed。六条用例分别钉住：
1. `trojan` 包在 v3 里：伪装握手带着配置的 SNI、假服务端确认客户端通过验证、目标名字未经本机解析就到了 trojan 服务端（DNS 无查询）。
2. `http` 包在 v2 里且不写 `shadow-tls-sni`：**没有 SNI 发出**（手册），证书按策略自己的服务器名校验；明文请求的绝对 URI 转发走同一条路。
3. 口令错误（v3）：客户端拿到 502，会话记录的 `error` 恰好是 `shadow-tls: the server did not authenticate itself`，不含口令；伪装站点收到了那个 `GET / HTTP/1.1`，trojan 服务端一个请求都没收到。
4. 链：带 Shadow TLS 的出口经 `underlying-proxy` 拨出——入口被要求连接的是 Shadow TLS 服务端，不是目标。
5. 不带 TLS 的 `vmess` 包在 v2 里（协议直接跑在帧上，请求头要等首段负载——v2 最不喜欢的晚到首帧）与 `anytls` 包在 v3 里（两个请求复用一条 AnyTLS 会话，所以只有一条 Shadow TLS 连接）：同时钉住这两个出站的构造函数与工厂分支确实把这一层传了下去。
6. 指纹与脱敏：无关的重载沿用同一个出站（`Arc::ptr_eq`），改了 `shadow-tls-password` 的重载换新的；`policy_detail` 里口令是 `***`、`shadow-tls-sni` 原样保留。

再连跑 20 轮确认没有偶发失败（写计划时 40 轮无失败）：

```bash
cargo test -p rurge-engine --test outbounds_shadow_tls --no-run
for i in $(seq 1 20); do cargo test -p rurge-engine --test outbounds_shadow_tls 2>&1 | grep -q "6 passed" || echo "round $i failed"; done
```

- [ ] **Step 4: 门禁与提交**

```bash
git add crates/rurge-engine
git commit -m "feat(engine): 根证书库进 EngineShared（随引擎存续、不随代际变化）；经 Shadow TLS 的端到端用例——v3 包 trojan、v2 包 http 且不发 SNI、口令错误的会话文本、链、不带 TLS 的 vmess 与 anytls、指纹与脱敏"
```

---

### Task 8: 互操作——sing-box 的 `shadowtls` 入站

本机不装 sing-box：这些用例在本机只会打印 `skipping …` 后返回，由首次推送后的 CI 证明。夹具自身的单元用例（渲染形状、"配置绝不碰本机"的守卫）照常在本机运行。

**Files:**
- Modify: `tests/interop/src/lib.rs`
- Modify: `tests/interop/Cargo.toml`（`rustls` dev 依赖；`Cargo.lock` 随之多一行）
- Create: `tests/interop/tests/sing_box_shadow_tls.rs`

**Interfaces:**
- Consumes: Task 6 的 `tests/interop/tests/common`；`rurge_proto::testing::{Camouflage, TlsFixture}`。
- Produces: `rurge_interop::InboundKind::ShadowTls { version: u8, handshake_port: u16, detour: usize }`——渲染成 `type: "shadowtls"`、`version`、v3 的 `users: [{name, password}]` + `strict_mode: true` / v2 的 `password`、`handshake: { server: "127.0.0.1", server_port }`、`detour: "in-<detour>"`；口令取 `users[0].1`（sing-box 1.14.1 的文档 `docs/configuration/inbound/shadowtls.md` 核对过字段名）。

- [ ] **Step 1: 先改夹具的单元用例**

`tests/interop/src/lib.rs`——`every_kind()` 末尾加两个入站、`inbounds_are_rendered_as_sing_box_spells_them` 末尾加断言（下面 Step 2 的代码块里的后两处就是）。先只应用那两处。

Run: `cargo test -p rurge-interop --lib`

Expected: 编译错误——`no variant named `ShadowTls``。

- [ ] **Step 2: 夹具**

`tests/interop/src/lib.rs`——把

```rust
    Trojan,
    Vmess,
    AnyTls,
}
```

换成

```rust
    Trojan,
    Vmess,
    AnyTls,
    /// Relays the TLS handshake to `127.0.0.1:handshake_port` and hands what
    /// it unwraps to the inbound at index `detour`. `users[0]` holds the
    /// password (version 2 has no user names; the name is ignored).
    ShadowTls {
        version: u8,
        handshake_port: u16,
        detour: usize,
    },
}
```

`tests/interop/src/lib.rs`——把

```rust
                    InboundKind::AnyTls => "anytls",
                },
```

换成

```rust
                    InboundKind::AnyTls => "anytls",
                    InboundKind::ShadowTls { .. } => "shadowtls",
                },
```

`tests/interop/src/lib.rs`——把

```rust
            if !inbound.users.is_empty() {
                v["users"] = inbound
```

换成

```rust
            if let InboundKind::ShadowTls {
                version,
                handshake_port,
                detour,
            } = inbound.kind
            {
                let (name, password) = inbound.users.first().expect("a Shadow TLS password");
                v["version"] = json!(version);
                if version == 3 {
                    v["users"] = json!([{ "name": name, "password": password }]);
                    v["strict_mode"] = json!(true);
                } else {
                    v["password"] = json!(password);
                }
                // a loopback IP literal: sing-box resolves nothing
                v["handshake"] = json!({ "server": "127.0.0.1", "server_port": handshake_port });
                v["detour"] = json!(format!("in-{detour}"));
            } else if !inbound.users.is_empty() {
                v["users"] = inbound
```

`tests/interop/src/lib.rs`——把

```rust
                1006,
            ),
        ]
    }
```

换成

```rust
                1006,
            ),
            (
                Inbound {
                    kind: InboundKind::ShadowTls {
                        version: 3,
                        handshake_port: 1100,
                        detour: 3,
                    },
                    users: vec![("u".into(), "st-pw".into())],
                    tls: None,
                    ws_path: None,
                },
                1007,
            ),
            (
                Inbound {
                    kind: InboundKind::ShadowTls {
                        version: 2,
                        handshake_port: 1100,
                        detour: 3,
                    },
                    users: vec![("ignored".into(), "st-pw".into())],
                    tls: None,
                    ws_path: None,
                },
                1008,
            ),
        ]
    }
```

`tests/interop/src/lib.rs`——把

```rust
        assert_eq!(anytls["users"], json!([{ "name": "u", "password": "pw" }]));
        assert_eq!(anytls["tls"]["enabled"], true);
    }
```

换成

```rust
        assert_eq!(anytls["users"], json!([{ "name": "u", "password": "pw" }]));
        assert_eq!(anytls["tls"]["enabled"], true);
        // version 3 has users and a strict mode, version 2 one password
        let v3 = &config["inbounds"][6];
        assert_eq!((&v3["type"], &v3["version"]), (&json!("shadowtls"), &json!(3)));
        assert_eq!(v3["users"], json!([{ "name": "u", "password": "st-pw" }]));
        assert_eq!(v3["strict_mode"], true);
        assert_eq!(
            v3["handshake"],
            json!({ "server": "127.0.0.1", "server_port": 1100 })
        );
        assert_eq!(v3["detour"], "in-3");
        assert!(v3.get("password").is_none() && v3.get("tls").is_none());
        let v2 = &config["inbounds"][7];
        assert_eq!((&v2["version"], &v2["password"]), (&json!(2), &json!("st-pw")));
        assert!(v2.get("users").is_none() && v2.get("strict_mode").is_none());
        assert_eq!(v2["detour"], "in-3");
    }
```

Run: `cargo test -p rurge-interop --lib` → 5 passed。`the_configuration_never_touches_the_machine` 现在也覆盖两个 `shadowtls` 入站：只监听 `127.0.0.1`、`handshake.server` 是回环 IP 字面量、不出现 `set_system_proxy` / `tun` / `auto_route`。

- [ ] **Step 3: 依赖**

`tests/interop/Cargo.toml`——把

```toml
rurge-proto = { workspace = true, features = ["testing"] }
tempfile.workspace = true
```

换成

```toml
rurge-proto = { workspace = true, features = ["testing"] }
rustls.workspace = true
tempfile.workspace = true
```

（用例要写 `rustls::version::TLS13`。`rustls` 早已在工作区里，`Cargo.lock` 只是在 `rurge-interop` 的依赖列表里多一行 `"rustls",`。）

- [ ] **Step 4: 用例**

新建 `tests/interop/tests/sing_box_shadow_tls.rs`：

```rust
//! Shadow TLS against sing-box: a `shadowtls` inbound that relays the
//! handshake to a TLS server of ours on the loopback (the camouflage site)
//! and hands what it unwraps to a `trojan` inbound. The same helpers as
//! `sing_box.rs`.

mod common;

use common::*;
use rurge_proto::testing::Camouflage;

const SITE: &str = "site.test";

/// sing-box with a `shadowtls` inbound (index 0) in front of a `trojan`
/// inbound (index 1), and the site the handshake is borrowed from.
async fn shadow_tls_in_front_of_trojan(
    bin: &Path,
    dir: &Path,
    fixture: &Arc<TlsFixture>,
    version: u8,
    tickets: usize,
) -> (SingBox, Camouflage) {
    let site = Camouflage::spawn(fixture, &[&rustls::version::TLS13], tickets).await;
    let front = Inbound {
        kind: InboundKind::ShadowTls {
            version,
            handshake_port: site.addr().port(),
            detour: 1,
        },
        users: vec![("u".into(), "st-pw".into())],
        tls: None,
        ws_path: None,
    };
    let sb = SingBox::spawn(
        bin,
        dir,
        vec![front, trojan_inbound(leaf_files(fixture, dir), None)],
    );
    (sb, site)
}

fn profile(port: u16, version: u8, password: &str) -> String {
    format!(
        "[Proxy]\nT = trojan, 127.0.0.1, {port}, password=s3same, shadow-tls-password={password}, shadow-tls-version={version}, shadow-tls-sni={SITE}\n[Rule]\nFINAL,DIRECT\n"
    )
}

#[tokio::test]
async fn trojan_behind_shadow_tls_v2_and_v3() {
    let Some(bin) = sing_box_or_skip("trojan_behind_shadow_tls_v2_and_v3") else {
        return;
    };
    // v2: sing-box compares the client's digest with its own as it stands
    // now or stood one write ago, so the site sends no session tickets here;
    // v3 has no such race, and its tickets exercise the records sing-box is
    // still relaying when the data phase begins
    for (version, tickets) in [(2u8, 0usize), (3, 2)] {
        let dir = tempfile::tempdir().unwrap();
        let fixture = TlsFixture::new(&["127.0.0.1", SITE]);
        let (sb, _site) =
            shadow_tls_in_front_of_trojan(&bin, dir.path(), &fixture, version, tickets).await;
        let echo = echo_server().await;
        let out = outbound(&profile(sb.port(0), version, "st-pw"), "T", Some(&fixture));
        roundtrip(&out, echo).await;
        // frames in both directions, well past one record
        roundtrip_big(&out, echo).await;
        // the site saw one handshake per connection, with the configured name
        let seen = fixture.seen_at_least(2).await;
        assert_eq!(seen[0].sni.as_deref(), Some(SITE), "version {version}");
    }
}

#[tokio::test]
async fn a_wrong_shadow_tls_password_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_shadow_tls_password_is_not_relayed") else {
        return;
    };
    let bound = std::time::Duration::from_secs(15);
    for version in [2u8, 3] {
        let dir = tempfile::tempdir().unwrap();
        let fixture = TlsFixture::new(&["127.0.0.1", SITE]);
        let (sb, site) =
            shadow_tls_in_front_of_trojan(&bin, dir.path(), &fixture, version, 0).await;
        let echo = echo_server().await;
        let out = outbound(
            &profile(sb.port(0), version, "an0ther"),
            "T",
            Some(&fixture),
        );
        let err = tokio::time::timeout(
            bound,
            out.connect_tcp(&target(echo), &ConnectOpts::default()),
        )
        .await
        .expect("refused within the bound")
        .err()
        .expect("sing-box relays a stranger to the site, never to trojan");
        let text = err.to_string();
        assert!(!text.contains("an0ther"), "{text}");
        if version == 3 {
            // v3 notices during the handshake, and leaves like a visitor
            assert_eq!(text, "shadow-tls: the server did not authenticate itself");
            assert!(site.received().starts_with(b"GET / HTTP/1.1\r\n"));
        }
        assert!(
            matches!(
                err,
                OutboundError::Proxy(_) | OutboundError::Tls(_) | OutboundError::Io(_)
            ),
            "{err:?}"
        );
    }
}
```

为什么 v2 的伪装站点不发 session ticket、v3 的发两张：见 P19。

- [ ] **Step 5: 运行**

```bash
cargo clippy -p rurge-interop --all-targets -- -D warnings
cargo test -p rurge-interop
```

Expected: 本机没有 sing-box——两条新用例各打印一行 `skipping …` 后通过；其余与 Task 6 之后相同。**不要安装 sing-box 来"验证"它们。**

- [ ] **Step 6: 门禁与提交**

```bash
git add tests/interop Cargo.lock
git commit -m "test(interop): sing-box 的 shadowtls 入站（v2 / v3）前置 trojan 的用例；夹具的渲染与安全守卫覆盖新入站"
```

---

### Task 9: 文档

代码到此不再变化。每一处与 Surge 的行为差异、每一条新的错误文本、每一处与设计文档文字不同的决定，都要落到文档里。中文；引用的错误文本与代码里的逐字一致（改之前先 `grep` 代码确认）。

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`、`docs/api/phase1.md`、`docs/api/phase2.md`
- Modify: `README.md`（中英两段）、`CLAUDE.md`
- Modify: `docs/acceptance/phase2-manual.md`、`tests/interop/README.md`
- Modify: `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`
- Modify: 本计划文件末尾的两张表

**Interfaces:**
- Consumes: Task 1 – 8 的最终行为；各任务报告里记下的、与本计划文字不同的地方。
- Produces: 无代码接口。

- [ ] **Step 1: 兼容性清单 `docs/surge-compatibility-matrix.md`**

4.4 节表格的最后四行（`shadow-tls-password`、`shadow-tls-sni`、`shadow-tls-version`、约束）的"备注"列现在是空的，整行替换为：

```markdown
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
| `shadow-tls-sni` | 主机名；v3 必填 | ✅ | 2 | M2c 已实现：必须是 DNS 名（IDN 写成 `xn--` 形式；IP 字面量 `E0018`）；v3 缺它 `E0018`。v2 不写时**不发 SNI**（手册），证书按策略自己的 TLS 会用的名字校验——`sni` 写了名字用它，否则用服务器主机名；所以服务器写成 IP、伪装站点是别人的网站时必须写它，否则握手以 `shadow-tls: the camouflage handshake failed: …` 失败 |
| `shadow-tls-version` | `2` / `3`；默认 2 | ✅ | 2 | M2c 已实现；其它取值 `E0018`。**v3**：用 stock rustls 两遍构造 ClientHello（SessionID 后 4 字节是对 ClientHello 自身的 HMAC）；伪装握手只提供 X25519 一个密钥交换组、关闭会话恢复；要求伪装站点支持 TLS 1.3（否则 `shadow-tls: the handshake server does not support TLS 1.3`）；服务端没有通过验证（口令不对，或对端根本不是 Shadow TLS v3 服务端）时，像普通访客那样经真实会话向伪装站点发一个 HTTP 请求、读完应答后离开，再以 `shadow-tls: the server did not authenticate itself` 失败（多出的时间以 2 秒为限）。**v2**：口令错误在握手期分辨不出，表现为第一次读取时的 `shadow-tls: the handshake server closed the session`；服务端靠"首帧到达时自己已转发的字节"比对摘要，首帧越晚越可能失败——rurge 的首帧在握手完成后立即发出，只有不带 TLS 的 `vmess` 会等最多 100 ms（请求头的合并窗口）。**两个版本**：每帧负载 ≤ 16384 字节，读取接受 16 位长度字段能表示的任何长度（sing-box 的帧可以大过一条 TLS 记录）；收到的 alert 记录跳过，流的结束以 TCP 连接结束为准（参考客户端把 alert 当作读方向的结束；sing-box 服务端对半关闭回 alert 之后仍可能继续发数据，照那样做会截断下行）；rurge 自己从不发 alert 记录 |
| 约束：Shadow TLS 不能与 TUIC / WireGuard / Tailscale / 其他 QUIC 类协议组合 | 配置错误 | ✅ | 2 | 判断函数已落地（`tuic` `tuic-v5` `hysteria2` `masque` `wireguard` `tailscale`：`` E0018 Shadow TLS cannot be combined with a `<type>` policy ``）；这些协议的 spec 出现之前该错误不会真的报出——它们的整行目前都还没有被读取 |
```

10.4 节 `GET /v1/profiles/current?sensitive=0` 那一行：内联参数名单的末尾 `` `ws-headers` / `ws-path` `` 之后追加 `` / `shadow-tls-password` ``。

`W0029`（"解析但尚未生效"）的说明里如果列举了 Shadow TLS 的三个参数，删掉它们并注明"M2c 起生效"（先 `grep -n "shadow" docs/surge-compatibility-matrix.md` 确认还有没有别处提到）。

- [ ] **Step 2: API 参考**

`docs/api/phase1.md`：`GET /v1/profiles/current` 那一行的内联参数名单里，`ws-path` 之后加上 `shadow-tls-password`。

`docs/api/phase2.md`：
1. `GET /v1/policies/detail` 一节与 `lineHash` 一节里列举被脱敏字段的两处（`… / `ws-headers` / `ws-path` 等参数`、`…`headers=`、`ws-headers=`、`ws-path=` 等…`），各加上 `shadow-tls-password`。
2. 「会话日志里的出站错误文本」一节的第一句，把"trojan、vmess（± WebSocket）与 anytls 出站失败时"改成"trojan、vmess（± WebSocket）、anytls 出站，以及任何带 Shadow TLS 的出站失败时"。
3. 在「WebSocket（trojan、vmess 共用）」小节之后新增：

```markdown
### Shadow TLS（所有 TCP 类出站共用）

写了 `shadow-tls-password` 的策略，连接先经过 Shadow TLS 一层；这一层的失败带 `shadow-tls:` 前缀。它发生在策略自己的 TLS 之下，所以转发期间才出现的那几条，在带 TLS 的协议上会被里层再包一次（例如 `tls: shadow-tls: the handshake server closed the session`）。

| 错误文本 | 何时出现 |
| --- | --- |
| `shadow-tls: the camouflage handshake failed: <原因>` | 与伪装站点的 TLS 握手失败：证书不被信任、证书与用来校验的名字对不上（没写 `shadow-tls-sni` 而服务器写的是 IP 时最常见）、对端根本不说 TLS 等。`<原因>` 是 rustls 的错误文本，已去除控制字符且截到 200 个字符 |
| `shadow-tls: the server closed the connection during the handshake` | 伪装握手还没完成，对端就关闭了连接 |
| `shadow-tls: the handshake server does not support TLS 1.3` | 仅 v3：伪装站点选了 TLS 1.2。握手照常完成、向站点发一个 HTTP 请求之后才以这条文本失败 |
| `shadow-tls: the server did not authenticate itself` | 仅 v3：握手期的记录没有带上正确的 HMAC——口令不对，或对端不是 Shadow TLS v3 服务端（连接被转给了伪装站点本身）。同样先像普通访客那样离开，再失败 |
| `shadow-tls: cannot sign the ClientHello` | 仅 v3：两遍构造 ClientHello 的某条自检不满足（只可能出现在 rustls 升级之后）；加载配置时就会以 `E0022` 报出，连接期不会带着错误的 HMAC 发出任何字节 |
| `shadow-tls: the handshake server closed the session` | 仅 v2，出现在第一次读取时：服务端一直没有切到数据阶段，伪装站点结束了会话。**v2 的口令错误就是这一条**——握手本身分辨不出口令对错 |
| `shadow-tls: a record cannot be authenticated` | 仅 v3，转发期间：一条数据记录的 HMAC 不对 |
| `shadow-tls: unexpected record type` | 转发期间：收到既不是数据也不是 alert 的记录 |
| `shadow-tls: the connection ended in the middle of a record` | 转发期间：连接在一条记录中间断开（在两条记录之间断开是正常的流结束） |

口令、由它派生的 HMAC 与异或密钥不会出现在任何一条文本里。
```

- [ ] **Step 3: `README.md`**

中文状态段：把"M2b（VMess / AnyTLS）已完成——…不影响正在使用的连接池；其余出站协议（Shadow TLS 等）与…"里的"；其余出站协议（Shadow TLS 等）与"改成"；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；其余出站协议与"。

英文状态段同一位置：把 "; the remaining outbound protocols (Shadow TLS and so on), the group algorithms" 改成 "; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); the remaining outbound protocols, the group algorithms"。

特性表 / 路线图里如果有 Shadow TLS 或 M2 的状态格，一并更新（`grep -n "Shadow\|M2" README.md`）。README 必须与 PRD 保持一致：只改状态，不改范围。

- [ ] **Step 4: `CLAUDE.md`**

1. 「当前状态」一段末尾的"M2c（Shadow TLS）尚未开始。"替换为：

```markdown
M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。
```

2. 同一段里"M2（TLS 族）按三份计划推进：…M2b（VMess / AnyTLS）与 M2c（Shadow TLS）尚未开始"之类的旧说法如果还在，按实际状态改掉。
3. 「先读这些文档」：在 M2a 计划那一条之后加入 M2b 计划（如果还没有）与本计划：

```markdown
- `docs/superpowers/plans/2026-09-21-phase2-m2c-shadow-tls-plan.md`：阶段 2 / M2c（Shadow TLS v2 / v3）实施计划（9 个任务）。开头「计划期决定」表（P1–P20）记录对照参考实现与手册核对出的逐字节细节，以及与设计文字不同的决定（不写 `shadow-tls-sni` 时不发 SNI、alert 记录跳过、读取不设 16 KiB 上限、v2 握手后仍在转发的记录先交给伪装会话等）；末尾「执行期修正记录」与「延后事项」两张表。
```

4. 「常用命令」：`cargo test -p rurge-engine --test outbounds` 那一行之后加：

```bash
cargo test -p rurge-engine --test outbounds_shadow_tls   # 经 Shadow TLS 的端到端用例（回环假服务端 + 夹具自己的伪装站点）
```

- [ ] **Step 5: 手工验收清单 `docs/acceptance/phase2-manual.md`**

末尾新增一节（沿用文件里已有各节的写法与勾选框格式）：

```markdown
## M2c　Shadow TLS

需要一台真实的 Shadow TLS 服务端（官方 `shadow-tls` 或 sing-box 的 `shadowtls` 入站）与一个真实的伪装站点，自动化测试（只用回环）覆盖不了。

- [ ] v3：`<协议>, <服务器>, <端口>, …, shadow-tls-password=…, shadow-tls-version=3, shadow-tls-sni=<伪装站点>`，经它打开几个 HTTPS 网站、下载一个 100 MB 以上的文件、保持一条长连接 10 分钟以上，均正常。
- [ ] v3：服务端分别是官方 `shadow-tls`（`--v3 --strict`）与 sing-box（`strict_mode: true`）时都成立。
- [ ] v3：口令写错——会话记录的错误是 `shadow-tls: the server did not authenticate itself`；在服务端一侧抓包可见客户端与伪装站点完成了握手并发了一个 HTTP 请求。
- [ ] v3：`shadow-tls-sni` 指向一个只支持 TLS 1.2 的站点——错误是 `shadow-tls: the handshake server does not support TLS 1.3`。
- [ ] v2：同样的三项（浏览、大文件、长连接）；服务器以 IP 配置而不写 `shadow-tls-sni` 时，错误文本明确指向证书与名字对不上。
- [ ] v2：伪装站点是自己的域名（服务端把握手转给同一个域名的真实站点）且不写 `shadow-tls-sni`：连接成功，服务端一侧抓包可见 ClientHello 里没有 SNI。
- [ ] 半关闭：`curl --http1.0` 之类"发完请求就关闭写方向"的客户端经 sing-box 服务端下载一个大文件，内容完整（对应清单 4.4 里"alert 记录跳过"那一条）。
- [ ] `underlying-proxy`：带 Shadow TLS 的策略作为链的出口。
- [ ] `GET /v1/policies/detail` 与 `GET /v1/profiles/current` 里 `shadow-tls-password` 的值是 `***`。
```

- [ ] **Step 6: `tests/interop/README.md`**

1. 第一段列举被驱动的出站处，加上"以及包在 Shadow TLS 里的 `trojan`"。
2. 「本地运行」里"sing-box 的九个互操作用例"改成"sing-box 的十一个互操作用例"（`sing_box.rs` 的 5 个用例里有 1 个是夹具自检，互操作用例是 `sing_box.rs` 4 + `sing_box_tls_family.rs` 5 + `sing_box_shadow_tls.rs` 2 = 11；写之前先数一遍）。
3. 「覆盖范围」：把"`tests/sing_box.rs` 驱动的九个用例覆盖"改成三个文件的说明，并新增一条：

```markdown
- Shadow TLS（`tests/sing_box_shadow_tls.rs`）：sing-box 的 `shadowtls` 入站（v2 与 v3，v3 开 `strict_mode`）把握手转发给夹具自己在回环上起的 TLS 服务端（伪装站点，证书由夹具的 CA 签发），解出来的流量经 `detour` 交给同一个 sing-box 里的 `trojan` 入站；覆盖小负载与跨多帧的往返，以及口令错误（v3 的会话文本、伪装站点确实收到了那个 HTTP 请求）。v2 的用例让伪装站点不发 session ticket（sing-box 对"首帧之前又转发了字节"只多容忍一次写），v3 的发两张（覆盖数据阶段开头的残留记录）。
```

4. 「安全约束」第一条里补一句：`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。

- [ ] **Step 7: M2 设计文档 `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`**

直接改正文的三处：
1. 4.1 的代码块里 `ShadowTlsOpts` 的 `password: String` 改成 `password: Secret<String>`。
2. 5.3 的 **v2** 一句：把"SNI 取 `shadow-tls-sni`（没写则用策略的有效 SNI 名；未核对，登记）"改成"SNI 取 `shadow-tls-sni`；没写则**不发 SNI**（手册），证书按策略自己的 TLS 会用的名字校验（`sni` 写了名字用它，否则用服务器主机名）"；同一句末尾"之后是普通帧"后面补"；服务端切到数据阶段之前仍在转发的记录（session ticket）先交给伪装会话，它解不开的第一条记录起才是数据"。
3. 13 节"4.4 Shadow TLS 三行"那一格：把"`shadow-tls-sni` 缺省取策略的有效 SNI（v2，未核对）"改成"v2 不写 `shadow-tls-sni` 时不发 SNI（手册）；alert 记录跳过；读取不设 16 KiB 上限"。

第 15 节 V7、V8 两行的"事项"末尾各加"（已核对：M2c 计划 P1 – P7、P9）"。

在第 18 节之后、附录 A 之前新增第 19 节：

```markdown
## 19. M2c 实施期的订正

本节登记 M2c 计划的「计划期决定」里与本文件文字不同的地方。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-21-phase2-m2c-shadow-tls-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P2 | 5.3："握手后客户端的第一帧带 8 字节摘要前缀，之后是普通帧" | 补一段：服务端在看到首帧之前一直在转发伪装站点，TLS 1.3 的 session ticket 就落在这段时间里。客户端把收到的记录先交给伪装会话，它解不开的第一条起才是数据；伪装会话自己结束（alert / `close_notify`）→ `shadow-tls: the handshake server closed the session`，v2 的口令错误就表现为这一条 |
| P4 | 5.3："按参考实现的做法体面收尾" | 具体动作：握手照常完成 → 经真实会话发一个格式正确、长度随机的 `GET / HTTP/1.1`（参考实现用的是裸 LF 且头部没有结尾空行，不照抄）→ `close_notify` → 读到对端关闭，整段以 2 秒为限。两种拒绝原因（不是 TLS 1.3、没通过验证）共用。sing-box 的客户端不做这一步。伪装握手失败时先把 rustls 排队的 alert 发出去 |
| P5 | （无对应文字） | 收到的 alert 记录跳过，流的结束以 TCP 连接结束为准；rurge 从不发 alert 记录。与两个参考客户端不同：sing-box 服务端对客户端的 FIN 回 alert 之后另一方向可能还在发数据 |
| P6 | 5.3："单条记录的上限按 TLS 规范（16 KiB + 扩展余量）" | 只对握手期成立（由 rustls 执行）。数据阶段写 ≤ 16384 字节负载 / 帧，读接受 16 位长度字段能表示的任何长度：sing-box 的 `WriteBuffer` 不切分 |
| P7 | 5.3："SNI 取 `shadow-tls-sni`（没写则用策略的有效 SNI 名；未核对，登记）" | 手册："If not set, no SNI is sent." 不写就不发 SNI；证书照常校验，名字取策略自己的 TLS 会用的那个。`shadow-tls-sni` 只接受 DNS 名。已直接改写 5.3 正文 |
| P8 | 第 3 节新依赖表：`hmac` `sha1` | 不引入：`ring::hmac`（`HMAC_SHA1_FOR_LEGACY_USE_ONLY`）与已有的 `sha2`，零新增条目 |
| P9 | 附录 A：spike 只开 TLS 1.3、用 `Box::leak` | 伪装握手的配置是 TLS 1.2 + 1.3（TLS 1.2 的站点能把握手走完以便体面收尾）；脚本化的两件是 `static` 单元结构体；线程局部状态由 `Drop` 守卫复位；`ShadowTlsClient::build` 对 v3 先演一遍 |
| P11 | 4.1：`password: String` | `Secret<String>`（M2b 的约定）。已直接改写 4.1 |
| P12 | 4.2 末句："M2c 起生效，`W0029` 对它们退役" | 配置层分两步交付：类型与公开的读取函数先到，`to_spec` 的调用、`PolicySpec.shadow_tls` 与 `W0029` 的退役同"出站真的会用这一层"在一个提交里，避免出现"配置不再警告、连接却不带 Shadow TLS"的中间状态 |
| P16 | 5.3："测试与互操作经已有的 `EngineFactory::with_roots` 注入自己的根" | 互操作如此；引擎的端到端夹具经 `Runtime::build` 构建，拿不到工厂。`EngineShared` 增加 `roots`（`None` = 系统根），`Runtime::build` 据此选 `with_roots`——根证书库本来就必须随引擎存续而不随代际变化 |
| P17 | （无对应文字：夹具） | `TlsFixture` 的 TLS echo 写完即 `flush`（否则大负载偶发卡死，与 M2b P16 同一类）；假服务端的数据阶段是两个互不等待的循环 |
| P19 | （无对应文字） | v2 的固有局限：首帧越晚，服务端越可能已经转发了 ticket 而比对失败。rurge 的首帧在握手完成后立即发出，只有不带 TLS 的 `vmess` 会等 `LazyHead` 的 100 ms |

实施中发现的新出入由各任务追加。
```

- [ ] **Step 8: 本计划末尾的两张表**

把各任务报告里"与计划文字不同的地方"逐条写进「执行期修正记录」（任务号、计划原文、实际做法、原因、提交），没有就写"无"；把评审留下的 Minor 与不属于 M2c 的发现写进「延后事项」。

- [ ] **Step 9: 门禁与提交**

文档任务同样跑完整门禁（确认没有误改代码）。

```bash
git add docs README.md CLAUDE.md tests/interop/README.md
git commit -m "docs: M2c——兼容性清单、API 参考、README、CLAUDE.md、手工验收、互操作 README、M2 设计第 19 节与计划收尾表"
```

提交之后把这条提交的 SHA 回填进「执行期修正记录」里自己那一行，再提交一次（`docs: 回填 Task 9 收尾表里自己那条提交的 SHA`）；不 amend。

---

## 验收对照（M2 设计第 12 节里属于 M2c 的条目）

| 设计的验收标准 | 由谁证明 |
| -------------- | -------- |
| 2. Shadow TLS v2、v3 各对回环假服务端转发通过 | Task 4 的 `v2_carries_…` `v3_carries_…` `v2_works_over_a_tls_1_2_handshake_too` `the_policy_s_own_tls_runs_inside_the_frames`；Task 5 的阶梯与 `http` / `socks5-tls` 用例；Task 7 的六条端到端用例 |
| 2. …与 sing-box 的 shadowtls 入站转发通过 | Task 8 的两条用例——**只在首次推送后的 CI 上运行** |
| 2. 附录 A 的自检有用例钉住 | Task 3 的三条 `sign::tests` 与 Task 4 的 `a_name_rustls_cannot_use_…v3_checks_itself_at_build_time` |
| 1.（补）`vmess`（无 `tls`）与 `anytls` 经 Shadow TLS | Task 7 的 `vmess_without_tls_and_anytls_run_inside_shadow_tls_too` |
| 3. 六个 TLS 参数只作用于里层 TLS | Task 5 的 `without_shadow_tls_sni_…`（校验名的来源）与 `shadow_tls_sits_below_tls_and_websocket`（伪装握手与里层握手各自的 SNI）；Task 7 第 1 条（里层用指纹钉扎、伪装握手用注入的根） |
| 4. 能作链的入口与出口 | Task 7 的 `shadow_tls_runs_through_an_underlying_proxy` |
| 5. 重载复用 | Task 7 的 `the_layer_is_part_of_what_a_reload_compares_and_of_what_the_api_hides` |
| 7. 凭据不外泄 | Task 1（诊断不回显口令、脱敏）、Task 4（错误文本）、Task 7（会话记录、`policy_detail`）；各类型不实现 `Debug` |
| 8. fmt / clippy / 全部测试 | 每个任务结尾的门禁；CI 在首次推送后 |
| 9. 需要真实节点的项目 | Task 9 写进 `docs/acceptance/phase2-manual.md` |
| 4.2 的校验表（M2c 两行） | Task 1 的四条用例；Task 5 的 `shadow_tls_is_part_of_the_spec_of_every_tcp_protocol` |
| 4.4 脱敏 | Task 1 Step 1 – 3（在旧名单上取过 RED） |

## 执行期修正记录

执行时填写：与本计划文字不同的每一处。

| 任务 | 计划原文 | 实际做法 | 原因 | 提交 |
| ---- | -------- | -------- | ---- | ---- |
| 4 | `wrap_v3` 里 `stream.write_all(&hello).await?;` 写出签名过的 ClientHello，后面没有 `flush` | 加一行 `stream.flush().await?;`；新增回归用例 `the_handshake_does_not_hang_on_a_stream_that_only_forwards_bytes_on_flush`（测试专用类型 `FlushGated`，只在 `poll_flush` 时才把已写字节转发到底层） | 评审发现（Important，计划强制）：`sign::signed_hello` 已经把 rustls 内部缓冲榨干，紧随其后的 `Handshake::step` 第一次调用 `Handshake::send` 时 `conn.wants_write()` 是 `false`，不会再发送或顺带 flush 这笔 hello；对一个把"写"缓冲到自己内部、只在显式 `flush` 时才真正转发字节的流（如 `underlying-proxy` 一跳的 `tokio-rustls`，或 vmess 的 `LazyHead`），ClientHello 会一直卡在上一层缓冲区里，握手挂起直到调用方自己的超时——违反项目"写完即 flush"的规矩（M2b P16 / 本计划 P17） | 2934ea5 / 123bb78 |
| 5 | dispatch 预期"`rurge-proto` lib 160 passed（Task 4 之后的 156 + 4 条新用例）" | 实际 161 passed（157 + 4） | Task 4 的修复轮在 `wrap_v3` 里新增了一条回归用例 `the_handshake_does_not_hang_on_a_stream_that_only_forwards_bytes_on_flush`，把 Task 4 之后的基数从 156 提到 157；dispatch 给 Task 5 的数字已按 157 + 4 = 161 调整过，与本计划文字里仍写着的 160 不一致 | 67a0a2e |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | 每 8 KiB 一次 `flush` 在 TLS / WS / VMess / Shadow TLS 栈上的成本没有测过（只在裸 TCP 上做过 A/B） | M8 |
| 2 | 首次推送后盯住互操作 job 的 Shadow TLS 用例：① v2 对 sing-box（`LastSum` 只多容忍一次写——用例已让伪装站点不发 ticket，仍失败就是别的原因）；② v3 的大负载往返（sing-box 的 `verifiedConn.WriteVectorised` 先回喂 4 字节再取摘要，与它自己的 `write` / `WriteBuffer` 不一致——如果 sing-box 在服务端到客户端方向走到那条路径，客户端会报 `a record cannot be authenticated`；这是 sing-shadowtls 的问题，出现时记录并上报，不在 rurge 里迁就）；③ v3 口令错误用例里"伪装站点收到 HTTP 请求"的断言依赖 sing-box 把未通过验证的连接原样转给伪装站点 | 首次推送之后 |
| 3 | `vmess`（不带 TLS）+ Shadow TLS v2：首帧要等 `LazyHead` 的 100 ms，是 v2 协议下最容易输掉摘要竞争的组合。需要的话可以让 `LazyHead` 在下层是 Shadow TLS v2 时不等 | 有用户报告再说 |
| 4 | HelloRetryRequest：伪装握手只提供 X25519，站点要求别的组时握手失败（设计附录 A 的已知限制）；两遍构造不覆盖第二个 ClientHello | 有用户报告再说 |
| 5 | M2b 计划「延后事项」里不属于 M2c 的条目（`environment()` 仍是人工维护的约定、`ws-path` / `headers` 不是 `Secret`、偶发的 `STATUS_ACCESS_VIOLATION` 未查明根因等）原样继续有效 | 见 M2b 计划 |
| 6 | 崩溃恢复顺序缺陷（`rurge run` 在坏配置上先退出、后 `sysproxy.recover()`）：与 M2c 无关，等项目所有者点头后单独做一份小设计 | 单独跟进 |
| 7 | 调用方若在一次 `Pending` 写与它的重试之间插入一次 `flush`，会让被搁置的帧被发送两次（约定与 `VmessStream` 一致：调用方不该这么做；`tokio` / `tokio-rustls` 自己不会） | 补一句文档；有用户报告再说 |
| 8 | 没有测试覆盖零长度的 v3 ApplicationData 记录（`[23,3,3,0,0]` → "cannot be authenticated"） | 有用户报告再说 |
| 9 | `hello_tag` 在固定偏移处切片，没有写明前提条件（调用方：`signed_hello` 在 `well_formed` 之后调用；Task 4 的假服务端先检查长度 ≥ 76） | 补一句文档；有用户报告再说 |
| 10 | 自检 3 与自检 4 没有失败路径的测试（只有 rustls 升级才能触发） | rustls 升级时一并覆盖 |
| 11 | 一个"握手期部分验证通过、随后被当作伪装流量"的 v3 握手直接在 rustls 里失败（`the camouflage handshake failed`），不会走"体面收尾" | 有用户报告再说 |
| 12 | 假服务端的 `upwards` 循环经 `?` 提前返回时会跳过 `stop.notify_one()`，导致夹具里的 `downwards` 任务泄漏 | 测试夹具专用缺陷，不影响生产代码；下次改动这个夹具时顺带修 |
| 13 | Task 4 报告写"去掉三个 `#[allow(dead_code)]`"，实际去掉了四个（brief 原文写的是三个） | 已在此更正；无需后续动作 |
| 14 | 测试专用类型 `FlushGated::poll_shutdown` 不会先把挂起的缓冲写出（目前这条路径未被使用） | 有用户报告或复用该类型时再说 |
