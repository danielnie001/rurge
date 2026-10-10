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
| M8-D13 | `smart` 的失败计分 | 同一站点在记忆期内已经经另一个成员失败过时，失败只记进站点记忆；本地就拒绝的目标不计入、不换成员（M3c 延后事项 #11 的 (b) 与 (c)，原留给项目所有者）（第 5.5 节；执行时 (b) 改为 (b′)，见第 13 节第 5 条） |
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

- 测试一步改为 `cargo test --workspace --no-fail-fast`，输出同时存进 `$RUNNER_TEMP/test.log`；限时 60 分钟（`rurge-dns` 的用例有过空转数小时的记录，M3a 延后事项 #20；M8c 找到并修复了一个与那次观察相符的空转，但那次运行没有留下日志，不能证明就是它，见第 13 节第 15 条）。一个二进制失败不再挡住后面的：互操作测试排在最后，一次运行要给出全部结果。
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

现状：每条流 8 MiB、每条连接 20 MiB（Hysteria 参考客户端的取值，它的文档说这个 2 : 5 的比例是为了"一两条阻塞的流不至于堵住整条连接"）。连接级的额度要等数据被读走才放回，所以三条读端停住的隧道——三个暂停的下载就够——占满 20 MiB，这个策略其余 TCP 隧道的下行随之停住。`trust-tunnel` 的 HTTP/3 有流数上限，M7c 已把它的连接窗口取为 max(`max-streams` × 8 MiB, 20 MiB)（M8c 又改了这个取值，见第 13 节第 1 条）。

决定：不设流数上限的会话（`masque`、`hysteria2`、`tuic`、`tuic-v5`）的连接窗口取 32 × 每流窗口 = 256 MiB，每流窗口不变。理由：

- 连接窗口是上限，不是预先占用的内存；实际占用是各条读端停住的隧道里没读走的数据，每条至多 8 MiB——与一条停住的 TCP 连接在内核里占的收缓冲同一量级（Linux 缺省至多 6 MiB，Windows 自动调节可到 16 MiB）。
- quinn 自己的缺省是连接窗口不设限（`VarInt::MAX`），内存只由"流数 × 每流窗口"约束；TUIC 的参考客户端沿用它。rurge 仍留一个有限的上限作保险：同一条连接上要有 29 条以上的隧道同时停住且各自攒满 8 MiB——quinn 一次公布连接窗口的八分之一，没人读的数据超过窗口的八分之七时其余隧道才被拖住，所以是 29 条而不是 32 条，见第 13 节第 1 条——，才会再出现互相拖住的情形，登记为已知限制。
- 有流数上限的会话的窗口另算：⌈`max-streams` × 8 MiB × 8/7⌉，至少 20 MiB（第 13 节第 1 条）。

用例：五条隧道里四条的读端停住、对端各发满 8 MiB 以上，第五条照常收发（旧的窗口下它会停住）。

### 5.2 MASQUE 经链式载体时的 UDP 余量（M7c #15）

现状：MASQUE 的 UDP 走 HTTP/3 数据报，放不下的包丢弃，能发的负载就是 QUIC 数据报的余量。直连时连接从 1200 字节起步、路径 MTU 探测在握手后约一秒内把它抬到路径允许的大小；经 `underlying-proxy` 的载体（`may_fragment() == true`）quinn 不做探测，一直停在 1200——内层 QUIC 的 Initial（至少 1200 字节）永远发不出去，经这样的 `masque` 策略的 QUIC 流量与 DoQ / DoH3 都不通。

决定：经载体的 MASQUE 连接从 1350 字节起步（quinn 的 `initial_mtu`；`min_mtu` 仍是 1200）。路径带不动 1350 字节的数据报时，quinn 的黑洞检测——不做探测时它照样工作，连续四批（quinn 的阈值是超过 3 批，见第 13 节第 2 条）"只丢大包"之后退回 1200——把它降回去，代价是这样的路径上每条连接起步时有一次短暂的丢包；带得动时（UDP over TCP 的载体总带得动，经 SOCKS5 / Shadowsocks 的 UDP 也带得动；缺省 `mtu`（1280）的 WireGuard 带不动，那时的行为没有实测，见第 13 节第 2 条）数据报的余量约 1300 字节，够内层 QUIC 的 Initial 与浏览器缺省大小（1250）的包。直连的连接与其它 QUIC 出站都不变：直连有探测，Hysteria 2 与 TUIC v5 自己分片。

不采用"放不下就改用流上的 DATAGRAM capsule"：参照服务端 masque-server 在 HTTP/3 上不读 capsule，包会被悄悄丢掉（M7c 计划期决定 P9 的理由仍然成立）。

登记的限制，数字随执行改了（原写 1280，见第 13 节第 2 条）：满尺寸（1420）的 WireGuard 包在任何路径上都放不下，经 `masque` 的 `wireguard` 要把 `mtu` 设到 1264 以下（`masque` 自己经载体的数据报余量是 1298 ～ 1310 字节，WireGuard 的包比内层 IP 包多 32 字节；按余量算出，没有实测，见第 13 节第 2 条）。

### 5.3 载体的错误（M7c #28 ～ #32）

五处，都在 QUIC 经载体与内部 UDP 会话这条路径上：

1. `Carried::try_send` 在载体"刚才发不了、现在说可以了"的窗口里把包丢掉（#30）：再试一次发送，仍发不了才丢。
2. 包装载体对 `poll_send_ready` 的转发没有用例（#31）：补一条看首包耗时的用例（实现为不看耗时的转发用例，见第 13 节第 3 条）——去掉引擎里任一层的转发，改走直连的 DoQ / DoH3 载体会退回"第一个包丢失、慢约一秒"。
3. 经 `underlying-proxy` 的 QUIC 策略，载体延后报出的发送错误没有 `via <策略>: ` 前缀（#29）：`ChainConnector::connect_udp` 交出的载体给自己的错误补上前缀，与打开时的错误一致。
4. `packet_datagram` 的接收一侧把载体自己的结束原因换成固定的一句（#32）：留下载体的原话。
5. `ConnectionReset` 一类的接收错误三处处理不一致（#28）：quinn 对它直接跳过、连接照用这条载体，引擎却已把内部 UDP 会话以失败结束并记了一次成员失败，`Carried` 也把它记作载体的失败、之后握手若因别的原因失败会拿这条陈旧的错误当原因。统一为：quinn 会忽略的错误（`ConnectionReset`）不结束会话、不记失败、不留作载体的失败（载体自己因它而结束时除外，见第 13 节第 3 条）；`quic::connect` 只在握手错误是"endpoint 没了"时取载体的话（quinn 对它有两种说法，见第 13 节第 3 条）。

### 5.4 DNS（M7c #9、#10）

- `https://` 的 DoH 上游没写路径——URL 在主机与端口之后就结束——时发往 `/dns-query`（原来发往 `/`，没有哪个已知的 DoH 服务端在那里应答；RFC 8484 自己不规定路径，Surge 的缺省没有写明）。明写了 `/` 的照旧发往 `/`；上游的名字不带补上的路径（没写路径的显示成 `https://dns.example`，此前是 `https://dns.example/`；名字是规范形式——主机小写、缺省端口与用户名口令去掉——，见第 13 节第 4 条）。`h3://` 自 M7c 起两种写法都发往 `/dns-query`，不动。
- RFC 8484 5.1：应答带 `Age` 头时，缓存前把各条记录的 TTL 减去它（不低于 0；`Age` 写成列表时取第一个成员，见第 13 节第 4 条）。DoH 与 DoH3 共用 `doh::check_head` 之后的那一段。

### 5.5 `smart` 的失败计分（M3c #11）

现状：经 `smart` 组的会话失败——拨不通、三秒没有回应、上游在回话之前断开——既记进站点记忆，也给成员的全局分数加一次罚分。目标网站本身不通时成员无辜受罚：一个后台应用反复访问一个失效的地址，每次换一个成员去试，就把整组的排序拉乱，访问够频繁时全部成员都被判为失败。M3c 当时把它留给项目所有者，列了三种改法。

决定：(b) 加 (c)（执行时 (b) 改为 (b′)，见第 13 节第 5 条）。

- (b′) 记一次失败时，若同一站点的记忆里有任何未过期的失败记录——别的成员的，或这个成员自己上一次在那里的——，这次只记进站点记忆、不罚全局分数。设计原来写的 (b) 只看**另一个**成员的失败，并发下不成立（同时发起的会话都选中排在最前的成员，那一刻记忆里只有它自己）。失效的站点于是只要还有人在一小时之内接着访问，总共只罚一次；真正坏掉的成员在每个新站点上照样受罚，自愈的速度不变。
- (c) 在本地就被拒绝的目标（名字超长或不能写进代理请求）既不计入也不换成员：换谁都一样（实现为这条会话整个当作不经这个组，见第 13 节第 5 条）。
- 不采用 (a)（只把"连不上成员自己的服务器"计入全局分数）：trojan、vmess、ss 这类不回报目标错误的协议分不清两种失败，一个悄悄吞掉全部流量的成员会一直算健康、只能等测速发现。

测速的结果照旧只看探针，不受影响。

### 5.6 AnyTLS 的 SYNACK 看门狗（M2b 计划期）

现状：复用到一条"半死"的空闲会话（对端已经没了、没有 RST）时，新流的数据发进虚空，要等转发阶段的空闲超时（缺省 10 分钟）才结束。参考客户端（anytls-go）的做法：对端协议版本不低于 2、且不是会话的第一条流时，发出 SYN 后计时 3 秒，期间收到任何 SYNACK 就取消，到时则关闭整条会话。

决定：照做，常数与条件相同（出入见第 13 节第 6 条：只认当前这条流的应答、对端版本取自服务端设置里的 `v=<n>`）。会话关闭后流上的读写以 `anytls: the session is closed` 结束，池不再交出这条会话。请求已经写出、不能透明重试，但 3 秒失败远好于挂 10 分钟。假服务端加一个"收到 SYN 不回 SYNACK"的开关。

### 5.7 WireGuard 的收尾（M4b #14、#20、#23、#28、#31）

只收会让连接白等或让策略用不了的四处，其余（日志用的策略名、端口 0 的报错文字、几条缺的用例）留在遗留事项里：

