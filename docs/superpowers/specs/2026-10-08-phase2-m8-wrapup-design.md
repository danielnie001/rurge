# 阶段 2 / M8「收尾与验收」细化设计

细化阶段 2 总设计（`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`）第 1.4 节的 M8 里程碑、第 14 节的验收标准与 M7 细化设计的 M7-D12；与总设计不一致处以本文为准。阶段 2 各份计划「延后事项」里标给 M8 的条目在本文第 7 节逐条定去向。

调研只读（2026-10-08）：Surge 手册（2026-10-07 版）、知识库与博客，RFC 8484 / 9297 / 9298，shadowsocks 的流式加密规范，v2fly 的 VMess 协议文档，shadowsocks-rust / sing-box / Xray / mihomo / sing-snell / opensnell / anytls-go / tailscale / tailscale-rs / headscale / quinn / h3 的源码与发布记录，crates.io 的元数据；本机的 `~/.cargo/registry` 源码。没有下载或安装任何二进制。项目所有者的授权（2026-10-06）：决定由实施者自己做，不逐项询问；本文第 2 节的每一条都可以事后改。

## 1. 目标与范围

阶段 2 的协议与策略组在 M1 ～ M7 做完了。M8 把阶段收住：让三平台的 CI 第一次真正转绿，补上 P2 的协议项，把一路登记下来的遗留事项逐条定去向——做掉、关闭或移交——，写出可行性报告与阶段验收文档。

| 项目 | 本里程碑 | 不做 / 去向 |
| ---- | -------- | ----------- |
| CI | 测试一步的失败摘要公开可读；首次真正跑到测试的 CI 上暴露的失败逐个修到三平台全绿（含互操作） | — |
| 凭据不外泄 | `[General]` 的诊断、`redact_profile`、配置对象的 `Debug`、DNS 上游 URL 的几处回显；DoH URL 里的用户名口令改为真正起认证作用 | — |
| P2 协议项 | Shadowsocks 的 11 种流式方法；Snell v1 ～ v3 | VMess 的旧握手（不实现，改用 AEAD 握手）；手册没列的 9 个 `ss` 方法名；Snell v6 与 v5 的 QUIC Proxy Mode（第 6.1 节） |
| `W0007` 与 `OutboundError::Unsupported` | 不退役，改定义为"rurge 不实现的协议或其变体"（第 4.4 节） | — |
| QUIC 族 | 不设流数上限的出站的接收窗口；MASQUE 经链式载体时的 UDP 余量；载体错误的几处不一致 | 两份单飞会话的合并（第 7 节） |
| DNS | DoH 没写路径时的缺省、`Age` 头 | 被放弃的内部拨号不留记录（第 7 节） |
| 策略组 | `smart` 的失败计分不再被失效的目标拖累 | — |
| 其余出站 | AnyTLS 的 SYNACK 看门狗；WireGuard 的几处收尾；订阅里指向构建失败成员的中继；出站一侧的 IDN | 规则、`[Host]` 与解析器的 IDN（阶段 3 之后） |
| 阶段 1 遗留 | 坏配置时也恢复崩溃留下的系统代理；布尔环境变量接受 `1` / `0` 等写法 | — |
| 性能 | 转发循环的 flush 与缓冲大小实测，按结果定 | 订阅重建的开销（有数据再说） |
| 报告与文档 | Snell v6 / v5 QUIC Proxy Mode / Tailscale 的可行性报告；阶段验收清单；遗留事项总表；兼容性清单、README、PRD、CLAUDE.md 同步；`CORE_VERSION` | 手册基线从 Mac 6.9 / iOS 5.22 更新到 6.10 / 5.23 的系统比对（阶段 3 开工前单独做） |

对应需求：FR-OUT-06 / 15，以及总设计第 14 节的九条验收标准。验收：第 14 节按本文第 6.2 节订正后的九条全部满足。

## 2. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M8-D1 | 拆分与顺序 | 三份计划：M8a（CI 转绿与凭据不外泄）→ M8b（P2 协议项与 `W0007` 的归宿）→ M8c（遗留事项、报告与验收）。M8a 排第一：CI 的每一轮都要项目所有者推送，越早让失败可读越好；凭据的修正与协议无关、最不该再拖 |
| M8-D2 | 计划的写法 | M8 的任务大多是彼此独立的小修正，计划写到"改哪里、要哪些用例、判据是什么"为止，不再像协议里程碑那样先在副本上把全部代码做一遍再转写；每个任务照旧先写失败的用例、再实现、再评审，执行中的出入记进计划的「执行期修正记录」 |
| M8-D3 | CI 的失败怎样读到 | 任务日志只有登录后才能读，注解与任务摘要公开可读：测试一步跑完全部二进制（`--no-fail-fast`），失败时由仓库里的脚本把失败用例输出的开头、异常退出的二进制、编译错误、没跑完的用例发成注解并写进摘要（第 3.1 节） |
| M8-D4 | 偶发的 `STATUS_HEAP_CORRUPTION` | 不是 rurge 或其依赖的缺陷：来源是开发机上终端管控软件向每个进程注入的钩子 DLL（第 3.5 节）。关闭这一串延后事项，本机门禁遇到照旧重跑；GitHub 的 Windows runner 是干净的对照 |
| M8-D5 | DoH URL 里的用户名口令 | 以 `Authorization: Basic` 发出（RFC 7617），并从请求的目标里去掉；`h3://`、`quic://`、`tls://`、`tcp://` 照旧拒绝（第 3.3 节） |
| M8-D6 | 布尔环境变量 | `RURGE_WATCH`、`RURGE_SYSTEM_PROXY`、`RURGE_EMPTY_GROUP_REJECT`、`RURGE_NO_NETWORK` 接受 `1` / `0`、`true` / `false`、`yes` / `no`、`on` / `off`；别的取值是错误（M3a 延后事项 #5，原留给项目所有者） |
| M8-D7 | Shadowsocks 的流式方法 | 实现 Surge 手册今天列出的 11 种（TCP 与 UDP）；rurge 另认得的 9 个名字（出自 libev 的表、手册没有）照旧不支持。不新增 crate：`chacha20` 与 `salsa20` 已在依赖树里，CFB、CTR 与 RC4 手写并用标准向量钉住。用了流式方法的策略告警 `W0031`（新）（第 4.1 节） |
| M8-D8 | VMess 的旧握手 | 不实现。没写 `vmess-aead=true` 的行不再拒绝，改用 AEAD 握手并告警 `W0029`：各家服务端都已弃用或删除旧握手，而 2020-09 之后的服务端都收 AEAD（第 4.2 节） |
| M8-D9 | Snell v1 ～ v3 | 实现：Surge 自带的 Snell 服务端说的是 v1，策略行的 `version` 缺省也是 1；记录层是已有的 Shadowsocks AEAD 分块，成本低。参照是 mihomo v1.19.32 的 `snell` 监听端（三平台，CI 新装）（第 4.3 节） |
| M8-D10 | `W0007` 与 `OutboundError::Unsupported` | 不退役，改定义为"rurge 不实现的协议或其变体"（`tailscale`、`snell` 的 `version=6`、手册没列的 `ss` 方法名）；`Unsupported` 另承担"这个出站不载 UDP"的说明，本来就不会退役。订正 M7-D12 与验收标准第 8 条（第 4.4、6.2 节） |
| M8-D11 | 不设流数上限的 QUIC 会话的接收窗口 | 每条连接 256 MiB（32 × 每流 8 MiB），每流窗口不变（第 5.1 节） |
| M8-D12 | MASQUE 经链式载体 | 连接从 1350 字节的包起步，带不动时由 quinn 的黑洞检测退回 1200；直连与其它 QUIC 出站不变（第 5.2 节） |
| M8-D13 | `smart` 的失败计分 | 同一站点在记忆期内已经经另一个成员失败过时，失败只记进站点记忆；本地就拒绝的目标不计入、不换成员（M3c 延后事项 #11 的 (b) 与 (c)，原留给项目所有者）（第 5.5 节） |
| M8-D14 | AnyTLS 的 SYNACK 看门狗 | 照参考客户端做：3 秒，对端协议版本不低于 2、非会话的第一条流；到时关闭整条会话（第 5.6 节） |
| M8-D15 | IDN | 只做出站一侧：策略行的 `server` 转成 A-label，`ssh` 与 `wireguard` 的隧道内 DNS 对目标名同样处理；规则、`[Host]` 与解析器留到阶段 3 之后（第 5.9 节） |
| M8-D16 | 坏配置时的系统代理 | `rurge run` 因配置加载不了而退出之前，先恢复上一次崩溃留下的系统代理设置；退出码不变（第 5.10 节） |
| M8-D17 | 转发循环的 flush 与缓冲 | 先实测再定：某种取法在 TLS、WebSocket、VMess 三种栈上都至少快 10% 且没有一种变慢才换（第 5.11 节） |
| M8-D18 | `CORE_VERSION` | 22（第 6.3 节） |
| M8-D19 | Snell v6、v5 的 QUIC Proxy Mode、`tailscale` | 阶段 2 都不实现；结论与出路写成可行性报告（第 6.1 节） |
| M8-D20 | 手册基线 | 仍是 2026-09 版（Mac 6.9 / iOS 5.22）；与 2026-10-07 版（6.10 / 5.23）的系统比对留到阶段 3 开工前（第 6.4 节） |