- 设备任务死掉（协议栈的锁中毒）之后不会被替换，经这条策略的每个连接都 panic 到重载为止（#14）：出站发现设备任务已经结束时换一条新隧道，与"被接替"走同一条重建的路；锁中毒不再向上 panic。
- 隧道被更晚的配置接替或关闭时，正在等它的调用要等满各自的时限才失败——`Device::connect` 等到拨号时限（并不总是等满：排队中的 SYN 立刻就结束，只是错误文本不对，见第 13 节第 7 条）、`Device::query` 等满每一轮、`handshake()`（原生测速）等满 `test-timeout` + 10 秒、`established()` 等到拨号时限（#20、#28、#31，其中 #31 只做了前半，细节见第 13 节第 7 条）：关闭唤醒全部等待者，立即以 `wireguard: the tunnel has closed` 或"已被更新的配置接替"结束。
- 共用隧道时不提升代次：重载 S → S′ → S 而第一条隧道还有连接占着时，S′ 残留的会话会接替一次、切断当前配置的连接（#23）：共用时取较大的代次。

### 5.8 订阅：指向构建失败成员的中继（M3a #13）

导入的策略 X 在注册表构建时失败被略去，而另一条导入策略的 `underlying-proxy` 指向 X 时，它仍留在成员表里，每次拨号都以 `via X: the policy no longer exists` 失败。决定：构建之后再过一遍，把中继链上任何一环已被略去的导入条目与派生条目一并略去（每个因此少了成员的组告警一条，列出至多 20 个成员名与总数、不带任何参数取值；见第 13 节第 8 条），与装配期对悬空中继的处理一致。

### 5.9 出站一侧的 IDN（M2b P15、M4a #7、M4b #8）

- 策略行的 `server` 以 Unicode 写时，生成 spec 的时候按 IDNA（UTS #46，`idna` 已在依赖树里）转成 A-label；转不了是 `E0018`。此后 TLS 的名字、解析、防环用的服务器名单拿到的都是 ASCII。订阅导入的行同样处理（细节与没有转换的名字见第 13 节第 9 条）。
- `ssh` 把目标主机名发给服务器之前、`wireguard` 的隧道内 DNS 发问之前，与其它出站一样先经 `hostname::to_ascii`。

规则、`[Host]` 的键与解析器对 Unicode 目标名的处理不在本里程碑：那要把规则与规则集的域名在加载时一并归一，是阶段 1 组件的改动，留到阶段 3 之后；兼容性清单改写为"出站一侧已支持，规则与 `[Host]` 须写成 `xn--` 形式"。

### 5.10 坏配置时的系统代理恢复（M1b 起登记的阶段 1 遗留）

现状：`rurge run` 遇到加载错误的配置在系统代理的崩溃恢复之前就退出 2。上一次运行崩溃、系统代理还指着 rurge，而这次启动的配置又是坏的，系统代理就一直指着一个没人监听的端口。

决定：配置读不了、有错误或（配置能加载但）日志文件打不开、即将以非 0 退出时，先试一次恢复——数据目录存在、其中有 `state.json`、拿得到实例锁（拿不到——另有实例在跑，备份是它的；或者数据目录根本锁不了——都不碰）时，取后端、恢复备份、清掉备份。恢复的结果直接写到标准错误（这条路径上日志还没有初始化）；恢复失败、备份清不掉或后端的环境变量取值不对只告警，退出码不变（四句话与输出的先后见第 13 节第 10 条）。"先拿锁再恢复"的次序与正常启动一致。

### 5.11 转发循环的 flush 与缓冲（M2b、M2c #1）

现状：转发循环每次读至多 8 KiB、写出后立即 flush（M2b 为了不让 TLS 写端压住最后一条记录）。flush 的成本只在裸 TCP 上量过（那里是空操作），M2b 的终审要求在 TLS / WebSocket / VMess 的栈上实测。

做法：给忽略的吞吐用例加上 TLS（trojan）、WebSocket 与 VMess 三种上游，各在两种策略下跑——现在的"每次写后 flush"与 tokio 的 `copy` 那样"读端暂时没有数据时才 flush"——，缓冲取 8 / 16 / 32 / 64 KiB；用两份预先编好的 release 二进制交替各跑 8 次（顺序测量会把漂移看成差别，M2b 的教训；实际的做法见第 13 节第 11 条）。判据：某种取法在三种栈上都至少快 10% 且没有一种变慢，就换成它；否则保持现状。结果见本节末尾的「结果」一段与计划的「执行期修正记录」。

顺带（M2c #15）：`http` / `socks5` 出站握手里的写在读应答之前补上显式的 flush——下层是会缓冲的一跳时，握手的请求不该等到下一次写才发出。

**结果**（M8c 任务 9，提交 `a96eabf`、`6752afa`，修正轮 `d309f33`、`771b94f`；与上面做法的出入见第 13 节第 11 条）。在 Windows 回环上量：一个线程跑整条链（应用、转发、源站），先上行再下行在同一条会话上，四个栈——裸 TCP、TLS（trojan）、WebSocket、VMess——乘四种缓冲，两遍各 8 轮，按两遍 16 轮合计取中位数。数字是这台机器的：基准进程只用了约 0.3 个核，说的更多是每字节的读写次数，不是计算量；纯 TCP 的 110 MiB/s（0.92 Gbps）是整条链在一个线程上的吞吐，不是 NFR-01 的测量。

- flush 的时机没有差别：「读端没有数据时才 flush」÷「每次写后 flush」的 16 个比值在 96.5% ～ 102.0%，平均 100.3%，正负都有、不随缓冲或栈变化；读代码也是这样，三个加密流在没有积压时 `poll_flush` 是空操作。M2b、M2c 留下的问题就此有了答案：每次写后 flush 保留。
- 缓冲越大越快，相对 8 KiB（16 / 32 / 64 KiB）：裸 TCP 128.0% / 146.1% / 152.8%，TLS 110.2% / 112.3% / 114.8%，WebSocket 108.1% / 111.2% / 110.3%，VMess 104.0% / 108.9% / 116.1%。加密栈的增益全在上行方向（基准的假服务端每次只转或读 8 KiB，下行用不上大缓冲）；VMess 在 16 KiB 上几乎不动（`MAX_PAYLOAD` 是 16384 − 16，写满 16 KiB 就成一个满块加一个 16 字节的尾块）。
- 判据（三个加密栈都至少快 10%、裸 TCP 不慢于 97%）按两遍 16 轮合计读，**64 KiB 满足**（WebSocket 上只高出 0.3 个百分点）。只读一遍则结论不同：第 1 遍单看是「读端空闲时 flush、32 KiB」（WebSocket 上 110.1%），第 2 遍单看只有 64 KiB 过线，每次过线都只高出 0.1 ～ 0.3 个百分点，而同一组合各轮的极差是中位数的 12% ～ 57%。
- **缺省没有改**，仍是每次写后 flush、8 KiB。PRD 的 NFR-01（1 万并发连接常驻内存 < 300 MB）在里程碑设计之上：每条会话从建立到结束一直拿着两块缓冲，8 KiB 是 156 MB，16 KiB 是 312 MB，64 KiB 是 1.22 GiB。自适应缓冲（从 8 KiB 起、某个方向接连把它读满时才换大）是新设计，要有自己的内存与吞吐实测，移交阶段 3 建立性能基准时一并定。基准 `relay_throughput` 留在仓库里（`cargo test -p rurge-engine --release --lib relay_throughput -- --ignored --nocapture`；第一版带 flush 一维的整张矩阵约 26 ～ 27 分钟，`RELAY_BENCH_MIB=<n>` 可缩小；不作门禁）。

### 5.12 小项

- SOCKS5 客户端发往 IPv4 映射地址（`[::ffff:a.b.c.d]`）的 UDP：入站把目标归一成 IPv4，回包按来源归流时才对得上（M7b #8；执行时扩到 HTTP 入站的目标、双栈监听上 TCP 客户端自己的地址与出站报回的来源，见第 13 节第 12 条）。
- `rurge-proto-wireguard` 与 `hysteria2` 用例里剩下的墙钟断言与没加时限的等待（M7c #33、M7b #16；数出来是五处与十二处）。

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

决定：阶段 2 结束时报告 22，照需求 FR-CFG-08 写下的方案（M3c 延后事项 #4，原留给项目所有者）；已执行（M8c 任务 10，`f58678b`；见第 13 节第 13 条）。22 在手册里唯一的含义是 Smart Group，rurge 已有；20 所指的 Body Rewrite 与内联 Map Local 自阶段 1 起就以"节解析但不生效"（`W0016`）对待，上调不改变这一点。

### 6.4 手册基线

本项目的兼容基线是 2026-09 的手册（Surge Mac 6.9 / iOS 5.22）。调研时手册已是 2026-10-07 版，出现了 Mac 6.10 / iOS 5.23 的条目，页面路径也换了（策略页在 `policies/<协议>.html`）。顺带看到的几处：`test-url` 接受 HTTPS（rurge 的探针本来就支持）、`[IP Rewrite]` 节、策略组的 `category` 参数、`[Snell Server]` 可由模块提供。系统的比对不在 M8：阶段 3 开工前单独做一次，结果进兼容性清单；M8 只登记本文用到的几处，并把 CLAUDE.md 里的手册路径示例改对。

## 7. 遗留事项的处置

阶段 2 的 20 份计划各有一张「延后事项」表，合计约四百行。M8 的文档任务把它们收成一张总表（`docs/superpowers/plans/` 下 M8c 计划的附录），每行标上去向。这里列的是标给 M8 的与原先留给项目所有者的，其余按表里原有的去向不变——"有用户报告再说"的仍是已知限制，"阶段 3"的随阶段 3 的设计承接。M8c 执行中的变化（多做掉的几行、一行改为关闭、交给项目所有者决定的几件）见第 13 节第 14 条。

**M8 做掉**（括号里是本文的小节）：M7c #9、#10（5.4），#14（5.1），#15（5.2），#24、#25、#34、#36（3.3），#28 ～ #32（5.3），#33（5.12），#35（3.5）；M7b #8、#16（5.12）；M7a #8、#13（4.5）；M6a #1（4.1），#7（4.5、3.3）；M6b #1 的 v1 ～ v3（4.3）；M4b #14、#20、#23、#28 与 #31 的前半（5.7）；M4a #7（5.9）；M4b #8（5.9）；M3c #4（6.3），#11（5.5）；M3b #21（3.3）；M3a #5（3.4），#13（5.8），#19、#23、#31（3.3）；M2c #1（5.11），#15（5.11）；M2b #1 的 SYNACK 看门狗（5.6）、#3 的 `Secret<T>` 覆盖面（3.3）、#16、#30 的 flush 成本（5.11）、#2（P15）的出站一半 IDN（5.9）、#4 的 VMess 旧握手（4.2，以"不实现"结案）；M1b 的坏配置与系统代理恢复（5.10；同一缺陷在 M2c #6、M3a #9 各登记过一次）；M2b #23 / M3a #4 / M3b #7 的异常退出（3.5）。执行中增补与改动的几行——M4b #31 只做前半、M2b #2 只做出站一半、多出来的"M8 做掉"——见第 13 节第 7、9、14 条。

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
| M4b #9、#10、#11、#13、#17、#22、#25 与只缺用例的几条（#16、#18、#21、#29） | 同名的 `[WireGuard]` 节、端口 0 的报错文字、最后一个连接的 FIN、启动等每个 peer 的载体、SERVFAIL 等满一轮、共用隧道的日志名、日志级别提示 | 有用户报告再说 |
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
5. DoH：URL 里的用户名口令以 `Authorization: Basic` 发出（Surge 的做法没有写明）；主机之后没写路径时发往 `/dns-query`（名字不带补上的路径，是规范形式——主机小写、缺省端口与用户名口令去掉）；应答的 `Age` 计入 TTL（写成列表时取第一个成员）。（第 13 节第 4 条）
6. QUIC 出站的接收窗口：不设流数上限的会话每条连接 256 MiB；同一条连接上 29 条以上的隧道同时停住且各攒满 8 MiB 时仍会互相拖住（第 13 节第 1 条）；`trust-tunnel` 的 HTTP/3 的连接窗口是 ⌈`max-streams` × 8 MiB × 8/7⌉。256 MiB 是上界、不是预留；一个 QUIC 策略能占多少内存对着 NFR-01 说不过去，是留给项目所有者的一条陈述（第 13 节第 1 条）。
7. `masque` 经 `underlying-proxy`：连接从 1350 字节的包起步；带不动的路径上在四批只丢大包之后退回 1200（每条连接各自退回，此后不再升回），那时 UDP 负载不到 1200 字节（内层 QUIC 不通）；缺省 `mtu` 的 WireGuard 经它的行为没有实测。（第 13 节第 2 条）
8. `smart`：同一站点的记忆里有任何未过期的失败记录时（(b′)，第 13 节第 5 条），失败只记进站点记忆；本地就拒绝的目标整条会话不经这个组：不计入、不换成员。
9. `anytls`：SYNACK 看门狗（3 秒，对端协议版本 ≥ 2、非会话的第一条流）。
10. 策略行的 `server` 以 Unicode 写时转成 A-label；规则与 `[Host]` 须写成 `xn--` 形式。`[WireGuard]` peer 的 `endpoint`、`sni` 等以 Unicode 写时不转换（`endpoint` 直连时被解析器拒绝），"出站一侧已支持"那句要把它们除外（第 13 节第 9 条）。
11. `rurge run` 遇到加载不了的配置时，也恢复上一次崩溃留下的系统代理设置（配置读不了、有错误或日志文件打不开；第 13 节第 10 条）。
12. 布尔环境变量的取值；`CORE_VERSION` 为 22。
13. 诊断、`Debug`、日志与脱敏输出里的凭据（第 3.3 节的表；M8c 又收了几处，第 13 节第 16 条；终审之后的最终修正波做的见第 13 节第 20、21 条与本节第 16 ～ 18 条）；`russh` 自己的日志只留 WARN 及以上（M4a #11 原留给项目所有者，由控制者代定）。

14. M8c 执行中另外产生的、需要登记的：IPv4 映射的 IPv6 地址（`::ffff:a.b.c.d`）当它指的 IPv4 地址，在这些地方——SOCKS5 的目标（地址类型 4 或写成名字的文本）、HTTP 的 `CONNECT` 与绝对 URI 的主机、双栈监听上 TCP 客户端自己的地址，以及比较出站报回的来源与目标的四处（两侧先归一）：引擎 UDP 流水线的 `receive`、`packet_datagram` 的接收任务、`udp_probe`（`test-udp`）、`rurge-proto` 的 `from_relay`（SOCKS5 UDP 与 Shadowsocks UDP 共用）。没有归一的：IP 规则对 DNS 应答的比较（只比较每个地址族的第一个地址，`::ffff:` 形式的 AAAA 不归一，阶段 1 的设计）。另有：`external-policy-modifier` 按策略行解析器的读法认键，行尾一项留着未闭合的引号或括号时追加的参数写在它前面；`[General] test-timeout` 再大也只是等得久；`vmess` 的 UDP 连接被服务端一建成就关时一个数据报至多再拨一次；`wireguard` 上合法的 `tfo=true` 只得到 `W0028`；`russh` 的日志只留 WARN 及以上（M4a #11 原留给项目所有者，由控制者代定）；AAAA 抑制的现行计数是待定行为（第 13 节第 15 条）。脱敏后的配置、诊断与资源刷新的差异见下面第 15 ～ 18 条。

15. 刷新间隔：`update-interval=0`（规则集、域名集）读作没写，按缺省的 86 400 秒刷新——手册没有说明 0 的含义（做完就又到期的刷新会让任务以一次 HTTP 往返的速度反复抓取）；同一个来源被多处引用时，0 不参与取最短。策略组的 `update-interval` 不是这样读的：它要求正数，`0` 是 `E0018`（``policy group `G`: invalid value `0` for `update-interval` (expected a positive number of seconds)``，`crates/rurge-config/src/spec/group.rs`）；两种读法该不该一致，是项目所有者的问题（计划「延后事项」第 99 行）。没有缓存时服务器对不带验证头的请求答 304，算一次失败的抓取（状态 `failed`，说明 ``the server answered "not modified" but nothing is cached``），照失败退避；有缓存时的 304 照旧只更新抓取时间。自动刷新关闭（`update-interval` 为负）的 URL 任务每分钟醒一次、只看间隔而不抓取：同一个 URL 的另一处引用并入的正间隔只有靠这次醒来才被看见，管理器消失由 `gone` 立刻通知（第 13 节第 22 条）。流量采样错过的秒数（机器休眠）不补采：速率是最近两次采样之差，醒来后第一次读到的是这一段里走的字节（第 13 节第 23 条）。

16. 脱敏后的视图（`GET /v1/profiles/current` 不带 `sensitive=1`、`policies/detail`、原样保留的行的 `Debug`）。**尽力而为，不是没有秘密会出现的保证**（第 13 节第 20 条）；逐条以 `crates/rurge-config/src/redact.rs` 为准，下面的清单从那里抄来。
    - **隐去**。(a) 整行键的整个值：`password`、`ca-passphrase`、`ca-p12`、`private-key`、`psk`、`pre-shared-key`、`preshared-key`、`token`、`auth-key`、`proxy-test-url`、`internet-test-url`、`geoip-maxmind-url`、`secret`（`[MTProto]` 的键）、`passphrase`（`SECRET_KEYS` 的 14 个）——键名不分大小写、不计 `-` 与 `_`；键是分隔符前面的那段文字，它**以**清单里的键结尾就算（所以 `；ca-passphrase = hunter2`、`# was http-api = KEY@127.0.0.1:6171`、`# old: ca-passphrase = hunter2`、`"password" = hunter2` 是键行，`my.token = 1.2.3.4`、`password-hint: none` 不是）；分隔符是 `=`、`＝`、`:`、`：`，第一个 `=` 之前的每个冒号都看；键前面另有文字时取值就地换成 `***`；取值是以 `#` 开头的秘密（`ca-passphrase = #Secret1`，节解析器读成注释）时注释的文字换成 `***`、标记保留。`http-api` 与 `external-controller-access` 的整个值只在最后一个 `@` 处切（不按逗号切）；`wifi-access-http-auth`（`用户名:口令`）不按 `@` 切：用户名留着，第一个冒号之后换成 `***`（`wifi-access-http-auth = alice:hunter2` 显示成 `wifi-access-http-auth = alice:***`），值里没有冒号时整个值换成 `***`；`http-listen`、`socks5-listen` 与三个 DNS 键（`dns-server`、`encrypted-dns-server`、`doh-server`）逐项处理，每项最后一个 `@` 之前换成 `***`；三个 DNS 键只在前面除空白外没有别的文字时才是整行键，前面另有文字——`[SSID Setting]` 的行、`old dns-server = …`——按策略定义读：URL 的用户信息换成 `***`，其余原样。
    - (b) 行内参数（`SECRET_PARAMS` 的 24 个）：`password`、`passphrase`、`psk`、`private-key`、`pre-shared-key`、`preshared-key`、`base64`、`token`、`uuid`、`username`、`headers`、`ws-headers`、`ws-path`、`shadow-tls-password`、`salamander-password`、`gecko-password`、`policy-path`、`external-policy-modifier`、`test-url`、`url`、`args`、`argument`、`obfs-host`、`obfs-uri`。条目照解析器的读法认（`split_list` 去掉整个条目外面的引号，`parse_key_value` 在第一个 `=` 处切开；整个条目写在引号里的 `"password=x"` 显示成 `password=***`；括号组逐项处理）；按名字搜索（注释里的秘密、读不成一项的取值）认 `=`、`＝`、`:`、`：`，名字前面是行首、ASCII 空白、逗号、`(`、引号或任何非 ASCII 字符。
    - (c) 位置凭据：类型取解析器读出的第一个字段；带服务器与端口的策略以及第三个字段是端口数字的任何行，从第四个字段起不是具名参数的字段一律抹掉；类型后面漏了逗号时（`P = http h.example, 8080, alice, hunter2`）这些词按前几个字段读——只在拆开之后第三个字段是端口号（`u16`）时。
    - (d) URL：每个 `scheme://authority` 的 authority 里最后一个 `@` 之前换成 `***`；scheme（`://` 前面以字母开头的那一串）不是 `http`、`https`、`h3`、`quic`、`tls`、`tcp`（`READ_SCHEMES`）的 URL，从 `://` 到下一个空白整段换成 `***`；注释文字里的 URL 只显示 scheme 与 authority，其后到下一个空白的部分换成 `/***`。
    - (e) `#!` 行按形状：行首的字节顺序标记与空白原样；指令名是 `#!` 加字母、数字、`-`、`_`；`#!MANAGED-CONFIG` 之后只显示写成 `interval=<数字>`、`strict=<字母>` 的词；别的 `#!` 行含 `://` 的词换成 `***`；`#!include` 按逗号分目标，含 `://` 的目标换成 `***`、本地路径保留。行首的要求指令（`#!IOS-ONLY`、`#!MACOS-ONLY`、`#!TVOS-ONLY`、`#!REQUIREMENT <表达式>`）原样保留，其后的内容当普通的一行脱敏。被注释掉的行与行尾的注释（嵌套的跟随四层）照"没有被注释"那样脱敏。
    - **有意多隐的**（不是缺口；例子取自 `redact.rs` 的用例，读代码得出而没有用例钉住的另行写明）：名字以清单里的键结尾的策略、策略组或脚本（`HK Token`、`Work Password`、`Refresh Token`：读代码得出，没有用例钉住；同一条规则在注释里有用例，`// the token = t0k3n was` 显示成 `// the token = ***`）显示成 `<名字> = ***`；`password:`、`token:`、`url:`、`args:` 后面的散文（`# an old password: hunter2` 显示成 `# an old password: ***`）；以协议类型词开头、第三个词恰好是端口号的注释句子（`# http server 8080 is the proxy port` 显示成 `# http server 8080 *** *** *** ***`：读代码得出，没有用例钉住，同一条规则在没有逗号的策略行上有用例，`P = http h.example 8080 alice hunter2` 显示成 `P = http h.example 8080 *** ***`；`# HTTP DNS blocking list`、`// ss node via Hong Kong` 自 `087f399` 起原样）；scheme 紧跟在以字母开头的转义之后的正则（`^\w+://example\.com/x` 显示成 `^\w+://***`；`^[a-z]+://example\.com/path` 与 `^.+://example\.com/(.*)` 自 `087f399` 起原样）；注释里 URL 的路径（`# https://github.com/owner/repo` 显示成 `# https://github.com/***`）；scheme 不是上面六种的整个 URL（重写目标 `myapp://item/$1` 显示成 `myapp://***`），连同粘在后面的逗号。
    - **有意留着的**：不是注释的行上规则集的 URL（含路径与查询串；规则集到处以 URL 命名）、`encrypted-dns-server` 的路径与查询串、`[General]` 里不认识的键的取值、`[Script]` 的 `script-path` 的 URL、`[URL Rewrite]` 与 `[Body Rewrite]` 各行的文字、`[Header Rewrite]` 的头值与 `[Map Local]` 的 `header=` / `data=`（`argument=` 自 `b4a5759` 起隐去）。
    - `lineHash`（`GET /v1/policy_groups` 的成员；脱敏后定义的哈希）对每个定义被现在的脱敏显示得不一样的策略变了一次。已知没有覆盖的见计划「延后事项」第 58 ～ 77 行。