## 3. M8a：CI 转绿与凭据不外泄

### 3.1 CI 的失败摘要

背景：M6 的复查把 CI 钉在 1.89 之后，2026-10-08 的一次推送（0154e73）让 CI 第一次跑到测试——三个平台的格式检查、clippy 与全部参照服务端的安装都通过了，`cargo test` 一步都失败；而公开可读的只有"退出码 101"这一条注解，任务日志要登录才能读。

- 测试一步改为 `cargo test --workspace --no-fail-fast`，输出同时存进 `$RUNNER_TEMP/test.log`；限时 60 分钟（`rurge-dns` 的用例有过空转数小时的记录，M3a 延后事项 #20）。一个二进制失败不再挡住后面的：互操作测试排在最后，一次运行要给出全部结果。
- 这一步失败时运行 `.github/scripts/report-test-failures.py`：从日志里取出每个失败用例输出的开头（至多 60 行 / 3000 字符）、异常退出的测试二进制与它的退出原因、第一个二进制开始之前的编译错误、日志结束时还没有结果的二进制与其中报过"已运行超过 60 秒"而没有完成的用例，各发成一条注解——GitHub 每一步每个级别留 10 条，依次用 `error`、`warning`、`notice`，装不下的在最后一条里列出名字——，并写进任务摘要。脚本自己总是成功退出：任务已经失败，报告做不出来不该再盖上一层失败。
- 只在第一个二进制之前认编译错误：`rurge` 的用例会打印同样形状的诊断（`error[E0022]: …`）。

### 3.2 CI 首跑的修正

失败的用例要等带着 3.1 的提交推送之后才读得到；届时逐条定因、先补失败用例（本机能复现的）再修，记进 M8a 计划的「执行期修正记录」。已知会在首跑时见分晓的事项（各计划「延后事项」里标着"首次推送后看 CI"的）一并核对：`rurge-platform` 与 `ExternalOutbound` 的 Unix 分支、`sshd` 与各参照服务端的夹具（命令行、配置、就绪日志都只照源码读出）、Linux 上 UDP 缓冲封顶在 `rmem_max`、macOS 回环 UDP 数据报 9216 字节的上限、sing-box 的 anytls 鉴权与 Shadow TLS 的三项、Snell `reuse=false` 的半关闭。

修不了的（参照服务端自身的限制）登记进兼容性清单并让对应用例跳过那一种情形，不让它留着红。

### 3.3 凭据不进诊断、`Debug` 与脱敏输出

约束（FR-OBS-01）：凭据、密钥与口令不出现在日志、错误、API 输出与 `Debug` 里。M7c 的终审与复查留下几处阶段 1 起就有的回显（M7c 延后事项 #24、#25、#34、#36），连同更早登记的几条（M2b 的 `Secret<T>` 覆盖面，M3a #19、#23、#31，M3b #21，M6a #7）一起收掉：

| 位置 | 原来 | 改为 |
| ---- | ---- | ---- |
| `[General]` 的 `W0012`：`dns-server` / `encrypted-dns-server` / `doh-server` 里认不出的项 | 整项进告警（`htps://user:pass@host/…`） | 只引用 scheme 与最后一个 `@` 之后的部分：`htps://***@host/…` |
| `http-api` / `external-controller-access` 解析不了 | 整个取值（连同 key）进告警 | 有 `@` 时只引用其后的部分，没有时不引用（那可能就是 key 本身） |
| `http-listen` / `socks5-listen` 的 `E0013` 与"不支持口令"的告警 | 引用带口令的整项 | 同上 |
| `wifi-access-http-auth` 没有 `:` | 引用取值 | 不引用 |
| `redact_profile`（`GET /v1/profiles/current` 的脱敏输出） | DNS 服务器 URL 里的用户名口令原样；`[Tailscale <名>]` 的 `auth-key` 原样；`key@address` 按第一个 `@` 切（key 里有 `@` 时露出后半截） | DNS 三个键逐项抹掉；`auth-key` 进独立成行的密钥名单；按最后一个 `@` 切，与解析器一致 |
| 配置对象的 `Debug` | `EncryptedDns`、`ControllerAccess`、`wifi_access_http_auth`、`ProxyPolicy`（`positional` / `params` / `definition`）、`KeystoreItem`、`WsOpts`、`ObfsOpts`、`CommonOpts.test_url`、原样保留的节的每一行（`Entry.raw`：`[MITM]` 的 `ca-passphrase`、`[Tailscale]` 的 `auth-key`） | 手写 `Debug`：凭据写成 `***`，原文按 `redact_profile` 的规则显示 |
| `rurge-net` 资源管理器的 `Debug` | 资源的完整 URL（订阅 token）与正文（节点的凭据） | URL 只显示到主机与端口，正文只显示长度 |
| `<random-string(…)>` 的长度不是数字 | 引用括号里的文字（请求头取值的一段） | 不引用 |
| DNS 上游 URL 的全角 `＠`（U+FF20） | 不算 `@`：带它的写法绕过"带用户名口令即拒绝"，错误文本引用口令 | 与 `@` 同样对待 |
| DNS 上游 URL 的端口不是数字 | `invalid port` 引用那一段（前面没有 `@` 可认时，那一段是口令的后半） | 不引用 |
| `https://` 的 DoH URL 里的用户名口令 | 不起认证作用：HTTP/1.1 上不发，HTTP/2 上被写进 `:authority`（RFC 9113 8.3.1 不允许） | 百分号解码后以 `Authorization: Basic` 发出，并从请求的目标里去掉（M8-D5） |

判据是一条"标记"用例（`crates/rurge-config/tests/secrets.rs`）：一份每个凭据字段都带同一个标记串的配置，加载后的 `Debug`、全部诊断与脱敏输出里都找不到标记；以后新加的凭据字段写进这份配置就被同一条用例看住。

不动的：缓存目录 `meta.json` 里资源的完整 URL（与缓存的订阅正文、配置文件本身同在使用者的数据目录里）；Windows 上"无法监视文件"告警里的目录路径（路径不是凭据）。

### 3.4 布尔环境变量

四个开关的环境变量原来只认 `true` / `false`（clap 对 `bool` 的缺省读法），`RURGE_NO_NETWORK=1` 是"invalid value"。改用 clap 的 `BoolishValueParser`：`1` / `0`、`true` / `false`、`yes` / `no`、`on` / `off`、`y` / `n`、`t` / `f`，不分大小写；别的取值照旧是错误——比"凡不是否定词都算真"稳妥，写错的值不会被悄悄当成开。

### 3.5 偶发的异常退出

现象自阶段 1 起：本机 `cargo test` 的测试二进制偶发以 0xc0000374（`STATUS_HEAP_CORRUPTION`，少数 0xc0000005）退出、没有失败用例，出现过的二进制遍及凡是联网的 crate。2026-10-08 查明：

1. 测试进程里有被注入的第三方 DLL（`tasklist /m /fi "IMAGENAME eq <测试程序>"` 看得到）：开发机上终端管控软件的七个组件，其中一个是网络抓取钩子；它们还带进来一批控制台程序本不会加载的系统 DLL（USER32、GDI32、winspool、WinUsb）。
2. 系统的应用程序事件日志（Application Error）里，同一批 DLL 让本机别的联网程序崩溃：故障模块是那个网络钩子（已卸载之后仍被执行，0xc0000005）的有二十多条，另一个组件数条。
3. 复现实验（`outbounds_tls_family`，同一份可执行文件连跑）：默认并行 1821 次里 4 次异常退出（0.22%），其中 3 次停在 `running 10 tests` 之后、任何用例完成之前；`--test-threads=1` 1528 次 0 次；不联网的对照（`rurge-config` 的单元测试，299 个用例并行）1837 次 0 次。
4. 工作区禁用 `unsafe`；tokio、mio、rustls、ring、socket2 的版本自阶段 1 结束起没有变过。

结论：注入的网络钩子在多个线程同时开始用 socket 时出错，rurge 没有要修的东西（M8-D4）。关闭 M2b、M3a #4、M3b #7、M7c #35 这一串。实验的副产品：连续高频地跑联网的测试二进制会把系统的临时端口用完（`connect` 报 10048），那是压测造成的，与此无关。

## 4. M8b：P2 协议项与 `W0007` 的归宿