17. 不再引用用户写的东西的诊断（精确的文本取自三轮的报告；"引用"指把用户写的取值放进反引号）。
    - ``invalid value `<取值>` for `<键>` (expected …)``（`E0018`）：取值不含空白、不含 `=`、`＝`、`，`、至多 64 个字符时才引用，否则 ``invalid value for `<键>` (expected …)``；策略与策略组共用，组的 ``invalid `policy-regex-filter` `<取值>`: …`` 同（不满足时 ``invalid `policy-regex-filter`: …``）。
    - ``<键> references unknown <种类> `<名字>` ``（`underlying-proxy` 的策略与策略组、`include-other-group` 的策略组、`private-key` 与 `client-cert` 的 keystore 项）：名字满足同一条规则且不是链接时才引用，否则 ``<键> references unknown <种类>``。
    - 含 `://`、全角 `＝` 或全角 `，` 的文本不是名字，诊断引用策略、策略组、组成员、规则里的策略、keystore 项的名字时不引用它：``rule references an unknown policy``（`E0007`）、``policy group `<组>` references an unknown policy``（`E0008`）、``keystore item: missing `base64` `` / ``keystore item: unknown type …`` / ``keystore item: `base64` is not valid Base64``（`E0001` / `E0011` / `E0021`）、``policy: …`` 与 ``policy group: …``、``policy groups a link and a link form a cycle; they behave as REJECT``（`W0030`）、``policy `P`: `underlying-proxy` references unknown policy``（没有名字）、``… keystore item, but the item is `p12` ``（`E0020`）；顺带提到的名字写成 ``a link``（``(via a link)``）。名字里有空格、emoji、CJK、`|`、`-`、`.` 的是普通名字，照旧引用。
    - ``unknown parameter `<名字>` ignored``（`W0001`）：名字写成参数名——小写 ASCII 字母、数字、`-`、`_`，至多 32 个字符——时才引用，否则 ``unknown parameter ignored``；策略与策略组共用。``unknown [Keystore] field `<名字>` ignored``：同一条规则，没有名字的字段 ``unknown [Keystore] field ignored``；``keystore item `<名>`: unknown type `<类型>` ``：类型满足上面的取值规则时才引用，否则 ``keystore item `<名>`: unknown type``。``unknown [General] key `<键>` ignored``（`W0001`）：键写成键名——ASCII 字母、数字、`-`、`_`，至多 64 个字符——时才引用，否则 ``unknown [General] key ignored``。
    - ``<类型>: missing value`` / ``<类型>: missing policy``（`E0001`）与 ``unknown rule type `<类型>` ``（`E0012`）：类型（行的第一个字段，转成大写）长得像规则类型——ASCII 字母、数字、`-`，至多 32 个字符——时才引用，否则 ``missing value`` / ``missing policy`` / ``unknown rule type``；`[Ruleset …]` 里转述的 ``line skipped: …`` 同。``policy `<名>`: unknown type `<类型>` ``（`E0004`，策略与策略组共用）：名字是取值为秘密的键（`psk`、`password`、`ca-passphrase`、`token`、`private-key`、`http-api`、`http-listen`、`wifi-access-http-auth` 等，与脱敏认的是同一份，以这样的词结尾也算）时不引用类型：``policy `psk`: unknown type``。
    - `[WireGuard]` 的 peer：``invalid `endpoint` `<值>` (expected host:port)``、``invalid `keepalive` `<值>` (…)``、``invalid `client-id` `<值>` (…)``、``invalid `allowed-ips` entry `<条目>` ``——取值满足上面的规则时才引用，否则去掉反引号里的取值（``invalid `endpoint` (expected host:port)``）；``` `dns-server` `<URL>`: an encrypted-DNS URL is not accepted here ``` 与 `[Host]` 的 ``invalid DNS server `<URL>` `` 里的 URL 过 `url_without_credentials`。
    - ``unknown directive ignored: #!<名字>``：名字是 `#!` 之后紧跟的一段 ASCII 字母、数字、`-`、`_`，超过 32 个字符时不引用，只说 `unknown directive ignored`；``unknown MANAGED-CONFIG parameter `<名字>` ``：名字是不超过 32 个字符的小写字母与 `-` 时才引用，否则 ``unknown MANAGED-CONFIG parameter (not written as a lower-case `name=value`)``；`invalid interval` / `invalid strict`：取值是不超过 20 个字符的 ASCII 数字（interval）/ 字母（strict）时才跟取值；`remote include not supported yet`（`W0011`）不再引用目标。
    - `Debug`：`general::UnknownKey` 显示脱敏后的行，`KeystoreItem` 只显示未知字段的个数，`text::Directive` 与脱敏后的配置一致，`PolicyGroup` 与 `GroupSpec` 里不是名字的成员显示 `***`。

18. 视图之外的显示与行为：规则文本（规则列表、请求记录与会话日志里的 `rule`、`rurge rule match` 的文本与 `--json` 输出、`Config::summary()` 的 `rules`）里，`RULE-SET` 与 `DOMAIN-SET` 的取值——逻辑规则、内联规则集与规则集文件里的同类行也一样——若是写着用户名与口令的 URL，authority 里最后一个 `@` 之前的部分显示为 `***`（`https://***@host/…`），路径、查询与片段原样（其中的 `@` 不是凭据）；日志与诊断里集合的名字、`[Host]` 的 `DOMAIN-SET:` / `RULE-SET:` 键同样。rurge 的 HTTP 客户端不把 URL 里的用户名口令当 Basic 认证发出，所以别的客户端带来的这种行会因 `http status 401` 抓取失败（DoH 自 M8a 起发出这些凭据）。`[Host]` 下含 `://` 又不是这两种集合键的键（贴进来的分享链接）是 `E0017`（``expected `<host> = <value>` ``），此前加载成一条永远匹配不到的别名。

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

## 13. M8c 实施期的订正

M8c（第 5 节与第 6.3 节）按 `docs/superpowers/plans/2026-10-09-phase2-m8c-leftovers-acceptance-plan.md` 做完：任务 1 ～ 11 的代码，加上执行中新加的任务 13、14、15（第 14 条），都在提交 `28340f3` 之前——那是交给整分支终审的冻结点——；任务 12 是文档。冻结之后有一次最终修正波（九个提交，到 `087f399`，第 19 ～ 23 条），任务 12 的 A 部分在它之后又补了一次，那一次经评审订正过（计划「执行期修正记录」第 94 行）；对更早的文档的订正与旁注见第 24 条，需求审计见第 25 条。下面是与第 5、6、7、8 节的文字不同的地方，以代码为准。第 5 ～ 8 节的原文作为计划期的记录留着，只有已经不成立的几处就地改了，各在所属的条目里注明。逐项的理由与提交见该计划的「执行期修正记录」，留下的事项见它的「延后事项」，阶段 2 全部 403 行遗留事项的去向见 `docs/superpowers/plans/2026-10-09-phase2-leftovers.md`。M8c 的提交没有推送过，CI 没有在它们上面跑过，互操作夹具的改动没有对真实的参照服务端跑过；下面凡写"读源码"的结论，都没有在别的平台上验证。

1. **接收窗口**（5.1，已就地改）。
   - 不设流数上限的会话：连接窗口 256 MiB、每流 8 MiB，照设计做了；`Transport::DNS` 与 `capped_transport` 的 20 MiB 各自写成常数，不再随 `Transport::STANDARD`。
   - 登记的限制是 **29** 条，不是 32 条。quinn（quinn-proto 0.11.19 的 `add_read_credits`）攒到配置窗口的八分之一才公布一次连接级额度，所以没人读的数据超过窗口的八分之七（224 MiB）时，其余隧道才被拖住——也就是 29 条攒满 8 MiB 的隧道（232 MiB；28 条是 224 MiB，还不够）。
   - "有流数上限的会话不变"不成立。同一条规则让 M7c 的 max(`max-streams` × 8 MiB, 20 MiB) 在 `max-streams` ≥ 9 时保不住"其余隧道都停住时最后一条仍在读"；`trust-tunnel` 的 HTTP/3 的连接窗口改为 ⌈`max-streams` × 8 MiB × 8/7⌉、至少 20 MiB（缺省 `max-streams` 为 3 时 28,760,942 字节，原 24 MiB）。窗口是 u32，从 `max-streams` 448 起饱和在 `u32::MAX`，那条保证到 449 为止成立、从 450 起保不住；一条连接最坏缓冲的未读数据仍是 `max-streams` × 8 MiB（各流自己的窗口封顶）。每流窗口不变；HTTP/2 池的窗口不需要这个余量（读 `h2` 0.4.19 的流控）。
   - **内存，对着 NFR-01**（终审记下，留给项目所有者；一条陈述，不是改动的建议）。256 MiB 的连接窗口是上界，不是预留：一条连接占的内存是 min(256 MiB, 8 MiB × 有未读数据的隧道数)，一个 QUIC 策略除此之外没有自己的上界（被取代而因租约还活着的连接要再加上它自己的）。同一个 QUIC 策略上 19 条停住的隧道就超过 NFR-01 的 300 MB 在 1 万个会话的转发缓冲（156 MB）之后剩下的——MB 与 MiB 不分的粗算。任务 9 以 NFR-01 拒绝了 64 KiB 的转发缓冲（第 11 条）；任务 1 的窗口从没对着 NFR-01 量过（计划「延后事项」第 53 行）。

2. **MASQUE 经链式载体**（5.2，已就地改三处）。
   - 经载体的连接从 1350 字节起步、`min_mtu` 仍是 1200，照设计做了。接线的形状不同：`Transport::carried_initial_mtu`、`Transport::apply_carried`、`Path::is_carried()`，`rurge-proto-quic` 的 `Dialer` 留直连与经载体两份 `quinn::ClientConfig`，按真实打开的路径选一份；`quic::connect` 与 `rurge-dns` 没有动。
   - 黑洞检测要**四**批"只丢大包"（quinn 的阈值是超过 3 批），不是三批。回落是每条连接各自的、此后不再升回：窄路径上每建一条连接都先丢一轮满包。回环上（debug 构建）回落用了：UDP 驱动 6 次 187 ～ 226 毫秒，TCP 驱动 3 次 158 ～ 217 毫秒。回落完成之前超过余量的包悄悄丢在路上，完成之后被拒绝（错误是 `masque: a datagram longer than <余量> bytes`，回落到 1200 之后余量是 1160）。
   - "缺省 MTU 的 WireGuard 也带得动"不成立：rurge 的缺省 `mtu` 是 1280（与 Surge 一致），隧道里的 UDP 负载至多 1252（IPv4）/ 1232（IPv6）字节，放不下 1350 字节的数据报。放不下时是分片送出、整包送出还是被丢弃而回落到 1200，只读了 smoltcp 0.12 的源码，**没有实测**，文档不写成事实。"要把 `mtu` 设到 1280 以下"也改了：`masque` 自己经载体的数据报余量是 1310（服务端连接 ID 8 字节）/ 1298（20 字节）字节，WireGuard 的包比内层 IP 包多 32 字节，1280 的满包（1312 字节）差 2 ～ 14 字节放不下，要设到 1264 以下，同样是按余量算出的、没有实测。

3. **载体的错误**（5.3，已就地加指针）。
   - 第 5 条第三项："endpoint 没了"是 quinn 的**两种**握手错误——`DRIVER_LOST` 的传输错误与 `LocallyClosed`（连接的驱动退出时，隐式关闭把前者改写成后者）；握手期间两种读作同一句 `quic: the UDP path to the server failed`。
   - 第 2 条做成不看耗时的转发用例，三层各一条（`FinishOnDrop`、`CountingDatagram`、`Relayed`）。第 1 条的规则是"试两次都收不下、且载体两次都说可发才丢；第二次之后载体说发不了的留给 quinn 等"。
   - 第 3 条：前缀只加在 `ChainConnector::connect_udp` 交出的载体上，两层中继时里层载体打开之后的错误只带外层的名字。第 4 条：载体有自己的话（自己的文本，或系统错误）才留作结束原因；载体自己因 `ConnectionReset` 而**结束**时（`trojan`、`anytls` 的流上 UDP 载体在服务端重置连接时就是这样）以 `ConnectionAborted` 加同样的文本交出——"错误的种类不变"唯一的例外，否则 quinn 把它当作被弹回的一个数据报跳过，用户拿到固定的那句而不是原话。第 5 条里"不给成员记失败"对 `smart` 只在 `dns_pipeline` 的单元用例里用真的 `SmartBook` 测。
   - Windows 把已连接 UDP socket 的 ICMP 端口不可达报成下一次接收的 `ConnectionReset`；Linux 交给下一次发送，macOS 的接收拿到 `ConnectionRefused`（后两条是评审读出来的，没有跑）。端到端用例只在 Windows 上编译与运行。

4. **DNS**（5.4，已就地改两处）。
   - "没写路径"的判据取自写出来的文本：`https://` 之后直到结尾没有 `/`、`?`、`#`，另外也数 `\`（`url` crate 把 `https` URL 里的 `\` 当 `/`）。没写路径的上游的名字就是写出来的样子（`https://dns.example`），此前显示成 `https://dns.example/`；名字是规范形式（主机小写，缺省端口与用户名口令去掉），`h3://dns.example` 的名字仍带 `/dns-query`；自己托管、在 `/` 上应答的服务端从此必须明写 `/`。
   - `Age`：计划期决定 P5 的"有多个时不动"被取代，按 RFC 9111 5.1 取列表的第一个成员（两侧去掉空格与制表符、跳过空成员；该成员不是非负十进制整数时这个头才不算，装不进 `u32` 的按 `u32::MAX`）。TTL 在报文原处改写（`message::lower_ttls`：应答、权威、附加三个段，OPT 不动；走不到最后一条记录的报文原样交出），不经 hickory 重新编码；减到 0 的应答这一次照用、不进缓存。留下的：最高位为 1 的 TTL（RFC 2181 第 8 节）按普通无符号数处理，与各传输读 TTL 的地方一致。

5. **`smart` 的失败计分**（5.5，已就地改；设计 D13 与第 8 节第 8 条随之改）。
   - (b) 改为 **(b′)**：记下一次失败的那一刻，站点的记忆里没有任何未过期的失败记录——不论是别的成员的，还是回报者自己上一次在那里的——才另算成员自己的失败；"看"在回报者自己的条目被替换之前，与"记"是同一次调用、同一次加锁。原因：设计的 (b)（只看**另一个**成员）在并发下不成立——同时发起的 6 条会话都选中排在最前的成员，那一刻记忆里只有它自己，它被罚 6 次、判为失败。后果：一个失效的地址只要还有人在一小时之内接着访问，总共只罚一次（每次失败，被豁免的也是，都刷新记忆，一小时从最后一次失败算）；其后别的成员在那里的成功不结束豁免；回报者自己上一次是成功，下一次失败照罚；N 个不同的失效地址仍罚 N 次，摊在成员身上（没有采用 (a) 的代价）；只有一个可用成员的组里，同一站点上反复失败的成员不再被会话判为失败；被豁免的失败不碰成员的记录，包括 `reported` 标记；站点记忆按策略名记，提前丢条目（站点数超过 4096、一个站点超过 16 个策略、重载去掉了策略）时，同一站点一小时内可能再罚一次。
   - (c) 实现为：目标发不出去（域名转不了 ASCII，或转出来超过 255 字节）时，这条会话**整个当作不经这个组**——不回报、不换成员，首字节的样本与使用计数也不记，TCP 与 UDP 相同。判断落在引擎的拨号路径上（`unsendable`），不给 `OutboundError` 加变体。

6. **AnyTLS 的看门狗**（5.6，已就地加指针）。"常数与条件相同"之外的出入：只认**当前这条流**的 `cmdSYNACK`（参考客户端任何 `cmdSYNACK` 都解除）；流在应答之前被放下时时限照旧，回到池里的会话到时同样关闭；对端版本取自服务端设置里的 `v=<n>`（没收到设置按 1）；时限先起、再写 SYN；请求不重试，3 秒的失败出现在转发阶段，不是 502。继承参考客户端的后果：服务端拨通目标之后才回 `cmdSYNACK`，所以复用的会话上目标 3 秒拨不通时同样在 3 秒失败；一次大的传输被中断后，下一个请求的应答排在旧流的数据后面，慢的链路上可能在 3 秒时失败；服务端从未发过设置的会话从不受监视。UDP：参考服务端对 UDP over TCP 的流读到 UoT 请求之后才回 `cmdSYNACK`，rurge 的请求随第一个数据报发出，而看门狗对 UDP 流仍在 `open` 时起表，所以"开了、没发"的载体会让所在的那条池中的会话在 3 秒后被关——`wireguard` 经 `underlying-proxy` 到 `anytls`、endpoint 写成域名时，隧道空闲的五分钟重拨拨出又丢掉的载体就是这样（隧道本身不受影响）；隧道启动时遇到已有的共用隧道、打开载体的过程中被取消的 UDP 流、第一个包被载体拒绝，三处同样的代价。第 9 节 V7 的答案：按会话记一个时限，因为 rurge 的 AnyTLS 会话同一时刻只承载一条流。**看门狗没有对真实服务端验证过**（终审记下）：互操作用例在几毫秒内放掉每条流，UDP 用例用新会话的第一条流，看门狗不看它；假服务端应答的次序是照参考实现的源码写的，第一次 CI 运行也改变不了这一点。会验证它的是一条在复用的会话上活过三秒的流的互操作用例（计划「延后事项」第 87 行）。

7. **WireGuard 的收尾**（5.7，已就地加指针）。隧道只有一种结束方式（`Ending::{Superseded, Closed}`，`Shared::end`），三个入口：被更晚的配置接替；设备任务以任何方式结束（含 panic）；协议栈的锁中毒。与设计的出入：
   - "最后一个持有者放手时关闭"不可能有等待者（每个等待者自己持有 `Arc<Device>`），所以 `the tunnel has closed` 用设备任务死掉与锁中毒这两种等待者看得到的关闭来测。
   - `connect` 以前并不总是等满拨号时限：排队中的 SYN 立刻结束，只是文本不对，现在是 `wireguard: a newer configuration of this tunnel is in use`。
   - 隧道结束后它上面已有的 TCP 连接读到 reset、写得 broken pipe（只有锁中毒才是 `wireguard: the tunnel has closed`），等待者与 UDP 载体读到那两句；还没发过数据报的 UDP 载体同样当场结束。
   - 隧道意外结束（不是被接替）后，策略在下一个连接到来时记一条 warn；载体的两把小锁中毒不再 panic（旧代码在 `Drop` 里二次 panic，进程中止）；M4b #12：假对端沉默时不再空转。
   - M4b #31 只做了前半（隧道被接替时 `established()` 立刻返回）；后半——多个 peer 时只等任一 peer 的握手——还在。留下的限制：隧道死掉到当前配置的下一个连接之间，旧配置里与之冲突的残留出站可能自己起一条隧道。

8. **订阅里指向构建失败成员的中继**（5.8，已就地加指针）。派生条目 `M (via X)` 在真实配置里写不出来（组级 `underlying-proxy` 只能指向主配置的策略或组，主配置的策略构建失败是整代失败），规则照做、用例手工造装配结果；一个成员全都挂在被略去的中继上的组成了空组（`--empty-group-reject` 没开时回落 DIRECT 并告警，作中继时一律拒绝，此前经它的每次拨号都失败）。告警每个因此少了成员的组一条，字段 `group`、`count`、`members`，至多列 20 个名字（第一版列全部成员名，而一个订阅至多 10,000 条）。