总设计给 M8 留了三个 P2 协议项（FR-OUT-06）：Shadowsocks 的流式旧方法、VMess 的旧握手、Snell v1 ～ v3（"需要时"）。调研之后的取舍各不相同。

### 4.1 Shadowsocks 的流式方法：实现手册列出的 11 种

**范围**。Surge 手册今天在"Stream (legacy, not recommended)"一组里列 11 种：`rc4`、`rc4-md5`、`aes-128-cfb` / `aes-192-cfb` / `aes-256-cfb`、`aes-128-ctr` / `aes-192-ctr` / `aes-256-ctr`、`salsa20`、`chacha20`、`chacha20-ietf`。rurge 认得的名字有 20 个——M6a 照 shadowsocks-libev 的表录的——，多出来的 9 个（`bf-cfb`、`camellia-128/192/256-cfb`、`cast5-cfb`、`des-cfb`、`idea-cfb`、`rc2-cfb`、`seed-cfb`）手册没有；其中后五个今天没有任何一家实现支持，连 libev 自己都标成不支持，Surge 的一则测试版更新说明（二手来源）也写着已把 `bf-cfb` 到 `seed-cfb` 这些方法移除。

决定：实现手册的 11 种，TCP 与 UDP 都做；另外 9 个名字照旧认得、照旧按"不实现"处理（`W0007` + REJECT，文本改为"不支持"而不是"尚未实现"）——把它们改成未知方法会让今天能加载的配置变成加载错误。

**线上格式**（shadowsocks.org 的流式规范；shadowsocks-rust、libev、mihomo、sing-shadowsocks2 的实现一致）：

- 主密钥：口令经 `EVP_BytesToKey`（MD5，rurge 已有）取方法的密钥长度。
- TCP：每个方向以一个随机 IV 开头，其后是密文流；客户端方向的明文是"目标地址（SOCKS5 式）‖ 负载"。两个方向用同一把密钥、各自的 IV。
- UDP：每个包 `IV ‖ 密文`，明文是"地址 ‖ 负载"，每个包一个新的随机 IV。
- 没有任何完整性保护；OTA 早已从规范里去掉，不做。

| 方法 | 密钥 | IV | 构造 |
| ---- | ---- | -- | ---- |
| `aes-128/192/256-cfb` | 16 / 24 / 32 | 16 | AES-CFB128（整块反馈） |
| `aes-128/192/256-ctr` | 16 / 24 / 32 | 16 | AES-CTR，计数块从 IV 起、按 128 位大端整数加一 |
| `rc4-md5` | 16 | 16 | RC4，密钥为 MD5(主密钥 ‖ IV) |
| `rc4` | 16 | 0 | RC4 直接用主密钥；线上没有 IV，每条连接、每个包的密钥流相同 |
| `chacha20-ietf` | 32 | 12 | RFC 8439 的 ChaCha20，块计数从 0 起 |
| `chacha20` | 32 | 8 | 原版 ChaCha20，8 字节 nonce，块计数从 0 起 |
| `salsa20` | 32 | 8 | Salsa20/20，8 字节 nonce，块计数从 0 起 |

**依赖**：不新增 crate。`chacha20` 0.9.1 与 `salsa20` 0.10.2 已在依赖树里（只加依赖边，与 `rurge-proto` 现用的 `aes` 0.8 同属 `cipher` 0.4 一代）；CFB128、CTR 与 RC4 各十几二十行，直接写在 `aes` 的单块加密之上，用 NIST SP 800-38A（F.3.13 ～ F.3.18、F.5.1 ～ F.5.6）与 RFC 6229 的向量钉住——shadowsocks-rust 自己的 CFB 与 RC4 也是手写的。`chacha20` 0.9.1 的原版变体内部是 32 位块计数：单条连接超过 256 GiB 时出错而不是进位，登记。

**结构**：`rurge-config` 的 `SsMethod` 加 11 个流式取值；`rurge-proto::shadowsocks` 加 `stream` 模块（按方法造密钥流、`CipherStream`：首次写带出 IV、首次读先取对端的 IV）与 UDP 的一种 `Packets`；请求头照旧经 `LazyHead` 与首段负载一起写出；`obfs`、Shadow TLS、`udp-port` 与 AEAD 方法同样可用。`FakeShadowsocks` 加流式方法（密钥流与出站共用 `Keystream`，其余是它自己的；第 12 节第 3 条）。

**诊断**：用了流式方法的策略每种方法每次加载一条 `W0031`（新）：`` `ss` cipher `<方法>` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher ``；`rc4` 的文本另说明密钥流在连接之间重复。订阅导入的行同样告警（每个组、每种方法每次装配一条，文本另写；第 12 节第 2、3 条）。

**测试**：向量（上面两份标准，另加 RFC 8439 2.4.2 的 ChaCha20、ECRYPT 的 Salsa20/20 256 位向量）；回环（TCP 往返、UDP、两种 obfs、口令错误的表现——服务端解出乱码后关闭或一直读）；经引擎的端到端；互操作：`ssserver` v1.25.0（CI 已装；它的发布包按 `full` 构建、含 `stream-cipher`）覆盖 `rc4`、`rc4-md5`、AES-CFB 三种、AES-CTR 三种与 `chacha20-ietf` 共 9 种，`chacha20` 对 mihomo（第 4.3 节装的）的 `shadowsocks` 监听端，`salsa20` 没有可用的预编译参照（只有 libev 支持、它没有官方二进制），由向量与回环覆盖并登记。

### 4.2 VMess 的旧握手：不实现，没写 `vmess-aead=true` 的行改用 AEAD 握手

事实：

- Surge 的 `vmess-aead` 缺省 false——不写就是旧握手（MD5 认证）；Surge 没有 `alterId` 参数。
- 服务端一侧旧握手已被弃用：v2ray-core 自 2022-01-01 起缺省拒绝（要设 `V2RAY_VMESS_AEAD_FORCED=false` 才收，并预告将来去掉这条退路），Xray 自 v1.8.3（2023-06）起删除，sing-box 与 mihomo 只在 `alterId ≥ 1` 时收、且只认派生的 alter ID——Surge 没有 `alterId`，用哪个 ID 做认证密钥查不到。
- 自 v2ray-core 4.28.1（2020-09）起的服务端都收 AEAD 握手，不论那个用户配的 `alterId` 是多少。只收旧握手的只剩 2020-09 之前、六年没升级的服务端。