9. **出站一侧的 IDN**（5.9，已就地加指针）。ASCII 的 `server` 不经 `to_ascii`（设计写的是"以 Unicode 写时"，计划写"是域名时"，按设计）：全转会让今天能加载的行变成加载错误。于是字母表之外的名字有三种结局：没有 ASCII 形式的 Unicode 名是加载时的 `E0018`（全是点的，如 `。`，同样）；ASCII 的字母表之外的名字（`a@b.example`、带空格）在需要它做 TLS 名字或 WebSocket `Host` 时是构建错误（`E0022`），否则能加载、到第一次拨号才失败（解析器编不出它；经 `underlying-proxy` 时下一跳以 `<协议>: the host name cannot be sent to the server` 拒绝），后两句拨号时的文本是读代码得出的、没有跑。`rurge_config::hostname::to_ascii` 在 `rurge-config` 里，`rurge-proto` 的同名函数转调它；契约：`to_ascii(name: &str) -> Option<String>`，ASCII 输入原样返回（大小写与结尾的点都留着），转换之后为空、或含 `[A-Za-z0-9._-]` 之外的字节时是 `None`，不检查长度。`idna` 的依赖边随之从 `rurge-proto` 搬到 `rurge-config`（`Cargo.lock` 两行变动，包数不变；计划的判据 `cargo tree -p rurge-config -e normal | grep idna` 在改动之前就有一条，经 `url`，有意义的检查是加 `--depth 1`）。`Config::proxy_hostnames()` 同时含两种写法。没有转换的：`[WireGuard]` peer 的 `endpoint`、`sni`、`shadow-tls-sni`、`server-cert-verify-name`、`obfs-host`、`ws-headers` 里的 host。其中 `endpoint` 直连时会被解析器的 `Name::from_ascii` 拒绝；兼容性清单在"出站一侧"那句里要把它除外（终审，计划「延后事项」第 78 行）。`ssh` 的检查在 `dial` 最前面（不建会话、不登录、不开通道）；`wireguard` 的错误是 `OutboundError::Dns`（`Display` 前面多 `dns: `），隧道里的 DNS 服务器对问不了的名字跳过、之后的 `system` 仍被问，搜索在这样一次跳过之后没有应答时才报 `wireguard: the host name cannot be looked up`。

10. **坏配置时的系统代理恢复**（5.10，已就地改）。与设计的出入：
    - 出口是三个，不是两个：配置读不了、配置有错误、配置能加载但 `--log-file` 打不开；都在日志建起来之前，都以 2 退出（配置读不了时是 2，不是计划正文写的 1）。
    - 实例锁必须拿到：锁不了的数据目录（`lock_data_dir` 给 `Ok(None)`）不做提前恢复、不出声。与正常启动的"不带锁继续"不同，因为这里进程本来就要退出，在活着的实例底下恢复是更糟的错。
    - 看 `state.json` 是只读的（用 `StateStore::open` 的话，解析不了的文件会被悄悄改名成 `.broken`）；读不了的文件按"没有备份"处理、不出声。
    - 话是四句，不是三句：多一句"恢复成功而 `state.json` 里的备份没能清掉"（`… the saved settings were restored, but state.json could not be updated: the next start restores them again`），免得下一次启动悄悄盖掉用户这期间的设置。
    - 提前恢复期间这个进程持着 `rurge.lock`，同一数据目录上的正常启动此刻以 `another rurge instance is already running` 退出 1；标准错误上的次序因出口而异（配置有错误时诊断在前、告警在后，配置读不了时告警在前、`error: cannot read …` 在后）。
    - 实现：`recover_without_a_log`，`SystemProxyManager::recover()` 的返回值改成 `Option<Result<(), String>>`，`RuntimeArgs::data_dir()` 取数据目录而不创建。仍不恢复的出口（数据目录建不出来、锁被别的实例持着、后端取不到、运行时建不出来）各有理由，都不是漏洞。真实的系统代理后端从没在这条路径上跑过，阶段 1 手工验收清单要加一项。后端的报错文本可能带凭据，已在 `SystemRunner::run` 处遮掉（第 16 条）。

11. **转发循环的 flush 与缓冲**（5.11，结果补在 5.11 末尾）。
    - 基准与设计写的"两份预先编好的 release 二进制交替各跑 8 次"不同：一个进程里、一个线程跑整条链（应用、转发、源站），先上行再下行在同一条会话上，轮次在外层、组合轮换，两遍各 8 轮（多线程运行时上的四次尝试在这台机器上量不出加密栈上的任何差别）。
    - 第一版加过 `Flush` 与 `Tuning`（时机与缓冲两个维度）；修正轮去掉了 flush 这一维——它的问题有了答案，答案不依赖机器——，`BUF` 回到常量，私有的 `pump_sized` 带缓冲大小，两种时机的对比可从提交 `6752afa` 重现。修正轮之后的矩阵只跑过冒烟（`RELAY_BENCH_MIB=4`）。
    - 顺带的握手 flush 做了：`http` 与 `socks5` 出站的握手每一步写完先 flush 再等应答。`http` 把代理在 CONNECT 期间关了连接统一说成一句 `http proxy closed the connection during CONNECT`；下一跳自己带话的 reset / abort / broken pipe 原样通过，EOF 仍一律算代理的（已知限制：下层的拒绝若以 EOF 表现，仍被说成上层代理关了连接）；`response header larger than N bytes` 只在响应头真的超长时说，流自己的 `InvalidData` 带着原话（如 `received fatal alert: CertificateRequired`）。
    - 一处缺口补在任务 11：把 flush 挪出与 `stop` 竞速的那个 future 的变异活过了全部用例（`dd3aabc` 起如此），`cancelling_stop_interrupts_a_flush_that_never_completes` 补上。

12. **小项**（5.12，已就地加指针）。IPv4 映射地址不只在 SOCKS5：HTTP 入站的 `CONNECT [::ffff:a.b.c.d]:port` 与绝对 URI 的主机、双栈监听上 TCP 客户端自己的地址（`SRC-IP` 的 IPv4 规则此前永远命不中它们，阶段 1 起如此）、出站报回的数据报来源都归一成 IPv4。目标地址与出站报回的来源归一的唯一实现是 `rurge-config` 的 `HostName::canonical`（入站里另有几处各自归一的老代码：接受循环的 `canonical_peer`、`udp.rs` 的 `canonical`、`restrict.rs`、`socks5.rs` 的 `reply_bound`（UDP ASSOCIATE 的应答里报给客户端的中继地址）；`rurge-proto` 的 `from_relay` 直接用 `IpAddr::to_canonical`）。比较出站报回的来源与目标的地方是**四处**，两侧都先归一：引擎 UDP 流水线的 `receive()`（认流）、`rurge-net` 的 `packet_datagram` 接收任务（只收对端的包）、`rurge-policy` 的 `udp_probe`（`test-udp`）、`rurge-proto` 的 `from_relay`（SOCKS5 UDP 与 Shadowsocks UDP 共用）；其中后三处是终审之后补的——任务 10 的"每一道门"其实只做了第一处——（`8e581e7`、`b14b45a`，第 19 条）。出站自己不改它报回的来源。双栈中继（`::ffff:a.b.c.d`）后面的 `wireguard` 与 QUIC 策略因此能完成握手，`POST /v1/policies/test` 的 `udp` 测试不再报 `udp test timed out`。没有归一的：IP 规则对 DNS 应答的比较（只比较每个地址族的第一个地址，`::ffff:` 形式的 AAAA 不归一，阶段 1 的设计；计划「延后事项」第 81 行）。SOCKS5 UDP 回包头里的来源写回地址类型 1。改动之前，经 DIRECT 向 `[::ffff:127.0.0.1]` 发 UDP 在本机根本发不出去（os error 10049）。墙钟断言：wireguard 的是五处，不是两处；hysteria2 没加时限的 `send_to` 是十二处。

13. **`CORE_VERSION`**（6.3，已就地加一句）。已执行：`f58678b`，用例 `check_reports_core_version_22`。

14. **执行中新加的三个任务，与第 7 节的处置**。计划之外加了任务 13（遗留事项的核对查出九行无人认领的小项，外加任务 8 的评审查出的外部命令报错带凭据）、任务 14（只因本机回环 9 号端口开着才通过的用例）、任务 15（M3a #20 的空转）。第 7 节因此多出这些"M8 做掉"：M3b #9、#18、#19、#20，M3a #33，M6c #10 的后一半，M4a #5、#11，M8a #4（任务 13）；M3a #20（任务 15，与任务 11 的 bootstrap 一条）；M5c #6、M1a #20（任务 11、15）；M4b #12（任务 6 的修正轮）；M5a #20（任务 10 的修正轮）；M5c #9（终审之后的最终修正波：`packet_datagram` 与 `udp_probe` 比较来源时两侧先归一，`8e581e7`）。M3c #1（组环检测把 `smart` 的嵌套组当作边）起初也在内，随后裁定不改——要先证明每一处沿成员关系往下走的遍历都不会从 `smart` 组走进嵌套组，风险与收益不相称——，作为已知限制关闭，绕开的办法是把那个本来就不起作用的嵌套组从 `smart` 组里删掉。留给项目所有者决定的，在终审与修正波之后是计划「延后事项」里去向写"由项目所有者决定"的几行：AAAA 抑制的计数（第 36 行；本节第 15 条）、脱敏后的视图怎么做（第 51 行）与它已知没有覆盖的（第 58 ～ 77 行；第 20 条）、一个 QUIC 策略的内存对着 NFR-01（第 53 行；第 1 条）、HTTP 客户端发不发 URL 里的用户信息（第 54 行）与 `[Host]` 的 `E0017`（第 55 行；第 21 条）、`update-interval=0` 在规则集与策略组上的两种读法（第 99 行；第 22 条）、需求文档的阶段列同兼容性清单与设计的矛盾（第 105 行；第 25 条）、两个提交说明的排版（第 26 行）。自适应的转发缓冲移交阶段 3。第 7 节里的出处已就地补全。第 8 节加了第 14 条，列 M8c 另外产生的差异。

15. **任务 15 找到的产品缺陷**（3.1 里 CI 限时那句的原因）。`rurge_dns::fanout::resolve_name` 在"还有轮次没发、而时钟已过了查询的截止时间、又没有有效应答"时没有出口：不再发送、轮次不再增加，接着等的重发时刻已经过去，`sleep_until` 立即返回，于是不停地转——一个核烧着（tokio 的协作预算让每次 poll 至多转 128 圈再让出，所以守护进程仍响应），这次查询永不返回，同名的后续查询挂在它的 in-flight 条目上，只有重启 rurge 才停。触发条件是跑这次查询的任务在某次重发之后直到截止时间（产品里 5 秒）一直没得到运行：进程被停住再继续、机器睡眠而平台的单调时钟把睡眠算进去（Windows；Linux 与 macOS 不计睡眠）、运行时被饿住。自 `667af52`（阶段 1 M2b，2026-09-05）起就在代码里；一个条件修复（`f28d67c`），用例 `a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout`。**把握**：与 M3a #20 记下的空转相符，但那次运行没有留下日志，不能证明就是它，也没有证据表明还有第二个。同时扫了整个工作区里同一类循环：`rurge-dns` 之外的 `sleep_until` 每一圈都有进展，唯一的发现是每秒一次的流量采样的 `interval` 取缺省的 `Burst`（机器睡过之后背靠背触发），终审之后的最终修正波改成了 `Delay`（`9c5ee82`，第 23 条）。另：AAAA 抑制"连续 5 次"现行的计数是——一次 A 有应答而 AAAA 超时的查询计一次，它的后台补查再超时又计一次——，所以 AAAA 被黑洞时抑制在**第三个**名字的查询期间生效（背靠背地问是第四个），不是第五个；M2 设计写的"连续 5 次"逐字满足每次 fanout，后台补查该不该算进去是设计问题，本里程碑不改，测试把阈值钉住并如实写出现行计数。由项目所有者决定。终审之后的 `cf8df9a` 又改了 `rurge-dns` 里剩下的几条靠时间窗的用例：多线程用例改名为 `all_dropped_is_a_timeout_on_a_multi_thread_runtime`，TCP 等锁的用例与假服务端的 `misbehaviours` 不再靠时间窗，三条解析器用例改用不重发的选项。

16. **凭据**（3.3 的延伸）。任务 13 与它的修正轮多收了这些，都不在 3.3 的表里（终审之后脱敏与诊断又做了一遍，见第 20、21 条）：
    - `[General]` 的 `proxy-test-url`、`internet-test-url`、`geoip-maxmind-url`（后者是 MaxMind 自己的下载链接，带账号的 `license_key`）在脱敏后的配置里显示 `***`，`General` 手写 `Debug`；该链接解析不了时 `rurge` 的启动告警不再引用取值；GeoIP 更新器的资源以数据库的文件名称呼（`resource updated`、`resource fetch failed` 此前打印整条 URL）；`--help` 不再显示 `RURGE_API_KEY`、`RURGE_GEOIP_URL`、`RURGE_GEOIP_ASN_URL` 的当前值。
    - `#!MANAGED-CONFIG <URL>` 的链接在 `ManagedConfig` 的 `Debug` 与脱敏后的配置里显示 `***`（指令与参数留着），两条诊断只说参数名 / 指令名；组的旧参数 `url=` 与策略自己的 `test-url` 一样隐去；外部命令（`kwriteconfig`、`gsettings`、`networksetup`、`schtasks`）失败时的报错里，命令行参数中最后一个 `@` 之前的部分显示为 `***`。判据仍是 `crates/rurge-config/tests/secrets.rs` 的标记用例（覆盖配置层的部分）。
    - **有意留着、列给项目所有者的**：规则集的 URL（含查询串里的 token；规则集到处以 URL 命名——诊断、`set reloaded`、请求记录与 API 里规则的文字——，与 Surge 一样）、`encrypted-dns-server` 的路径与查询串、`[General]` 里不认识的键的取值；终审之后的清单见第 20 条与第 8 节第 16 条。写错或长得像的托管指令（`#!MANAGED-CONFIGX …`）在脱敏输出里的处理，由最终修正波的 `ab91157` 做了：每个 `#!` 行按形状处理。
    - 同一任务另有三处行为：`with_params` 按解析器的读法取键（整项带引号的 `"k=v"` 与不带引号的同名项是同一个参数；类型、服务器与端口三个字段不取键；行尾一项留着未闭合的引号或括号时，追加的修饰写在它前面），此前靠安全检查跳过的两种订阅行现在照修饰的值导入；`[General] test-timeout` 再大也只是等得久（运算饱和，此前加载有 `wireguard` 策略的配置时就 panic）；`vmess` 的 UDP 连接被服务端一建成就关时，一个数据报至多再拨一次，随后以 `vmess: the server ended the UDP connection` 失败；`russh` 自己的日志只留 WARN 及以上（M4a #11 原留给项目所有者，由控制者代定；代价：`russh` 在 INFO 及以下说的话看不到）。

17. **测试不再依赖这台机器**（任务 14、11；第 5 节没有对应物）。开发机开着 Windows 的 Simple TCP/IP Services（回环 TCP 与 UDP 的 7、9、13、17、19），关着的回环端口在 Windows 上约 2 秒才拒绝连接（实测 2.03 ～ 2.05 秒），在 Linux / macOS 上立刻拒绝；整套用例里写到 9 号端口的 66 行换成关着的端口后只有一条用例变红，已改；"立刻拒绝"只能用"接了立刻关"顶替，真的要等 CI。测试约定：要先于主人绑定就写进配置的端口或必须一直关着的端口，取自 20000–29999 并现场试绑（`closed_udp_port()`、`closed_tcp_port()`、`tests/external` 的 `free_port()`），不用 `bind(0)` 再放掉，也不用"绑了不听的 TCP socket"（macOS 默默丢掉 SYN）；数问题的用例用不重发的选项；对守护进程做事之前先等它说准备好了。

18. **第 9 节 V6、V7、V9、V10 的核对结果**。V6：`initial_mtu` 在 `may_fragment()` 的 socket 上生效，黑洞检测在不做探测时仍把 MTU 降回 `min_mtu`，要**四**批；`set_receive_window` 用不到。V7：按会话记一个时限，只认当前这条流的应答（第 6 条）。V9：站点记忆里"看"与"记"是同一次调用、同一次加锁（(b′)，第 5 条）。V10：在同一个进程里交替、一个线程，不是两份二进制（第 11 条）。

19. **终审与最终修正波**（`28340f3` 之后；第 5 ～ 8 节没有对应物）。整分支终审（opus，`68ba8ff..28340f3`）的结论是"Ready to merge — with fixes"：没有 Critical；Important 两项，都早于本分支——脱敏认秘密参数的边界比解析器窄（自 M8a），`update-interval=0` 与没有缓存的 304 让 URL 任务不歇地重取（自阶段 1）——；Minor 四项；一处"未证实"（第 6 条的看门狗）。它读了 13 个 crate 的产品代码，什么也没运行，没有打开大部分测试文件、`testing` 模块、各任务的报告与文档，所以它的结论只管产品代码。随后的最终修正波是九个提交、两部分：第 1 部分（`rurge-config`，凭据）三轮加三处窄化——`ab91157`、`b4a5759`、`2ebab2a`、`ec3bc9d`、`087f399`——，第 2 部分（其余）一轮加一次触碰——`8e581e7`、`9c5ee82`、`cf8df9a`、`b14b45a`。第 1 部分的脱敏经受了两次对抗性复审，每一次都在普通配置里找到了仍然露出秘密的行；第二位复审者判断"藏什么的清单"对这种格式做不成可靠的；控制者代项目所有者裁定再做最后一轮、之后不再追（挡板），把设计问题留给项目所有者——见第 20 条。最后一个提交（`087f399`）没有评审：控制者读了 diff、跑了全量门禁。门禁：`087f399` 上 70 个结果行 / 2587 通过 / 0 失败 / 3 忽略——全量运行里 `outbounds_masque` 一个二进制以本机已知的堆损坏退出码退出，单独重跑 11 条全过——；`Cargo.lock` 没变。逐次的提交、数字、评审意见与裁定见计划「执行期修正记录」第 71 ～ 91 行。这些提交没有推送过，CI 没有在它们上面跑过。

20. **脱敏后的视图（期末状态）**（3.3 的延伸，取代第 16 条里关于它的说法）。`GET /v1/profiles/current`（不带 `sensitive=1`）、原样保留的行的 `Debug` 和 `policies/detail` 显示的，是**一张"藏什么"的清单应用到一行上的结果**，这一行按解析器读它的样子来读：先摘掉节解析器摘掉的——行首的空白，注释标记，行首的要求指令，行尾的行内注释与后缀指令——，脱敏剩下的核心，再放回去（脱敏另外把行首的字节顺序标记与零宽字符也当空白跳过，`redact.rs` 的 `is_blank`，比解析器宽：解析器只在整个文本的开头去掉一个 U+FEFF）；被注释掉的行和行尾的注释照"没有被注释"那样脱敏（旧密钥常留在那里）；整行键、位置凭据与 URL 里的用户信息各照各自解析器的读法。清单在 `crates/rurge-config/src/redact.rs`：`SECRET_KEYS`（14 个）、`SECRET_PARAMS`（24 个）、`CONTROLLER_KEYS`、`LISTENER_KEYS`、`DNS_KEYS`、`READ_SCHEMES`（6 个）；隐去什么、有意多隐什么、有意留着什么，逐条写在第 8 节第 16 条。**它是尽力而为，不是没有秘密会出现的保证。**
    - **经过**。第一轮（`ab91157`）让条目照解析器的读法认。第一次对抗性复审（对 `ab91157`）：脱敏读原始的一行，节解析器读的是摘掉注释标记、要求指令、行尾注释与后缀指令之后的那一行；手工注释掉的托管行原样显示，而第一轮的测试把这钉成了预期文字——控制者的简报写错了。第二轮（`b4a5759`）改成"照 `parse_str` 的顺序读"。第二次对抗性复审（对 `b4a5759`）：十种普通配置会有的泄漏——`[MTProto]` 的 `secret`（手册里的键，两张清单都没有）、整行键前面另有东西（输入法的 `；`、注释里的一个词）、用冒号写的秘密、其余整行键后面的冒号、秘密不在用户信息里的分享链接、类型后漏了逗号、注释里的链接、落进别的节的行、行内注释只按名字搜索，外加位置口令里带 `=`（它要一张已知参数名的表，没做）。第三轮（`2ebab2a`、`ec3bc9d`）做了其中的九种，并让 URL 里的用户信息在视图之外也隐去（第 21 条）。第三轮的范围小的复审（Approved，六条 Minor）之后，三处窄化（`087f399`）收窄了这一轮自己的规则弄坏的普通行的显示——`[SSID Setting]` 行里带凭据的 DNS URL 丢掉参数名、以协议词开头的散文丢词、正则里写在 `://` 前面的通配被当成 scheme——，并关掉一条没有人点名的行（`x dns-server: 1.1.1.1 password: hunter2`，前面有文字的 DNS 键吞掉整行，后面的 `password:` 没人找）；这三处没有评审。两次对抗性复审读代码、什么也没运行，各自写了要跑的测试，下一轮把它们当作 RED。
    - **已知没有覆盖的**：计划「延后事项」第 58 ～ 77 行（二十项，来自两次复审与三轮实现者的清单），例如位置口令里带 `=`（`P = http, h.example, 8080, alice, Xk9=mQ2p`）、`{password: hunter2}` 这样的 Clash 流式映射、Loon 的 `peers=[{preshared-key=…`、写在 `=` 之后的整行键、折成几行的 Base64、没有分隔符的 `ca-passphrase hunter2`、参数名里的 `_`、落进它不属于的节的行（`[MITM] # note` 之下的内容归给上一节）。不是 Surge 配置的文件（Clash YAML、Base64 订阅）不在考虑之内。
    - **控制者代项目所有者裁定（挡板）**：第二次复审之后再做一轮、之后不再做对抗性的复审，把"藏什么的清单有长长的尾巴"这个设计问题留给项目所有者；代价：上面那些种类的行仍会在视图里露出它们的秘密，第三个复审者会找到第十一种拼法。理由：那十种是两次复审在普通配置里能找到的，每一种都是上一轮已经动过的地方的小改动。
    - **另一种做法，是项目所有者的问题**：第二位复审者的判断是视图应当反过来只显示已知无害的内容——认得的节头；已知键加已知无害形状的取值；策略行的名字、类型、服务器、端口与已知的非秘密参数；其余连注释在内一律 `***`。它会遮住每一条注释和每一行 rurge 不认识的内容，读起来差很多，也离 Surge 更远（Surge 自己的端点遮口令字段）。M8c 没有决定，这里也不把它当作定了的计划（计划「延后事项」第 51 行）。
    - **权衡时要知道**：能取到脱敏视图的人拿着 API key，能用 `sensitive=1` 取到原样的配置；视图护着的是一张截图、一次粘贴、一份贴出去的报告，不是对 API 的客户端藏秘密。