决定：rurge 只说 AEAD 握手。没写 `vmess-aead=true`（或写了 `false`）的 `vmess` 行不再按"协议未实现"拒绝，而是照样用 AEAD 握手，并告警一次（`W0029`：`` `vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it ``，每次加载一条；这句文本终审时改过，见第 12 节第 4 条）。

理由：这样的行在今天的服务端上，Surge 自己用旧握手多半连不上，rurge 用 AEAD 反而连得上；只有对 2020 年之前的服务端 rurge 连不上而 Surge 连得上。比起实现一种各家都在删除的不安全握手、再为它在 CI 里多装一个参照服务端（v2ray-core），这是更有用也更省的取法。`NotImplemented::LegacyVmess` 随之去掉。差异登记进兼容性清单；需求 FR-OUT-06 的"VMess 旧握手"一项以"不实现"结案。

### 4.3 Snell v1 ～ v3：实现

"需要时"的条件成立了：

- Surge 自带的 Snell 服务端（Mac，iOS 5.21 起，tvOS）说的是 v1——`[Snell Server]` 的 `version` 只收 1 与 6、缺省 1；要把一台 Surge 设备当服务端用，客户端就得会 v1（v6 见第 6.1 节，眼下不实现）。
- 策略行的 `version` 缺省就是 1，不写 `version` 的行现在一律被拒。
- 成本低：v1 ～ v3 的记录层就是 Shadowsocks AEAD 的分块格式（rurge 已有），密钥直接取已有的那组 Argon2id 的输出；请求头、应答、复用池、UDP 的数据报格式与 v4 相同。不需要新依赖。

**线上格式**（没有官方文档；事实取自 open-snell 与 mihomo 两份 GPL 实现，只取事实）：

| | v1 | v2 | v3 |
| -- | -- | -- | -- |
| AEAD | ChaCha20-Poly1305 | AES-128-GCM | AES-128-GCM |
| 会话密钥 | Argon2id（与 v4 同一组参数）输出的 32 字节 | 前 16 字节 | 前 16 字节 |
| 记录 | 每个方向先发 16 字节 salt；每块是密封的 2 字节大端长度加密封的负载，负载至多 0x3FFF；nonce 是小端计数、每次密封加一 | 同左 | 同左 |
| TCP 命令 | `0x01` | `0x05`，总是复用 | `0x01` |
| 复用 | 无 | 一个请求的每个方向以零长度块（只有密封的长度那一段）结束，之后连接承载下一个请求 | 无 |
| UDP | 无 | 无 | 命令 `0x06`，数据报格式同 v4 |
| `obfs` | `http` / `tls` | `http` / `tls` | `http` / `tls` |

**配置**：`SnellVersion` 加 `V1`、`V2`、`V3`；`obfs=tls` 在 v1 ～ v3 上可用（simple-obfs 的 `tls` 已为 `ss` 实现）；`reuse` 在 v2 上恒为真、在 v1 / v3 上不起作用（写了 `reuse=true` 是 `W0028`）；v1 / v2 不载 UDP（照 `udp-policy-not-supported-behaviour` 办）。`version=6` 仍是 `W0007` + REJECT。顺带照手册订正一处：`mode` 写在低于 6 的版本上是配置错误（`E0018`；原来是告警并忽略）。

**测试**：记录层的已知答案（Argon2id 的输出是既有的，分块的密封与 Shadowsocks AEAD 的向量同构）；`FakeSnell` 加三个版本；经引擎的端到端；互操作对 mihomo v1.19.32 的 `snell` 监听端（接受 `version` 1 ～ 5，三平台都有带 `digest` 的发布包；GPL，只作为外部进程运行）——CI 新增一步安装，环境变量 `RURGE_TEST_MIHOMO`。它的最小配置（只有 `listeners`、`mode: direct`）启动时不联网这一点读自源码（`config.Init` 只建目录与缺省配置，`geo-auto-update` 缺省关），第一次 CI 运行时核对。对 Surge 自带服务端的验证进手工验收清单。

### 4.4 `W0007` 与 `OutboundError::Unsupported` 的归宿

总设计说两者随"最后一个未实现的协议"退役，M7-D12 把时间改到 M8。M8 之后仍然没有实现、也不打算在阶段 2 实现的有：`tailscale`（第 6.1 节）、`snell` 的 `version=6`（同）、手册没列的 9 个 `ss` 方法名。它们是范围之外，不是"还没轮到"。

决定：两者都不退役，改定义。

- `W0007`：策略指向 rurge 不实现的协议或其变体；该策略按 REJECT 处理。文本从"is not implemented in this version / yet"改为说明原因与出路——`tailscale` 指向兼容性清单里经 tailscaled 的写法，`snell` v6 说明它仍是测试版协议。
- `OutboundError::Unsupported` 留着：它除了"协议未实现"还承担"这个出站不载 UDP"的说明（`external`、`socks5`、`ss`、`h2-connect` 在没开 `udp-relay` 时），后者本来就不会退役。
- 会话日志的 `policy protocol not implemented: <类型>` 文本不变（API 的使用者可能在匹配它）。
- 验收标准第 8 条相应订正（第 6.2 节）。`W0008` 只剩 `subnet` 组，属阶段 3。

### 4.5 配置层的小项

- `ecn` 写在纯 TCP 的协议上：从 `W0029`（解析但不生效）改为 `W0028`（不适用于这种策略）——手册说它对这些协议本来就没有意义（M7a #8）。
- Hysteria 2 的混淆口令短于 4 字节：参考实现不接受这样的口令（这样配的服务端起不来），rurge 加载时告警 `W0012`、照常使用（M7a #13）。
- `ss` 写了 `udp-port` 却没开 `udp-relay`：`W0028`（M6a #7）。

## 5. M8c：遗留事项

每一小节是一个任务；出处写的是登记它的计划与「延后事项」的编号。

### 5.1 QUIC 的接收窗口（M7c #14）

现状：每条流 8 MiB、每条连接 20 MiB（Hysteria 参考客户端的取值，它的文档说这个 2 : 5 的比例是为了"一两条阻塞的流不至于堵住整条连接"）。连接级的额度要等数据被读走才放回，所以三条读端停住的隧道——三个暂停的下载就够——占满 20 MiB，这个策略其余 TCP 隧道的下行随之停住。`trust-tunnel` 的 HTTP/3 有流数上限，M7c 已把它的连接窗口取为 max(`max-streams` × 8 MiB, 20 MiB)。

决定：不设流数上限的会话（`masque`、`hysteria2`、`tuic`、`tuic-v5`）的连接窗口取 32 × 每流窗口 = 256 MiB，每流窗口不变。理由：

- 连接窗口是上限，不是预先占用的内存；实际占用是各条读端停住的隧道里没读走的数据，每条至多 8 MiB——与一条停住的 TCP 连接在内核里占的收缓冲同一量级（Linux 缺省至多 6 MiB，Windows 自动调节可到 16 MiB）。
- quinn 自己的缺省是连接窗口不设限（`VarInt::MAX`），内存只由"流数 × 每流窗口"约束；TUIC 的参考客户端沿用它。rurge 仍留一个有限的上限作保险：同一条连接上要有 32 条以上的隧道同时停住且各自攒满 8 MiB，才会再出现互相拖住的情形，登记为已知限制。
- 有流数上限的会话不变。

用例：五条隧道里四条的读端停住、对端各发满 8 MiB 以上，第五条照常收发（旧的窗口下它会停住）。

### 5.2 MASQUE 经链式载体时的 UDP 余量（M7c #15）

现状：MASQUE 的 UDP 走 HTTP/3 数据报，放不下的包丢弃，能发的负载就是 QUIC 数据报的余量。直连时连接从 1200 字节起步、路径 MTU 探测在握手后约一秒内把它抬到路径允许的大小；经 `underlying-proxy` 的载体（`may_fragment() == true`）quinn 不做探测，一直停在 1200——内层 QUIC 的 Initial（至少 1200 字节）永远发不出去，经这样的 `masque` 策略的 QUIC 流量与 DoQ / DoH3 都不通。

决定：经载体的 MASQUE 连接从 1350 字节起步（quinn 的 `initial_mtu`；`min_mtu` 仍是 1200）。路径带不动 1350 字节的数据报时，quinn 的黑洞检测——不做探测时它照样工作，连续三批"只丢大包"之后退回 1200——把它降回去，代价是这样的路径上每条连接起步时有一次短暂的丢包；带得动时（UDP over TCP 的载体总带得动，经 SOCKS5 / Shadowsocks 的 UDP 与缺省 MTU 的 WireGuard 也带得动）数据报的余量约 1300 字节，够内层 QUIC 的 Initial 与浏览器缺省大小（1250）的包。直连的连接与其它 QUIC 出站都不变：直连有探测，Hysteria 2 与 TUIC v5 自己分片。

不采用"放不下就改用流上的 DATAGRAM capsule"：参照服务端 masque-server 在 HTTP/3 上不读 capsule，包会被悄悄丢掉（M7c 计划期决定 P9 的理由仍然成立）。

登记的限制不变的部分：满尺寸（1420）的 WireGuard 包在任何路径上都放不下，经 `masque` 的 `wireguard` 要把 `mtu` 设到 1280 以下。

### 5.3 载体的错误（M7c #28 ～ #32）

五处，都在 QUIC 经载体与内部 UDP 会话这条路径上：

1. `Carried::try_send` 在载体"刚才发不了、现在说可以了"的窗口里把包丢掉（#30）：再试一次发送，仍发不了才丢。
2. 包装载体对 `poll_send_ready` 的转发没有用例（#31）：补一条看首包耗时的用例——去掉引擎里任一层的转发，改走直连的 DoQ / DoH3 载体会退回"第一个包丢失、慢约一秒"。
3. 经 `underlying-proxy` 的 QUIC 策略，载体延后报出的发送错误没有 `via <策略>: ` 前缀（#29）：`ChainConnector::connect_udp` 交出的载体给自己的错误补上前缀，与打开时的错误一致。
4. `packet_datagram` 的接收一侧把载体自己的结束原因换成固定的一句（#32）：留下载体的原话。
5. `ConnectionReset` 一类的接收错误三处处理不一致（#28）：quinn 对它直接跳过、连接照用这条载体，引擎却已把内部 UDP 会话以失败结束并记了一次成员失败，`Carried` 也把它记作载体的失败、之后握手若因别的原因失败会拿这条陈旧的错误当原因。统一为：quinn 会忽略的错误（`ConnectionReset`）不结束会话、不记失败、不留作载体的失败；`quic::connect` 只在握手错误是"endpoint 没了"时取载体的话。

### 5.4 DNS（M7c #9、#10）

- `https://` 的 DoH 上游没写路径——URL 在主机与端口之后就结束——时发往 `/dns-query`（原来发往 `/`，没有哪个已知的 DoH 服务端在那里应答；RFC 8484 自己不规定路径，Surge 的缺省没有写明）。明写了 `/` 的照旧发往 `/`；上游的名字照旧按写出来的样子显示。`h3://` 自 M7c 起两种写法都发往 `/dns-query`，不动。
- RFC 8484 5.1：应答带 `Age` 头时，缓存前把各条记录的 TTL 减去它（不低于 0）。DoH 与 DoH3 共用 `doh::check_head` 之后的那一段。