21. **视图之外：URL 里的用户信息、不是名字的名字、`[Host]` 的 `E0017`**（`ec3bc9d`；第 3.3 节的表没有这些）。
    - URL 里的用户信息在脱敏后的配置**之外**也隐去：`ResourceRef` 的 `Debug` 与 `ResourceRef::shown_url`（只在 `scheme://authority` 之内切：对整个 URL 调 `url_without_credentials` 会把 `cdn.jsdelivr.net/gh/user/repo@main/list.txt` 这样路径里带 `@` 的普通规则集弄成 `https://***@main/list.txt`）、规则与子规则的显示文字（`Rule.raw`、`SubRule.raw`，在构造处就存隐藏后的文字，规则列表、请求记录、会话日志、`rurge rule match` 与摘要的 `rules` 都读它）、`HostEntry.raw_key` 与主机的诊断、旧资源管理器的日志名（`Entry::log_name` 经 `ResourceSource::shown()`；`Display for ResourceSource` 不动，`settle` 在用它）、规则集注册表给集合起的名字（`Key::display()`：诊断 `W0022` / `W0025`、`set reloaded`、`SetStatus.name`、`rurge rule match` 的 `in <名>`）。带用户信息的规则集行的显示文字因此与文件里的那一行不再逐字相同。用户信息里带原始 `/`、`?` 或 `#` 的 URL（不是合法的 URL，从不抓取）整段显示，因为 authority 在那个字符处结束、里面没有 `@`。
    - **rurge 的 HTTP 客户端不把 URL 里的用户信息当 Basic 认证发出**（读 `HttpClient::build`，并用一条对回环 `TestServer` 的临时用例证实：服务器只收到 `user-agent` 与 `host`，用例随后删掉）：规则集、域名集、订阅、脚本里从别的客户端带来的 `https://user:password@host/…` 不认证，抓取以 `http status 401` 失败（DoH 自 M8a 起发出这些凭据）。发不发是项目所有者的问题（计划「延后事项」第 54 行）。
    - 不是名字的名字（`is_name`：含 `://`、全角 `＝` 或 `，`）不在诊断里引用；不是名字的组成员在 `PolicyGroup` 与 `GroupSpec` 的 `Debug` 和 `ConfigSummary.groups` 里显示 `***`；未知 `[General]` 键只在长得像键名时引用；落进别的节的行（规则、策略与主机解析器）不再引用。确切的文本见第 8 节第 17 条。`GroupSpec` 的 `Debug` 也手写了（简报点名的只有 `PolicyGroup` 与摘要，`Config` 的 `Debug` 把成员再带出一遍），解构全部九个字段，加字段会编译失败。
    - **行为变化，控制者代项目所有者裁定**：`[Host]` 下含 `://` 又不是 `DOMAIN-SET:` / `RULE-SET:` 的键现在是 `E0017`（``expected `<host> = <value>` ``），此前加载成一条永远匹配不到的别名，贴进来的链接的口令留在条目的键里。一份贴错了一条分享链接、原来能起的配置现在加载失败，直到那一行删掉；同一条链接放在 `[Rule]` 或 `[Proxy]` 下、或放在 `[Host]` 下而没有 `=`，原来就是错误（计划「延后事项」第 55 行）。

22. **外部资源的刷新**（`8e581e7`，终审的 Important 2；第 5 ～ 8 节没有对应物）。`update-interval=0`（规则集、域名集）读作没写，按缺省的 86 400 秒刷新——在 `ResourceManager::register` 的入口，创建条目与并入共享间隔都经它，0 不参与"取最短"；手册没有说明 0 的含义。策略组的 `update-interval=0` 不经这里：组的参数读成类型之后（M3a 起）它就是 `E0018`，要求正数（第 8 节第 15 条）；两种读法的取舍在计划「延后事项」第 99 行。没有缓存时服务器对不带验证头的请求答 304，算一次失败的抓取（状态 `failed`，说明 ``the server answered "not modified" but nothing is cached``），照失败退避；有缓存时的 304 照旧只更新抓取时间。自动刷新关闭（`update-interval` 为负）的 URL 任务每分钟醒一次、只看间隔而不抓取——`28340f3` 去掉过这次醒来，复评指出它也是停着的任务看到同一 URL 的另一处引用并入的间隔的办法，`8e581e7` 恢复了它；管理器消失由 `gone` 立刻通知，不靠这次醒来（任务 13，`8812e3f`）。留下的：较小的正间隔并入一个正在睡较大间隔的任务时，要等那次长睡眠结束才被看到，M8c 之前就是这样（计划「延后事项」第 79 行）。

23. **流量采样**（`9c5ee82`，任务 15 扫出的同类问题）。每秒一次的流量采样的定时器取 `MissedTickBehavior::Delay`，与 `anytls/pool.rs`、`h2pool`、`snell/pool.rs`、`rurge-proto-quic` 的 `session.rs` 同一写法：机器睡过之后不补发采样，醒来后只多采一次，所以休眠后第一次读到的速率是这一段里走的字节，不会被补发的采样冲成 0。一行产品代码；用例 `the_traffic_sampler_does_not_make_up_for_missed_seconds` 经 `traffic().rate()` 在暂停的时钟上观察，不需要新的产品代码。

24. **对更早的文档的订正与旁注**（任务 12 A 部分的补充及其订正；第 5 ～ 8 节没有对应物）。M2 设计 7.3 的 AAAA 抑制一条下加了旁注：现行的计数是待定行为，由项目所有者决定（第 15 条）。总设计第 13 节（测试策略）里 M7 一行关于 tuic 0.8.5 的 SHA-256，从"待项目所有者填写"改成已钉住（取自发布自带的校验文件）。`tests/interop/README.md` 补了 `Failures` 的守卫与 `free_ports` 两段，并改了 dnsproxy 一节里讲 9 号端口的几处。M6 设计第 18 节第 7 条（Snell 的 `udp-port`）下加了旁注：那一条里"那是 v5 QUIC Proxy Mode 的 UDP 端口"是 rurge 自己的推断，没有来源——手册在 Snell 与 UDP Relay 两页只说 `udp-port` 是进行 UDP 转发时用的另一个服务端口号，用于服务端的 TCP 与 UDP 不在同一端口时（例如 Shadow TLS 占着 TCP 端口），并说 QUIC Proxy Mode 是运行期的行为、没有配置参数，两页没有把二者联系起来；Surge 往那个端口发什么不知道。rurge 的行为不变（`udp-port` 只解析，`W0029`，UDP 走主端口），那一条给的另一个理由仍然成立：UDP 会话那条 TCP 连接若连到 `udp-port` 并照样叠 Shadow TLS，在 Shadow TLS 占着 TCP 端口的部署里必然握手失败。代码注释与兼容性清单里的同一处说法由任务 12 的 B 部分按手册的原话改过（`e7d0eef`）。

25. **阶段 1 与阶段 2 的需求审计**（2026-10-10，任务 12 A 部分的订正之后；第 5 ～ 8 节没有对应物，第 8 节也不加：没做的条文不是差异）。起因：托管配置的更新与远程 `#!include` 是在改兼容性清单开头一句时偶然发现没有实现的（计划「延后事项」第 100 行），发现它的写者说，没有人核对过其余的条目。两个只读的助手（较小的模型），每个阶段一个，把需求文档第 4 节（以及第 7 节的交付物与验收标准）里属于阶段 1、阶段 2 的功能需求逐句对照代码，每一句写出作用于它的代码和一个用例，或者一个判词；什么也没运行；报告的每一处缺口由控制者在代码里核对过，判为"做了"的没有人复核，所以审计显示缺口，不证明完整。数字——阶段 1：49 条需求、287 句（做了 214、做了而没找到用例 11、只解析 12、没有 8、有意不同 13、属于后面的阶段 29）；阶段 2：23 条需求与另外两条的阶段 2 部分、196 句（做了 173、做了而没找到用例 3、只解析 5——其中 4 个由 `W0029` 登记——、没有 0、有意不同 5、按需求文档自己的话属于后面的阶段 9、未能判定 1）。控制者的裁定：没做到的每一句都登记——验收文档的「需求逐条对照代码」一节、兼容性清单里说得比代码多的几行、需求文档第 7 节给阶段 1 的一条状态行、计划「延后事项」第 100 ～ 105 行——，一句也不在 M8c 里实现；它们与托管配置一起，是 M8c 之后、阶段 3 开始之前的下一项工作（"阶段 1 补遗"），先写它自己的设计与计划。教训出在流程上：它们都是某份设计交给同一阶段"以后的里程碑"的，没有一份计划的「延后事项」接住，所以收集那些表的总表也不会有它们；一个阶段要在它的需求逐句对照过代码之后才称齐备，不是在它的里程碑都合并之后。