### 5.5 `smart` 的失败计分（M3c #11）

现状：经 `smart` 组的会话失败——拨不通、三秒没有回应、上游在回话之前断开——既记进站点记忆，也给成员的全局分数加一次罚分。目标网站本身不通时成员无辜受罚：一个后台应用反复访问一个失效的地址，每次换一个成员去试，就把整组的排序拉乱，访问够频繁时全部成员都被判为失败。M3c 当时把它留给项目所有者，列了三种改法。

决定：(b) 加 (c)。

- (b) 记一次失败时，若同一站点在记忆期内（一小时）已经经**另一个**成员失败过，这次只记进站点记忆、不罚全局分数。失效的站点于是每小时至多让一个成员受一次罚；真正坏掉的成员在每个新站点上照样受罚，自愈的速度不变。
- (c) 在本地就被拒绝的目标（名字超长或不能写进代理请求）既不计入也不换成员：换谁都一样。
- 不采用 (a)（只把"连不上成员自己的服务器"计入全局分数）：trojan、vmess、ss 这类不回报目标错误的协议分不清两种失败，一个悄悄吞掉全部流量的成员会一直算健康、只能等测速发现。

测速的结果照旧只看探针，不受影响。

### 5.6 AnyTLS 的 SYNACK 看门狗（M2b 计划期）

现状：复用到一条"半死"的空闲会话（对端已经没了、没有 RST）时，新流的数据发进虚空，要等转发阶段的空闲超时（缺省 10 分钟）才结束。参考客户端（anytls-go）的做法：对端协议版本不低于 2、且不是会话的第一条流时，发出 SYN 后计时 3 秒，期间收到任何 SYNACK 就取消，到时则关闭整条会话。

决定：照做，常数与条件相同。会话关闭后流上的读写以 `anytls: the session is closed` 结束，池不再交出这条会话。请求已经写出、不能透明重试，但 3 秒失败远好于挂 10 分钟。假服务端加一个"收到 SYN 不回 SYNACK"的开关。

### 5.7 WireGuard 的收尾（M4b #14、#20、#23、#28、#31）

只收会让连接白等或让策略用不了的四处，其余（日志用的策略名、端口 0 的报错文字、几条缺的用例）留在遗留事项里：

- 设备任务死掉（协议栈的锁中毒）之后不会被替换，经这条策略的每个连接都 panic 到重载为止（#14）：出站发现设备任务已经结束时换一条新隧道，与"被接替"走同一条重建的路；锁中毒不再向上 panic。
- 隧道被更晚的配置接替或关闭时，正在等它的调用要等满各自的时限才失败——`Device::connect` 等满拨号时限、`Device::query` 等满每一轮、`handshake()`（原生测速）等满 `test-timeout` + 10 秒、`established()` 等到拨号时限（#20、#28、#31）：关闭唤醒全部等待者，立即以 `wireguard: the tunnel has closed` 或"已被更新的配置接替"结束。
- 共用隧道时不提升代次：重载 S → S′ → S 而第一条隧道还有连接占着时，S′ 残留的会话会接替一次、切断当前配置的连接（#23）：共用时取较大的代次。

### 5.8 订阅：指向构建失败成员的中继（M3a #13）

导入的策略 X 在注册表构建时失败被略去，而另一条导入策略的 `underlying-proxy` 指向 X 时，它仍留在成员表里，每次拨号都以 `via X: the policy no longer exists` 失败。决定：构建之后再过一遍，把中继链上任何一环已被略去的导入条目与派生条目一并略去（告警一次、不带取值），与装配期对悬空中继的处理一致。

### 5.9 出站一侧的 IDN（M2b P15、M4a #7、M4b #8）

- 策略行的 `server` 以 Unicode 写时，生成 spec 的时候按 IDNA（UTS #46，`idna` 已在依赖树里）转成 A-label；转不了是 `E0018`。此后 TLS 的名字、解析、防环用的服务器名单拿到的都是 ASCII。订阅导入的行同样处理。
- `ssh` 把目标主机名发给服务器之前、`wireguard` 的隧道内 DNS 发问之前，与其它出站一样先经 `hostname::to_ascii`。

规则、`[Host]` 的键与解析器对 Unicode 目标名的处理不在本里程碑：那要把规则与规则集的域名在加载时一并归一，是阶段 1 组件的改动，留到阶段 3 之后；兼容性清单改写为"出站一侧已支持，规则与 `[Host]` 须写成 `xn--` 形式"。

### 5.10 坏配置时的系统代理恢复（M1b 起登记的阶段 1 遗留）

现状：`rurge run` 遇到加载错误的配置在系统代理的崩溃恢复之前就退出 2。上一次运行崩溃、系统代理还指着 rurge，而这次启动的配置又是坏的，系统代理就一直指着一个没人监听的端口。

决定：配置读不了或有错误、即将以非 0 退出时，先试一次恢复——数据目录存在、其中有 `state.json`、拿得到实例锁（拿不到说明另有实例在跑，备份是它的，不碰）时，取后端、恢复备份、清掉备份。恢复的结果直接写到标准错误（这条路径上日志还没有初始化）；恢复失败或后端的环境变量取值不对只告警，退出码不变。"先拿锁再恢复"的次序与正常启动一致。

### 5.11 转发循环的 flush 与缓冲（M2b、M2c #1）

现状：转发循环每次读至多 8 KiB、写出后立即 flush（M2b 为了不让 TLS 写端压住最后一条记录）。flush 的成本只在裸 TCP 上量过（那里是空操作），M2b 的终审要求在 TLS / WebSocket / VMess 的栈上实测。

做法：给忽略的吞吐用例加上 TLS（trojan）、WebSocket 与 VMess 三种上游，各在两种策略下跑——现在的"每次写后 flush"与 tokio 的 `copy` 那样"读端暂时没有数据时才 flush"——，缓冲取 8 / 16 / 32 / 64 KiB；用两份预先编好的 release 二进制交替各跑 8 次（顺序测量会把漂移看成差别，M2b 的教训）。判据：某种取法在三种栈上都至少快 10% 且没有一种变慢，就换成它；否则保持现状。结果无论如何写进计划的「执行期修正记录」与本文末尾。

顺带（M2c #15）：`http` / `socks5` 出站握手里的写在读应答之前补上显式的 flush——下层是会缓冲的一跳时，握手的请求不该等到下一次写才发出。

### 5.12 小项

- SOCKS5 客户端发往 IPv4 映射地址（`[::ffff:a.b.c.d]`）的 UDP：入站把目标归一成 IPv4，回包按来源归流时才对得上（M7b #8）。
- `rurge-proto-wireguard` 与 `hysteria2` 用例里剩下的墙钟断言与没加时限的等待（M7c #33、M7b #16）。

## 6. 报告、验收与文档

### 6.1 可行性报告（FR-OUT-15）

写成 `docs/reports/2026-10-08-phase2-feasibility-snell-v6-quic-tailscale.md`，结论：

| 项 | 结论 | 理由 |
| -- | ---- | ---- |
| Snell v6 | 阶段 2 不实现；技术上可行（规模 M，不需要新依赖，sing-box 1.14.2 与官方 v6.0.0rc2 可作参照） | 仍是测试版协议、变过；没有规范，`default` 模式的全部常数只能从一份 GPL 的逆向实现得知，Surge 写明有意不公开——是否照此实现要项目所有者表态。先做不涉及这些常数的 `unshaped` / `unsafe-raw` 是可选的第一步 |
| Snell v5 的 QUIC Proxy Mode | 不做 | Surge 在 v6 取消了它并写明效果不及预期；规格只来自一家第三方的抓包逆向；缺省路径上用不到 |
| `tailscale` | 阶段 2 不实现；现在经本机的 tailscaled（用户态模式的 SOCKS5，支持 UDP）以 `socks5` / `external` 策略接入 | 嵌官方 C 库要 Go 与 FFI 且不支持 Windows；官方 Rust 库（tailscale-rs）仍是预览、缺 MagicDNS 与 exit node、MSRV 与 TLS 后端同 rurge 相抵；原生自研约 1.5 ～ 2.5 万行并要跟随一个没有规范的协议。跟踪 tailscale-rs，条件具备时作为可选特性接入 |

另：自写的 HTTP/3 客户端（M7-D5）不换。crates.io 上的 `h3` 仍是 2025-05 的 0.0.8，当初的四个缺陷里三个在上游主干修了但没有发布，读对端 SETTINGS 的那个没动；等 `h3` 0.0.9 / `h3-datagram` 0.0.3 发布后复评。

### 6.2 阶段 2 的验收标准（订正总设计第 14 节）

九条里订正两条，其余不变：

- 第 8 条"三平台 CI 绿（含互操作 job）；clippy 零警告；`W0007` 与 `W0008` 不再出现"改为：三平台 CI 绿（含互操作）；clippy 零警告；`W0007` 只对 rurge 不实现的协议与变体出现（`tailscale`、`snell` 的 `version=6`、手册没列的 `ss` 方法名），`W0008` 只对 `subnet` 组出现（阶段 3）。
- 第 1 条"每种协议对参考服务器的 TCP 转发通过；支持 UDP 的协议 UDP 转发通过"补一句：没有可用参照服务端的情形（`socks5-tls`、CONNECT-UDP over HTTP/2、TUIC 的 `quic` 模式、端口跳跃、`ss` 的 `salsa20`）由回环假服务端覆盖并进手工验收清单。simple-obfs 原来也在这张名单里；M8b 起 mihomo 的监听端是它的参照服务端——CI 上装了，用例还没有对真实的二进制跑过，第一次 CI 运行时核对（第 12 节第 8 条）。

逐条的证据（用例名、CI 运行）写成 `docs/acceptance/phase2-acceptance.md`；需要真实节点的项目仍在 `docs/acceptance/phase2-manual.md`，M8 给它补上流式方法、Snell v1（对 Surge 自带的服务端）与经 tailscaled 接入三节。

### 6.3 `CORE_VERSION`

手册（`profile/requirement.html`）给的对应：20 是 Surge Mac 5.6.0 / iOS 5.10.0（Body Rewrite、内联 Map Local），22 是 Mac 5.7.0 / iOS 5.11.0（Smart Group）；Mac 6.8.0 起改为由版本号派生（6.8.0 即 6008000）。

决定：阶段 2 结束时报告 22，照需求 FR-CFG-08 写下的方案（M3c 延后事项 #4，原留给项目所有者）。22 在手册里唯一的含义是 Smart Group，rurge 已有；20 所指的 Body Rewrite 与内联 Map Local 自阶段 1 起就以"节解析但不生效"（`W0016`）对待，上调不改变这一点。

### 6.4 手册基线

本项目的兼容基线是 2026-09 的手册（Surge Mac 6.9 / iOS 5.22）。调研时手册已是 2026-10-07 版，出现了 Mac 6.10 / iOS 5.23 的条目，页面路径也换了（策略页在 `policies/<协议>.html`）。顺带看到的几处：`test-url` 接受 HTTPS（rurge 的探针本来就支持）、`[IP Rewrite]` 节、策略组的 `category` 参数、`[Snell Server]` 可由模块提供。系统的比对不在 M8：阶段 3 开工前单独做一次，结果进兼容性清单；M8 只登记本文用到的几处，并把 CLAUDE.md 里的手册路径示例改对。

## 7. 遗留事项的处置

阶段 2 的 20 份计划各有一张「延后事项」表，合计约四百行。M8 的文档任务把它们收成一张总表（`docs/superpowers/plans/` 下 M8c 计划的附录），每行标上去向。这里列的是标给 M8 的与原先留给项目所有者的，其余按表里原有的去向不变——"有用户报告再说"的仍是已知限制，"阶段 3"的随阶段 3 的设计承接。

**M8 做掉**（括号里是本文的小节）：M7c #9、#10（5.4），#14（5.1），#15（5.2），#24、#25、#34、#36（3.3），#28 ～ #32（5.3），#33（5.12），#35（3.5）；M7b #8、#16（5.12）；M7a #8、#13（4.5）；M6a #1（4.1），#7（4.5、3.3）；M6b #1 的 v1 ～ v3（4.3）；M4b #14、#20、#23、#28、#31（5.7）；M4a #7（5.9）；M4b #8（5.9）；M3c #4（6.3），#11（5.5）；M3b #21（3.3）；M3a #5（3.4），#13（5.8），#19、#23、#31（3.3）；M2c #1（5.11），#15（5.11）；M2b 的 SYNACK 看门狗（5.6）、`Secret<T>` 的覆盖面（3.3）、flush 的成本（5.11）、出站一侧的 IDN（5.9）、VMess 旧握手（4.2，以"不实现"结案）；M1b 的坏配置与系统代理恢复（5.10）；M2b / M3a #4 / M3b #7 的异常退出（3.5）。

**关闭**（不做，理由如下）：

| 出处 | 事项 | 理由 |
| ---- | ---- | ---- |
| M3a #2 | `ChainConnector` 的运行期深度守卫 | 有界性由装配期的环检查保证；M3b 已把"链深超限"两处说法不一致的问题处理掉。没有能构造出无界链的输入 |
| M6b #1 | Snell v5 的 QUIC Proxy Mode | 第 6.1 节 |
| M7a #2、M7b #13 | 应用层 PING、保活间隔 | 保活 10 秒加空闲超时 30 秒是 Hysteria 与 quinn 的常用取值；空闲超时不超过 10 秒的服务端上安静的连接约 10 秒后重拨，已登记 |
| M7 设计 | 自写的 HTTP/3 客户端是否换成 `h3` | 第 6.1 节 |
| M4b #26、#27，M7c #18、#19、#21 | 吞吐基准的时限、重复的断言、夹具与假服务端的脚手架重复、用例文件过长 | 只影响测试代码的整洁；动到那些文件时再整理 |

**移交**：

| 出处 | 事项 | 去向 |
| ---- | ---- | ---- |
| M2b P15 的其余部分 | 规则、`[Host]` 的键与解析器对 Unicode 名字的处理 | 阶段 3 之后；兼容性清单登记"须写成 `xn--` 形式" |
| M6b #1 的 v6 | Snell v6 | 项目所有者表态之后（第 6.1 节） |
| M7c #17 | 被放弃的内部 DNS 拨号不留请求记录、不向 `smart` 回报，调用方的时限没有传给出站 | 有用户报告再说：只影响记录与一次回报 |
| M7c #20 | 策略的与 DNS 的两份 QUIC 单飞会话规则已分叉 | 下一次动其中一份时合并到 `rurge_net::quic`；现在合并是没有行为收益的重构 |
| M7a #12 | 被淘汰的连接在最后一个租约放手时立刻关闭，可能截掉最后一条流的尾部 | 有用户报告再说 |
| M4b #9、#10、#11、#13、#17、#22、#25 与只缺用例的几条 | 同名的 `[WireGuard]` 节、端口 0 的报错文字、最后一个连接的 FIN、启动等每个 peer 的载体、SERVFAIL 等满一轮、共用隧道的日志名、日志级别提示 | 有用户报告再说 |
| M4b #1 | smoltcp 0.12 的 TCP 限制 | MSRV 升到 1.91 时换 smoltcp 0.13 并逐条核对 |
| M3a #1、#7、#15、#32 | 订阅节点的主机名与 `[Host]`、每次重建重新解析全部订阅、重建占住工作线程、单行与本地文件没有大小上限 | 有用户报告或性能数据再说 |
| M3b #5、#11，M4b #3、#19，M5a #19 | "网络已变化"的探测器与依赖它的几处 | 阶段 3 |
| M4c #5 的其余部分 | `external` 的 `addresses` 与"外部进程的流量走 DIRECT" | 阶段 3（要有 TUN） |
| M7c #13 | sing-box 1.15 的 `http` 入站可作 MASQUE 的三平台参照 | 它发布正式版之后 |

## 8. 需登记进兼容性清单的差异

1. `vmess`：rurge 只说 AEAD 握手；没写 `vmess-aead=true` 的行也用 AEAD（Surge 用旧握手），`W0029`。2020-09 之前的服务端连不上。
2. `ss`：手册的 11 种流式方法已实现（`W0031`）；手册没列的 9 个名字认得但不支持（`W0007` + REJECT）；`chacha20` 单条连接至多 256 GiB；`salsa20` 没有对参照服务端验证过。
3. `snell`：v1 ～ v5 已实现；v6 不实现（测试版协议、没有规范）；v5 的 QUIC Proxy Mode 不实现（Surge 在 v6 已取消），`udp-port` 只解析；`mode` 写在低于 6 的版本上是配置错误。
4. `tailscale`：不实现；经 tailscaled 的 SOCKS5 的写法。
5. DoH：URL 里的用户名口令以 `Authorization: Basic` 发出（Surge 的做法没有写明）；主机之后没写路径时发往 `/dns-query`；应答的 `Age` 计入 TTL。
6. QUIC 出站的接收窗口：不设流数上限的会话每条连接 256 MiB；同一条连接上 32 条以上的隧道同时停住且各攒满 8 MiB 时仍会互相拖住。
7. `masque` 经 `underlying-proxy`：连接从 1350 字节的包起步；带不动的路径上退回 1200，那时 UDP 负载不到 1200 字节（内层 QUIC 不通）。
8. `smart`：同一站点在一小时内已经经另一个成员失败过时，失败只记进站点记忆；本地就拒绝的目标不计入、不换成员。
9. `anytls`：SYNACK 看门狗（3 秒，对端协议版本 ≥ 2、非会话的第一条流）。
10. 策略行的 `server` 以 Unicode 写时转成 A-label；规则与 `[Host]` 须写成 `xn--` 形式。
11. `rurge run` 遇到加载不了的配置时，也恢复上一次崩溃留下的系统代理设置。
12. 布尔环境变量的取值；`CORE_VERSION` 为 22。
13. 诊断与脱敏输出里的凭据（第 3.3 节的表）。

## 9. 写各份计划时必须核对的事项

| 编号 | 事项 | 属于 |
| ---- | ---- | ---- |
| V1 | CI 首跑的失败清单（等带着失败摘要的提交推送之后） | M8a |
| V2 | `chacha20` 0.9.1 的 `ChaCha20Legacy` 与 `salsa20` 0.10.2 的 `Salsa20` 对 libsodium / ECRYPT 向量的输出；它们的 seek 与计数边界 | M8b |
| V3 | `ssserver` v1.25.0 对流式方法的配置写法（只换 `method`）与口令错误时的表现 | M8b |
| V4 | mihomo v1.19.32：三个发布包的 `digest`（从 GitHub 的发布 API 逐字读出）、最小配置的写法、`-d` / `-f` 参数与就绪日志、启动时不联网、`snell` 监听端 `udp` 的缺省、`shadowsocks` 监听端收 `chacha20` | M8b |
| V5 | Snell v2 的零长度块在 mihomo 两条写路径上的编码不同（只有密封的长度段，或再加一段空负载）：读端两种都要认；v3 的 UDP 应答字节由谁消化 | M8b |
| V6 | quinn 0.11.12 / quinn-proto 0.11.19：`initial_mtu` 在 `may_fragment()` 的 socket 上生效、黑洞检测在不做探测时仍把 MTU 降回 `min_mtu`（读过源码，写用例钉住）；`set_receive_window` 不需要用到 | M8c |
| V7 | anytls-go 的看门狗只盯最近一次 SYN（新开的流取消上一个计时）——照做还是按流计 | M8c |
| V8 | `HttpClient` 的重定向：DoH 用的 `send` 不跟随重定向（`Authorization` 不会被带到别的源）——读过源码，并有用例钉住 | M8a |
| V9 | `SmartBook` 里"同一站点经另一个成员失败过"的判据与并发的两次回报 | M8c |
| V10 | 转发循环的两种 flush 策略在 Windows 回环上的测量方法（release、两份二进制交替） | M8c |

## 10. 任务草图

**M8a**（CI 转绿与凭据不外泄）
1. CI 的失败摘要（3.1）
2. `[General]` 的诊断、`redact_profile`、配置对象的 `Debug`（3.3）
3. DNS 上游 URL：DoH 的 Basic 认证、全角 `＠`、端口回显（3.3）
4. 布尔环境变量（3.4）；资源管理器的 `Debug`（3.3）
5. CI 首跑的修正（3.2；任务数随失败清单定）
6. 文档：兼容性清单、API 说明、计划的两张表

**M8b**（P2 协议项与 `W0007` 的归宿）
1. 配置：`SsMethod` 的流式取值与 `W0031`、`vmess-aead` 的新处理、`SnellVersion` 的 v1 ～ v3 与 `mode` 的错误、4.5 的三处、`W0007` 的文本
2. `ss` 的流式方法：密钥流、TCP 流、UDP、`FakeShadowsocks`
3. `snell` v1 ～ v3：记录层、隧道与池、UDP、`FakeSnell`
4. 引擎的端到端用例与 CLI 的诊断用例
5. 互操作：`ssserver` 的流式方法；CI 安装 mihomo，Snell v1 ～ v3 与 `ss` 的 `chacha20`
6. 文档：兼容性清单、PRD（FR-OUT-06）、手工验收、CLAUDE.md

**M8c**（遗留事项、报告与验收）
1. QUIC 的接收窗口与 MASQUE 的起步包长（5.1、5.2）
2. 载体的错误（5.3）
3. DNS：DoH 的缺省路径与 `Age`（5.4）
4. `smart` 的失败计分（5.5）
5. AnyTLS 的 SYNACK 看门狗（5.6）
6. WireGuard 的收尾（5.7）
7. 订阅里指向构建失败成员的中继（5.8）；出站一侧的 IDN（5.9）
8. 坏配置时的系统代理恢复（5.10）
9. 转发循环的 flush 与缓冲的实测，握手的 flush（5.11）；小项（5.12）
10. 文档：可行性报告、阶段验收清单、遗留事项总表、`CORE_VERSION`、兼容性清单与统计附录、README（中英）、PRD、CLAUDE.md

## 11. 需同步的文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| 总设计 | 第 1.4 节 M8 一行加指引；第 14 节按 6.2 订正；`W0007` / `Unsupported` 按 4.4 | 随本文 |
| `docs/surge-compatibility-matrix.md` | 第 8 节的各条；`tailscale`、`snell`、`ss`、`vmess` 几行的状态；附录的统计重数 | 各份计划的文档任务 |
| `docs/requirements.md` | FR-OUT-06（流式方法与 Snell v1 ～ v3 已实现、VMess 旧握手不实现）、FR-OUT-15（报告已出）、FR-CFG-08（22）、阶段 2 的验收一节 | M8b、M8c |
| `docs/acceptance/phase2-manual.md`、`docs/acceptance/phase2-acceptance.md` | 6.2 | M8b、M8c |
| `docs/api/phase2.md` | DoH 的认证；诊断文本的变化 | M8a |
| `README.md` / `README_en.md` | 阶段 2 完成；特性表与路线图 | M8c |
| `CLAUDE.md` | 状态、文档索引、命令、手册路径示例 | 各份计划的文档任务 |

## 12. M8b 实施期的订正

M8b（第 4 节）按 `docs/superpowers/plans/2026-10-08-phase2-m8b-p2-protocols-plan.md` 做完：配置层（提交 4203eb3、0076f61）、`ss` 的流式方法（3ce73f9、d6d3835）、Snell v1 ～ v3（23af242、b106a60）、互操作（a521b65、6ac8bf8），以及收尾的注释与文档。下面是与第 4 节、第 6.2 节的文字不同的地方，以代码为准。第 4 节与第 8 节的原文作为计划期的记录留着，只有已经不成立的几处就地改了，各在所属的条目里注明。逐项的理由与提交见该计划的「执行期修正记录」，留下的事项见它的「延后事项」。

1. **`W0007` 的文本**（4.4）。M8b 之后它只有三种来源，各一句：`tailscale` 是 `` policy type `tailscale` is not supported; a tailnet is reached through tailscaled's SOCKS5 proxy with a `socks5` or an `external` policy; such policies behave as REJECT ``（文本自己说出能用的写法，不是指向兼容性清单）；`snell` 的 `version=6` 是 `` `snell` version 6 is not supported: it is a beta protocol without a public specification; such policies behave as REJECT ``；手册不再列出的 9 个 `ss` 方法名是 `` `ss` cipher `<方法>` is not supported (Surge no longer lists it); such policies behave as REJECT ``。第 4.4 节没有提到订阅导入的行：它们的告警同样改了，`` policy group `<组>`: imported policies of type <种类> are not supported; they behave as REJECT ``。会话日志的 `policy protocol not implemented: <说明>` 没有动。代码注释里把 `W0007`、`OutboundError::Unsupported` 与没有 spec 的策略说成"还没实现"的地方，一并改成了"不实现"。
2. **提示的机制**（4.1、4.2）。"策略照建，每次加载按种类说一次"的两条告警——`vmess-aead` 的 `W0029` 与流式方法的 `W0031`——共用一个新的 `Remark`，与"不建策略"的 `NotImplemented` 并列。主配置里每种每次加载一条，落在第一条这样的策略行上。订阅导入的行按"组 × 种类"各一条（第 4.1 节原来写"每种方法每次装配一条"，已就地改），文本另写：`` policy group `<组>`: imported `vmess` policies without `vmess-aead=true` use the AEAD handshake `` 与 `` policy group `<组>`: imported `ss` policies use the stream cipher `<方法>`: it gives no integrity protection and is deprecated ``——后一句没有"use an AEAD cipher"，订阅的行使用者改不了。按组说是因为组的 `external-policy-modifier` 是使用者能消掉这类提示的地方；导入行的 `W0007` 则照旧按种类跨组只说一次。行有错误或没有 spec 时不给提示。
3. **流式方法**（4.1）。
   - 第 4.1 节就地改了四处。一处是订阅导入的行按什么去重，见上面第 2 条；另三处：`W0031` 的文本是 `… and is deprecated; use an AEAD cipher`（原写 `has been deprecated`）；流的类型叫 `CipherStream`（原写 `StreamCipherStream`）；`FakeShadowsocks` 的密钥流与出站共用 `Keystream`（原写"密钥流独立实现"）。流式方法的"封帧"只有一个 IV，把密码再写一遍没有意义：`Keystream` 的正确性由标准向量与互操作钉住，假服务端的读 IV、解析地址、转发与 UDP 的拆包封包仍是它自己的代码。
   - 密钥流有尽头：`chacha20` 0.9.1 的两种 ChaCha20——原版与 `chacha20-ietf`——都以 32 位计块，一条连接的每个方向在 (2^32 − 1) × 64 字节（256 GiB 少 64 字节）处到头。到头时这条连接以 `ss: the cipher's keystream is used up` 失败：不 panic，也不在回绕的密钥流下发出字节。第 4.1 节与第 8 节第 2 条只提了原版 `chacha20`。`salsa20` 以 64 位计块，实际上没有尽头。
   - 口令或方法不对：流式方法什么都不认证，两端各自解出乱码，服务端多半不回一个字节就走。第一个字节到达之前连接结束或被重置，按错误报出，文本与 AEAD 方法相同（`ss: the server closed the connection without answering`、`ss: the server reset the connection without answering (wrong password or method?)`）；IV 收到一半就结束是 `ss: the connection ended in the middle of the server's IV`。代价与 AEAD 方法一样：目标自己一个字节不回就关闭的会话也记成第一句。
   - 我方的 IV 与首段密文在同一次写里发出，一次写至多取 16 KiB；未知 `encrypt-method` 的错误列出全部 19 个可用的名字。
   - 假服务端（对所有方法）只连接、只发往 127.0.0.1 与 ::1 本身：口令不对时流式方法解出的"地址"是任意地址，测试不能因此碰公网。
4. **VMess**（4.2）。照设计做了。告警的文本终审时改过，第 4.2 节已就地改：设计原来写的 `` `vmess-aead=false` has no effect: rurge always uses the AEAD handshake, which every server since V2Ray 4.28.1 accepts `` 对根本没写这个参数的行说不通，现在没写与写了 `false` 的行同是一句 `` `vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it ``（订阅导入行的那一句没有动）。不写 `vmess-aead`、写 `false` 与写 `true` 的行得到同一个 spec：重载时在这个参数上来回改不重建出站。
5. **Snell v1 ～ v3**（4.3）。
   - 零长度块只认一种写法。第 9 节 V5 说"读端两种都要认"；核对 mihomo v1.19.32 的源码之后改了主意：监听端两个方向写的都只是密封的长度那一段，"再密封一段空负载"的写法只在它的另一条写路径上、监听端的应答到不了那里，两份开源实现的读端也都不认它。代价：对写另一种的服务端，复用的 v2 连接上的下一个请求会解不开。
   - 各版本怎样结束自己的方向（第 4.3 节没有写）：v1 / v3 以 TCP 的写关闭结束，不发零长度块——v1 的服务端不认得它；v2 以零长度块结束，连接留着；服务端发来的零长度块在三个版本上都结束它的方向。
   - `reuse` 的 `W0028` 两个方向都有：v1 / v3 上写 `reuse=true`，v2 上写 `reuse=false`。spec 里存的是生效的值，所以 `version=2` 与 `version=2, reuse=true` 是同一个 spec。
   - 错误文本（第 4.3 节没有写，计划沿用 v4 的三句）。口令或版本不对的服务端多半什么也不回就关，所以"没有应答"也带提示，并按服务端有没有发过字节分开，所有版本如此：一个字节没到就关闭是 `snell: the server closed the connection without answering (wrong psk or version?)`；一个字节没到就被重置是新的一句 `snell: the server reset the connection without answering (wrong psk or version?)`（错误种类不变）；服务端的 salt 已到而没有应答，是不带提示的 `snell: the server closed the connection without answering`；回了解不开的数据仍是 `snell: the server's data failed to decrypt (wrong psk or version?)`。M6b 登记过的 v4 / v5 文本随之改了。已知限制：提示只管读方向（重置若先出现在一次写上，拿到的是系统自己的文本）；它以"服务端随第一次写才发 salt"为前提；上传途中、服务端一个字节还没回时被重置，同样算"什么都没发"。
   - `udp-port` 在 v1 ～ v3 上与 v4 / v5 一样只解析（`W0029`）。v1 / v2 不载 UDP：经它们的 UDP 流照 `udp-policy-not-supported-behaviour` 处理。
   - 目标一个字节没回就结束时，参照服务端回的是错误应答，会话记 `snell: the server refused: Remote EOF`；回环假服务端 `FakeSnell` 在这三个版本上照此作答。
6. **配置层的小项**（4.5）。`ecn` 的 `W0028` 是 `` `ecn` does not apply to `<类型>` policies; ignored ``，写在不带 `h3=true` 的 `trust-tunnel` 上是 `` … policies without `h3=true`; ignored ``（经 HTTP/3 时它生效）；被忽略的 `ecn` 在 spec 里置回 `auto`，重载不因它重建出站；告警从"每个参数名每次加载一条"变成"每行一条"。`snell` 的 `version` 本身写错时不再对 `mode` 报第二条。Hysteria 2 的短混淆口令按参数各自检查、按字节计，4 字节的下限读自参考实现 Salamander 与 Gecko 两份源码。`ss` 的 `udp-port` 在没有 `udp-relay=true` 时被忽略（不进 spec）。
7. **互操作**（4.1、4.3）。
   - 计划之外加了 simple-obfs 对 mihomo 监听端的用例：`ss` 的 `http` 与 `tls`（AEAD 与流式方法各走 mihomo 的一套实现），`snell` v1 配 `tls`、v4 配 `http`。
   - mihomo 的就绪不能看日志行或端口：它先开监听端，稍后才开始转发，其间交来的连接被直接关掉。夹具多配一个 `socks` 监听端，经它真的转发成一次才算就绪。其余读自源码的事实（`obfs-opts` 必须同时写 `mode` 与 `host`、要从子进程环境里去掉的十三个 `CLASH_*` 变量、回环检测的开关、主目录里的 `cache.db`）与出处写在 `tests/interop/README.md` 的「mihomo」一节。
   - mihomo 的流式方法表里没有单独的 `rc4` 与 `salsa20`：`rc4` 由 `ssserver` 覆盖，`salsa20` 仍没有参照。
   - 流式方法不写口令错误的互操作用例：参照服务端会按解出的乱码去连任意地址，或一直读到客户端离开。Snell 的 PSK 或版本不对的用例只收带提示的"结束"与"重置"两句。
   - 一次拉起的端口由 `rurge_interop::free_ports` 一次取齐，免得同一份配置的两个监听端拿到同一个号。
   - mihomo 与 `ssserver` 流式方法的用例都还没有对真实的二进制跑过（本机不装参照服务端）：第一次 CI 运行时核对，要核对的项目列在同一份 README 里。
8. **验收标准第 1 条**（6.2，已就地改；总设计第 14 节第 1 条同改）。simple-obfs 不再属于"没有可用参照服务端的情形"：mihomo 的监听端说它，CI 上装了 mihomo。确切的说法是"有参照、还没跑过"——直到第一次 CI 运行。`ss` 的 `salsa20` 仍在那张名单里。
9. **终审之后的订正**（整分支评审之后、合并之前的一轮；逐项与提交见该计划「执行期修正记录」里任务一栏写"终审"的各行）。
   - `W0029` 的文本（4.2，已就地改；见上面第 4 条）。
   - CI（3.1）。参照服务端的每个安装步骤——连同把 `RURGE_TEST_SSHD` 指向 `sshd` 的那一步——带 `continue-on-error: true`，`RURGE_INTEROP_REQUIRED=1` 从 sing-box 的安装步骤提到工作流一级的 `env`（留在那一步里的话，那一步一失败变量就没有了，全部互操作用例会不声不响地跳过）。某个参照服务端装不上（下载失败、资产挪了地方）时任务不再停在那一步：要用它的互操作用例各自以"找不到二进制"失败，其余的测试照常运行、照常进失败摘要。代价：安装失败不再表现为一个失败的步骤，而是一批失败的用例。3.1 说的"一次运行要给出全部结果"由此也管到安装步骤；同一轮还让互操作用例把一张表里失败的策略全部报出（原来在第一条失败处就停）。
   - 其余各项不改变本文的说法，只记在该计划里：已经报错的参数不再多说一句（`ecn` 与 `ss` 的 `udp-port`；与第 6 条里 `snell` 的 `version` 写错时不再说 `mode` 同一个道理）；`tests/interop/tests/snell.rs` 对 sing-box 与官方 snell-server 各加一个"版本或 PSK 不符"的用例，兼容性清单 4.2 `snell` 一行里说没写 `version` 的行对 v4 / v5 服务端很快失败的那一句随之加了前提（服务端读不出请求就关闭或重置连接），等第一次 CI 运行核对；回环假服务端 `FakeSnell` 不再按自己的版本拒绝命令；README 里随合并变成不实的几处与文档评审的几处订正。
