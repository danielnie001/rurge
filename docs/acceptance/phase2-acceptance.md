# 阶段 2 验收清单

阶段 2（出站协议与策略组）的验收标准有九条。本文逐条给出现有的证据：用例的文件与名字、证据属于哪一种，以及这一条要算作满足还缺什么。没有证据的地方照实写，不写成通过。

另有两节不属于九条标准。「需求逐条对照代码」一节记 2026-10-10 把需求文档里阶段 1、2 的功能需求逐句对着代码读过一遍的结果：阶段 1 没有自己的验收文档，它的缺口也记在那里，因为项目所有者读的是这一份；2026-10-11 的阶段 1 补遗（M5a）补上了其中的大部分，补在哪里记在同一节里。最后的「留给项目所有者的」一节列出要做的事与要定的事。

依据两份设计：

- `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md` 第 14 节：九条标准的出处；
- `docs/superpowers/specs/2026-10-08-phase2-m8-wrapup-design.md` 第 6.2 节：订正其中第 1、8 条。下文引用的是订正后的文字。

写于提交 `44a0204` 之上（分支 `phase2-m8c-leftovers-acceptance`）。文中每个文件路径、用例名与命令都在这个提交上用脚本和 `git grep` 核对过。全量测试的数字取自提交 `14b1183` 上的一次运行；那以后的提交只改文档（碰过源文件的最后一个提交是 `e7d0eef`，只改了注释行）。

## 怎么读

证据分三种，各有各的写法：

- **回环**：用例对回环上的假服务端运行，在本机（Windows）的全量运行里通过。最近一次全量运行是提交 `14b1183`（2026-10-10；格式检查与 clippy 零警告，没有测试二进制异常退出）：70 个结果行，2587 个通过，0 个失败，3 个忽略；忽略的是三个吞吐基准（`crates/rurge-engine/src/relay.rs`：`relay_throughput`，`crates/rurge-proto-quic/src/hysteria2/mod.rs`：`throughput`，`crates/rurge-proto-wireguard/src/outbound.rs`：`throughput`）。只在 Unix 编译的用例（`crates/rurge/tests/cli.rs`：`run_shuts_down_gracefully_on_sigint`）在本机没有编译过，只在 Windows 编译的用例（`crates/rurge-engine/tests/pipeline.rs`：`a_doq_carrier_that_outlives_a_port_unreachable_is_no_failed_session`）只在本机编译过；`tests/interop/tests/sshd.rs` 的 2 个也只在 Unix 上编译。本机与 CI 另有一处不同：本机回环的 7、9、13、17、19 号端口开着，CI 上关着；现在没有用例依赖它们（「留给项目所有者的」一节第 2 条第 6 点）。
- **互操作**：`tests/interop` 里的用例拉起真实的第三方服务端（回环上的子进程），再用 rurge 的出站去连。本机没有安装任何参照服务端，这些用例打印 `skipping …` 后直接返回，在 `cargo test` 里记为通过，但不是证据：需要参照二进制的用例共 63 个，其中 61 个在本机编译并跳过，算在上面的 2587 个通过里（`tests/interop/tests/sshd.rs` 的 2 个在 Windows 上不编译）。CI 上这些用例也没有一次可读的结果（第 8 条）。本文把它们写作"已写，尚未对真实的二进制运行"。
- **手工**：`docs/acceptance/phase2-manual.md`，需要真实公网节点，由项目所有者来做（第 9 条）。阶段 1 的系统代理与服务安装另有 `docs/acceptance/phase1-manual.md`。两份清单都没有做过。

引用格式：`路径`：`用例名`（一句话说明它证明什么）；同一文件的几个用例用"、"连写。常用命令：`cargo test --workspace`（全部回环用例；互操作用例在没有二进制时跳过）；`cargo test -p rurge-interop`（只跑互操作）；`RURGE_INTEROP_REQUIRED=1 cargo test -p rurge-interop`（找不到参照二进制时让用例失败，CI 在工作流一级设置它）。

## 结论

阶段 2 要实现的每种出站协议和每种组类型都实现了、有回环用例（不实现的 `tailscale`、Snell v6、手册不再列出的 `ss` 方法名与排在阶段 3 的 `subnet` 组不在其内，见第 8 条）；需求逐句对照代码在阶段 2 没有查出缺失的条款，只查出 5 句只做到解析。验收没有完成。下面一行一条，用词不会读成"全部完成"：

- **第 1 条　未满足**。回环用例覆盖每种协议的 TCP 和能转发 UDP 的协议的 UDP（`socks5-tls` 的 UDP 与 `https` 的引擎级用例除外）；标准要求的是对参考服务器，63 个互操作用例没有一个对真实的二进制运行过。（2026-10-18：它们在运行 39022550224（`7bd6589`）里对真实的二进制跑过一次，装了参照服务端的平台上除五条之外都通过，见第 8 节的更新；CI 仍没有绿过，这一条仍是未满足。）
- **第 2 条　回环用例满足，等第一次 CI 绿**。两级 TCP 链与 UDP 载体链都有用例，底层都是回环假服务端。
- **第 3 条　回环用例满足，等第一次 CI 绿**。五种算法与六个端点都有用例；"重启后保留"是进程内由同一份 `state.json` 新建引擎，没有两次真实的进程。
- **第 4 条　回环用例满足，等第一次 CI 绿**。订阅样本是用例里手写的，没有真实订阅。
- **第 5 条　回环用例满足，等第一次 CI 绿**。官方 `shadow-tls` 服务端没有互操作用例。
- **第 6 条　回环对端上满足，等第一次 CI 绿**。真实的 WireGuard 服务端与 WARP 要手工验。
- **第 7 条　回环用例满足，等第一次 CI 绿**。DoQ / DoH3 的 4 个互操作用例没有对真实二进制运行过。（2026-10-18：这 4 个用例在运行 39022550224（`7bd6589`）里对真实的 dnsproxy 与 CoreDNS 跑过一次，三个平台都通过；CI 仍没有绿过。）
- **第 8 条　未满足**。三平台 CI 没有绿过；clippy 零警告与 `W0007` / `W0008` 两项在本机成立。
- **第 9 条　清单已有，手工验收一项没做**。22 节 187 项。
- **需求逐条对照代码　阶段 1 有 20 句不在（2026-10-10 的数），阶段 1 补遗（M5a）补上了大部分**。阶段 1：287 句里 20 句没有落地或只做到解析（只解析 12、没有 8），其中有托管配置的更新与远程 `#!include` 两项 P0；2026-10-11 的阶段 1 补遗（M5a）之后，表里的 13 行补上了 10 行，剩下托管配置的更新、远程 `#!include` 与外部资源的手动强制更新三行，属阶段 1 补遗（M5b）；阶段 2：196 句里 5 句只做到解析（4 句启动时由 `W0029` 登记，1 句 `hidden` 没有登记），没有完全缺失的，另有 1 句未能判定。审计只读代码，没有运行，不证明别处没有缺口；阶段 1 补遗（M5a）就在判为"做了"的 FR-RULE-05 上查出一条只做了一部分的。

| 标准 | 回环用例（本机通过） | 互操作用例（已写，未运行） | 还缺什么 |
| ---- | -------------------- | -------------------------- | -------- |
| 1　每种协议对参考服务器的 TCP 与 UDP | 19 种策略写法与 Shadow TLS、simple-obfs 两个层，各有引擎级或协议级的 TCP 用例，能转发 UDP 的各有 UDP 用例（`https`、`socks5-tls` 只有协议级，`socks5-tls` 的 UDP 无用例） | 63 个：sing-box 28、xray 2、`ssserver` 3、snell-server 5、TrustTunnel endpoint 6、tuic-server 3、masque-server 3、mihomo 7、sshd 2、dnsproxy 2、CoreDNS 2 | 对真实二进制的一次运行；`socks5-tls` 没有 UDP 用例与引擎级用例，只有手工清单 M8c 一节的一项；AnyTLS 的看门狗对真实服务端没有验证过；Linux 与 macOS 上的回环用例还没有一次可读的结果 |
| 2　链式代理 | 两级 TCP 链；`hysteria2`、`tuic-v5`、`masque`、`wireguard` 经 SOCKS5 UDP 载体链；`ss`、`snell`、`h2-connect`、Shadow TLS 经 `underlying-proxy` | 无 | 没有一条链的底层是第三方实现；`trust-tunnel` 的 `h3=true` 无链用例；经链的 `masque` 只量过一个 1250 字节的 UDP 包往返，没有跑过一次内层 QUIC 握手；手工清单对应项未做 |
| 3　策略组与六个 API 端点 | 五种算法各有单元与引擎级用例；六个端点各有 API 用例；选择写入 `state.json`，新引擎读回 | 不适用 | 没有真实进程重启的用例；四个端点的响应形状是"暂定"；手工清单 M3a ～ M3c 未做 |
| 4　订阅样本与装配 | 策略行列表与含 `[Proxy]` 的完整配置两种形式的解析；过滤 → 前缀 → 修饰与装配顺序；引擎级的载入与热重建 | 不适用 | 样本都是用例里手写的，仓库里没有真实订阅；手工清单 M3a 未做 |
| 5　Shadow TLS 与六个 TLS 参数 | v2、v3 的协议层、引擎级与配置层用例；六个参数各有配置读取与握手用例 | 3 个：sing-box `shadowtls` 入站 2 个，sing-box `https` 入站 1 个（覆盖六个参数中的三个：`sni`、`server-cert-fingerprint-sha256`、`client-cert`；另有根证书校验） | 官方 `shadow-tls` 服务端只在手工清单；`skip-cert-verify`、`server-cert-verify-name`、`alpn` 没有对参照服务端的用例 |
| 6　WireGuard 与带 `client-id` 的端点 | `FakeWgPeer` 上带 `client-id` 的握手与转发（TCP、UDP、测速）；保留字节的编解码；隧道的结束方式与重建 | 1 个：sing-box 的 WireGuard 端点，`client-id = 1/2/3` | 对真实 WireGuard 服务端与 WARP 的运行（手工清单 M4b）；重新握手 / keepalive / 过期的计时器没有用例让 90 或 120 秒过去 |
| 7　`h3://` 与 `quic://` 上游 | URL 解析、DoQ、DoH3、两者共用的会话、经解析器与经跟随出站模式的用例 | 4 个：dnsproxy v0.86.0 与 CoreDNS v1.14.7 各 DoQ、DoH3 | 对真实二进制的运行；夹具的命令行、Corefile 与就绪日志读自源码；公共 DoQ / DoH3 服务（手工清单 M7c） |
| 8　CI 绿、clippy、`W0007` / `W0008` | `W0007`、`W0008` 的出现条件在配置层、注册表、引擎与 `rurge check` 各有用例；clippy 在本机零警告 | 不适用 | CI 没有绿过：最近一次运行（`0154e73`）的 `cargo test` 在三平台都失败，失败清单未读到；那以后的提交（到 `44a0204` 是 76 个）没有一个被 CI 见过；CI 里没有单独的互操作任务，互操作用例在 `cargo test --workspace` 一步里。2026-10-18 的更新：下一次运行（39022550224，`7bd6589`）读得到失败清单——Linux 跑完全部测试二进制、5 条失败，macOS 与 Windows 停在 `rurge-proto` 的单元测试里、排在它后面的四个包的测试没有跑到；六类失败里五类已修，第 6 类换了找端口的做法并补了诊断、原因仍不知道（`f8d0389`、`4fc1bad`、`cae06c3`、`26be5e2`、`c048315`、`2519b3f`、`0deba9f`、`f9377a8`、`94cfd9e`），修好没有要再推送一次才知道，见第 8 节的更新 |
| 9　手工验收清单 | 不适用 | 不适用 | `docs/acceptance/phase2-manual.md` 存在，22 节共 187 项，没有一项做过；阶段 1 的 `docs/acceptance/phase1-manual.md` 的 34 项也没做（2026-10-13 的更新：这份清单现在是 45 行，仍是一行也没有做过，见「留给项目所有者的」第 5 项；2026-10-14 的更新：手册基线重对又加了 1 行，现在是 46 行） |

能说的：第 2、3、4、5、7 条的文字要求的是用例，这些用例存在，并在本机（Windows）的全量运行里通过；第 6 条在回环对端上成立；第 8 条的 clippy 与 `W0007` / `W0008` 两项在本机成立；第 9 条要求的清单存在。不能说的：第 1 条要求对参考服务器，63 个互操作用例没有一个对真实的二进制运行过；第 8 条要求三平台 CI 绿，CI 没有绿过——本地的 `origin/main` 是 `0154e73`，之后的提交（到 `44a0204` 是 76 个）没有一个被 CI 见过——；第 9 条的手工验收，187 项没有一项做过。回环用例的结果只有本机（Windows）的，Linux 与 macOS 上还没有一次可读的结果（`0154e73` 那次 CI 运行的日志读不到）。阶段 1 也不能说"齐备"：2026-10-10 查出 20 句需求条款没有落地或只做到解析，阶段 1 补遗（M5a）补上了其中的大部分，还剩托管配置的更新、远程 `#!include` 与外部资源的手动强制更新（阶段 1 补遗（M5b））；判为"做了"的条款没有人复核过，阶段 1 补遗（M5a）就在 FR-RULE-05 上查出一条只做了一部分的，见「需求逐条对照代码」一节。 2026-10-13 的更新：这三项已由阶段 1 补遗（M5b）做出来；阶段 1 的需求也逐句对着代码重读了一遍（416 句，`docs/acceptance/phase1-acceptance.md`），结果是三个数，要一起看：3 句没有做完，37 句的阶段栏写着阶段 1 而登记在后面的阶段，14 句有意做成了别的样子并已登记。所以阶段 1 仍然不说"齐备"；这一遍也没有覆盖阶段 1 的手工验收清单（一行也没有做过）与 CI（没有绿过）。

2026-10-14 的更新：手册基线重对（报告 `docs/reports/2026-10-14-manual-baseline-recheck.md`）把项目对照的手册前移到 Surge Mac 6.10.0 / iOS 5.23.0，阶段 1 的三个数随之成了 **2 句没有做完，37 句的阶段栏写着阶段 1 而登记在后面的阶段，10 句有意做成了别的样子并已登记**（上面 2026-10-13 的更新写的是 3、37、14）：手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN` 那一句补上了（`9b0bcfe`，原来记"没有做完"），DNS 连接的三个协议标签与 `PROTOCOL` 规则匹配那 4 句随标签改按 URL 的方案补上了（`42c2294`，原来记"有意不同"）。所以阶段 1 仍然不说"齐备"。这一轮没有观察过 Surge 的实际行为，也没有对着代码重读阶段 1、2 的其余各行（只核对了 47 处）；手工验收清单现在是阶段 2 的 22 节 187 项与阶段 1 的 46 行，一行也没有做过，CI 没有绿过。细节见「需求逐条对照代码」一节的「手册基线重对之后（2026-10-14）」。

2026-10-18 的更新：最近一次 CI 运行是 `7bd6589`（2026-10-17，运行号 39022550224），这一阶段第一次读得到三个平台的失败清单。结论不变：CI 没有绿过，第 1 条与第 8 条仍是未满足。但上面和下面说互操作用例没有对真实的二进制运行过、说 Linux 与 macOS 上还没有可读的结果的几处，要这样读：那一次运行里，Linux 跑完了全部测试二进制，macOS 与 Windows 只跑到 `rurge-proto` 的单元测试（排在它后面的四个包的测试没有跑到），装了参照服务端的平台上互操作用例除五条之外都通过，六类失败里五类已修、第 6 类换了找端口的做法并补了诊断（原因仍不知道），修好没有要下一次推送才知道，详见第 8 节的更新。表头与小标题里的“已写，尚未对真实的二进制运行”“均未运行”是写这份文件当天的写法，没有逐处改；各条的互操作一节、“还缺什么”和「留给项目所有者的」第 2 节里写着这类话的地方，各补了一句注明日期的说明；没有逐处注明的同类话（例如「怎么读」一节里的互操作一条，和结论里能说的与不能说的那一段）也按这一段读。

## 1. 每种协议对参考服务器的 TCP 转发通过；支持 UDP 的协议 UDP 转发通过

> 每种协议对参考服务器的 TCP 转发通过；支持 UDP 的协议 UDP 转发通过。没有可用参照服务端的情形（`socks5-tls`、CONNECT-UDP over HTTP/2、TUIC 的 `quic` 模式、端口跳跃、`ss` 的 `salsa20`）由回环假服务端覆盖并进手工验收清单（M8 细化设计 6.2）。simple-obfs 原来也在这张名单里：M8b 起 mihomo 的监听端是它的参照服务端——CI 上装了，用例还没有对真实的二进制跑过，第一次 CI 运行时核对（M8 细化设计第 12 节第 8 条）。

### 回环（本机通过）

回环假服务端按协议放在 `crates/rurge-proto/src/testing/`（`FakeSocks5`、`FakeHttpProxy`、`FakeTrojan`、`FakeVmess`、`FakeAnyTls`、`FakeShadowsocks`、`FakeSnell`、`FakeH2Proxy`、`FakeShadowTls`）、`crates/rurge-proto-ssh/src/testing/`（`FakeSsh`）、`crates/rurge-proto-wireguard/src/testing/`（`FakeWgPeer`）、`crates/rurge-proto-quic/src/testing/`（`FakeHysteria2`、`FakeTuic`、`FakeH3Proxy`）。用例分两层：引擎级用例（`crates/rurge-engine/tests/`）经 HTTP 或 SOCKS5 入站、规则与出站走到假服务端；协议级用例（各协议 crate 自己的 `tests` 模块）直接驱动出站对象。下表每个单元格只列几个有代表性的用例。不实现的协议与变体（`tailscale`、`snell` 的 `version=6`、手册没列的 `ss` 方法名）不在表内，见第 8 条。

| 协议 | TCP 回环 | UDP 回环 | 互操作（参照服务端与用例；均未运行） | 备注 |
| ---- | -------- | -------- | ------------------------------------ | ---- |
| `http` | `crates/rurge-engine/tests/outbounds.rs`：`a_connect_leaves_through_the_http_upstream_with_the_name_unresolved`（CONNECT，目标名留给上游）、`a_plain_request_goes_to_an_http_upstream_in_absolute_form`（明文请求的绝对 URI 转发）、`a_forward_mode_407_is_a_failed_session_and_a_502_page`；`crates/rurge-proto/src/http.rs`：`connect_tunnels_with_credentials` | 不载 UDP。`crates/rurge-engine/tests/udp.rs`：`a_policy_without_udp_rejects`（缺省拒绝）、`a_policy_without_udp_may_fall_back_to_direct`（`udp-policy-not-supported-behaviour = DIRECT` 时改走 DIRECT） | sing-box 1.14.2 的 `http` 入站（三平台）。`tests/interop/tests/sing_box.rs`：`http_connect_with_and_without_credentials`、`a_plain_request_in_absolute_form_is_served_by_sing_box` | |
| `https` | `crates/rurge-proto/src/http.rs`：`https_wraps_the_proxy_connection_in_tls`。没有引擎级用例；`crates/rurge-engine/src/outbounds.rs`：`every_implemented_protocol_builds` 只验证能构建 | 不载 UDP（同 `http`） | sing-box 的 `http` 入站加 TLS。`tests/interop/tests/sing_box.rs`：`https_with_a_private_ca_a_pinned_fingerprint_and_a_client_certificate` | TLS 参数见第 5 条 |
| `socks5` | `crates/rurge-engine/tests/outbounds.rs`：`always_use_connect_and_other_protocols_tunnel_plain_requests`、`a_chain_enters_through_the_groups_current_member`、`rurge_talks_to_rurge`（两个 rurge 互为对端）；`crates/rurge-proto/src/socks5.rs`：`connects_without_and_with_credentials`、`names_are_resolved_by_the_proxy_and_ipv6_is_its_own_type` | `crates/rurge-engine/tests/udp.rs`：`udp_goes_through_a_socks5_proxy`、`udp_goes_through_a_chain`；`crates/rurge-proto/src/socks5.rs`：`udp_goes_through_the_association`（`udp-relay=true` 的 UDP ASSOCIATE） | sing-box 的 `socks` 与 `mixed` 入站。`tests/interop/tests/sing_box.rs`：`socks5_with_and_without_credentials_and_the_mixed_inbound`、`socks5_udp_goes_through_sing_box` | |
| `socks5-tls` | `crates/rurge-proto/src/socks5.rs`：`socks5_tls_wraps_the_session_in_tls`、`socks5_tls_runs_inside_shadow_tls`（对 `FakeSocks5::spawn_tls`）。没有引擎级用例 | 没有用例。UDP 是明文 UDP，只有控制连接是 TLS（兼容性清单 4.2）；`crates/rurge-proto/src/socks5.rs` 里 UDP 关联的实现与 `socks5` 共用，但没有一个用例在 TLS 控制连接上做 UDP ASSOCIATE | 无参照：sing-box 的 `socks` 与 `mixed` 入站没有 TLS 字段 | 第 1 条点名要进手工清单；M8c 一节有一项，没有做 |
| `trojan` | `crates/rurge-engine/tests/outbounds_tls_family.rs`：`a_connect_leaves_through_a_trojan_upstream_with_the_name_unresolved`、`a_plain_request_is_tunnelled_through_trojan_over_websocket`；`crates/rurge-proto/src/trojan.rs`：`over_websocket`、`a_wrong_password_cannot_be_told_at_connect_time` | `crates/rurge-engine/tests/udp_tls_family.rs`：`udp_goes_through_trojan`、`anyone_may_answer_through_trojan`（全锥）；`crates/rurge-proto/src/trojan.rs`：`udp_goes_through_one_connection` | sing-box 的 `trojan` 入站。`tests/interop/tests/sing_box_tls_family.rs`：`trojan_with_and_without_websocket`、`a_wrong_trojan_password_is_not_relayed`、`udp_through_trojan_vmess_and_anytls` | 密码错误在连接期分辨不出 |
| `vmess` | `crates/rurge-engine/tests/outbounds_tls_family.rs`：`a_connect_leaves_through_a_vmess_upstream_with_the_name_unresolved`、`a_vmess_line_without_vmess_aead_leaves_through_the_aead_handshake`、`a_wrong_vmess_id_ends_the_session_with_a_text_that_says_so`；`crates/rurge-proto/src/vmess/header.rs`：`the_request_head_matches_the_reference_byte_for_byte` | `crates/rurge-engine/tests/udp_tls_family.rs`：`udp_goes_through_vmess_one_connection_per_target`、`through_vmess_every_answer_is_the_targets`（对称型）；`crates/rurge-proto/src/vmess/mod.rs`：`udp_opens_one_connection_per_target`；`crates/rurge-proto/src/vmess/udp.rs`：`a_first_datagram_whose_write_is_blocked_past_the_grace_is_still_sent`（读与写在两个任务里，第一次写被堵过了宽限；2026-10-18） | sing-box 的 `vmess` 入站。`tests/interop/tests/sing_box_tls_family.rs`：`vmess_with_and_without_tls_and_websocket`、`a_wrong_vmess_id_is_not_relayed`。xray v26.3.27。`tests/interop/tests/xray.rs`：`vmess_with_either_cipher_with_and_without_websocket`、`vmess_udp_with_either_cipher` | 只说 AEAD 握手，旧握手不实现 |
| `anytls` | `crates/rurge-engine/tests/outbounds_tls_family.rs`：`an_anytls_upstream_carries_two_requests_over_one_session`（会话复用）、`a_request_on_a_half_dead_anytls_session_fails_within_seconds`、`an_unrelated_reload_keeps_an_anytls_pool_and_a_change_of_its_own_drops_it`；`crates/rurge-proto/src/anytls/mod.rs`：`a_relay_moves_a_megabyte_each_way_and_the_session_comes_back` | `crates/rurge-engine/tests/udp_tls_family.rs`：`udp_goes_through_anytls`、`anyone_may_answer_through_anytls`；`crates/rurge-proto/src/anytls/mod.rs`：`udp_goes_through_a_stream_of_its_own` | sing-box 的 `anytls` 入站。`tests/interop/tests/sing_box_tls_family.rs`：`anytls_reuses_its_session_and_can_be_told_not_to`、`udp_through_trojan_vmess_and_anytls` | UDP 走 UDP over TCP v2 |
| `ss` | `crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`a_connect_leaves_through_ss_with_every_kind_of_method`（AEAD、`none`、SS 2022 多用户）、`a_connect_leaves_through_ss_with_a_stream_cipher`、`both_obfs_modes_carry_the_session`；`crates/rurge-proto/src/shadowsocks/aead.rs`：`every_cipher_writes_the_known_answer` | `crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`udp_goes_through_ss_aead_and_2022`、`udp_goes_through_ss_with_a_stream_cipher`、`udp_goes_to_udp_port`；`crates/rurge-proto/src/shadowsocks/mod.rs`：`udp_is_full_cone` | shadowsocks-rust v1.25.0 的 `ssserver`（三平台）。`tests/interop/tests/shadowsocks.rs`：`aead_methods_against_ssserver`、`ss_2022_against_ssserver_with_one_key_and_as_a_user`、`stream_methods_against_ssserver`。sing-box 的 `shadowsocks` 入站。`tests/interop/tests/shadowsocks.rs`：`ss_against_sing_box_aead_2022_and_a_user`。mihomo。`tests/interop/tests/mihomo.rs`：`ss_chacha20_against_mihomo` | `salsa20` 无参照；`none` 只有假服务端；`aes-192-gcm` 与 `xchacha20-ietf-poly1305` 只有 sing-box 的用例（`ssserver` 的发布包不带） |
| `snell` | `crates/rurge-engine/tests/outbounds_snell.rs`：`a_connect_leaves_through_snell_v4_and_v5`、`a_connect_leaves_through_snell_v1_to_v3`、`v2_shares_one_connection_and_v1_and_v3_never_do`、`reuse_carries_sessions_one_after_another_on_one_connection`、`obfs_http_carries_the_session` | `crates/rurge-engine/tests/outbounds_snell.rs`：`udp_goes_through_snell`（v4、v5）、`udp_goes_through_snell_v3`、`snell_v1_and_v2_carry_no_udp`（v1、v2 不载 UDP） | sing-box 的 `snell` 入站（v4、v5）。`tests/interop/tests/snell.rs`：`v5_and_v4_clients_against_sing_box`、`reuse_against_sing_box`、`udp_against_sing_box`。官方 snell-server v5.0.1（仅 Linux）。`tests/interop/tests/snell.rs`：`v5_and_v4_clients_against_snell_server`、`udp_against_snell_server`。mihomo（v1 ～ v3）。`tests/interop/tests/mihomo.rs`：`snell_v1_to_v3_against_mihomo`、`snell_v2_requests_share_a_connection_against_mihomo`、`snell_v3_udp_against_mihomo` | v6 与 v5 的 QUIC 模式不实现；v1 ～ v3 只有 mihomo 一个参照 |
| `ssh` | `crates/rurge-engine/tests/outbounds_ssh.rs`：`a_connect_leaves_through_ssh_with_the_name_unresolved`、`a_keystore_key_logs_in_to_a_pinned_server`、`an_ssh_session_runs_inside_shadow_tls`；`crates/rurge-proto-ssh/src/outbound.rs`：`password_and_key_logins_forward_a_connection`、`a_server_with_only_surges_algorithms_is_reached`、`one_session_carries_every_connection` | 不载 UDP（兼容性清单 4.5）。没有 `ssh` 专属的 UDP 用例，不载 UDP 的通用路径见 `http` 一行 | OpenSSH 的 `sshd`（仅 Unix）。`tests/interop/tests/sshd.rs`：`ssh_forwards_through_openssh`、`ssh_reaches_an_openssh_offering_only_surges_algorithms` | 互操作用例在 Windows 上不编译；CI 只在 Linux 与 macOS 上设置 `RURGE_TEST_SSHD` |
| `wireguard` | `crates/rurge-engine/tests/outbounds_wireguard.rs`：`a_connect_leaves_through_the_tunnel`、`a_name_is_resolved_on_this_machine_with_host_items`、`a_test_url_is_fetched_through_the_tunnel`；`crates/rurge-proto-wireguard/src/outbound.rs`：`a_connection_through_the_tunnel_echoes`、`a_large_transfer_goes_through_whole` | `crates/rurge-engine/tests/outbounds_wireguard.rs`：`udp_leaves_through_the_tunnel`（全锥）、`a_udp_test_asks_through_the_tunnel`；`crates/rurge-proto-wireguard/src/outbound.rs`：`udp_goes_through_the_tunnel` | sing-box 的 WireGuard 端点（用户态，`client-id = 1/2/3`）。`tests/interop/tests/sing_box_wireguard.rs`：`a_connection_goes_through_a_sing_box_wireguard_endpoint`（TCP、UDP、握手测速） | 回环对端是 `FakeWgPeer`；见第 6 条 |
| `external` | `tests/external/tests/engine.rs`：`the_first_dial_starts_the_program`（经引擎，真实拉起辅助程序）；`tests/external/tests/outbound.rs`：`the_first_use_starts_the_program`、`stopping_ends_the_whole_tree`；`crates/rurge/tests/cli.rs`：`run_starts_an_external_program_and_stops_it_on_exit`（`rurge run` 拉起第二个 rurge 当 SOCKS5 程序） | `tests/external/tests/outbound.rs`：`udp_goes_through_the_program`（`udp-relay=true`） | 无参照服务端：`external` 不说协议，只拉起本机的 SOCKS5 程序；`tests/external/src/bin/socks-helper.rs` 是仓库自己的辅助程序 | 手工清单 M4c 要求三个平台各验一遍 |
| `h2-connect` | `crates/rurge-engine/tests/outbounds_http2.rs`：`a_connect_leaves_through_h2_connect`、`max_streams_bounds_the_sessions_on_one_connection`、`refused_credentials_fail_the_session`；`crates/rurge-proto/src/h2connect/mod.rs`：`a_tunnel_echoes_over_tls_with_alpn_h2` | `crates/rurge-engine/tests/outbounds_http2.rs`：`udp_goes_through_h2_connect_as_connect_udp`（每个目标一条 CONNECT-UDP 流）、`udp_without_extended_connect_fails_the_flow` | TrustTunnel endpoint v1.1.0（仅 Linux、macOS）的普通 CONNECT。`tests/interop/tests/http2.rs`：`h2_connect_against_the_endpoint`、`h2_connect_streams_share_connections_on_the_endpoint` | CONNECT-UDP over HTTP/2 没有参照 |
| `trust-tunnel`（HTTP/2） | `crates/rurge-engine/tests/outbounds_http2.rs`：`a_connect_leaves_through_trust_tunnel`、`a_refused_trust_tunnel_login_fails_the_session`；`crates/rurge-proto/src/trust_tunnel.rs`：`a_tunnel_echoes_with_basic_credentials_and_a_user_agent` | 不载 UDP。`crates/rurge-engine/tests/outbounds_http2.rs`：`trust_tunnel_carries_no_udp` | TrustTunnel endpoint v1.1.0（仅 Linux、macOS）。`tests/interop/tests/http2.rs`：`trust_tunnel_against_the_endpoint`、`a_wrong_trust_tunnel_password_is_refused` | |
| `trust-tunnel`（`h3=true`） | `crates/rurge-engine/tests/outbounds_http2.rs`：`a_connect_leaves_through_trust_tunnel_over_http3`、`a_refused_trust_tunnel_login_over_http3_fails_the_session`、`max_streams_bounds_the_tunnels_of_each_quic_connection`；`crates/rurge-proto-quic/src/trust_tunnel.rs`：`a_tunnel_is_a_classic_connect_with_the_credentials_and_a_user_agent` | 不载 UDP；同一个协议级用例断言 `udp()` 为 `Unsupported` | TrustTunnel endpoint 的 QUIC 监听（仅 Linux、macOS）。`tests/interop/tests/http2.rs`：`trust_tunnel_over_http3_against_the_endpoint`、`a_wrong_trust_tunnel_password_over_http3_is_refused` | 夹具的 QUIC 监听部分读自源码，没有对真实二进制验证过 |
| `hysteria2` | `crates/rurge-engine/tests/outbounds_hysteria2.rs`：`tcp_leaves_through_hysteria2_on_one_connection`、`a_refused_password_fails_the_session`、`either_obfuscation_carries_tcp_and_udp`、`a_policy_test_goes_through_the_connection`；`crates/rurge-proto-quic/src/hysteria2/mod.rs`：`a_tunnel_carries_a_large_payload_and_a_half_close` | `crates/rurge-engine/tests/outbounds_hysteria2.rs`：`udp_leaves_through_hysteria2_in_datagrams`（全锥、分片）、`a_server_that_relays_no_udp_fails_the_flow`；`crates/rurge-proto-quic/src/hysteria2/mod.rs`：`long_datagrams_go_in_fragments_both_ways` | sing-box 的 `hysteria2` 入站。`tests/interop/tests/sing_box_hysteria2.rs`：`tcp_against_sing_box`、`udp_against_sing_box`、`a_wrong_password_fails_the_authentication_against_sing_box`、`salamander_against_sing_box`、`gecko_against_sing_box` | 端口跳跃无参照；Gecko 与 Surge 的 `gecko-password` 是否同一规范未证实（M7-D11） |
| `tuic`（v4） | `crates/rurge-engine/tests/outbounds_tuic.rs`：`tcp_leaves_through_either_version_on_one_connection`、`a_refused_token_fails_the_session`；`crates/rurge-proto-quic/src/tuic/mod.rs`：`v4_a_wrong_token_fails_the_request_and_the_next_dials_again`、`v4_the_servers_close_codes_say_what_went_wrong` | `crates/rurge-engine/tests/outbounds_tuic.rs`：`v4_udp_goes_whole_in_datagrams_or_not_at_all`（放不进数据报的包丢弃）、`udp_goes_on_streams_where_the_server_takes_no_datagrams` | 官方 tuic 0.8.5 的 `tuic-server`（仅 Linux；GPL，只作外部进程）。`tests/interop/tests/tuic.rs`：`v4_tcp_against_tuic_server`、`v4_udp_against_tuic_server`、`v4_a_wrong_token_against_tuic_server` | `quic` 模式无参照 |
| `tuic-v5` | `crates/rurge-engine/tests/outbounds_tuic.rs`：`tcp_leaves_through_either_version_on_one_connection`、`a_refused_password_fails_the_session`；`crates/rurge-proto-quic/src/tuic/mod.rs`：`the_authentication_carries_the_uuid_and_a_token_of_the_tls_session` | `crates/rurge-engine/tests/outbounds_tuic.rs`：`udp_leaves_through_tuic_v5_in_datagrams`（分片与重组）、`udp_goes_on_streams_where_the_server_takes_no_datagrams`；`crates/rurge-proto-quic/src/tuic/mod.rs`：`long_packets_go_in_fragments_both_ways` | sing-box 的 `tuic` 入站。`tests/interop/tests/tuic.rs`：`v5_tcp_against_sing_box`、`v5_udp_against_sing_box`、`v5_a_wrong_password_against_sing_box` | sing-box 总是接受 QUIC 数据报，只覆盖 `native` 模式 |
| `masque` | `crates/rurge-engine/tests/outbounds_masque.rs`：`tcp_leaves_through_masque_on_one_connection`、`refused_credentials_fail_the_session`；`crates/rurge-proto-quic/src/masque/mod.rs`：`a_tunnel_is_a_classic_connect`、`every_connect_carries_the_credentials_and_none_goes_without_them` | `crates/rurge-engine/tests/outbounds_masque.rs`：`udp_goes_through_masque_as_connect_udp`、`a_server_without_connect_udp_fails_the_flow_and_carries_tcp`、`a_packet_too_long_for_a_datagram_is_dropped_alone`；`crates/rurge-proto-quic/src/masque/udp.rs`：`udp_goes_in_http3_datagrams_a_stream_for_each_target` | masque-server v0.13.0（仅 Linux）。`tests/interop/tests/masque.rs`：`tcp_against_masque_server`、`udp_against_masque_server`、`a_wrong_masque_password_is_refused` | 端口跳跃与"服务端不支持 CONNECT-UDP"无参照 |
| Shadow TLS（层） | `crates/rurge-engine/tests/outbounds_shadow_tls.rs`：`a_connect_leaves_through_trojan_wrapped_in_shadow_tls_v3`、`an_http_upstream_behind_shadow_tls_v2_needs_no_sni`、`vmess_without_tls_and_anytls_run_inside_shadow_tls_too`；`crates/rurge-engine/tests/outbounds_ssh.rs`：`an_ssh_session_runs_inside_shadow_tls`；`crates/rurge-proto/src/socks5.rs`：`socks5_tls_runs_inside_shadow_tls` | 随所叠的协议；只叠在 TCP 类协议上 | sing-box 的 `shadowtls` 入站（v2、v3）前接 `trojan`。`tests/interop/tests/sing_box_shadow_tls.rs`：`trojan_behind_shadow_tls_v2_and_v3`、`a_wrong_shadow_tls_password_is_not_relayed` | 不能与 `tuic`、`tuic-v5`、`hysteria2`、`masque`、`wireguard`、`tailscale`、`external` 组合（`E0018`：`crates/rurge-config/src/spec/shadow_tls.rs`：`it_wraps_tcp_and_nothing_else`）；见第 5 条 |
| simple-obfs（层） | `crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`both_obfs_modes_carry_the_session`（`ss` 的 `http` 与 `tls`）；`crates/rurge-engine/tests/outbounds_snell.rs`：`obfs_http_carries_the_session`；`crates/rurge-proto/src/transport/obfs/mod.rs`：`round_trips_through_the_fake_server_with_a_half_close` | 随所叠的协议 | mihomo 的监听端。`tests/interop/tests/mihomo.rs`：`ss_behind_simple_obfs_against_mihomo`、`snell_behind_simple_obfs_against_mihomo`。`snell` 自带的 `obfs=http`。`tests/interop/tests/snell.rs`：`obfs_http_against_sing_box`、`obfs_http_against_snell_server` | 原在"没有参照服务端"的名单里；mihomo 是它的参照，用例未运行 |

M8c 另加的、属于本条的回环证据：

- 握手的 flush：`http` 与 `socks5` 出站的握手每一步写完先 flush 再等应答（下层是会缓冲的一跳时，请求不必等到下一次写才发出）。`crates/rurge-proto/src/http.rs`：`the_request_is_flushed_before_its_answer_is_awaited`；`crates/rurge-proto/src/socks5.rs`：`every_step_is_flushed_before_its_answer_is_awaited`。代理在 CONNECT 期间关了或重置连接是一句 `http proxy closed the connection during CONNECT`，下一跳自己带话的失败原样通过，`https` 代理要客户端证书时报 TLS 的原话：`crates/rurge-proto/src/http.rs`：`a_connection_the_proxy_ends_during_connect_is_its_refusal`、`a_hop_below_that_says_what_went_wrong_is_not_overruled`、`a_proxy_that_wants_a_client_certificate_says_so`。
- 转发循环：每次写后恰好一次 flush，半关闭前先 flush，停着的写不被别的事插队，flush 永不完成时 `stop` 仍能打断。`crates/rurge-engine/src/relay.rs`：`every_write_is_followed_by_one_flush`、`a_half_close_flushes_first`、`nothing_comes_between_a_parked_write_and_its_retry`、`cancelling_stop_interrupts_a_flush_that_never_completes`。吞吐基准 `relay_throughput`（忽略项，不作门禁；整张表在 M8 细化设计 5.11 末尾）的结论：flush 的时机量不出差别，缓冲越大越快；按两遍 16 轮合计读，64 KiB 过了设计的判据线（余量只有 0.1 ～ 0.3 个百分点），缺省仍是 8 KiB——每条会话从建立到结束拿着两块缓冲，1 万条会话 8 KiB 是 156 MB、64 KiB 是 1.22 GiB，对着 NFR-01 的 300 MB 不行。
- IPv4 映射地址（`::ffff:a.b.c.d`）在入站的每道门与比较来源的四处都按 IPv4 处理：`crates/rurge-inbound/src/socks5.rs`：`an_ipv4_mapped_target_is_dialed_as_ipv4`（SOCKS5 的目标）；`crates/rurge-inbound/src/http.rs`：`an_ipv4_mapped_target_is_dialed_as_ipv4`（HTTP 的 CONNECT 与绝对 URI）；`crates/rurge-inbound/src/listener.rs`：`an_ipv4_client_of_a_dual_stack_listener_is_named_by_its_ipv4_address`（双栈监听上 TCP 客户端自己的地址）；`crates/rurge-inbound/src/udp.rs`：`an_ipv4_mapped_address_is_the_ipv4_address_both_ways`（UDP 数据报头）；`crates/rurge-engine/tests/udp.rs`：`an_ipv4_mapped_target_is_the_ipv4_address`（引擎里的流）；比较出站报回的来源的四处是 `crates/rurge-engine/src/udp.rs`：`an_answer_from_a_source_in_ipv6_form_finds_its_flow`，`crates/rurge-net/src/packet_datagram.rs`：`a_peer_is_heard_from_its_ipv4_address_in_ipv6_form`，`crates/rurge-policy/src/udp_probe.rs`：`an_answer_from_the_other_form_of_the_servers_address_is_its_answer`，`crates/rurge-proto/src/socks5.rs`：`the_relay_is_heard_from_its_address_in_either_form`（SOCKS5 UDP 与 Shadowsocks UDP 共用的 `from_relay`）。
- UDP 连接一建成就被服务端关掉时，一个数据报至多再拨一次：`crates/rurge-proto/src/vmess/udp.rs`：`a_connection_that_has_ended_is_dialled_again_only_once`；`crates/rurge-proto/src/h2connect/udp.rs`：`a_stream_that_has_ended_is_opened_again_only_once`。
- QUIC 的接收窗口：不设流数上限的会话（`masque`、`hysteria2`、`tuic`）每条连接 256 MiB。用例（在 `masque` 上）钉住的是 4 条读端停住的隧道留给第 5 条它自己的窗口——M8c 之前的 20 MiB 里 3 条就占满——；"读端停住的隧道要 29 条才拖住其余隧道"是照 quinn 每攒够窗口的八分之一才通告额度的做法算出来的，没有用例。`trust-tunnel` 的 `h3=true` 的连接窗口取 `max-streams` 条流的窗口之和的七分之八（至少 20 MiB）：算术对 `max-streams` 1 ～ 256 有用例（其余的流都停住时留给最后一条的不少于窗口的八分之一），真实的隧道只在两个取值上试过——`max-streams=9` 下其余 8 条都停住时第 9 条照旧收到数据，`max-streams=4` 下 3 条停住时第 4 条照常建立并往返。`crates/rurge-proto-quic/src/masque/mod.rs`：`tunnels_nobody_reads_leave_the_others_their_window`；`crates/rurge-proto-quic/src/session.rs`：`a_capped_connections_window_takes_a_window_of_each_stream`；`crates/rurge-proto-quic/src/trust_tunnel.rs`：`the_last_tunnel_goes_on_beside_all_the_others_unread`、`tunnels_nobody_reads_leave_the_others_their_window`。

### 互操作（已写，尚未对真实的二进制运行）

2026-10-18 的更新：下表的参照服务端在运行 39022550224（`7bd6589`）里对真实的二进制跑过一次（CI 上装了它们的平台）：用例除五条之外都通过——`ss` 对 sing-box 与对 `ssserver` 的 SS 2022 用例（三个平台，收回包的缓冲只有 64 字节）、`snell` 对 snell-server 的 UDP 与 obfs 用例（Linux，回包的来源没有归一）、`masque` 对 masque-server 的 TCP 用例（Linux，masque-server 每条隧道只排 1 MiB 客户端的字节）——，根因与修正见第 8 节的更新。这是一次运行的结果，CI 仍没有绿过，修正有没有用要下一次推送才知道；各平台实际跑了多少个互操作用例没有读到（任务日志要登录才能读）。

互操作用例经配置加载与 `EngineFactory` 构建出站（`tests/interop/tests/common/mod.rs`：`outbound`），直接驱动出站的 `connect_tcp` 与 `open_udp`，目标是回环上的回显服务器；不经入站与规则。除 `sshd` 外，参照服务端都固定了版本，CI 下载并校验 SHA-256（见 `tests/interop/README.md`）。

| 参照服务端 | 固定版本 | CI 上的平台 | 用例数 | 文件 | 覆盖 |
| ---------- | -------- | ----------- | ------ | ---- | ---- |
| sing-box | 1.14.2 | 三平台 | 28 | `tests/interop/tests/sing_box.rs`、`tests/interop/tests/sing_box_tls_family.rs`、`tests/interop/tests/sing_box_shadow_tls.rs`、`tests/interop/tests/sing_box_wireguard.rs`、`tests/interop/tests/sing_box_hysteria2.rs`，另有 `tests/interop/tests/shadowsocks.rs`、`tests/interop/tests/snell.rs`、`tests/interop/tests/tuic.rs` 里对它的各一部分 | `http`、`https`、`socks5`、`trojan`、`vmess`、`anytls`、Shadow TLS、`wireguard`、`hysteria2`、`ss`、`snell`（v4、v5）、`tuic-v5` |
| xray | v26.3.27 | 三平台 | 2 | `tests/interop/tests/xray.rs` | `vmess` |
| shadowsocks-rust `ssserver` | v1.25.0 | 三平台 | 3 | `tests/interop/tests/shadowsocks.rs` | `ss`（AEAD、SS 2022、九种流式方法） |
| snell-server | v5.0.1 | 仅 Linux | 5 | `tests/interop/tests/snell.rs` | `snell` v4、v5 |
| TrustTunnel endpoint | v1.1.0 | Linux、macOS | 6 | `tests/interop/tests/http2.rs` | `trust-tunnel`（HTTP/2、`h3=true`）、`h2-connect` 的普通 CONNECT |
| tuic-server | 0.8.5 | 仅 Linux | 3 | `tests/interop/tests/tuic.rs` | `tuic`（v4） |
| masque-server | v0.13.0 | 仅 Linux | 3 | `tests/interop/tests/masque.rs` | `masque` |
| mihomo | v1.19.32 | 三平台 | 7 | `tests/interop/tests/mihomo.rs` | `snell` v1 ～ v3、`ss` 的 `chacha20`、simple-obfs |
| OpenSSH `sshd` | 系统自带 | Linux、macOS | 2 | `tests/interop/tests/sshd.rs` | `ssh` |
| dnsproxy、CoreDNS | v0.86.0、v1.14.7 | 三平台 | 2 + 2 | `tests/interop/tests/dns.rs` | DoQ、DoH3（第 7 条） |

设了 `RURGE_INTEROP_REQUIRED=1`，CI 上各平台应当跑到的用例数：Windows 44，macOS 52，Linux 63（按上表的平台一栏算出，不是 CI 的实测）；三个数都含第 7 条的 4 个 DNS 用例。

夹具自身的单元用例不需要参照二进制，在本机运行并通过，例如 `tests/interop/src/lib.rs`：`the_configuration_never_touches_the_machine`、`tests/interop/src/mihomo.rs`：`the_command_line_is_written_as_mihomo_reads_it`。它们证明夹具按作者对参照服务端的理解写出配置与命令行，不证明参照服务端接受。互操作用例逐项记失败、最后一起报，用的 `Failures` 有 Drop 守卫：记过失败而没有 `finish` 就被丢掉时，用例当场失败（`tests/interop/tests/shadowsocks.rs`：`failures_that_are_never_finished_fail_the_test`、`nothing_that_failed_need_not_be_finished`、`failures_dropped_by_a_test_that_fails_otherwise_say_nothing`，不需要参照二进制）；守卫当场找出了一条一直静默通过的用例（`tests/interop/tests/mihomo.rs`：`only_the_two_sentences_of_an_unanswered_request_are_taken`）。

第一次 CI 运行要核对的夹具细节（读自参照实现的源码，没有对真实二进制验证过）：mihomo 的配置键、就绪探测、日志行，以及 PSK 或版本不符时它关闭还是重置连接；masque-server 的命令行、配置键、就绪日志，以及 CONNECT 的 2xx 之后会不会再发 HEADERS；TrustTunnel endpoint 的 QUIC 监听（空表启用、TCP 与 UDP 同一个端口号、就绪日志）；tuic-server 的命令行、七个配置键与就绪日志；dnsproxy 与 CoreDNS 的命令行、Corefile 与就绪日志；官方 snell-server 读不出请求时的反应；一次拉起多个端口的夹具（sing-box、xray、dnsproxy、CoreDNS、mihomo、ssserver）改成一次取齐端口（`tests/interop/src/lib.rs`：`free_ports`），这个改动也没有对真实的参照服务端跑过。逐项见 `tests/interop/README.md` 各服务端一节。2026-10-18 的更新：运行 39022550224（`7bd6589`）里，这些夹具在 CI 装了对应参照服务端的平台上都拉起来了，用例除第 8 节更新里的五条之外都通过，所以上面列的命令行、配置键、就绪探测与日志行，以及一次取齐端口的改动，对真实的二进制行得通（mihomo 的配置与就绪探测、masque-server 的命令行与就绪日志、TrustTunnel endpoint 的 QUIC 监听、tuic-server 的七个键与就绪日志、dnsproxy 与 CoreDNS 的命令行与 Corefile）。通过的用例定不下来的仍然开着：sing-box、mihomo 与官方 snell-server 在 PSK 或版本不符时是关闭还是重置连接（用例收两句话之一，只说明它们很快结束了连接）、masque-server 与 TrustTunnel endpoint 会不会在 CONNECT 的 2xx 之后再发 HEADERS（要看任务日志，任务日志要登录才能读，公开的只有注解与摘要）、mihomo 启动时联不联网和在主目录里留下什么。

### 没有参照服务端的情形

| 情形 | 回环假服务端与用例 | 手工清单 |
| ---- | ------------------ | -------- |
| `socks5-tls` | `FakeSocks5::spawn_tls`。`crates/rurge-proto/src/socks5.rs`：`socks5_tls_wraps_the_session_in_tls`、`socks5_tls_runs_inside_shadow_tls`。UDP 无用例 | M8c 一节"`socks5-tls` 对真实的 SOCKS5 over TLS 服务器" |
| CONNECT-UDP over HTTP/2 | `FakeH2Proxy`（通告 extended CONNECT）。`crates/rurge-proto/src/h2connect/mod.rs`：`a_datagram_crosses_a_connect_udp_stream`、`every_target_has_its_own_stream`、`a_server_without_extended_connect_carries_no_udp`；`crates/rurge-engine/tests/outbounds_http2.rs`：`udp_goes_through_h2_connect_as_connect_udp`、`udp_without_extended_connect_fails_the_flow` | M6c 一节"`h2-connect` 的 UDP" |
| TUIC 的 `quic` 模式 | `FakeTuic`（不接受 QUIC 数据报）。`crates/rurge-proto-quic/src/tuic/mod.rs`：`without_datagrams_each_packet_goes_on_a_stream_of_its_own`、`v4_without_datagrams_each_packet_goes_on_a_stream_of_its_own`；`crates/rurge-engine/tests/outbounds_tuic.rs`：`udp_goes_on_streams_where_the_server_takes_no_datagrams` | M7b 一节"不接受 QUIC 数据报的服务端" |
| 端口跳跃（`hysteria2`、`tuic`、`tuic-v5`、`masque`） | 监听几个端口的假服务端。`crates/rurge-net/src/quic/mod.rs`：`a_hopping_path_moves_among_the_ports`；`crates/rurge-proto-quic/src/hysteria2/mod.rs`、`crates/rurge-proto-quic/src/tuic/mod.rs`、`crates/rurge-proto-quic/src/masque/mod.rs`：同名用例 `tunnels_keep_working_while_the_port_hops`；`crates/rurge-engine/tests/outbounds_hysteria2.rs`、`crates/rurge-engine/tests/outbounds_tuic.rs`、`crates/rurge-engine/tests/outbounds_masque.rs`：同名的引擎级用例 | M7a、M7b 各一项"端口跳跃"，M7c 一节"MASQUE 的端口跳跃" |
| `ss` 的 `salsa20` | 已知答案的向量。`crates/rurge-proto/src/shadowsocks/stream.rs`：`salsa20_is_ecrypts`、`salsa20_under_an_iv_is_ecrypts_too`；回环。`crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`udp_goes_through_ss_with_a_stream_cipher`（含 `aes-128-ctr` 与 `salsa20`） | M8b 一节"`salsa20`" |
| simple-obfs（已有参照，未运行） | `crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`both_obfs_modes_carry_the_session`；`crates/rurge-proto/src/transport/obfs/mod.rs`：`http_the_first_write_carries_the_head_and_later_ones_are_raw`、`tls_the_first_write_rides_in_the_ticket_and_later_ones_split_at_16384` | M6a、M6b 各一项"obfs" |

### 还缺什么

- 63 个互操作用例（本条 59 个，第 7 条的 DNS 4 个）没有一个对真实的二进制运行过：本机没有任何参照服务端，CI 没有可读的结果（第 8 条）。要把第 1 条算作满足，需要一次三平台 CI 全绿的运行，且 `RURGE_INTEROP_REQUIRED=1` 保证这些用例跑了而不是跳过。（2026-10-18：它们在运行 39022550224（`7bd6589`）里对真实的二进制跑过一次，装了参照服务端的平台上除五条之外都通过，见第 8 节的更新；这只是一次运行的结果，CI 仍没有绿过，这一条仍是未满足。）
- 上节"第一次 CI 运行要核对的夹具细节"里的各项是读源码得出的，没有对真实二进制验证过；用例失败时，先分清是夹具与参照服务端不符，还是 rurge 的问题。（2026-10-18：见上节末尾的更新。）
- AnyTLS 的 SYNACK 看门狗（复用的会话上 3 秒内没有应答的新流就关会话）没有对真实服务端验证过，第一次 CI 运行也验证不了：互操作用例在几毫秒内放掉每条流，UDP 用例用新会话的第一条流，看门狗不看它；假服务端应答的次序是照 anytls-go 的源码写的。会验证它的是一个在复用的会话上活过 3 秒的流的互操作用例（手工清单 M8c 一节有对应的两项）。（2026-10-18：运行 39022550224 也没有验证到它，这一条不变：互操作用例照旧在几毫秒内放掉每条流。）
- `socks5-tls`：没有 UDP 用例，没有引擎级用例；手工清单 M8c 一节有一项对真实的 SOCKS5 over TLS 服务器，没有做。
- `https`：没有引擎级用例，只有协议级用例与对 sing-box 的互操作用例。
- Gecko 混淆与 Surge 的 `gecko-password` 是否同一规范未证实（M7-D11，手工清单 M7a 一节记下结果）；`masque` 的认证头 `proxy-authorization` 是 rurge 的选择，手册没写 Surge 用哪个字段（手工清单 M7c 一节"与 Surge 对照"）。
- 回环用例的结果只有本机（Windows）的；Linux 与 macOS 上还没有一次可读的结果（第 8 条）。（2026-10-18：运行 39022550224（`7bd6589`）里，Linux 上全部测试二进制、macOS 与 Windows 上排在 `rurge-proto` 单元测试之前的各包有了可读的结果，见第 8 节的更新；这两个平台上其余的要下一次推送。）

## 2. 链式代理通过

> 链式代理通过：两级 TCP 链；QUIC 类与 WireGuard 经底层代理的 UDP 载体链。

### 回环（本机通过）

两级 TCP 链：

- `crates/rurge-engine/tests/outbounds.rs`：`a_chain_enters_through_the_groups_current_member`（`socks5` 入口经 `select` 组，`http` 出口；入口被问的是出口服务器的名字，出口被问的是目标的名字，切换组的选择后下一条连接换入口）、`a_broken_hop_is_named_in_the_error`（出错的一跳在错误里带 `via` 前缀）、`a_reload_leaves_a_chained_session_alone_and_moves_the_next_one`。
- `crates/rurge-engine/tests/outbounds_tls_family.rs`：`a_trojan_exit_is_reached_through_a_socks5_entry_by_name`、`a_socks5_exit_is_reached_through_a_trojan_entry`、`the_new_protocols_work_at_either_end_of_a_chain`（`vmess` 入口、`anytls` 出口）。
- 其余协议各有一个经 SOCKS5 入口的用例：`crates/rurge-engine/tests/outbounds_snell.rs`：`snell_goes_through_an_underlying_socks5_proxy`；`crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`ss_goes_through_an_underlying_socks5_proxy_for_tcp_and_udp`；`crates/rurge-engine/tests/outbounds_http2.rs`：`h2_connect_goes_through_an_underlying_socks5_proxy`；`crates/rurge-engine/tests/outbounds_shadow_tls.rs`：`shadow_tls_runs_through_an_underlying_proxy`。
- 单元：`crates/rurge-policy/src/registry.rs`：`a_three_hop_chain_hands_each_server_to_the_hop_below`、`the_entry_of_a_chain_follows_the_group_selection`；`crates/rurge-policy/src/cell.rs`：`failures_name_the_hop_and_keep_their_kind`。

UDP 载体链：

- `crates/rurge-engine/tests/udp.rs`：`udp_goes_through_a_chain`（两级 SOCKS5 的 UDP ASSOCIATE）、`a_chain_without_udp_fails_the_flow`（底层策略不载 UDP 时流失败并说明原因）。
- QUIC 类，QUIC 连接的数据报走入口的 UDP 关联，TCP 与 UDP 都过：`crates/rurge-engine/tests/outbounds_hysteria2.rs`：`hysteria2_goes_over_an_underlying_socks5_proxy`、`a_new_member_of_the_underlying_group_gets_a_new_connection`（底层是组，换成员后新会话走新连接）、`a_hopping_member_of_a_relayed_group_is_left_out`（端口跳跃不能过链，该成员被略去）；`crates/rurge-engine/tests/outbounds_tuic.rs`：`tuic_goes_over_an_underlying_socks5_proxy`；`crates/rurge-engine/tests/outbounds_masque.rs`：`masque_goes_over_an_underlying_socks5_proxy`、`over_an_underlying_proxy_a_packet_of_1250_bytes_goes_through`（经载体的连接从 1350 字节的包起步，浏览器 QUIC 的 1250 字节包往返）。
- 载体层：`crates/rurge-net/src/quic/mod.rs`：`a_carried_path_reaches_the_server_by_name`、`a_carrier_that_cannot_send_fails_the_dial_in_its_own_words`、`hopping_cannot_go_through_a_carrier`；`crates/rurge-proto-quic/src/masque/udp.rs`：`over_a_carrier_a_datagram_holds_another_connections_packet`。
- 经载体的起步包长与回落（M8c）：`crates/rurge-net/src/quic/dial.rs`：`only_a_connection_over_a_carrier_starts_from_the_carried_size`；`crates/rurge-proto-quic/src/masque/udp.rs`：`a_path_that_cannot_carry_the_first_size_brings_the_connection_down_to_1200_bytes`、`a_tunnel_gets_through_a_path_that_cannot_carry_the_first_size`（载体丢弃长数据报时，连接自己退回 1200，隧道照常）。
- 载体的错误（M8c）：链上载体打开之后报出的错误带 `via <策略>: ` 前缀，载体自己的话原样交出，`ConnectionReset` 不算载体的失败：`crates/rurge-policy/src/cell.rs`：`a_dial_over_a_relay_fails_under_the_relays_name`、`a_relay_whose_carrier_was_reset_fails_the_dial_in_the_carriers_words`；`crates/rurge-net/src/packet_datagram.rs`：`a_carrier_that_was_reset_ends_in_its_own_words_as_an_abort`；`crates/rurge-net/src/quic/carrier.rs`：`a_reset_is_not_the_carriers_failure`、`a_carrier_that_cannot_send_after_all_has_quinn_wait_for_it`。
- `wireguard`：`crates/rurge-engine/tests/outbounds_wireguard.rs`：`a_tunnel_goes_over_an_underlying_socks5_proxy`、`a_tunnel_follows_the_member_of_its_underlying_group`、`a_tunnel_over_an_underlying_proxy_without_udp_fails_saying_so`、`a_reload_that_changes_the_carriers_takes_the_tunnel_over`。
- 跟随出站模式的 DoQ / DoH3 载体：`crates/rurge-engine/tests/pipeline.rs`：`a_doq_carrier_follows_the_rules_through_a_socks5_proxy`、`a_doh3_carrier_follows_an_ip_rule_through_a_socks5_proxy`。

### 互操作

无。`tests/interop` 没有一个用例用到 `underlying-proxy`。

### 还缺什么

- 所有链用例的底层都是回环上的假 SOCKS5（`FakeSocks5`）、假 HTTP 代理或 rurge 自己，没有一条链的底层是第三方实现。
- `trust-tunnel` 的 `h3=true` 经 `underlying-proxy` 没有自己的用例；`hysteria2`、`tuic`、`masque` 的载体代码是共用的，但共用不等于有用例。
- 手工清单里对应的项没有做：M5c 一节"经底层策略的隧道"，M7a、M7b 各一项"`underlying-proxy`"，M7c 一节"MASQUE 经 `underlying-proxy`"，M8c 一节"`masque` 经 `underlying-proxy` 时内层 QUIC 可用"。
- 经链的 `masque` 自动化里量到的只是一个 1250 字节的 UDP 包往返（`crates/rurge-engine/tests/outbounds_masque.rs`：`over_an_underlying_proxy_a_packet_of_1250_bytes_goes_through`；`crates/rurge-proto-quic/src/masque/mod.rs` 里 `CARRIED_MTU` 是 1350），没有跑过一次真正的内层 QUIC 握手。缺省 `mtu`（1280）的 WireGuard 带不动 1350 字节的数据报，是分片送出、整包送出还是被丢弃而回落到 1200，只读了 smoltcp 0.12 的源码，没有实测；`mtu=1280` 的 WireGuard 满包（1312 字节）经 `masque` 放不下（按余量算要设到 1264 以下），同样没有实测。回落是每条连接各自的、此后不再升回：窄路径上每建一条连接都先丢一轮满包（回环上量到 0.19 ～ 0.23 秒）。

## 3. 策略组五种算法与六个 API 端点

> 策略组五种算法的单元测试与端到端测试通过；六个 API 端点的测试与切换行为正确，`select` 的选择重启后保留。

### 回环（本机通过）

五种算法（`select`、`url-test`、`fallback`、`load-balance`、`smart`）：

| 算法 | 单元 | 引擎级（回环上游，经 HTTP 或 SOCKS5 入站） |
| ---- | ---- | ------------------------------------------ |
| `select` | `crates/rurge-policy/src/registry.rs`：`groups_read_the_live_table`、`selections_api` | `crates/rurge-engine/tests/outbounds.rs`：`a_selection_applies_to_the_next_connection_and_survives_a_restart`、`only_a_member_can_be_selected`、`a_selection_whose_member_is_gone_falls_back_to_the_first_member` |
| `url-test` | `crates/rurge-policy/src/auto.rs`：`url_test_holds_its_member_within_the_tolerance` | `crates/rurge-engine/tests/auto_groups.rs`：`url_test_moves_to_the_quicker_member_after_a_round` |
| `fallback` | `crates/rurge-policy/src/auto.rs`：`fallback_takes_the_first_that_passes` | `crates/rurge-engine/tests/auto_groups.rs`：`fallback_passes_over_a_member_that_fails_its_test` |
| `load-balance` | `crates/rurge-policy/src/auto.rs`：`load_balance_spreads_over_those_that_pass` | `crates/rurge-engine/tests/auto_groups.rs`：`load_balance_with_persistent_keeps_each_host_on_one_member` |
| `smart` | `crates/rurge-policy/src/smart.rs`：`a_failure_adds_a_penalty_that_halves_every_five_minutes`、`three_failures_in_a_row_fail_a_member_and_a_success_restores_it`、`a_site_is_remembered_for_an_hour`、`a_dial_picks_at_random_among_those_close_to_the_best` | `crates/rurge-engine/tests/smart.rs`：`a_member_that_does_not_connect_hands_over_to_the_next`、`the_first_byte_back_is_reported`、`three_seconds_without_an_answer_count_against_the_member`、`a_smart_group_takes_an_override_and_the_test_endpoints` |

自动组共用的部分：`crates/rurge-engine/tests/auto_groups.rs`：`evaluate_before_use_waits_for_the_first_round`、`every_test_is_a_session_of_the_request_log`、`select_on_an_automatic_group_overrides_it_until_cleared`、`a_reload_keeps_an_override_while_the_group_stays_the_same`；`crates/rurge-policy/src/testbook.rs`：`no_more_than_eight_tests_run_at_once`。

M8c 另加的：`smart` 的失败计分——同一个失效站点一小时内只罚一次，不论经哪个成员、同时来多少条会话（N 个不同的失效地址仍罚 N 次）；发不出去的目标不经这个组，不记任何成员的失败。`crates/rurge-policy/src/smart.rs`：`a_member_failing_at_one_site_time_and_again_pays_once`、`of_the_members_failing_at_a_site_at_once_exactly_one_pays`、`the_hour_runs_from_the_last_failure_at_the_site`、`a_member_failing_at_three_sites_is_failed`；`crates/rurge-engine/tests/smart.rs`：`a_burst_of_sessions_to_a_site_that_is_down_costs_one_penalty`、`a_name_no_member_can_send_is_tried_once_and_counts_against_nobody`、`udp_flows_failing_at_one_address_cost_their_member_one_penalty`；`crates/rurge-engine/src/smart.rs`：`a_name_no_proxy_request_can_carry_is_unsendable`。`test-timeout` 写得再大也只是等得久，不再 panic：`crates/rurge-engine/tests/auto_groups.rs`：`an_absurd_test_timeout_does_not_stop_a_round`；`crates/rurge-policy/src/registry.rs`：`a_wireguard_policy_with_an_absurd_test_timeout_is_built`。引擎的代际锁被毒化后拨号照常：`crates/rurge-engine/src/subscriptions.rs`：`a_panic_under_the_generation_lock_stops_no_dial`。

六个 API 端点：

| 端点 | 用例 |
| ---- | ---- |
| `GET /v1/policies/detail` | `crates/rurge-api/tests/api.rs`：`policy_groups_are_listed_and_a_policy_detail_is_redacted`（凭据脱敏，未知策略 404，缺参数 400） |
| `GET /v1/policy_groups` | `crates/rurge-api/tests/api.rs`：`policy_groups_are_listed_and_a_policy_detail_is_redacted`（成员、`lineHash`） |
| `GET` / `POST /v1/policy_groups/select` | `crates/rurge-api/tests/api.rs`：`a_select_group_is_switched_for_the_next_request_and_persisted`（下一条请求走新选择，写入 `state.json`）、`select_refuses_what_is_not_a_member_of_a_select_group`、`select_on_an_automatic_group_overrides_it_until_cleared`（自动组的临时覆盖，不写 `state.json`） |
| `POST /v1/policies/test` | `crates/rurge-api/tests/api.rs`：`policies_are_tested_on_request` |
| `GET /v1/policy_groups/test_results` | `crates/rurge-api/tests/api.rs`：`a_group_is_tested_on_request` |
| `POST /v1/policy_groups/test` | `crates/rurge-api/tests/api.rs`：`a_group_is_tested_on_request` |

`select` 的选择重启后保留：

- `crates/rurge-engine/tests/outbounds.rs`：`a_selection_applies_to_the_next_connection_and_survives_a_restart`：切换后读到 `state.json` 里的选择，再由同一份 `state.json` 新建第二个引擎，读到同一个选择。
- `crates/rurge-engine/tests/pipeline.rs`：`persisted_group_selection_is_honored`：带着已存的选择启动，请求走已选的成员。
- `crates/rurge-engine/src/state.rs`：`loads_defaults_selections_and_tolerates_garbage`、`store_opens_updates_atomically_and_renames_garbage`。

复跑：`cargo test -p rurge-api`、`cargo test -p rurge-engine --test auto_groups`、`cargo test -p rurge-engine --test smart`、`cargo test -p rurge-policy smart`。

### 互操作

不适用：策略组与 API 是 rurge 自己的行为，没有参照服务端。

### 还缺什么

- "重启"在用例里是由同一份 `state.json` 在进程内新建第二个引擎，没有一个用例拉起两次真实的 `rurge run` 进程并在第二次读到选择。
- `GET /v1/policies/detail`、`GET /v1/policy_groups`、`POST /v1/policies/test`、`GET /v1/policy_groups/test_results` 四个端点的响应形状是"暂定"（`docs/api/phase2.md`）：按社区已知的 Surge 响应实现，没有对照过真实 Surge 实例的样本。
- `smart` 的打分、站点记忆等常数是 rurge 按手册描述的近似实现（兼容性清单 5.1），没有与 Surge 对照。
- 手工清单 M3a、M3b、M3c 三节（真实节点上的订阅、测速、自动选择、`smart`）没有做。

## 4. 订阅样本与装配

> 订阅样本（策略行列表与含 `[Proxy]` 的完整配置两种形式）解析正确；过滤、前缀、修饰与装配顺序符合清单 5.2。

### 回环（本机通过）

- 两种形式：`crates/rurge-policy/src/subscription.rs`：`a_plain_list_is_read_line_by_line`（策略行列表：BOM、CRLF、三种注释、行内注释）、`a_whole_profile_gives_its_proxy_section_only`（含 `[Proxy]` 的完整配置只取 `[Proxy]` 节）、`a_proxy_header_with_blanks_inside_is_the_proxy_section`。
- 坏行与别的格式：`crates/rurge-policy/src/subscription.rs`：`a_line_that_cannot_be_used_is_skipped_by_number_without_its_text`（只报行号，不引用行的内容）、`a_subscription_in_another_format_yields_nothing`、`an_include_line_is_never_followed`、`more_than_the_limit_is_dropped`。
- 装配顺序：`crates/rurge-policy/src/assemble.rs`：`members_come_in_the_manual_order_each_once`（显式成员 → `include-other-group` → `include-all-proxies` → `policy-path`，重名保留首个）、`include_other_group_is_recursive_and_a_cycle_gives_nothing`、`two_groups_share_an_identical_import_but_not_another_definition`。
- 过滤、前缀、修饰：`crates/rurge-policy/src/assemble.rs`：`the_filter_and_the_prefix_act_in_the_manual_order`（`policy-regex-filter` 不作用于写在行上的显式成员，作用于 `include-all-proxies` 与 `policy-path` 带来的成员，并且见到加前缀之前的名字）、`the_modifier_rewrites_the_imported_lines_only`（`external-policy-modifier` 在文本层改写导入行，同名参数原地替换，没有的追加）、`members_taken_from_another_group_pass_the_filter`（`include-other-group` 取来的成员同样要过本组的过滤）。
- 组级中继：`crates/rurge-policy/src/assemble.rs`：`every_proxy_member_is_chained_through_the_group_relay`、`groups_with_the_same_relay_share_a_derived_policy`。
- 订阅行的安全门：`crates/rurge-policy/src/assemble.rs`：`a_subscription_line_may_not_reach_into_the_profile`、`a_subscription_never_brings_an_external_policy`。
- `external-policy-modifier` 的键按解析器读这一行的方式读（M8c，一行怎样拆成各项那一轮订正过）：整项带引号的 `"k=v"` 是值、没有键，原样留在行里，修饰设的参数追加在后面（M8c 时它算同名参数、被替换），类型、服务器与端口三个字段不取键，行尾留着未闭合的引号时修饰写在它前面（没有配上的括号不分组，不再算）：`crates/rurge-config/src/policy.rs`：`with_params_writes_around_quotes`、`a_modified_line_reads_as_the_line_did_with_the_modifier_in_place`；`crates/rurge-policy/src/assemble.rs`：`a_line_does_not_get_around_the_modifier_by_its_spelling`、`a_line_that_is_not_as_the_modifier_sets_it_is_refused`。
- 指向被略去的成员的中继（M8c）：成员全都挂在被略去的中继上的组成了空组，告警每个因此少了成员的组一条、至多列 20 个名字：`crates/rurge-policy/src/registry.rs`：`a_derived_policy_whose_relay_was_left_out_is_left_out_too`、`an_imported_policy_relayed_through_one_that_was_left_out_is_left_out_too`、`the_warning_names_twenty_members_at_most_and_counts_them_all`。
- 外部资源（规则集、域名集、订阅）的刷新（M8c）：规则集与域名集上的 `update-interval=0` 读作没写，按缺省的 86 400 秒刷新；没有缓存时服务器答 304 算一次失败的抓取：`crates/rurge-net/src/resource/mod.rs`：`an_interval_of_zero_is_read_as_none_written`、`a_not_modified_with_nothing_cached_is_a_failed_fetch`。策略组上的 `update-interval=0` 是加载错误 `E0018`，要求正数：`crates/rurge-config/src/spec/group.rs`：`values_that_cannot_be_used_are_errors`。两种读法不一致，取舍是项目所有者的（留给项目所有者的一节）。
- 配置层：`crates/rurge-config/src/spec/group.rs`：`the_manual_examples_read_into_the_spec`。
- 引擎级：`crates/rurge-engine/tests/subscriptions.rs`：`a_subscription_file_is_in_from_the_first_generation`、`an_edited_subscription_file_rebuilds_the_registry`、`a_url_subscription_arrives_after_the_start_and_is_cached`（回环 HTTP 服务器）、`a_session_leaves_through_an_imported_policy`、`a_derived_member_is_reached_through_the_group_relay`、`an_empty_group_stands_in_direct_or_rejects`。
- 命令行：`crates/rurge/tests/cli.rs`：`check_assembles_the_subscriptions_it_has`（`rurge check --data-dir` 离线装配，订阅 URL 不进输出）。

复跑：`cargo test -p rurge-policy assemble`、`cargo test -p rurge-engine --test subscriptions`。

### 互操作

不适用。

### 还缺什么

- 订阅样本是用例里手写的几行 `http` / `trojan` 策略，仓库里没有一份记录下来的真实订阅（`tests/corpus` 里没有订阅样本）。
- 手工清单 M3a 一节（真实机场订阅，包括按 `User-Agent` 选格式的面板与 Clash YAML 链接）没有做。兼容性清单 5.2 记着：订阅下载经 rurge 自己的直连，这一点未与 Surge 核对；`User-Agent` 的选择未在真实面板上核对。
- rurge 的 HTTP 客户端不把 URL 里的用户信息当 Basic 认证发出（规则集、域名集、订阅、脚本都一样）：从别的客户端带来的 `https://user:password@host/…` 不认证，抓取以 `http status 401` 失败（DoH 的上游自 M8a 起发出这些凭据）。发不发是项目所有者的决定（留给项目所有者的一节）。 2026-10-13 的更新：不再是这样了。2026-10-12 代项目所有者作了裁定（阶段 1 补遗（M5b），提交 `b33e27e`）：发。URL 里的用户名与口令（百分号解码之后）作为 `Authorization: Basic …` 头发出，头的值标为敏感；HTTP 客户端对它建的每个请求这样做——规则集、域名集、订阅、GeoIP 库与 Profile 的来源（托管 Profile、远程 `#!include`），DoH 的上游早就如此——，调用方自己带了 `Authorization` 头的以它为准；重定向时只带到同一来源（方案、主机、端口都相同）、且目标自己没有写用户名口令时，别的来源不给，从 `https` 到 `http` 的重定向仍被拒。连通性测试不经这个客户端：它们在被测策略自己的出站上发两次 `HEAD`，头只有 `Host` 与 `User-Agent`（读 `crates/rurge-policy/src/probe.rs` 得出，没有用例断言这一点），测试 URL 里的用户名口令不发出。代价：这些用户名口令此前从不作为认证发出——HTTP/1 的请求里没有它们，所以 `http://` 的 URL 从不把它们送出本机；HTTP/2（只在 TLS 之内）把它们夹在 `:authority` 里——，现在随每个请求发给 URL 指名的那台服务器，对 `http://` 的 URL 是明文（Base64 不是加密）。所以它不再等所有者定，但所有者可以改回：拿掉用户名口令而什么也不发，抓取以 `http status 401` 失败，与此前相同，是 `HttpClient::build` 里的一个条件（阶段 1 补遗（M5b）的计划 `docs/superpowers/plans/2026-10-11-phase1-m5b-profile-remote-sources-plan.md` 的「执行期修正记录」第 49 ～ 51 行，「延后事项」第 25 行）。

## 5. Shadow TLS v2 / v3 与六个 TLS 参数

> Shadow TLS v2（以及 v3，取决于风险 A 的结论）与六个 TLS 参数各有测试。

风险 A（v3 要自定 ClientHello 的 SessionID）已解除，v3 实现了（总设计第 15 节）。六个 TLS 参数是兼容性清单 4.4 的 `skip-cert-verify`、`sni`、`server-cert-verify-name`、`server-cert-fingerprint-sha256`、`alpn`、`client-cert`。

### 回环（本机通过）

Shadow TLS：

- 协议层 v2：`crates/rurge-proto/src/transport/shadow_tls/mod.rs`：`v2_carries_a_megabyte_both_ways_and_swallows_the_session_tickets`、`v2_works_over_a_tls_1_2_handshake_too`、`v2_with_a_wrong_password_ends_when_the_site_ends_the_session`。
- 协议层 v3：`crates/rurge-proto/src/transport/shadow_tls/mod.rs`：`v3_carries_a_megabyte_both_ways_in_the_relay_s_shape`、`v3_says_good_bye_like_a_browser_when_the_server_is_not_ours`、`v3_needs_a_handshake_server_that_speaks_tls_1_3`。
- 已知答案与签名：`crates/rurge-proto/src/transport/shadow_tls/vectors.rs`：`the_xor_key_is_sha256_of_password_and_server_random`、`data_tags_chain_per_direction_and_feed_themselves_back`、`the_v2_digest_is_eight_bytes_over_everything_fed`；`crates/rurge-proto/src/transport/shadow_tls/sign.rs`：`the_hello_is_one_record_with_a_32_byte_session_id_and_carries_the_tag`。
- 引擎级：`crates/rurge-engine/tests/outbounds_shadow_tls.rs`：`a_connect_leaves_through_trojan_wrapped_in_shadow_tls_v3`、`an_http_upstream_behind_shadow_tls_v2_needs_no_sni`、`a_wrong_shadow_tls_password_fails_the_session_with_a_text_that_says_so`、`shadow_tls_runs_through_an_underlying_proxy`。
- 配置层：`crates/rurge-config/src/spec/shadow_tls.rs`：`the_manuals_two_examples`、`what_is_wrong_with_the_three_parameters_is_an_error`。

六个 TLS 参数：

| 参数 | 配置读取 | 握手 | 对参照服务端 |
| ---- | -------- | ---- | ------------ |
| `skip-cert-verify` | `crates/rurge-config/src/spec/tls.rs`：`sni_off_and_defaults` | `crates/rurge-proto/src/transport/tls.rs`：`skip_cert_verify_accepts_anything` | 无 |
| `sni` | `crates/rurge-config/src/spec/tls.rs`：`sni_off_and_defaults` | `crates/rurge-proto/src/transport/tls.rs`：`verify_name_and_sni_are_independent`、`an_ip_literal_sends_no_sni` | sing-box 的 `https` 入站，用例里的 `Named` 一行 |
| `server-cert-verify-name` | `crates/rurge-config/src/spec/tls.rs`：`verify_name_is_checked_at_load_time`、`verify_name_without_chain_validation_is_ignored_with_a_warning` | `crates/rurge-proto/src/transport/tls.rs`：`verify_name_and_sni_are_independent` | 无 |
| `server-cert-fingerprint-sha256` | `crates/rurge-config/src/spec/tls.rs`：`a_fingerprint_wins_over_skip_cert_verify_with_a_warning` | `crates/rurge-proto/src/transport/tls.rs`：`a_pinned_fingerprint_replaces_chain_validation` | sing-box 的 `https` 入站，`Pinned` 与 `WrongPin` 两行 |
| `alpn` | `crates/rurge-config/src/spec/tls.rs`：`all_six_parameters` | `crates/rurge-proto/src/transport/tls.rs`：`alpn_comes_from_the_options_or_the_protocol_default` | 无 |
| `client-cert` | `crates/rurge-config/src/spec/tls.rs`：`invalid_values_and_keystore_references` | `crates/rurge-proto/src/keystore.rs`：`a_decoded_identity_completes_mutual_tls`；`crates/rurge-proto/src/h2connect/mod.rs`：`a_client_certificate_comes_from_the_keystore` | sing-box 的 `https` 入站，`Mutual` 与 `NoCert` 两行 |

复跑：`cargo test -p rurge-engine --test outbounds_shadow_tls`。

### 互操作（已写，尚未对真实的二进制运行）

2026-10-18 的更新：下面三个用例在运行 39022550224（`7bd6589`）里对真实的 sing-box 1.14.2 跑过一次，三个平台都通过；这是一次运行的结果，CI 仍没有绿过，见第 8 节的更新。

- sing-box 的 `shadowtls` 入站（v2、v3，v3 开 `strict_mode`）前接 `trojan`。`tests/interop/tests/sing_box_shadow_tls.rs`：`trojan_behind_shadow_tls_v2_and_v3`（小负载与跨多帧往返，伪装站点收到配置的 SNI）、`a_wrong_shadow_tls_password_is_not_relayed`（v3 报 `shadow-tls: the server did not authenticate itself`，伪装站点确实收到那个 HTTP 请求）。
- sing-box 的 `http` 入站加 TLS，覆盖六个参数中的 `sni`、`server-cert-fingerprint-sha256`、`client-cert` 与根证书校验。`tests/interop/tests/sing_box.rs`：`https_with_a_private_ca_a_pinned_fingerprint_and_a_client_certificate`。

### 还缺什么

- 官方 `shadow-tls` 服务端（`--v3 --strict`）没有互操作用例，只在手工清单 M2c 一节。
- `skip-cert-verify`、`server-cert-verify-name`、`alpn` 只有回环用例。
- 手工清单 M2c 一节（真实服务端与真实伪装站点）没有做。

## 6. WireGuard 与带 `client-id` 的端点

> WireGuard 与带 `client-id` 的端点握手成功并转发。

### 回环（本机通过）

回环对端 `FakeWgPeer` 用 boringtun 写成：给发出的每个报文写上保留字节，收到保留字节与 `client-id` 不符的报文不作答。

- 保留字节：`crates/rurge-proto-wireguard/src/wire.rs`：`the_reserved_bytes_carry_the_client_id_out_and_are_cleared_in`；`crates/rurge-proto-wireguard/src/stack.rs`：`the_client_id_goes_out_in_every_message_and_is_cleared_coming_in`、`without_the_client_id_such_a_peer_never_answers`；`crates/rurge-proto-wireguard/src/outbound.rs`：`every_message_carries_the_client_id`（经出站，对端记下每个报文的保留字节）。
- 握手与转发：`crates/rurge-proto-wireguard/src/outbound.rs`：`a_connection_through_the_tunnel_echoes`、`a_large_transfer_goes_through_whole`、`handshake_initiations_go_out_marked`（握手报文的 TOS）、`the_native_test_is_a_handshake_with_the_peers`。
- 隧道的结束与重建（M8c）：隧道只有一种结束方式（被更晚的配置接替、设备任务以任何方式结束、协议栈的锁中毒），等待者当场以 `wireguard: a newer configuration of this tunnel is in use` 或 `wireguard: the tunnel has closed` 结束，下一个连接起一条新隧道，意外结束的隧道在下一个连接时记一条 warn：`crates/rurge-proto-wireguard/src/outbound.rs`：`a_dial_waiting_to_connect_ends_when_the_tunnel_closes`、`a_task_that_panics_with_the_stack_locked_ends_the_tunnel_all_the_same`、`a_tunnel_whose_task_died_is_replaced_by_the_next_connection`、`a_tunnel_that_ended_unexpectedly_is_said_so_once`、`a_shared_tunnel_belongs_to_the_latest_configuration_that_shares_it`。隧道里的 DNS 服务器问不了的名字跳过它，之后的 `system` 仍被问：`a_name_a_tunnel_server_cannot_be_asked_for_still_reaches_system`。
- 配置：`crates/rurge-config/src/wireguard.rs`：`the_manuals_warp_example`（手册的 WARP 例子）、`a_client_id_is_written_three_ways`。
- 引擎级：`crates/rurge-engine/tests/outbounds_wireguard.rs`：`a_connect_leaves_through_the_tunnel`、`udp_leaves_through_the_tunnel`、`a_policy_test_is_a_handshake_with_the_peers`、`a_reload_keeps_the_tunnel_unless_its_section_changes`。

复跑：`cargo test -p rurge-proto-wireguard`、`cargo test -p rurge-engine --test outbounds_wireguard`。

### 互操作（已写，尚未对真实的二进制运行）

2026-10-18 的更新：这个用例在运行 39022550224（`7bd6589`）里对真实的 sing-box 1.14.2 跑过一次，三个平台都通过；这是一次运行的结果，CI 仍没有绿过，见第 8 节的更新。

`tests/interop/tests/sing_box_wireguard.rs`：`a_connection_goes_through_a_sing_box_wireguard_endpoint`：sing-box 1.14.2 的 WireGuard 端点（`endpoints`，`system: false`，用户态）以 rurge 为唯一的 peer，给发往它的每个报文写保留字节 `1/2/3`；rurge 的策略写 `client-id = 1/2/3`，握手后经隧道往返单块与跨多块的 TCP、一个 UDP 回显，并做原生的握手测速。

### 还缺什么

- 上面这个用例没有对真实的 sing-box 运行过。（2026-10-18：它在运行 39022550224（`7bd6589`）里对真实的 sing-box 1.14.2 跑过一次，三个平台都通过。）
- 真实的 WireGuard 服务端（`wg-quick`、路由器、云主机）与 Cloudflare WARP 没有跑过：手工清单 M4b 一节的基本连通、吞吐、WARP 与"WARP 启动后的第一次查询"各项没有做。吞吐只有回环基准（`crates/rurge-proto-wireguard/src/outbound.rs`：`throughput`，忽略项，两端都是 smoltcp，不作门禁）。
- 重新握手、keepalive 与过期的计时器由 boringtun 的计时器驱动（设备任务每 250 毫秒推进一次，peer 的 `keepalive` 交给它），没有用例让 90 或 120 秒过去，假对端建时没有 keepalive（需求审计的"做了而没找到用例"一项）。

## 7. `h3://` 与 `quic://` 上游

> `h3://` 与 `quic://` 上游解析正常。

### 回环（本机通过）

- URL 的解析：`crates/rurge-dns/src/upstream/mod.rs`：`parses_every_form`、`a_quic_url_is_a_host_and_a_port`（`quic://` 缺省 853，多出路径是错误）、`an_h3_url_defaults_to_port_443_and_the_dns_query_path`、`a_user_name_or_a_password_is_refused_and_never_quoted`。
- DoQ：`crates/rurge-dns/src/upstream/doq.rs`：`a_query_is_its_padded_message_with_id_0_on_a_stream_of_its_own`（报文 ID 0，padding 到 128 字节的倍数）、`a_reply_with_a_message_id_closes_the_connection`、`the_certificate_is_verified_unless_skipped`、`a_name_goes_through_the_bootstrap_and_its_addresses_in_turn`。
- DoH3：`crates/rurge-dns/src/upstream/doh3.rs`：`a_query_is_a_post_of_the_message_with_id_0`、`the_request_names_the_urls_path_and_authority`、`another_status_or_content_type_is_refused`、`a_goaway_sends_the_next_query_to_a_new_connection`。
- DoH 的缺省路径与 `Age`（M8c）：没写路径的 `https://…` 上游发往 `/dns-query`（`h3://…` 本来如此），`Age`（RFC 8484 5.1）从应答每条记录的 TTL 里减去，DoH 与 DoH3 都是：`crates/rurge-dns/src/upstream/doh.rs`：`a_url_written_without_a_path_posts_to_dns_query`、`the_age_of_an_answer_comes_off_its_ttls`、`the_age_of_an_answer_is_what_the_first_member_of_its_age_says`；`crates/rurge-dns/src/upstream/doh3.rs`：`the_age_of_an_answer_comes_off_its_ttls`；`crates/rurge-dns/src/upstream/mod.rs`：`an_https_url_written_without_a_path_goes_to_dns_query`。
- 并发查询过了截止时间、还有轮次没发时以超时结束，不再空转（M8c；一个自阶段 1 起就在代码里的缺陷，已修复）：`crates/rurge-dns/src/fanout.rs`：`a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout`、`a_fanout_kept_from_running_until_past_its_deadline_hands_over_what_it_has`；这两条与同模块的其它计时用例走暂停的时钟和内存里的上游（`crates/rurge-dns/src/testing.rs`：`the_table_answers_in_memory`）。
- 经解析器（A 与 AAAA 在一条连接上）：`crates/rurge-dns/tests/resolver.rs`：`doq_answers_a_and_aaaa`、`doh3_answers_a_and_aaaa`、`a_doq_server_named_by_host_is_bootstrapped`、`a_host_entry_names_a_doq_server`（`[Host]` 指向 DoQ 服务器）。
- 跟随出站模式（DoQ / DoH3 的 QUIC 载体作为内部 UDP 会话走规则）：`crates/rurge-engine/tests/pipeline.rs`：`a_doq_carrier_follows_the_rules_through_a_socks5_proxy`、`a_doh3_carrier_follows_an_ip_rule_through_a_socks5_proxy`、`a_doq_carrier_routed_to_direct_is_an_internal_udp_session`。

复跑：`cargo test -p rurge-dns doq`、`cargo test -p rurge-dns doh3`。

### 互操作（已写，尚未对真实的二进制运行）

2026-10-18 的更新：这 4 个用例在运行 39022550224（`7bd6589`）里对真实的 dnsproxy v0.86.0 与 CoreDNS v1.14.7 跑过一次，三个平台都通过；这是一次运行的结果，CI 仍没有绿过，见第 8 节的更新。

`tests/interop/tests/dns.rs`：`doq_against_dnsproxy`、`doh3_against_dnsproxy`、`doq_against_coredns`、`doh3_against_coredns`。dnsproxy v0.86.0 与 CoreDNS v1.14.7 各自作答同一个名字的 A 与 AAAA；每个用例用两个解析器：一个直连，一个只经 `connect_udp` 拿载体（与跟随出站模式相同），后者的四个问题必须都在一条连接上。CoreDNS 对报文 ID 不是 0 或带 edns-tcp-keepalive 选项的 DoQ 查询以协议错误关连接，DoH3 只在 `/dns-query` 上作答（这两点读自它的源码，没有对真实的二进制验证过），所以它是严格的那个。

### 还缺什么

- 四个用例没有对真实的二进制运行过；dnsproxy 与 CoreDNS 的命令行、Corefile 与就绪日志读自源码。（2026-10-18：它们在运行 39022550224（`7bd6589`）里对真实的 dnsproxy 与 CoreDNS 跑过一次，三个平台都通过，命令行、Corefile 与就绪日志对真实的二进制行得通。）
- `rurge dns lookup` 命令行没有 `quic://` 或 `h3://` 的用例；手工清单 M7c 一节"公共 DoQ / DoH3"用公共服务做这件事。
- 手工清单 M7c 一节的"公共 DoQ / DoH3"、"跟随出站模式"、"混用加密上游"三项没有做。

## 8. 三平台 CI 绿；clippy 零警告；`W0007` 与 `W0008` 只在该出现时出现

> 三平台 CI 绿（含互操作 job）；clippy 零警告；`W0007` 只对 rurge 不实现的协议与变体出现（`tailscale`、`snell` 的 `version=6`、手册没列的 `ss` 方法名），`W0008` 只对 `subnet` 组出现（阶段 3）（M8 细化设计 6.2 订正；原文是"`W0007` 与 `W0008` 不再出现"）。

### 三平台 CI

状态：**未满足**。

- 工作流：`.github/workflows/ci.yml`，推送到 `main` 与拉取请求时触发。`check` 任务在 `ubuntu-latest`、`windows-latest`、`macos-latest` 上各跑一遍，工具链固定 `1.89.0`；`RURGE_INTEROP_REQUIRED=1` 在工作流一级的 `env` 里设置，不由哪一个安装步骤设置（否则那一步一失败，全部互操作用例会不声不响地跳过）。步骤依次是 `cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、各参照服务端的安装（连同把 `RURGE_TEST_SSHD` 指向 `sshd` 的那一步，每步都 `continue-on-error: true`：某个服务端装不上时任务不在那一步结束，要用它的用例各自以"找不到二进制"失败，其余照常运行）、`cargo test --workspace --no-fail-fast`（限时 60 分钟）、`Report the failed tests`（失败时把失败的用例发成注解与任务摘要，没跑完的二进制连同其中还在跑的用例也发成一条注解——2026-10-18 起那一条也列出其中已经失败的用例，`e499061`——，脚本是 `.github/scripts/report-test-failures.py`；任务日志要登录才能读，注解与摘要公开可读）。另有一个 `bench` 任务（`ubuntu-latest`，只做 `cargo bench -p rurge-rules --no-run` 与 `cargo bench -p rurge-dns --no-run`），不跑互操作。
- 没有单独的互操作任务：`tests/interop` 是工作区成员，互操作用例随 `cargo test --workspace` 一步运行。
- 最近一次运行在提交 `0154e73`（2026-10-08）：`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings` 与当时已有的各个参照服务端的安装步骤在三平台通过（mihomo 的安装步骤是之后的 `a521b65` 加的，没有运行过）；`cargo test` 一步在三平台都失败。失败的是哪些用例未知：任务日志要登录才能读。
- 那次运行的 `cargo test --workspace` 没有 `--no-fail-fast`：第一个失败的测试二进制之后的二进制不运行，互操作用例是否运行过无从得知。
- 本地的 `origin/main` 是 `0154e73`；它之后到 `44a0204` 有 76 个提交，没有一个被 CI 见过：M8a 的失败摘要与凭据不外泄，M8b 的流式方法、Snell v1 ～ v3、mihomo 的安装步骤与互操作用例，M8c 的全部改动与终审之后的修正波。
- 失败摘要（`a965db1`）已提交，没有推送；自 `0154e73` 起没有推送过。读失败清单并修掉是 M8a 任务 6，等推送之后做。
- 只在 Unix 编译的用例（`crates/rurge/tests/cli.rs`：`run_shuts_down_gracefully_on_sigint`）在本机（Windows）不编译；只在 Windows 编译的用例（`crates/rurge-engine/tests/pipeline.rs`：`a_doq_carrier_that_outlives_a_port_unreachable_is_no_failed_session`）只在本机编译；`tests/interop/tests/sshd.rs` 的 2 个也只在 Unix 上编译。它们在 CI 上都没有可读的结果。（2026-10-18：运行 39022550224（`7bd6589`）里，`run_shuts_down_gracefully_on_sigint` 与 `sshd.rs` 的 2 个用例在 Linux 与 macOS 上编译、运行，通过了；只在 Windows 编译的那条在 Windows 上跑到了，也通过了。）

2026-10-18 的更新：上面写的“本地的 `origin/main` 是 `0154e73`”已经过时——项目所有者推送过了，最近一次运行在 `7bd6589`（2026-10-17，运行号 39022550224；它是阶段 3 / M1 的合并，阶段 3 / M2 的提交不在里面）。这次运行读得到失败清单，结论仍是**未满足**：CI 没有绿过。`bench` 任务通过；`check` 任务里，`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings` 与全部参照服务端（连同 mihomo）的安装步骤三个平台都通过，三个平台都在 `cargo test` 一步失败。Linux 6 分半跑完了全部测试二进制，以 101 退出，5 条失败。macOS 与 Windows 又用满了测试一步的 60 分钟，都停在 `rurge-proto` 的单元测试二进制里（各有三条用例没有结束）；测试二进制按包名的次序一个接一个地跑，所以只跑到排在它前面的各包——`rurge`、`rurge-api`、`rurge-config`、`rurge-dns`、`rurge-engine`、`rurge-external-tests`、`rurge-inbound`、`rurge-interop`、`rurge-net`、`rurge-platform`、`rurge-policy`——，macOS 上 6 条失败、Windows 上 7 条失败；`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-rules` 这四个包的测试在这两个平台上没有跑到，`rurge-proto` 单元测试里别的用例在这两个平台上的结果也不知道（失败用例的输出要到二进制结束时才打印，那时的失败摘要也不列没跑完的二进制里已经失败的用例——`e499061` 起列出——，任务日志又要登录才能读）。失败摘要这一次认出了跑完的二进制里的全部失败，所以第 1 条与第 7 条要的互操作用例第一次读得到对真实二进制的结果：装了参照服务端的平台上，除下面第 2 ～ 4 类的五条之外都通过——这是那一次运行的结果，CI 仍没有绿过；各平台实际跑了多少个互操作用例没有读到（任务日志要登录才能读），按第 1 条那张表的平台一栏算，应当是 Windows 44、macOS 52、Linux 63。

六类失败按根因分（根因读自代码、参照服务端的源码与那次运行的注解，本机能复现的只有第 1 类），改在这些提交上：

- 第 1 类（macOS、Windows）：`rurge-proto` 里 `shadowsocks` 与 `snell` 的四条单元用例，每个平台有三条没有结束——`LazyHead` 的两半交给两个任务轮询时丢了写端的唤醒。`f8d0389` 在类型里修，`4fc1bad` 把在别的任务里写、在用例的任务里等它写完的 18 处限时 30 秒，`cae06c3` 加四条用例（其中一条用真实的管道、两半各在一个任务里复现了这一类，旧代码上红），`26be5e2` 收尾。整分支终审之后又加了两个提交：`199e584` 让写端的 `poll_flush` / `poll_shutdown` 完成时也了结读端欠着的 flush，`f0e41fd` 加一条 VMess 的 UDP 用例。这一类在生产里也碰得到：VMess 的 UDP 载体在拨号时起一个任务读、由 `send_to` 的任务写，旧代码下第一个数据报的写过了 100 毫秒的宽限仍是 `Pending` 时，数据报其实已由读端推了出去，`send_to` 却没有人唤醒，要等到引擎 15 秒的 `SEND_TIMEOUT` 到时——`tokio::time::timeout` 到时先再轮询一次里面的 future，发送这才完成，记为成功——，这期间那条流后面的数据报都等着（引擎这一段读代码得出，没有经引擎跑过）；直连时第一次写几乎不会 `Pending`，经有流控的 `underlying-proxy` 或在卡住的路径上可能碰到（这两句都是推断：没有观察过第一次写过了宽限仍是 `Pending`，证据只有下面这条人为把写堵住的用例）。`f0e41fd` 的用例让连接的写从第一次起被堵 200 毫秒：在旧代码上发送要到 5 秒的时限才完成，用例按时钟判它失败；现在约 0.2 秒完成。
- 第 2 类（三个平台）：`ss` 的 SS 2022 对 sing-box 与对 `ssserver` 的两个用例——收 UDP 回包的缓冲只有 64 字节，装不下 65 字节的回包。`c048315`。
- 第 3 类（Linux）：`snell` 对 snell-server 的 UDP 与 obfs 两个用例——snell-server 的双栈 socket 把 IPv4 的来源写成 `::ffff:127.0.0.1`，出站照原样报回，用例比较时没有归一。`2519b3f`。
- 第 4 类（Linux）：`masque` 对 masque-server 的 TCP 用例——masque-server 每条 TCP 隧道至多排 1 MiB 客户端发来而还没写给目标的字节，超出即以 H3_CONNECT_ERROR 重置。这是参照服务端的限制，用例改为节制上行（至多领先回读 512 KiB，总长不变），限制登记在兼容性清单 4.2 节 `masque` 一行。`0deba9f`。
- 第 5 类（macOS）：`udp` 的三条与 `udp_tls_family` 的一条——macOS 上 UDP socket 的缺省发送缓冲是 9216 字节，更大的数据报以 `EMSGSIZE` 失败，失败在用例自己的 socket 上。`f9377a8` 把要发大数据报的测试 socket 的发送缓冲调到 256 KiB，并预先改了 `rurge-proto` 里两处预料中的同一种失败。
- 第 6 类（Windows）：`rurge-api` 的五条用例——`MockDns::start` 找不到同时空闲的 TCP 与 UDP 端口，原因没有证据。`94cfd9e` 让两种绑定次序轮流试，找不到时说出两种绑定各自最后的错误。

**要下一次推送才知道修好了没有。**只有第 1 类在本机复现过——`cae06c3` 里用真实的管道、两半各在一个任务里的用例，与 `f0e41fd` 里 VMess 的 UDP 用例，在旧代码上红、现在绿——；CI 上失败的那四条用例本身在本机用旧代码也过（本机的回环 socket 从不让写变成 `Pending`），所以它们没有被复现。第 2 ～ 6 类在本机无从复现（参照服务端本机一个也没有，第 5、6 类出在 CI 的 macOS 与 Windows 上），判据是本机的全量通过。macOS 与 Windows 上没有跑到的四个包的测试和 `rurge-proto` 单元测试其余用例的结果也要下一次推送。第 6 类的原因仍然不知道。AnyTLS 的 SYNACK 看门狗仍没有对真实服务端验证过，这次运行也验证不了它（第 1 条的“还缺什么”）。第 8 条仍是未满足：要有一次三平台全绿的运行。

### clippy

`cargo clippy --all-targets -- -D warnings` 在本机（提交 `14b1183`，工具链 1.89.0，与 CI 钉的一致）零警告。CI 的 clippy 一步在 `0154e73` 上三平台通过；`14b1183` 上没有 CI 运行。

### `W0007` 与 `W0008`

`W0007` 的含义自 M8b 起改为"rurge 不实现的协议或变体"（M8-D10）；`W0008` 只指策略组类型不在能力表里。二进制的能力表在 `crates/rurge/src/capabilities.rs`：策略类型缺 `Tailscale`，组类型缺 `Subnet`。下列用例钉住出现条件。

回环（本机通过）：

- 配置层：`crates/rurge-config/src/config.rs`：`a_policy_type_that_is_not_supported_says_so_and_tailscale_how_else`（每种类型一条，`tailscale` 的文本指出经 tailscaled 的 SOCKS5 接入）、`ss_stream_ciphers_are_remarked_on_once_per_load_and_cipher`（流式方法是 `W0031` 而不是 `W0007`；手册没列的方法名是 `W0007`）、`snell_version_6_is_reported_once_per_load_and_the_others_are_built`（`version` 1 ～ 5 构建，6 是 `W0007`）、`vmess_lines_without_vmess_aead_are_remarked_on_once_per_load`（没写 `vmess-aead=true` 的行是 `W0029`，不是 `W0007`）、`capability_warnings`（`W0008` 的机制）。
- 手册没列的九个 `ss` 方法名：`crates/rurge-config/src/spec/ss.rs`：`a_cipher_surge_no_longer_lists_is_named_and_not_an_error`；`snell` 的 `version=6`：`crates/rurge-config/src/spec/snell.rs`：`version_6_is_valid_but_not_supported`。
- 订阅导入的行：`crates/rurge-policy/src/assemble.rs`：`an_imported_line_that_cannot_be_used_is_skipped_by_its_number`（导入的 `tailscale`、`ss` 的 `bf-cfb`、`snell` 的 `version=6` 各得一条 `W0007`，仍是组成员）、`imported_vmess_lines_without_vmess_aead_are_members_and_said_once_per_group`、`imported_ss_lines_with_a_stream_cipher_are_members_and_said_once_per_group_and_cipher`。
- 二进制的能力表，经 `rurge check`：`crates/rurge/tests/cli.rs`：`check_knows_the_m1_protocols_and_runs_the_dry_build`（只有 `tailscale` 出 `W0007`）、`check_knows_ss`（流式方法不出 `W0007`，`bf-cfb` 出一条）、`check_knows_snell`（v1 ～ v5 不出，v6 出一条）、`check_knows_vmess_and_anytls`（没写 `vmess-aead` 的行不出 `W0007`）、`check_knows_the_automatic_groups`（`W0008` 恰一条，只提 `subnet`，不提 `smart`）；`check_knows_trojan`、`check_knows_ssh`、`check_knows_wireguard`、`check_knows_external`、`check_knows_http2`、`check_knows_hysteria2`、`check_knows_tuic`、`check_knows_masque` 各断言本协议不出 `W0007`、整份配置只有 `tailscale` 那一条。
- 运行期这些策略按 `REJECT` 处理并说明原因：`crates/rurge-policy/src/registry.rs`：`a_line_that_asks_for_what_is_not_implemented_says_why_it_rejects`、`unsupported_protocols_and_devices_reject_with_a_note`；`crates/rurge-engine/tests/pipeline.rs`：`an_unimplemented_policy_rejects_with_an_explanation`；`crates/rurge-engine/tests/outbounds_snell.rs`：`snell_v6_rejects_and_says_which`；`crates/rurge-engine/tests/outbounds_shadowsocks.rs`：`a_cipher_that_is_not_supported_rejects_and_says_which`。
- API：`crates/rurge-api/tests/api.rs`：`profiles_current_check_and_reload`（`POST /v1/profiles/check` 报出 `W0007`）。

### 还缺什么

- CI 从没有绿过。要满足这一条：项目所有者推送（推送是项目所有者的事）；读三平台的失败清单——失败摘要把失败的用例发成了注解与任务摘要，不登录也读得到——，修掉（M8a 任务 6）；再有一次三平台全绿的运行，且其中互操作用例都运行了（`RURGE_INTEROP_REQUIRED=1` 保证找不到二进制就失败；装不上的服务端只让它自己的用例失败，不让别的用例不运行）。（2026-10-18：推送与读失败清单已经做了，六类失败的改动也做了（第 6 类的原因仍不知道，改的是找端口的做法与诊断），见上面的更新；还缺再推送一次与那一次的结果。）
- 标准写的是"含互操作 job"，CI 里没有这样一个任务，互操作用例在 `cargo test --workspace` 一步里。要么把标准读作"互操作用例在那一步里全部运行并通过"，要么另拆一个任务；本文按前一种读法，没有改标准的文字。
- clippy 在 `14b1183` 上还没有 CI 的结果。

## 9. 需要真实公网节点的项目进入手工验收清单

> 需要真实公网节点的项目进入 `docs/acceptance/phase2-manual.md`，由项目所有者用自己的节点手工验收。

清单 `docs/acceptance/phase2-manual.md` 存在。第 3 行规定每一项记下日期、平台与结果。清单共 22 节 187 项（M2a、M2b 的三节共 20 项是步骤表，其余 167 项是复选项），没有一项被勾选，也没有一项记过结果。这些项由项目所有者用自己的节点做，自动化测试（只用回环）覆盖不了。阶段 1 的系统代理与服务安装另有 `docs/acceptance/phase1-manual.md`（Windows 12 项、macOS 10 项、Linux 10 项，共 32 行，结果、日期与系统版本三栏全是空的），M8c 往里加了两项共 4 行：三个平台各一行"配置加载不了时的恢复"（系统代理），Windows 另有一行"睡眠唤醒后的解析器"（CPU 回落、解析恢复）。 2026-10-13 的更新：之后阶段 1 补遗（M5a）加了 2 行（macOS 与 Linux 各一行），阶段 1 补遗（M5b）加了 11 行，现在共 45 行（Windows 14、macOS 13、Linux 13，另有「与平台无关」一节 5 行），仍是一行也没有做过。 2026-10-14 的更新：手册基线重对在「与平台无关」一节加了 1 行（真实的 GeoLite2 库之下的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`），现在共 46 行（Windows 14、macOS 13、Linux 13，与平台无关 6 行），仍是一行也没有做过；阶段 2 的清单新增的一节「手册基线重对」（2 项）已计入上面的 22 节 187 项。

| 节 | 项数 | 内容 |
| -- | ---- | ---- |
| M2a　Trojan | 8 | 真实 `trojan` 节点：HTTP 与 SOCKS5 入口、明文 HTTP、服务端先说话的协议、口令错、WebSocket、脱敏、`select` 切换 |
| M2b　VMess | 6 | 真实 `vmess` 节点：UUID 错与本机时钟偏差的表现、WebSocket、脱敏 |
| M2b　AnyTLS | 6 | 真实 `anytls` 节点：会话复用、重载时连接池的保留与替换 |
| M2c　Shadow TLS | 9 | 官方 `shadow-tls` 与 sing-box 服务端，v2、v3，大文件与长连接，口令错，半关闭，`underlying-proxy` |
| M3a　成员装配与订阅 | 8 | 真实机场订阅：首次启动、缓存、日志不含订阅链接、过滤与前缀、组级中继、Clash YAML、订阅更新 |
| M3b　测速与自动组 | 8 | 两个延迟不同的真实节点：`url-test`、`fallback`、`load-balance`、`evaluate-before-use`、临时覆盖 |
| M3c　smart | 7 | 三个真实节点：质量回报、停掉节点后的换成员、站点记忆、`policy-priority`、日志 |
| M4a　SSH | 10 | 真实 `sshd`：口令与密钥登录、主机密钥钉住、会话复用、空闲断开、断线重建、认证失败的退避 |
| M4b　WireGuard | 12 | 真实 WireGuard 服务端与 WARP：连通、目标域名、路由、吞吐、重载、endpoint 跟随、网络变化 |
| M4c　external | 11 | 三个平台各验一遍：按需拉起、再拉起、停止与进程树、重载 |
| M5a　UDP 地基 | 11 | SOCKS5 UDP 客户端：DNS、游戏或语音、全锥、`block-quic`、不载 UDP 的策略、NAT 后的中继 |
| M5b　TLS 族的 UDP | 4 | `trojan`、`vmess`、`anytls` 节点的 UDP |
| M5c　WireGuard 的 UDP 与其余 | 7 | WireGuard 的 UDP、经底层策略的隧道、WARP 上的 UDP、`test-udp`、`dns-follow-interface` |
| M6a　Shadowsocks | 9 | AEAD 与 2022、UDP、多用户、obfs、`udp-port`、口令与时钟 |
| M6b　Snell | 7 | v5 节点：`reuse`、obfs、UDP、psk 与版本不符 |
| M6c　HTTP/2 族 | 8 | `h2-connect` 与 TrustTunnel 节点：`max-streams`、认证失败、CONNECT-UDP、休眠唤醒 |
| M7a　Hysteria 2 | 10 | TCP、UDP、Salamander、Gecko（含与 Surge 对照）、端口跳跃、`underlying-proxy` |
| M7b　TUIC | 12 | v5 与 v4 节点：TCP、UDP、心跳、端口跳跃、`quic` 模式、与 Surge 对照 |
| M7c　HTTP/3 族与 DoQ / DoH3 | 13 | `masque`、`trust-tunnel` 的 `h3=true`、公共 DoQ / DoH3、跟随出站模式 |
| M8b　流式方法、Snell v1 ～ v3 | 13 | 流式方法的 TCP 与 UDP、`salsa20`、Surge 自带的 Snell 服务端（v1）、没写 `vmess-aead` 的 `vmess` 行 |
| M8c　收尾与遗留的几项 | 6 | `masque` 经 `underlying-proxy` 时内层 QUIC 可用、AnyTLS 半死会话三秒内失败、复用的会话上的 UDP 过了三秒仍可用、`socks5-tls` 对真实的 SOCKS5 over TLS 服务器、`include-other-group` 取到的成员走不走那个组的中继（与 Surge 对照）、经 tailscaled 的 SOCKS5 接入 tailnet |
| 手册基线重对 | 2 | `wireguard` 行上的 `tos` 在线上的 DSCP 位（握手的发起包 `0x88`、其余是策略的 `tos`；Windows 上系统是否照办另记）、一行真实的旧 `custom` 策略对真实的 Shadowsocks 服务端 |

阶段 1 清单里 M8c 加的那两项，自动化里的证据只到回环和假后端为止：配置读不了、配置有错误、`--log-file` 打不开这三种坏配置时，`rurge run` 先把上一次崩溃留下的系统代理设置恢复再以 2 退出，用例只经 `RURGE_SYSTEM_PROXY_BACKEND=file:<路径>` 的文件后端（`crates/rurge/tests/cli.rs`：`run_restores_a_crashed_system_proxy_when_the_profile_cannot_be_read`、`run_restores_a_crashed_system_proxy_when_the_profile_has_errors`、`run_restores_a_crashed_system_proxy_when_the_log_file_cannot_be_opened`、`a_profile_that_does_not_load_leaves_a_live_instances_system_proxy_alone`、`run_keeps_the_backup_when_it_cannot_restore_it`），真实的注册表、`networksetup`、`gsettings` / `kwriteconfig` 从没在这条路径上跑过；睡眠唤醒后的解析器，用例拨的是暂停的时钟（`crates/rurge-dns/src/fanout.rs`：`a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout`），真实的睡眠只能手工验。

### 还缺什么

- 第 1 条点名要进清单的五种情形都有对应项：`socks5-tls`：M8c；CONNECT-UDP over HTTP/2：M6c；TUIC 的 `quic` 模式：M7b；端口跳跃：M7a、M7b、M7c；`salsa20`：M8b。
- 清单没有 M1（`http`、`https`、`socks5`）一节。这三种协议在 `tests/interop` 里有 sing-box 的用例；`socks5-tls` 没有参照服务端，只有 M8c 一节的一项。
- M8 细化设计 6.2 说要给清单补三节：流式方法、Snell v1（对 Surge 自带的服务端）、经 tailscaled 接入。前两者在 M8b 一节里，经 tailscaled 接入在 M8c 一节里。M8c 一节另有"`masque` 经 `underlying-proxy` 时内层 QUIC 可用"与"AnyTLS 半死会话三秒内失败"等。
- 清单上的写法有几处是读文档或源码得出的、没有人照着做过一遍：经 tailscaled 的 SOCKS5 接入（可行性报告 3.3 的写法）、MASQUE 经 `underlying-proxy` 的数字（按 quinn 的余量算出来的）。走不通时要订正兼容性清单与可行性报告里的写法。
- 187 项由项目所有者做。做完的项在清单里记日期、平台与结果，本文第 9 条才能改写为满足。

## 需求逐条对照代码

九条标准是阶段 2 自己定的验收；这一节查的是另一件事：需求文档（`docs/requirements.md`）列给阶段 1 和阶段 2 的功能需求，代码里是不是真的有。阶段 1 没有自己的验收文档，它的缺口也记在这里。下文说"计划「延后事项」第 N 行"，指 `docs/superpowers/plans/2026-10-09-phase2-m8c-leftovers-acceptance-plan.md` 的同名一节；说"遗留事项总表 M1a #1"，指 `docs/superpowers/plans/2026-10-09-phase2-leftovers.md` 里 M1a 一节的第 1 行；说"阶段 1 / M5a 计划「延后事项」第 N 行"（或「执行期修正记录」第 N 行），指 `docs/superpowers/plans/2026-10-11-phase1-m5a-small-items-plan.md`（阶段 1 补遗（M5a）的计划）的同名一节。

### 为什么做

2026-10-10，受派去改兼容性清单开头那一句的那个助手发现，需求文档列为阶段 1、P0 的两项需求从来没有做：托管配置的更新（FR-CFG-05）与远程 `#!include`（FR-CFG-04 的"远程 URL"）。两份 README 的特性表和 `CLAUDE.md` 却把阶段 1 写成功能齐备。它说，没有人核对过其余的条目；这次核对就是为此做的。

### 是什么

两个只读的助手（较小的模型），每个阶段一个，把需求文档第 4 节里属于该阶段的功能需求，加上第 7 节该阶段的交付物与验收标准，逐句同代码对照。一条需求拆成它的条款：枚举里的每一项、每一个"和"、每一个括号都是一句。每一句要写出作用于它的代码——不是只有解析器，要的是消费解析结果的那一处——和一个用例，或者一个判词：做了；做了而没找到用例；只解析（接受了、存下了、没有东西用它）；没有（连解析都没有，或明说跳过）；有意不同（在兼容性清单或设计里登记了）；按需求文档自己的话属于后面的阶段；未能判定。

这是一个助手读代码的结果，不是测试运行的结果：什么也没运行；"用例"指找到一个这个名字的测试函数，不是它通过。控制者（调度这次执行的代理）在代码里核对了报告里每一处缺口；判为"做了"的没有人复核。所以这次核对显示缺口，不证明别处没有缺口。

### 数字

| | 阶段 1 | 阶段 2 |
| ---- | ------ | ------ |
| 审计的需求 | 49 条，加第 7 节的 19 行（目标、前言、九项交付物、八条验收标准） | 第 7 节阶段 2 清单里的 23 条，加另两条（FR-CFG-06、FR-OBS-02）的阶段 2 部分，再加第 7 节的 12 行 |
| 条款 | 287 | 196 |
| 做了 | 214 | 173 |
| 做了而没找到用例 | 11 | 3 |
| 只解析 | 12 | 5 |
| 没有 | 8 | 0 |
| 有意不同（已登记） | 13 | 5 |
| 按需求文档自己的话属于后面的阶段 | 29 | 9 |
| 未能判定 | 0 | 1 |

本文说"不在"的条款，是上表里"只解析"与"没有"两行的和：阶段 1 是 12 + 8 = 20 句，阶段 2 是 5 + 0 = 5 句。下面两张表逐句列出它们。

### 阶段 1 补遗（M5a）之后（2026-10-11）

上面的数字是 2026-10-10 审计当时的，不改。2026-10-11 的阶段 1 补遗（M5a）（设计 `docs/superpowers/specs/2026-10-11-phase1-m5-addendum-design.md`，计划 `docs/superpowers/plans/2026-10-11-phase1-m5a-small-items-plan.md`，下称"阶段 1 / M5a 计划"）补上了其中的大部分。本节与下面表格最后一列引用的提交都在 `e347044..09f1a7a` 里（`e347044` 是 `main`）。引用的用例都存在；除了下面点名的几处，它们在本机（Windows）的全量运行里通过——`09f1a7a` 上的那一次：71 个结果行，2800 个通过，0 个失败，6 个忽略（三个吞吐基准与三个内存实测）——，通过的意思也只是这个。点名的几处：只在 Unix 上编译的 `sighup_reloads_the_profile` 与 `run_shuts_down_gracefully_on_sigint` 里新加的一行等待，以及 macOS、Linux 上的 `rurge_platform::sysinfo` 读取，在本机没有编译过、运行过；三平台 CI 没有绿过。引用的用例之一，`a_start_with_the_recorded_profile_leaves_state_json_as_it_is`，在后来一次按过滤词的运行里红过一次，原因在这个用例的辅助函数里（请求分几段写出，晚到的正文让服务端凭请求头就答复并关掉了连接，应答丢了），已修：提交 `bbf220d`，在 `09f1a7a` 之后，所以不在上面说的提交范围里（阶段 1 / M5a 计划「执行期修正记录」第 67 行）；有这个修正的提交（`6ceeba8`，同样在 `09f1a7a` 之后）上的全量运行：仍是 71 个结果行，2800 个通过，0 个失败，6 个忽略（第一遍里有一个测试二进制死于本机已知的偶发崩溃，没有用例失败，单独重跑通过）。

- **还剩什么**：下表有 13 行（审计数的是句，一行可以含几句，这里数的是行，没有按句重数），10 行补上了，最后一列逐行写补在哪个提交、凭哪些用例。没有做的是三行，都属阶段 1 补遗（M5b）：托管配置的缓存与按间隔更新（FR-CFG-05）、远程 `#!include`（FR-CFG-04、FR-CFG-14）、外部资源的手动强制更新（FR-CFG-14）。代码里没有它们的任何部分，设计在同一份设计文档的第 7 节，计划还没有写。
- **一条审计读成"做了"、其实只做了一部分的**：FR-RULE-05「规则集预处理索引：域名精确 / 后缀索引、IP 前缀树、ASN 常数时间查询；大集合内存可控」。2026-10-10 的审计把它判成"做了"，阶段 1 补遗（M5a）里量大集合的内存时发现，集合里带 `no-resolve` 的 `IP-CIDR` / `IP-CIDR6` 行不进前缀树、留在逐条求值的行里，而常用的列表每条 IP 行都带 `no-resolve`（blackmatrix7 的 `ChinaMax.list`，2026-10-11 读它自己的说明：124,509 行里 `IP-CIDR` 8,244 条、`IP-CIDR6` 4,281 条），所以"IP 前缀树"只对一半的写法成立；接着对着手册的原句（`rules/ruleset.html`："IP-ASN entries are checked in constant time."）和两份常用的 ASN 列表（`missuo/ASN-China`、`VirgilClyne/GetSomeFries`，每条规则行都写成 `IP-ASN,<号码> // 说明`）又发现，集合里的 `IP-ASN` 行是逐条比较而不是常数时间，规则集文件只认整行注释，这两份列表的每一行都因号码读不出来被跳过、集合是空的，使用者只在加载时看到一条 `W0023`。三处都补上了：带 `no-resolve` 的 IP 行进前缀树（`e4f0991`；用例 `crates/rurge-rules/src/set.rs`：`no_resolve_ip_lines_go_into_the_index_not_the_linear_list`、`among_no_resolve_ip_lines_the_longest_prefix_is_reported`），规则集文件的行尾注释（`92aa656`；用例 `crates/rurge-rules/src/set_format.rs`：`a_rule_line_may_end_in_a_note_that_is_not_part_of_the_rule`、`a_file_of_the_usual_shape_yields_its_rule_lines_and_nothing_is_skipped`），集合里的 `IP-ASN` 行按号码查（`ee5b458`；用例 `crates/rurge-rules/src/set.rs`：`asn_lines_are_not_in_the_list_of_lines_walked_one_by_one`、`a_walked_line_that_matches_is_reported_ahead_of_an_asn_line_that_also_matches`）。这是审计自己的保留（判为"做了"的没有人复核，见「审计没有查的」）第一次被印证。
- **由此该做的**：阶段 1 称齐备之前，把需求再对一遍代码，每条"做了"指到一条会因它而红的用例，并对着手册的原句、拿一份真实的列表或配置读（阶段 1 / M5a 计划「延后事项」第 28 行；排在阶段 1 补遗（M5b）收尾，阶段 3 开始之前）。

### 阶段 1 补遗（M5b）之后（2026-10-13）

上面的数字、上一节与下面的表是各自那一天的，不改。2026-10-11 ～ 13 的阶段 1 补遗（M5b）（设计 `docs/superpowers/specs/2026-10-11-phase1-m5-addendum-design.md` 第 7 节，实施期的订正是它的第 14 节；计划 `docs/superpowers/plans/2026-10-11-phase1-m5b-profile-remote-sources-plan.md`，下称"阶段 1 / M5b 计划"）做了上一节留下的三行，也照上一节末尾说的，把阶段 1 的需求再对了一遍代码。本节引用的提交都在 `75e1668..50d77a0` 里（`75e1668` 是这些提交之前的 `main`；`2068892` 是整分支评审之前最后一个改动代码的提交，其后到 `a5dd842` 只改文档和一行代码注释；`9905e2f` 是评审之后的修正里最后一个改动代码的提交，`50d77a0` 是那之后唯一一个改动代码的提交（本节最后一条），`9905e2f` 之后别的提交只改文档）；"`a` ～ `b`"是含两端的一段。

- **托管配置的更新（FR-CFG-05）做了。** 托管配置下载到数据目录的缓存里（`<数据目录>/resources/<链接的 SHA-256>/`），`rurge run` 启动时等它下载（至多 60 秒），之后按 `interval` 更新（不足 60 秒按 60 秒，只往后抖）；每次加载在本地文件与下载到的副本之间选生效的文本（本地文件在上次更新之后被改过、又有内容时，生效到下一次成功的检查为止；只有一行 `#!MANAGED-CONFIG` 的本地文件只是个指针，从不优先）；下载到的内容先判定再采用，加载不了的与不是 Profile 的（一页登录页）不采用，缓存里上一份照旧生效；`strict` 的更新到期而最近一次尝试失败（或离线）时，引擎拒绝新会话、API 照常，更新成功后自动重载并恢复。提交 `aa7777b` ～ `3c8ad92`：资源管理器 `aa7777b` ～ `f7560de`，加载器 `a873ae1` ～ `47868c3`，暂停与生效文本 `6865761`、`1417f29`，`ProfileSources`、采用之前的判定与启动时的引导 `a9d0343` ～ `c6e1396`，可执行文件的启动等待、主循环与其余命令 `c1f6519` ～ `eebb8a2`，缓存条目的摘要 `7919ec3`、`3c8ad92`；夹在其间的 `b33e27e`、`4aed224`、`dd88af0` 是 URL 里用户名口令的 Basic 认证与依赖库的日志。用例：`crates/rurge-config/src/config.rs`：`a_pointer_that_has_not_been_downloaded_yet_is_a_profile_without_rules`、`a_copy_newer_than_the_local_file_is_in_effect`、`a_local_file_changed_after_the_last_update_is_in_effect`、`a_local_file_with_no_section_is_never_in_effect`、`the_newest_copy_on_the_way_need_not_be_the_last_one`，`crates/rurge-config/src/managed.rs`：`an_interval_below_sixty_seconds_is_sixty`；`crates/rurge-engine/tests/profile_sources.rs`：`a_source_is_not_asked_for_before_its_interval_is_over`、`the_first_download_of_a_managed_profile_is_a_change`、`a_strict_profile_that_is_due_and_failed_is_suspended`、`a_strict_profile_that_is_due_is_not_suspended_before_its_attempt_fails`、`a_strict_profile_that_is_due_and_updated_is_not_suspended`，`crates/rurge-engine/tests/profile_updates.rs`：`a_page_is_not_taken_as_the_managed_profile`；`crates/rurge-engine/tests/pipeline.rs`：`a_suspended_engine_refuses_a_connect_before_the_rules_and_lets_it_through_again`、`a_suspended_engine_refuses_a_plain_request_and_a_socks5_connect`、`a_suspension_leaves_the_sessions_that_are_open_alone`、`the_profile_text_is_the_downloaded_copy_while_that_is_in_effect`，`crates/rurge-engine/tests/udp.rs`：`a_suspended_engine_drops_the_datagrams_and_serves_the_flow_again_when_it_ends`；`crates/rurge/tests/cli.rs`（在 `run::remote_sources` 模块里）：`a_profile_that_is_only_a_pointer_runs_on_the_downloaded_copy`、`a_new_version_of_the_managed_profile_is_taken_without_being_asked`、`a_start_that_could_not_make_a_strict_update_refuses_sessions_from_the_first_one`、`a_refusal_begins_when_the_update_falls_due_and_fails_while_the_daemon_runs`、`a_refusal_ends_when_the_next_update_works`、`the_copy_takes_over_from_a_local_file_once_a_check_has_succeeded`。
- **远程 `#!include`（FR-CFG-04 的"远程 URL"，连同 FR-CFG-14 的下载与磁盘缓存）做了。** 目标是 URL 的 `#!include` 现在被下载、缓存在数据目录里、按间隔刷新，并在原位展开；下载到的文本不因为被 include 而多出权力（里面的 `external` 策略不建、本地路径不读，`W0033`）；缺它被 include 的那个节的内容（一页 HTML）不采用；新版本提到一个从没下载过的来源时，先等它试完第一次再一并采用，而不是先换上缺着一块的一代。`W0011` 的文本现在是 `remote include not read: no data directory`，只有不给加载器远程来源的库调用方才见得到，可执行文件的命令不再产生它。提交：资源管理器、加载器、判定与引导、可执行文件，都在上一条的范围里（加载器的远程 include 是 `a873ae1`、`ff2ada0`）。用例：`crates/rurge-config/src/text/include.rs`：`a_remote_include_is_expanded_in_place`、`a_remote_include_that_was_not_downloaded_says_so`、`a_remote_text_can_include_another_remote_text`、`a_remote_text_without_the_section_is_flagged`、`a_remote_text_cannot_include_a_local_file`、`a_cycle_of_remote_includes_is_an_error_that_names_no_link`、`the_interval_of_a_remote_include_is_at_least_sixty_seconds`；`crates/rurge-engine/tests/profile_updates.rs`：`a_page_is_not_taken_as_an_included_file`、`the_include_that_the_managed_profile_needs_waits_for_it`；`crates/rurge-net/src/resource/cache.rs`：`round_trips_data_and_meta`、`a_body_the_metadata_was_not_written_for_is_no_entry`；`crates/rurge/tests/cli.rs`（同上）：`a_start_downloads_the_remote_includes_the_cache_lacks`、`a_start_waits_for_an_include_that_is_slow_to_answer`、`offline_a_remote_include_the_cache_lacks_is_a_warning_and_nothing_is_waited_for`、`a_copy_that_is_not_what_was_downloaded_is_downloaded_again`。
- **外部资源的手动强制更新（FR-CFG-14）做了。** `GET /v1/external_resources` 列出 Profile 的来源与当前这一代的外部资源，`POST /v1/external_resources/update` 立刻更新一个键或全部（`all`）；形状与同手册的差异见 `docs/api/phase1.md` 的「外部资源」。`ResourceManager::force_update` 现在有非测试的调用者（换代时的补问 `crates/rurge-engine/src/reload.rs`、向其余来源问更新 `crates/rurge-engine/src/profile/verdict.rs`），端点走的是可等待的 `update_now`。提交 `bbc457e` ～ `253b7a6`。用例：`crates/rurge-api/tests/api.rs`：`external_resources_are_listed_one_item_for_each_kind`、`external_resources_are_updated_one_key_at_a_time`、`external_resources_are_all_updated_but_the_files`、`external_resources_are_not_downloaded_offline`、`external_resources_say_no_to_a_bad_body_and_to_a_key_that_names_nothing`、`external_resources_of_the_profile_come_first_and_the_managed_one_is_flagged`；`crates/rurge-engine/tests/external_resources.rs`：`an_update_of_a_key_says_what_the_attempt_came_to`、`an_update_of_all_asks_every_remote_resource_of_both_holders`、`a_key_that_is_not_done_by_the_limit_is_timed_out_and_the_others_keep_their_answers`、`a_copy_checked_after_the_next_generation_was_built_is_asked_for_again`、`a_request_that_ends_under_another_generation_has_that_one_ask_too`；`crates/rurge-net/src/resource/mod.rs`：`update_now_does_not_wait_for_a_backoff_or_a_timer`；`crates/rurge/tests/cli.rs`（同上）：`an_update_asked_for_through_the_api_is_taken_and_reloaded`、`the_way_out_of_a_refusal_is_an_update_asked_for_through_the_api`。
- **用例通过的意思。** 引用的用例都存在；它们在一台 Windows 11 x86-64 的开发机上通过——`50d77a0` 上的那一次全量运行：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 都干净，`cargo test --workspace --no-fail-fast` 74 个结果行，3383 个通过，0 个失败，6 个忽略（三个吞吐基准与三个内存实测）——，通过的意思也只是这个：一台机器的一次运行，不是 CI（CI 没有绿过）；需要参照服务端二进制的互操作用例在这台机器上立刻返回（那些服务端本机一个也没有），算在通过的里面。macOS、Linux 上的分支本机没有编译过、运行过，第一次 CI 运行时核对；只在 Unix 上编译的用例也是（`crates/rurge/tests/cli.rs` 的 `run_shuts_down_gracefully_on_sigint` 与 `sighup_reloads_the_profile`，整个 `tests/interop/tests/sshd.rs`；清单在 `docs/acceptance/phase1-acceptance.md` 的「靠第一次 CI 运行的」一节），阶段 1 补遗（M5b）自己加过的一条这样的用例随托管配置副本的本地 include 一起去掉了。对真实服务商的托管配置、真实服务器上的远程 `#!include` 与真实的 `strict` 没有用例，只有手工清单里的三行（`docs/acceptance/phase1-manual.md`「与平台无关」一节的第 1 ～ 3 条），一行也没有做过。
- **接着读完阶段 1 的需求，查出的。** 阶段 1 的需求——需求文档第 4 节里阶段栏含 1 的 49 行，共 416 句——随后逐句对着代码读过，结果在 `docs/acceptance/phase1-acceptance.md`：一个读代码和用例断言的助手逐句读、补用例、补小缺口，每条结论另由一位评审者抽查；判为"做了"的行指到一条断言了这一句的用例，这不等于别处没有缺口，有几行在自己的格子里写着一处限制。结果有三个数，要一起看，缺一不可：416 句里 **3 句**登记为没有做完，都在优先级 P0 的需求行上（手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`；规则集文件行上的 `extended-matching`；"大集合内存可控"没有判据）；**37 句**是需求行的阶段栏写 1、而兼容性清单或某份设计把它们登记在后面的阶段的句子（优先级 P0 的需求行上 32 句，P1 的 5 句），其中 34 句今天不按需求的字面做、等阶段 3 ～ 6，另外 3 句登记在阶段 2 而今天有代码；**14 句**有意不同而已登记。改需求文档的阶段栏，还是改清单与设计里的登记，由项目所有者定，需求文档没有改。阶段 2：功能做出来了，验收没有完成（本文其余各节）。这个结果没有覆盖的：阶段 1 的手工验收清单（没有一行做过）、CI（没有绿过：所有用例只在一台 Windows 11 x86-64 的开发机上跑过）和阶段 2 的验收。也不要由它推出：真的系统解析库在真实网络上对 `.local` 名字做什么，在任何平台上都没有观察过。
- **整分支的评审之后又改的（2026-10-13）。** 合并之前的整分支评审只读代码与文档、什么也没运行，查出五条要处理的，都在合并之前处理了（阶段 1 / M5b 计划「执行期修正记录」第 118 ～ 124 行）。行为变了的：托管配置的副本不再 include 任何本地文件（`e8bfacd`；此前它可以用朴素的相对路径 include，而读进来的代理能被副本经它自己的 DNS 与中继用上），用例 `crates/rurge-config/src/text/include.rs`：`a_copy_includes_no_local_file`，`crates/rurge-config/src/config.rs`：`a_copy_takes_in_nothing_of_the_directory_of_the_local_file`、`the_copy_of_a_managed_profile_names_no_local_file`，`crates/rurge-engine/tests/profile_updates.rs`：`the_copy_of_a_managed_profile_builds_nothing_from_a_local_file`；通配命名节下，下载到的、写下来连一个节都没有的正文不采用（`f859495`；自己只有通配节的下载文本不算，它等它 include 的文件——第一个提交把它也拒了，`9905e2f` 订正），用例 `crates/rurge-config/src/text/include.rs`：`a_downloaded_text_without_any_section_is_flagged_under_a_wildcard`、`a_local_file_without_any_section_is_not_flagged_under_a_wildcard`、`a_downloaded_text_of_wildcard_sections_alone_is_not_flagged`，`crates/rurge-engine/tests/profile_updates.rs`：`a_page_is_not_taken_as_a_file_included_under_a_wildcard_section`、`a_text_of_wildcard_sections_alone_waits_for_the_file_it_includes`；重载失败时也去下载它点名而从没下载过的来源（`09fa962`），用例 `crates/rurge-engine/tests/profile_updates.rs`：`the_managed_profile_a_failed_load_names_is_downloaded_and_reported`、`the_include_a_failed_load_names_is_downloaded_and_reported`、`a_source_followed_for_a_failed_load_suspends_nothing`、`following_the_same_sources_twice_registers_them_once`、`the_next_generation_decides_whether_a_followed_source_stays`、`following_a_source_leaves_the_generation_in_use_as_it_was`、`only_a_source_without_a_copy_is_followed`、`an_offline_process_follows_nothing`，`crates/rurge/tests/cli.rs`：`a_link_rotated_while_the_daemon_runs_is_downloaded_and_taken`。只动测试的一处：资源管理器的测试建在拒绝一切名字的解析器上（`4671e58`）。这些用例同样只在那一台开发机上跑过；评审读出的这几条在修正之前没有哪条用例走到，修正之后的行为由上面的用例钉住，别的没有。修正之后把这一轮又读了一遍（只读，什么也没运行；阶段 1 / M5b 计划「执行期修正记录」第 125 行）：没有读出新的要紧的缺口；读出的几处小的里，行为上的一处在合并之前改了——会话因 `strict` 被拒时，`GET /v1/external_resources` 此前把列表里每一项 `profile` 都标成 `suspended`，一次失败的重载之后临时跟上的那一份托管配置也在内；现在只有 `strict` 所问的那一份带它与被拒的原因（`50d77a0`），用例 `crates/rurge-engine/tests/external_resources.rs`：`a_managed_profile_that_is_only_followed_is_not_flagged`、`nothing_is_flagged_where_no_managed_profile_says_strict`。同一个提交补了一条此前没有钉住的：文件不再点名的临时来源由下一次判定放掉，`crates/rurge-engine/tests/profile_updates.rs`：`a_followed_source_the_file_no_longer_names_is_let_go_of`。
- **仍没有的。** FR-CFG-05 的"记录 Logbook"属阶段 6（更新与拒绝的记录现在是日志）；外部资源没有 CLI 子命令，`rurge status` 不显示暂停（都属阶段 6）；阶段 1 / M5b 计划「延后事项」第 10、11 行。

### 手册基线重对之后（2026-10-14）

写明日期的段落与表是各自那一天的，不改；只有几处不带日期的手工清单项数改成了现在的数。2026-10-14 的手册基线重对（报告 `docs/reports/2026-10-14-manual-baseline-recheck.md`，设计 `docs/superpowers/specs/2026-10-14-manual-baseline-recheck-design.md`，计划 `docs/superpowers/plans/2026-10-14-manual-baseline-recheck-plan.md`）把那天取的整本手册逐句对着兼容性清单读了一遍，也改了几处阶段 1、2 已经声称支持的条目上的代码。本节引用的提交都在 `30b082a..a261122` 里（`30b082a` 是这些提交之前的 `main`；`c4b992d` 与 `6f33016` 只改兼容性清单，其余改代码、用例或文档）；"`a` ～ `b`"是含两端的一段。

- **基线前移。** 项目对照的手册从此是 Surge Mac 6.10.0 / iOS 5.23.0（2026-10-13 发布）的手册，2026-10-14 取的文本，87 页；需求文档的参考基线一行、兼容性清单的表头、`CLAUDE.md` 与两份 README 同改。官方频道里 2026-10-13 之后的测试版还有几件手册没有写到的（自适应的 `block-quic`、`GET /v1/policies?include_external=true`、跨多个包的 QUIC 主机名识别、QUIC v2 与 draft-29），下一次重对从那里开始。
- **阶段 1 的三个数变了。** 2026-10-13 写的是 3 句没有做完、37 句的阶段栏与登记不一致、14 句有意不同；现在是 **2 句没有做完、37 句的阶段栏与登记不一致、10 句有意不同**（`docs/acceptance/phase1-acceptance.md` 的 `## 数字`：补上了 14、没有做完 2、有意不同 10，合计仍是 416）。动的是五句：FR-RULE-01 手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN` 那一句，原来记"没有做完"，`9b0bcfe` 补上了——匹配数据库已载入而查不到的地址，库没有载入时不匹配；FR-RULE-08 的 DoH、DoT、DNS 三个连接标签与 FR-DNS-04 的 `PROTOCOL` 规则匹配那四句，原来记"有意不同"，`42c2294` 补上了——跟随出站模式时 DNS 会话的协议标签按 URL 的方案定，不再按端口猜。FR-RULE-08 的 DoH3、DoQ 两句的结论没有变，仍在"阶段栏与登记不一致"的 37 句里，说标签的话随之改了。
- **其余的改动。** 十一处里其余几处（`436ee74`、`b809298`、`9a01a32`、`24e1841`、`31c4245`、`05d2e1c`、`bd6f61d`、`a0dba4b`、`9027daf`，和钉住现有行为的用例 `5c5fe22`、`745beda`）属阶段 1、2 的条目，用例名在兼容性清单对应的行里，本节不重复。兼容性清单改了 123 处，现在 565 行。
- **用例通过的意思。** `a261122` 上的那一次全量运行：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 都干净，`cargo test --workspace --no-fail-fast` 74 个结果行，3417 个通过，0 个失败，6 个忽略；其后只有文档变过。通过的意思也只是这个：一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI（CI 没有绿过）；新的用例只对着回环上的假服务端与 MaxMind 的测试库，没有一条对真实的服务端、真实的库或真实的抓包跑过。
- **这一轮没有覆盖的。** Surge 的实际行为（一处也没有观察过，清单里"与 Surge 一致"说的是与手册的文字一致）；被读成"一致"的 2,439 条（没有人逐条复核）；代码（第二遍只核对了 47 处，阶段 1、2 其余各行比的是清单的文字与手册的文字，不是代码与手册）；阶段 3 及以后的行上手册多说的细节；取文本时丢掉的图、链接与表格的行列；手册之外的知识库、博客与社区文档（发行说明只读了最近的一页）；两件取自第三方的事实（旧 `custom` 行里字段的位置，`GET /v1/profiles/current` 的三个键）。CI 与手工验收照旧：CI 没有绿过，两份手工清单（阶段 2 的 22 节 187 项、阶段 1 的 46 行）一行也没有做过；这一轮各加了回环上验不了的项（阶段 2：`wireguard` 的 `tos` 在线上的 DSCP 位、一行真实的旧 `custom` 策略；阶段 1：真实的 GeoLite2 库之下的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`）。

### 阶段 1 里只解析或没有的条款

前五列是 2026-10-10 审计当时的样子，不改；最后一列是 2026-10-11 阶段 1 补遗（M5a）之后的结果。

| FR（优先级） | 需求文档的原话 | 代码现在做什么 | 用户今天看到什么 | 登记在 | 阶段 1 补遗（M5a）之后 |
| ------------ | -------------- | -------------- | ---------------- | ------ | ---------------------- |
| FR-CFG-05（P0） | 「托管配置：`#!MANAGED-CONFIG <URL> interval= strict=`；缓存、按间隔更新、`strict` 到期未更新成功时拒绝启动新会话但保留 API 可用、更新后自动重载并记录 Logbook」 | 只解析：`crates/rurge-config/src/managed.rs`：`parse_managed` 把指令读成 `ManagedConfig { url, interval, strict }`；`Config.managed` 只有 `Config::summary()`（`crates/rurge-config/src/config.rs`）读它，`rurge-net`、`rurge-engine`、`rurge-api` 与 bin 里没有任何地方碰它 | URL 从不被抓取，没有缓存，`interval` 与 `strict` 不起作用，不会因更新而重载（"记录 Logbook"属阶段 6） | 第 100 行；兼容性清单开头一句与 1.3 节 | 没有做：属阶段 1 补遗（M5b），代码里还没有它的任何部分 2026-10-13 的更新（阶段 1 补遗（M5b））：做了。托管配置下载到数据目录的缓存里，启动时等它下载，按 `interval` 更新，`strict` 到期未更新成功时引擎拒绝新会话（API 照常），更新成功后自动重载并恢复；提交 `aa7777b` ～ `3c8ad92`，用例见上面「阶段 1 补遗（M5b）之后（2026-10-13）」一节。需求原话里的"记录 Logbook"仍属阶段 6，更新与拒绝的记录现在是日志 |
| FR-CFG-04（P0）、FR-CFG-14（P0） | 「`#!include`：单文件、多文件、与普通内容混合（按位置展开）、通配命名节、远程 URL」；「…远程 include 的下载、磁盘缓存…」 | 没有：目标是 URL 的 `#!include` 被 `crates/rurge-config/src/text/include.rs` 跳过，报 `W0011`（`remote include not supported yet`），一条用例钉着这句文本；`rurge-config` 不依赖 `rurge-net`，也没有别处预先取回 | `#!include https://…` 什么也不加，只有一条告警 | 第 100 行；兼容性清单 1.3 节 | 没有做：同上；`W0011` 的文本与钉着它的用例照旧 2026-10-13 的更新：做了。目标是 URL 的 `#!include` 现在被下载、缓存在数据目录里、按间隔刷新并在原位展开；`W0011` 的文本现在是 `remote include not read: no data directory`，只有不给加载器远程来源的库调用方才见得到；提交与用例同上一行 |
| FR-CFG-14（P0） | 「…本地文件监视自动重载、手动强制更新」 | 只在库里：`crates/rurge-net/src/resource/mod.rs`：`force_update` 除用例外没有调用者，没有 API 路由，没有 CLI 命令（M2 设计写"M4 的 API 调用它"） | 要立刻重新抓一个规则集或 GeoIP 库，只能等间隔到期或重启 | 第 101 行 | 没有做：属阶段 1 补遗（M5b）（设计第 7.6 节：`GET /v1/external_resources` 与 `POST /v1/external_resources/update`）；`ResourceManager::force_update` 仍只有用例调用 2026-10-13 的更新：做了。`GET /v1/external_resources` 与 `POST /v1/external_resources/update`（形状见 `docs/api/phase1.md` 的「外部资源」）；`ResourceManager::force_update` 现在有非测试的调用者（换代时的补问、向其余来源问更新），端点走可等待的 `update_now`；提交 `bbc457e` ～ `253b7a6`，用例见上面「阶段 1 补遗（M5b）之后（2026-10-13）」一节。没有对应的 CLI 子命令（阶段 6） |
| FR-RULE-03（P0）、FR-RULE-02（P0） | 「评估语义：顺序与首个命中；`pre-matching` 规则优先；…」 | 只在库里：规则引擎按配置里的先后逐条走；`crates/rurge-rules/src/engine.rs`：`pre_match_domain`、`pre_match_ip` 与 `PreMatchingSet`（`crates/rurge-rules/src/pre_matching.rs`）除 `rurge-rules` 自己的用例外没有调用者（M2 设计写"M3 在 DNS 应答与 SYN 阶段调用"） | 排在另一条后面的 `…,REJECT,pre-matching` 输给先命中的那一条 | 第 101 行；兼容性清单 3.2、3.3 节 | 补上了。`23b5c75`：规则模式下 `RuleEngine::run` 先把带 `pre-matching` 的规则不解析地评估一遍，命中者的 `reason` 是 `pre-matching`；用例 `crates/rurge-rules/src/engine.rs`：`a_pre_matching_rule_decides_before_the_written_order`、`the_trace_has_one_step_for_a_pre_match_and_none_for_a_miss`、`a_pre_matching_ip_rule_only_resolves_at_its_own_place`、`direct_and_global_modes_do_not_look_at_pre_matching`，`crates/rurge-engine/tests/pipeline.rs`：`a_pre_matching_reject_written_last_still_rejects_first`，`crates/rurge/tests/cli.rs`：`rule_match::a_pre_matching_rule_decides_before_the_written_order`。DNS 查询与 TCP 握手两个阶段的预匹配仍在阶段 3：`pre_match_domain` / `pre_match_ip` 还是只有 `rurge-rules` 自己的用例调用 |
| FR-RULE-15（P0） | 「子规则命中日志 `Sub-rule matched: <rule> (in <set>)`」 | 只在库里：`Decision.sub_rule` 算出来了，只有离线的 `rurge rule match` 打印它（`crates/rurge/src/cli/rule.rs`，写成 `sub-rule: <entry> (in <set>)`）；源码里没有 `Sub-rule matched` 这个字符串 | 运行中的会话日志与请求记录只写 `RULE-SET` 那一行，不写集合里命中的那一行 | 第 101 行；兼容性清单 3.3 节 | 补上了。`0b07366`：集合里命中某一行时，引擎记一条 DEBUG 日志 `Sub-rule matched: <行> (in <集合>)`，并把同一句放进请求记录的 `notes`；用例 `crates/rurge-engine/tests/pipeline.rs`：`a_rule_set_hit_is_logged_and_noted_on_the_request`，`crates/rurge-api/tests/api.rs`：`requests_carry_the_notes_of_their_sessions`。命中的子规则只属于命中的那条规则（原来未命中的逻辑规则会把它留给后面命中的规则）：`crates/rurge-rules/src/engine.rs`：`a_rule_that_did_not_match_leaves_no_sub_rule_for_the_next`、`a_branch_that_did_not_decide_the_rule_leaves_no_sub_rule`、`a_not_carries_no_sub_rule` |
| FR-RULE-04（P0） | 「…同一资源不可同时作两种集…」 | 没有：集合注册表按（资源，种类）记条目（`crates/rurge-rules/src/registry.rs`），同一个 URL 或文件既当 `RULE-SET` 又当 `DOMAIN-SET` 时载入两次 | 没有错误，也没有告警 | 第 101 行；兼容性清单 3.5 节 | 补上了。`08b7246`（加载期 `E0024`，运行期 `W0032`），修正 `e7f870e`、`c2c9d88`、`519cbd4`（配置自己写出的身份总是压过集合文件里嵌套的引用）；用例 `crates/rurge-config/src/config.rs`：`one_resource_as_a_rule_set_and_a_domain_set_is_an_error_at_the_second_use`、`the_report_names_the_other_use_wherever_it_stands_in_the_file`，`crates/rurge-rules/src/registry.rs`：`a_domain_set_nested_in_a_file_cannot_reuse_a_rule_set`、`the_role_the_profile_gives_a_file_wins_over_one_nested_above_it`、`the_role_a_logical_rule_gives_a_file_wins_too`，`crates/rurge/tests/cli.rs`：`check_refuses_one_resource_as_two_kinds_of_set`。Surge 自己怎样处理没有证据（阶段 1 / M5a 计划「延后事项」第 9 行） |
| FR-RULE-06（P0） | 「…数据库日期可查询」 | 只在库里：`GeoDbInfo`（`crates/rurge-rules/src/geoip.rs`）里两个库的更新日期只被启动时的等待（`crates/rurge-engine/src/stack.rs`）和用例读 | 没有 API 路由，没有 CLI 命令显示数据库的日期 | 第 101 行 | 补上了。`82659e3`，修正 `e964445`：`GET /v1/geoip?ip=<地址>`（`GeoDb::describe`，`Engine::geoip_view`）；用例 `crates/rurge-rules/src/geoip.rs`：`describe_gives_what_the_loaded_databases_know`、`describe_does_not_warn_of_a_missing_database`，`crates/rurge-api/tests/api.rs`：`geoip_says_what_the_databases_in_use_know`、`geoip_without_databases_has_nothing_to_say`、`geoip_wants_an_address_and_the_key`。两个日期是库自己带的构建时间，手册没有说是构建时间还是下载时间；没有 CLI 命令（阶段 6） |
| FR-CFG-09（P0） | 「…`SYSTEM_VERSION` `DEVICE_MODEL` `LANGUAGE` `DEVICE_NAME` 取自操作系统」 | `crates/rurge/src/cli/mod.rs`：`environment` 里 `SYSTEM_VERSION` 是字面的 `unknown`，`DEVICE_MODEL` 是 CPU 架构（`std::env::consts::ARCH`），`LANGUAGE` 取环境变量 `LANG`（Windows 上没有，永远是 `en-US`），`DEVICE_NAME` 取 `COMPUTERNAME` / `HOSTNAME`（都没有时是 `rurge`） | 对这些变量写的 Requirement 表达式就按这些值比较；`LANGUAGE`、`DEVICE_NAME` 没有用例 | 第 102 行；兼容性清单 1.4 节 | 补上了。`0d3889f`，修正 `118a553`、`49bf061`：`rurge_platform::sysinfo` 读操作系统，`cli::environment_from` 把读不到的退回原来的取值；用例 `crates/rurge-platform/src/sysinfo.rs`：`a_locale_name_stands_for_one_language_or_for_none`、`a_text_that_begins_like_an_unfilled_template_is_no_model`（另有各平台的纯函数与对假来源的读取），`crates/rurge/src/cli/mod.rs`：`a_system_that_says_nothing_leaves_what_the_variables_always_gave`，`crates/rurge/tests/cli.rs`：`check_reads_the_system_version_from_the_operating_system`、`check_does_not_give_this_systems_version_to_another_platform`。Windows 的注册表读取在本机运行过；macOS 与 Linux 的读取只对着假的命令、文件与环境变量测过，没有在这两个平台上编译、运行过，第一次 CI 运行时核对（手工清单各有一行；阶段 1 / M5a 计划「延后事项」第 2、13 行）。下面"做了而没找到用例"里的 `LANGUAGE`、`DEVICE_NAME` 因此也有了用例 |
| FR-CFG-17（P0） | 「rurge 专有运行时选项（数据目录、TUN 设备名、日志文件、监听覆盖等）只通过命令行参数与环境变量提供」 | 没有监听覆盖：阶段 1 设计列在 `rurge run` 上的 `--config-dir`、`--listen-http ADDR`、`--listen-socks5 ADDR` 一个都没有，环境变量也没有；数据目录和日志文件有各自的选项 | 监听地址只能改配置文件 | 第 102 行 | 补上了。`25d0a25`，修正 `c9c327c`、`44787e1`：`rurge run --listen-http` / `--listen-socks5`（环境变量 `RURGE_LISTEN_HTTP` / `RURGE_LISTEN_SOCKS5`）整项替换配置里同一种的监听；用例 `crates/rurge-config/src/general.rs`：`listen_values_from_outside_a_profile_are_read_as_a_profile_reads_them`，`crates/rurge/src/cli/run.rs`：`an_http_override_replaces_every_http_listener_and_leaves_socks5_alone`，`crates/rurge/tests/cli.rs`：`listen_http_replaces_the_profiles_http_listeners_for_good`、`a_listen_value_that_cannot_be_used_ends_the_start_without_quoting_it`、`a_listen_override_on_the_port_of_an_external_policy_ends_the_start`。需求原话里的 `--config-dir` 不在内（多 Profile 管理，阶段 6）；`rurge service install` 不把这两个选项带进安装的服务（阶段 1 / M5a 计划「延后事项」第 11 行） |
| FR-CFG-03（P0） | 「`rurge check` 输出文件、行号、原因与建议」 | 几乎没有建议：`crates/rurge-config/src/config.rs` 里 `E0010`（"add `FINAL,DIRECT` as the last rule"）是唯一设了建议的诊断，别的都不带 | `rurge check` 的输出里只有这一条诊断有建议 | 第 102 行 | 补上了。`a299190`，修正 `7fc57b0`、`b88f7d8`、`66f2513`、`29de23a`：`codes::hint`，59 个诊断码里 33 个有缺省建议、`E0014` 的四处各自带、25 个不设；用例 `crates/rurge-config/src/diagnostic.rs`：`every_code_is_decided_once`，`crates/rurge-config/src/rule.rs`：`each_place_a_rule_part_is_not_allowed_says_what_to_do`，`crates/rurge/tests/cli.rs`：`check_says_what_to_do_about_a_problem`、`a_profile_that_does_not_load_is_printed_with_its_hint`。"每条建议对发出这个码的每一处都成立"是读发出点得出的，没有用例能证明（阶段 1 / M5a 计划「执行期修正记录」第 24 ～ 33 行） |
| FR-HTTP-10（P0；"1 基础 / 4 完整"） | 「…监听端口、协议、命中规则、策略与链、远端地址、状态、上下行流量、各阶段耗时…」 | `crates/rurge-engine/src/observe.rs`：`RequestRecord` 没有远端（上游）地址；只记监听器的种类，不记监听端口；耗时只有 `connect_ms / first_byte_ms / elapsed_ms`，没有 DNS 阶段。"基础"是否包含它们，需求文档没有写；阶段 1 设计 8.4 列了 `remote_addr / timings { dns, … }` | `GET /v1/requests/recent` 里看不到远端地址、监听端口和 DNS 耗时 | 第 102 行 | 补上了基础部分。`630fcd9`，修正 `b0d27c8`、`8106adf`、`e964445`、`4f208d1`：请求记录多 `listenPort`、`remoteAddr`、`dnsMs`（经 `rurge_net::trail::Trail` 从连接器与解析器带回），没有值时 JSON 里是 `null`；用例 `crates/rurge-engine/tests/pipeline.rs`：`a_direct_session_records_its_listener_and_the_address_it_connected_to`、`a_name_pinned_in_host_counts_as_a_lookup`、`sessions_dialling_together_record_their_own_address_and_lookup_time`，`crates/rurge-engine/src/observe.rs`：`the_record_of_a_udp_session_has_no_remote_address`，`crates/rurge-api/tests/api.rs`：`requests_carry_the_listen_port_the_remote_address_and_the_lookup_time`。`remoteAddr` 只在会话自己开了 TCP 连接时有，复用已有连接或隧道的会话与所有 UDP 会话是 `null`（登记的限制，完整补全随阶段 4；阶段 1 / M5a 计划「延后事项」第 3、20 ～ 22 行） |
| FR-CFG-15（P1） | 「多 Profile 管理：配置目录、当前 Profile 记录、列出 / 切换 / 校验 / 查看当前内容（可脱敏）」 | 当前 Profile 记录只在类型里：`crates/rurge-engine/src/state.rs` 的 `State.current_profile` 声明了、初始化成 `None`，没有任何地方写它或读它；配置目录、列出与切换登记在阶段 6 | `state.json` 里没有当前配置的记录 | 第 102 行 | 补上了阶段 1 的部分。`f0fc354`：`rurge run` 启动完成时把 `-c` 的绝对路径写进 `State.current_profile`，同一个提交加了用例 `crates/rurge/tests/cli.rs`：`a_running_daemon_records_its_profile_as_the_current_one`、`another_profile_becomes_the_recorded_one`、`a_start_that_fails_does_not_record_its_profile`、`a_start_with_the_recorded_profile_leaves_state_json_as_it_is`；`44787e1` 加了用例 `a_start_that_cannot_write_state_json_still_runs_and_serves`（`state.json` 写不了时，日志里有 `cannot write state.json` 一句，启动照常，API 照常服务）。读它的是阶段 6 的多 Profile 管理，现在没有端点显示它 |
| FR-CFG-10（P2） | 「可选扩展简写 `#!WINDOWS-ONLY` `#!LINUX-ONLY`，Surge 会将其视为普通注释」 | 没有：`crates/rurge-config/src/requirement.rs` 的 `SIMPLIFIED` 只有三个 Apple 标签，这样的行按未知指令丢掉并报 `W0014` | 在 Windows 上 `#!WINDOWS-ONLY` 不起作用 | 第 102 行 | 补上了。`76fc3dc`：`SIMPLIFIED` 多 `WINDOWS-ONLY`、`LINUX-ONLY`，`is_requirement_prefix` 与脱敏共用同一份清单；用例 `crates/rurge-config/src/requirement.rs`：`windows_only_and_linux_only_are_simplified_requirements`、`each_tag_holds_a_line_to_its_own_system_only`，`crates/rurge-config/src/config.rs`：`a_system_tag_gates_a_line_as_loaded`，`crates/rurge-config/src/redact.rs`：`the_windows_and_linux_tags_are_requirement_directives_too`。Surge 把行首写法的整行当注释，只推荐行首写法（兼容性清单 1.4 节） |

### 阶段 2 里只解析的条款

没有"没有"的条款。五句只解析：四句在加载时由 `W0029` 登记，一句没有登记。

| FR（优先级） | 需求文档的原话 | 代码现在做什么 | 用户今天看到什么 | 登记在 |
| ------------ | -------------- | -------------- | ---------------- | ------ |
| FR-OUT-03（P0） | 「14 个通用策略参数（… `tfo` …）及其平台差异」；平台差异表里 TCP Fast Open 写 Linux、macOS "支持" | `tfo` 解析并校验，`W0029`；没有一条 TCP 连接用过 Fast Open（`socket2` 没有它的封装，工作区禁 `unsafe`）；QUIC、WireGuard 与 `external` 上写它是 `W0028` | `tfo=true` 加载时报 `W0029`，连接照常建立，不用 Fast Open | 兼容性清单 4.3 的 `tfo` 行；遗留事项总表 M1a #1（移交：依赖升级时） |
| FR-OUT-03（P0） | 「…`ecn`…」，`wireguard` 一侧 | `wireguard` 策略上的 `ecn=on/off` 解析并校验，`W0029`；WireGuard 的数据报从不打 ECN 标记（QUIC 类策略上 `ecn` 生效，`on` 当作 `auto`） | 同上 | 兼容性清单 4.3 的 `ecn` 行、4.6 的 `wireguard` 行；遗留事项总表 M4b #2、M5c #5 |
| FR-OUT-07（P0） | 「UDP 中继：按协议支持矩阵；`udp-relay` `udp-port`；…」，`snell` 一侧 | `snell` 上的 `udp-port` 解析，`W0029`；Snell 的 UDP（v3 起）一律走主 TCP 端口（`ss` 的 `udp-port` 生效） | 期待 UDP 另有端口的服务端连不上 | 兼容性清单 4.5；遗留事项总表 M6b #3（移交：有用户报告再说） |
| FR-OUT-13（P1） | 「外部代理程序：…`addresses` 从 TUN 路由排除；…」 | `external` 上的 `addresses` 解析，`W0029`；没有 TUN，没有东西可排除 | 什么也不排除 | 兼容性清单 4.6 的 `external` 行；遗留事项总表 M4c #5（移交：阶段 3） |
| FR-GRP-03（P0） | 「全部组参数：…`hidden`…」 | 没有登记：`hidden` 被读（`crates/rurge-config/src/spec/group.rs`）并带到 `GroupInfo.hidden`（`crates/rurge-policy/src/registry.rs`）与 `GroupView.hidden`（`crates/rurge-engine/src/views.rs`），在那里只有用例读它；`GET /v1/policy_groups` 不带这个字段。Surge 里这个标志是在它自己的界面上隐藏一个组，rurge 在阶段 6 的 Dashboard 之前没有这样的界面 | 客户端看到的隐藏组同别的组没有区别 | 第 104 行；兼容性清单 5.2 的 `hidden` 行（M8c 订正） |

### 做了而没找到用例

阶段 1 的 11 句：FR-CFG-02「rurge 不改写用户配置文件」没有用例断言读完之后文件一字节未变；FR-CFG-09 的 `LANGUAGE`、`DEVICE_NAME`（取自环境变量）；FR-CFG-16 的 SIGHUP 重载（只在 Unix，靠读代码核对）；FR-CFG-17「只通过命令行参数与环境变量提供」没有用例断言；FR-RULE-05 一个 1,000,000 条的集合占多少内存从没量过；FR-OBS-01 与 FR-OBS-10 没有用例抓会话日志那一行，也没有用例拿日志里的连接 id 同请求记录里的对照；第 7 节三行自己的证据——"三平台 CI"这一项交付物只有工作流，"三平台 CI 绿"没有绿过，"系统代理开关后浏览器流量经 rurge"只有假后端的用例与没有做的手工清单。审计判为"做了"、但也缺一块用例的还有 FR-IN-03 的 `proxy-restricted-to-lan`：判断来源的谓词有单测，监听器一级的拒绝没有端到端用例（回环造不出非本子网的来源）。

阶段 1 补遗（M5a）之后：

- FR-CFG-02：`crates/rurge/tests/cli.rs`：`run_leaves_the_profile_and_the_files_it_includes_as_they_are`——带一个本地 `#!include` 与一个 `select` 组的 Profile，启动、经 API 重载、切一次组、停止之后，两个文件的字节与修改时间都和启动前一样（`518085b`、`72b0668`）。
- FR-CFG-16：`crates/rurge/tests/cli.rs`：`sighup_reloads_the_profile`（`#[cfg(unix)]`，`518085b`）。写好了，没有编译过、没有运行过——本机是 Windows，它用到的 `nix` 0.31.3 的每个签名对着锁定版本的源码核过——，第一次 CI 运行时核对（阶段 1 / M5a 计划「延后事项」第 1、23 行）。
- FR-OBS-01 / FR-OBS-10：`crates/rurge-engine/tests/pipeline.rs`：`a_rule_set_hit_is_logged_and_noted_on_the_request`、`the_log_line_of_a_completed_session_carries_what_its_record_does`、`the_log_line_of_a_rejected_session_carries_what_its_record_does`、`the_log_line_of_a_failed_session_carries_what_its_record_does`——会话日志那一行的 `session=` 等于请求记录的 `id`，并带 `rule=`、`policy=`（`742e8dc`、`875df43`；日志用 `crates/rurge-engine/tests/common/log_capture.rs` 的 `LogCapture` 抓）。注意日志行与记录不总是同一句：日志行的 `error=` 是失败的原因本身，请求记录的 `error` 在会话带备注时是 `<备注>; <原因>`；记录的 `rule` 是 `null` 时，日志行的 `rule=` 为空。
- FR-CFG-09 的 `LANGUAGE`、`DEVICE_NAME`：随上表那一行有了用例。
- FR-RULE-05：量了，不是断言。`crates/rurge-rules/tests/memory.rs` 的三条 `#[ignore]` 用例（`a_domain_set_of_a_million_entries`、`a_rule_set_of_a_million_entries`、`nothing_is_built_is_the_baseline`，`50b1f46`），普通的 `cargo test --workspace` 只编译不运行（所以全量运行的忽略数从 3 变成 6）。在这台开发机（Windows 11，x86-64）上，发布构建，每个用例单独一个进程，从进程外轮询读它的峰值工作集，四轮：什么也不建 14.2 ～ 15.6 MB；一个 100 万条的 `DOMAIN-SET` 峰值 416.7 ～ 416.9 MB（比基线多约 403 MB），建好之后约 137.5 MiB；一个 100 万条的 `RULE-SET`（五分之三 `DOMAIN-SUFFIX`、五分之一 `DOMAIN`、五分之一不带 `no-resolve` 的 `IP-CIDR`）峰值 470.1 ～ 470.4 MB（多约 456 MB），建好之后约 156.4 MiB。相减得编译后的集合本身约 101 / 111 MiB（每条约 106 / 116 字节），是 M2 设计估的"约 60 MB"（那只说排序数组）的 1.8 / 1.9 倍，两者在这次测量里分不开。峰值是载入那一刻的瞬时量（文本、解析出的行、条目与索引构建器同时在），约为建好之后的三倍；刷新时旧的集合要到新的编译完才放手，所以刷新的瞬时量比第一次载入再高约一个稳定水位——这一句是读代码的推断，没有量。只量了 Windows，工作集不等于分配量；测量的方法写在 `memory.rs` 的开头（Linux 上读 `VmHWM`）。FR-RULE-05 没有数字（"大集合内存可控"），NFR-01 写的是"1 万并发连接常驻内存 < 300 MB"：一个满额的集合建好之后比基线多 130 ～ 150 MB，两个约 280 MB，载入一个时的瞬时量单独就超过 300 MB。"可控"算不算、NFR-01 要不要在载了满额集合的前提下成立，是项目所有者的问题（阶段 1 / M5a 计划「延后事项」第 24 行）。
- 仍没有用例的：FR-CFG-17「只通过命令行参数与环境变量提供」（新加的两个选项有用例，没有用例断言 Profile 格式里没有这样的键）；FR-IN-03 的 `proxy-restricted-to-lan` 在监听器一级的拒绝；第 7 节三行自己的证据；阶段 2 的 WireGuard 计时器——都和上面一样。

阶段 2 的 3 句：FR-OUT-12 的 WireGuard 重新握手、keepalive 与过期计时器——设备任务每 250 毫秒推进 boringtun 的计时器，peer 的 `keepalive` 交给它，没有用例让 90 或 120 秒过去，假对端建时没有 keepalive。

### 需求文档的阶段与登记不一致的七处

这七处审计判为"有意不同"：行为与需求文档不同，差异在兼容性清单或某份设计里登记了；但需求文档那一行写的是阶段 1，验收文档不把它们算作做了。改需求文档的阶段列，还是改兼容性清单与设计，由项目所有者定（计划「延后事项」第 105 行）。

| FR（阶段） | 需求文档的原话 | 现在的行为，登记在哪 |
| ---------- | -------------- | -------------------- |
| FR-RULE-07、FR-RULE-08（P0，阶段 1） | 「`extended-matching`：从 TLS ClientHello 提取 SNI，从 HTTP/1.1 Host 与 HTTP/2 `:authority` 提取主机名」；「协议识别：HTTP、HTTPS（TLS）…」 | 规则匹配之前不识别：出站建立之后才读首个客户端 chunk 里的 ClientHello，只写进请求记录；基于 SNI 的路由与 dial 前的 `PROTOCOL,HTTPS` 匹配登记为阶段 4（兼容性清单 3.1、3.2 节） |
| FR-OUT-01（P0，阶段 1） | 「…RST 保护（3 秒 100 次）…」 | 没有；阶段 1 设计写"RST 限频保护属于阶段 3 的 SYN 层实现"（兼容性清单 4.1 节） |
| FR-IN-04（P0，阶段 1） | 「…运行时开关（启动参数 / API / CLI）；…macOS 通过 SystemConfiguration；…」 | macOS 用 `networksetup`；运行时开关有 `rurge run --system-proxy` 与 `POST /v1/features/system_proxy`，没有 CLI 子命令（兼容性清单 2.1、10.3 节） |
| FR-OBS-09（P1，"1 基础 / 6 完整"） | 「…崩溃后由服务管理器重启」 | systemd 的 unit 有 `Restart=on-failure`，launchd 有 `KeepAlive`；Windows 是登录时触发的计划任务，没有重启，真正的 Windows 服务登记在阶段 6（兼容性清单 10.3 节）。重启算"基础"还是"完整"，需求文档没有分 |
| FR-OBS-02（P0，"1 骨架，逐阶段补齐"） | 「…JSON 字段与 Surge 一致（清单 10.4）…」 | 阶段 1 的形状是暂定的，兼容性清单写阶段 6 对齐（10.4 节）；"骨架"里该有多少与 Surge 一致，需求文档没有分 |
| FR-RULE-09（P0，阶段 1） | 「出站模式与全局策略：运行时切换、持久化、API / CLI 读写」 | API 读写，`rurge status` 读，`rurge run` 的 `--outbound-mode` 在启动时写，运行期没有改它的 CLI 命令（兼容性清单 10.3 节） |
| FR-DNS-01（P0，阶段 1） | 「…网络切换清空」 | `crates/rurge-dns/src/resolver.rs` 里 `Resolver::on_network_change` 有用例，没有调用者；探测网络变化的东西在阶段 3（FR-IN-09） |

### 后面阶段的触发，现在没有调用者

阶段 2 的另一类：需求文档自己的话把触发它们的东西放在后面的阶段，所以机制在、没有人调用。网络变化时测试结果不失效（FR-OUT-10「网络变化失效」：`TestBook::invalidate_all` 没有调用者）、WireGuard 隧道不重建（FR-OUT-12「网络变化…时重建」：`WireGuardOutbound::network_changed` 没有调用者；一次发送失败、底层策略的去处变了、握手 90 秒没有应答三种情形代替它，让对端换一个新的 UDP 载体）、DNS 缓存不清空（见上一张表的 FR-DNS-01）——这三处都要等阶段 3 的网络变化探测（FR-IN-09）；`no-error-alert`（策略）与 `no-alert`、`icon-url`、`cellular`（组）被接受而没有任何东西用它们，也没有告警（消费者在阶段 6 与阶段 3）；`[MITM]` 的 `ca-keystore-name` 不对照 `[Keystore]` 检查（FR-CFG-11，阶段 4）；`block-quic` 只对经 SOCKS5 UDP ASSOCIATE 来的 UDP 443 生效（FR-OUT-11，没有增强模式）；外部程序自己的流量只靠子进程的环境变量里去掉代理变量来避开代理（FR-OUT-13「外部进程自身流量走 DIRECT」）。

### 未能判定

阶段 1 没有。阶段 2 一句：FR-OUT-05 的 `hysteria2` Gecko——需求文档自己写着「Gecko 与 Surge 的 `gecko-password` 是否同一算法未证实」。要判定它，要么抓 Surge 发出的 Gecko 流量，要么有一份逐字节的描述；现在有的只是 Hysteria 官网的公开规范、照同一种读法写的回环假服务端，和一个没有运行过的 sing-box 用例。

### 审计没有查的

- 没有运行任何东西；"用例"是按名字找到的测试函数，没有读多数用例的函数体。
- 阶段 1：系统代理与服务安装的真实后端（Windows 注册表与 WinINet、macOS `networksetup`、Linux `gsettings` / `kwriteconfig`、`schtasks` / `systemctl` / `launchctl`）只读了代码，自动化用例用的是文件后端与记录命令的执行器；`[General]` 各键的取值语义没有逐个对着手册核对（只查了解析器认得它们，以及属于阶段 1 的键有没有东西在 `rurge-config` 之外用它）；兼容性清单只读了一部分；API 的响应形状没有逐字段对照，只对了路由表与请求记录的字段。
- 阶段 2：FR-OUT-05 的"每种协议的专属参数"是一行一种协议，不是一个参数一行（每个参数是否有 `rurge-config` 之外的使用者，按 `PolicySpec` 公开字段扫了一遍，通用的名字——`password`、`headers`、`tls`——再读构造函数）；一个参数在某个传输上用了、在另一个上被忽略，扫不出来；Linux 与 macOS 的分支读过没有编过；非功能需求、别的阶段的需求、订阅"样本"（就是用例自己的文本）没有查；没有核对手工清单里是不是每个只有真实网络才验得出的标准都在。
- 判为"做了"的，没有人复核。

### 怎么处理

没做到的每一句都登记了：兼容性清单里说得比代码多的几行改成了实情（开头一句与 1.3、1.4、3.2、3.3、3.5、5.2 节），需求文档第 7 节给阶段 1 加了一条状态行，计划「延后事项」第 100 ～ 105 行逐项记下。2026-10-11 之后的状况见本节末尾的一段。一句也没有在 M8c 里实现：它们是有自己设计问题的功能，不是阶段 2 收尾的遗留。计划是：M8c 合并之后、阶段 3 开始之前，作为一项自己的工作补做——先写它自己的设计与计划，范围在设计里定——，它接下托管配置和上面阶段 1 的各项；这是写下的计划，代码里还没有它的任何部分。

教训出在流程上：这些条款都是某份设计交给同一阶段"以后的里程碑"的，没有一份计划的「延后事项」接住它们，所以收集那些表的 M8c 总表也不会有它们。一个阶段要在它的需求逐句对照过代码之后才称齐备，不是在它的里程碑都合并之后。

2026-10-11，阶段 1 补遗（M5a）（分支 `phase1-m5a-small-items`）把上面表里的 10 行补上了（最后一列），给有实现而没有用例的几条补了用例（上一节），并对 FR-RULE-05 另补两处：带 `no-resolve` 的 IP 行进前缀树（`00d97d3`、`39817cf`、`e4f0991`），规则集文件的行尾注释与集合里的 `IP-ASN` 行按号码查（`5b03f3d`、`92aa656`、`96b4cf7`、`3d964a9`、`ee5b458`，之后的三个修正 `ffb6477`、`3034ccb`、`09f1a7a`）。没有做的是阶段 1 补遗（M5b）的三行：托管配置的更新、远程 `#!include`、外部资源的手动强制更新——设计与阶段 1 / M5a 的设计写在同一份文档里（第 7 节），计划还没有写，代码里没有它们的任何部分。阶段 1 / M5a 的每一处行为变化都登记在兼容性清单（开头一句列出这次动过的行）与 `docs/api/phase1.md`；与计划的出入记在阶段 1 / M5a 计划的「执行期修正记录」（74 行），留下的事记在它的「延后事项」（第 5 ～ 39 行）。

## 留给项目所有者的

这一节收项目所有者要做的事与要定的事，每条都指向计划「延后事项」里写下它的那一行（行里有来龙去脉，这里不重复）。M8c 里由控制者（调度这次执行的代理，下同）在"自己决定、继续做"的常设授权下定下的事，写在计划「执行期修正记录」里，可以改；这里列的是留给所有者本人的。

### 1. 阶段 1 的两项 P0 需求从来没有做：托管配置的更新与远程 `#!include`

缺什么：`#!MANAGED-CONFIG <URL> interval= strict=` 只被解析，没有东西去抓那个 URL、缓存它、按 `interval` 更新、执行 `strict` 或在更新之后重载；目标是 URL 的 `#!include` 被跳过并报 `W0011`。两项在需求文档里都是阶段 1 的 P0（FR-CFG-05；FR-CFG-04 的"远程 URL"），所以要不要做不是问题，问题只是先后。阶段 1 的设计把它们交给"外部资源管理器（M2）"，M1 的计划写"按设计推迟到 M2"，此后没有一份计划、设计或遗留事项表记下它们从没落地；2026-10-10 在改兼容性清单的一句话时被偶然发现。

登记了，没有实现：兼容性清单、需求文档第 7 节、两份 README、`CLAUDE.md`、本文和计划「延后事项」第 100 行都写了（本文「需求逐条对照代码」一节表里的第一、二行）。它需要自己的设计——rurge 从不改写用户的文件（FR-CFG-02、FR-CFG-17），抓回来的配置放在哪里；`strict` 到期未更新成功时拒绝什么、什么保持可用；托管配置自己的 `#!include` 与相对路径怎么解析；远程 include 的缓存、刷新与启动时失败怎么办。

下一步的计划（控制者在常设授权下定的，所有者可以改次序）：M8c 合并之后、阶段 3 开始之前，作为一项自己的工作，先写设计与计划；「需求逐条对照代码」一节表里的其余阶段 1 缺口一并接下（计划「延后事项」第 101 ～ 103 行）。今天的后果：导入服务商的托管配置，得到的是一份永远不会自己更新的配置。

跟在它后面、同样没做的：`hidden`（第 104 行，移交阶段 6）；需求文档的阶段列同兼容性清单与设计的七处矛盾（第 105 行，由所有者定）；四个只解析的参数 `tfo`、`wireguard` 上的 `ecn`、`snell` 的 `udp-port`、`external` 的 `addresses`（已登记在兼容性清单，加载时 `W0029`；遗留事项总表各有一行：M1a #1、M4b #2 与 M5c #5、M6b #3、M4c #5，M8c 的计划「延后事项」里没有）。

2026-10-11 的更新：阶段 1 / M5a（阶段 1 补遗的第一部分）已经做完，上面「需求逐条对照代码」一节表里除这三行以外的 10 行都补上了；剩下的三行——托管配置的更新、远程 `#!include`、外部资源的手动强制更新——是阶段 1 补遗（M5b）：设计已写（`docs/superpowers/specs/2026-10-11-phase1-m5-addendum-design.md` 第 7 节，已决事项 M5-D1 ～ D11 是实施者在常设授权下定的，可以改），计划还没有写，代码里没有它们的任何部分。上面"其余阶段 1 缺口一并接下"的那一部分因此已经做完。

2026-10-13 的更新：阶段 1 补遗（M5b）做完了这一项。托管配置的更新（FR-CFG-05）、远程 `#!include`（FR-CFG-04 的"远程 URL"）与外部资源的手动强制更新（FR-CFG-14）在代码里有了，提交与用例在「需求逐条对照代码」一节的「阶段 1 补遗（M5b）之后（2026-10-13）」里；上面"今天的后果：导入服务商的托管配置，得到的是一份永远不会自己更新的配置"不再是现状。它的已决事项是代项目所有者作的裁定，来龙去脉在阶段 1 / M5b 计划的「执行期修正记录」（下面写"记录第 N 行"）与「延后事项」（"延后第 N 行"）里，都可以改。留给所有者的是：

- 下载到的文本不因为被 include 而多出权力：托管配置的副本不能 include 本地文件（整分支评审之后关上的：此前它可以用朴素的相对路径 include，而读进来的代理能被副本经它自己的 DNS 与中继用上，记录第 119 行），其中的 `external` 策略不启动，也不能点名本地的集合与 `policy-path` 文件（手册没有这样的限制，按手册的写法都行；没有在 Surge 上核对），被拒的 `policy-path` 让组没有导入的成员，组里没有别的成员时是空组、缺省走 DIRECT，加载时的 `W0033` 是唯一的信号；要不要一个命令行开关放开它，要不要把副本的本地 include 放回来：延后第 24、8 行（记录第 36、37、119 行）。
- URL 里的用户名口令作为 Basic 认证发出（已定，所有者可以改回；见第 4 项「订阅样本与装配」的「还缺什么」与下面「要定的事」里的更新）：延后第 25 行（记录第 49 ～ 51 行）。
- 托管配置能改 `http-api` 与监听地址（手册说整份 Profile 由链接更新，没有把这两项排除在外；没有在 Surge 上核对）；监听地址有 `--listen-http` / `--listen-socks5` 可以钉住，`http-api` 没有对应的选项；副本生效时 `rurge reload` / `stop` / `status -c` 从副本取 API 的地址，`--key` 或 `RURGE_API_KEY` 给的密钥因此发往一份下载到的文本选的地址（`--remote <地址> --key <密钥>` 不读 Profile，不受影响）：延后第 9 行。
- 托管配置或远程 `#!include` 的链接是 `http://` 时没有任何提示（路上的任何人这时就是服务商；链接里写了用户名口令时它们以明文发出）；要不要在加载时给一条告警：延后第 105 行，与第 25 行一起定。
- 重载失败时，rurge 去下载那次加载点名而从没下载过的来源（已定，所有者可以改回；代价是一份有错误的 Profile 会让 rurge 向它点名的地址发 GET）：记录第 121 行，延后第 107 行。
- Profile 来源的下载走直连、不经策略（与订阅、规则集相同），只有经代理才够得到的服务端取不到：延后第 1 行；`meta.json` 里存着完整的 URL（带令牌），数据目录的权限是使用者自己的，而现在这个目录里还放着托管配置的整份副本——能写它的人决定 Profile，能读它的人读得到副本里的每一个凭据（评审的建议：在 Unix 上给 `resources/` 设 0700 的模式）：延后第 2 行。
- 两代相邻的管理器可以同时写同一个缓存条目的临时文件（要不要给写的一方自己的后缀）：延后第 30 行；`crates/rurge-net/src/resource/mod.rs` 约 5000 行、大半是测试，要不要把测试拆出去：延后第 28 行。
- 几条启用的 `FINAL` 的读法（已照手册改，读法是推断的；见下面「要定的事」）：延后第 22 行。
- 读完阶段 1 的需求之后留给所有者的：大集合内存的判据（延后第 77 行）；阶段栏写 1 而兼容性清单或设计登记在后面的 37 句，改阶段栏还是改登记（延后第 78 行），FR-CFG-15 的配置目录、列出与切换三句是其中的一部分（延后第 79 行）；需求文字与手册或另一条需求不符的几处（延后第 80 行）；FR-CFG-02 的"保留在内存"（延后第 81 行）。

### 2. 验收证据还缺的

1. **CI 从没绿过。** 最近一次运行在本地 `origin/main` 的 `0154e73` 上，`cargo test` 在三个平台都失败，之后的提交（到 `44a0204` 是 76 个）没有一个被 CI 见过。要产生证据：项目所有者推送；读三平台的失败清单（失败摘要把它发成注解与任务摘要，不登录也读得到）；修掉——这是 M8a 的任务 6，等推送之后做；再有一次三平台全绿的运行（计划「延后事项」第 49 行）。（2026-10-18：推送了，运行 39022550224 的失败清单读到了，六类失败里五类修了、第 6 类换了做法并补了诊断（原因仍不知道），见第 8 节的更新；现在缺的是再推送一次：看修正管不管用，并拿到 macOS 与 Windows 上没跑到的四个包的测试的结果。）
2. **没有一个互操作用例对真实的二进制跑过。** 63 个用例在本机跳过（61 个在本机编译）；好几个参照服务端的命令行、配置与就绪日志是读源码写的，夹具改取端口的改动也没有对真实的服务端跑过。要产生证据：同一次三平台全绿的 CI 运行，其中 `RURGE_INTEROP_REQUIRED=1` 保证这些用例跑了而不是跳过；某个用例红了，先分清是夹具与参照服务端不符，还是 rurge 的问题。（2026-10-18：它们在运行 39022550224（`7bd6589`）里对真实的二进制跑过一次，装了参照服务端的平台上除五条之外都通过，夹具改取端口的改动也随之跑过；这一条现在缺的是修正之后一次三平台全绿的运行，其中互操作用例都跑了而不是跳过。）
3. **AnyTLS 的 SYNACK 看门狗没有对真实服务端验证过，第一次 CI 运行也验证不了**：互操作用例在几毫秒内放掉每条流，UDP 用例用新会话的第一条流，看门狗不看它；假服务端应答的次序是照参考实现的源码写的。要产生证据：一个在复用的会话上活过 3 秒的流的互操作用例（计划「延后事项」第 87 行）；手工清单 M8c 一节有对应的两项。（2026-10-18：运行 39022550224 也没有验证到它，这一条不变。）
4. **mihomo 的用例**（Snell v1 ～ v3、`ss` 的流式方法、两种协议前面的 simple-obfs）可以在本机对项目所有者自己的 mihomo 跑，设 `RURGE_TEST_MIHOMO` 即可——是否这样做由所有者定。
5. **两份手工清单一项也没做**：`docs/acceptance/phase2-manual.md` 22 节 187 项，`docs/acceptance/phase1-manual.md` 34 行。其中有几项的写法是读文档或源码得出的，没有人照着走过一遍（经 tailscaled 的 SOCKS5 接入、MASQUE 经 `underlying-proxy` 的数字）。 2026-10-13 的更新：阶段 1 的清单现在是 45 行（Windows 14、macOS 13、Linux 13，另有新的一节「与平台无关」5 行）。阶段 1 补遗（M5b）加了 11 行：真实服务商的托管配置、真实服务器上的远程 `#!include`、真实的 `strict`，三个平台各一行 API 连接收尾的检查，三个平台各一行真实的 `.local` 名字，以及由单播 DNS 服务器回答的 `.local` 区域与 `[Host]` 里的 `server:system`。仍是一行也没有做过；阶段 2 的清单没有变。 2026-10-14 的更新：手册基线重对在阶段 1 的清单「与平台无关」一节加了 1 行（现在是 46 行，与平台无关 6 行），在阶段 2 的清单加了新的一节「手册基线重对」2 项（现在是 22 节 187 项，第一句已按它写）；仍是一行也没有做过。
6. **这台开发机与 CI 不同**：本机开着 Windows 的 Simple TCP/IP Services（回环 TCP 与 UDP 的 7、9、13、17、19 号端口），CI 上关着。写到 9 号端口的 66 行换成关着的端口后，整套用例只有一条变红，已改（`crates/rurge-engine/tests/smart.rs`：`udp_silence_counts_only_where_answers_always_come`），现在没有用例依赖它们。"立刻拒绝"（Linux、macOS）在本机只能用"接了立刻关"顶替，真的要等 CI。
7. **阶段 1 补遗（M5a）里本机验不了的三处**：`sighup_reloads_the_profile` 与 `run_shuts_down_gracefully_on_sigint` 新加的一行等待只在 Unix 上编译，本机（Windows）没有编译过、运行过；`rurge_platform::sysinfo` 对 macOS 与 Linux 的读取只对着假的命令、文件与环境变量测过，macOS 的三种格式（`SystemVersion.plist`、`defaults read -g AppleLanguages`、`scutil --get`）取自公开描述。要产生证据：同一次三平台全绿的 CI 运行；`docs/acceptance/phase1-manual.md` 的 macOS 与 Linux 各有一行，由项目所有者在真实的机器上做（阶段 1 / M5a 计划「延后事项」第 1、2、13、23 行）。（2026-10-18：这三处所在的 `rurge-platform` 的 Unix 分支和 `rurge` 的命令行用例，在运行 39022550224（`7bd6589`）里在 Linux 与 macOS 上编译、运行，通过了一次，见兼容性清单平台变量一行；真实的 Mac 上读到什么，仍待手工清单。）

### 3. 要定的事

每条的论据在对应的行里。

- 脱敏后的视图换不换成只显示已知无害内容的视图：第 51 行；已知没有覆盖的二十项：第 58 ～ 77 行；有意显示的几类：第 43 行；两处多遮：第 98 行。下面单列一段。
- 节头后面跟注释（`[MITM] # my certificate`）对 rurge 不是节头，下面的行归给上一节；Surge 怎么读不知道，建议读成节头并给一条告警：第 52 行。
- AAAA 抑制的计数把一个名字数两次（后台补查再超时又计一次），抑制在第三或第四个名字而不是第五个时生效；测试把现状钉成了现状，不是钉成了对的：第 36 行。 2026-10-13 的更新：不再等所有者定，已照手册的"连续 5 次查找"改了（阶段 1 补遗（M5b），提交 `a72b5e9`、`4a0e0a9`，代项目所有者作的裁定）。原来是：一次查找数了两次——它自己的查询一次，为同一次查找补问 AAAA（好让迟到的 AAAA 应答仍进缓存）的后台查询再超时又一次——，AAAA 在第三个名字时就停了。现在：一个超时按发出的那次查询数一次，由发出它的调用方数——补问不数，同一时刻同名的查找共用一次查询、只数一次——，一个接一个地查，第五个名字的查找让 AAAA 停发，从第六个名字起不再问（用例 `aaaa_is_turned_off_by_the_fifth_lookup_in_a_row_and_a_flush_turns_it_on`、`five_lookups_of_one_name_at_once_count_one_aaaa_timeout`）；DNS 缓存刷新（`POST /v1/dns/flush`）、重载或重启让它回来。错了的代价，两个决定各一句：一次查找数一次（`a72b5e9`）——Surge 若是另一种数法，rurge 在一个从不回答 AAAA 的网络上晚两个名字（各一秒）才停，一个枚举、三处调用，可回退；同时发出的同名查找共数一次（`4a0e0a9`）——Surge 若数每个调用方（它怎样数不知道），许多连接同时去一个新主机时 rurge 比它晚停（五个不同的查询之后照样停），一个函数，可回退。所有者仍可改（阶段 1 / M5b 计划「执行期修正记录」第 104、105 行，「延后事项」第 95、96 行）。
- 一个 QUIC 策略（`masque` / `hysteria2` / `tuic`）能占多少内存对着 NFR-01 说不过去：第 53 行。
- HTTP 客户端发不发 URL 里的用户信息作 Basic 认证（规则集、域名集、订阅、脚本，现在不发，抓取以 `http status 401` 失败）：第 54 行。 2026-10-13 的更新：不再等所有者定。2026-10-12 代项目所有者作了裁定：发（阶段 1 补遗（M5b），提交 `b33e27e`；来龙去脉在第 4 项「订阅样本与装配」的「还缺什么」里同一件事的那一条下）。HTTP 客户端建的每个请求都带 `Authorization: Basic …` 头——规则集、域名集、订阅、GeoIP 库与 Profile 的来源，DoH 的上游本来就发——，重定向时只带到同一来源；连通性测试不经这个客户端，测试 URL 里的用户名口令不发出。代价：这些用户名口令此前从不作为认证发出，`http://` 的 URL 因此从不把它们送出本机，现在随每个请求发出，对 `http://` 的 URL 是明文。所有者可以改回：拿掉用户名口令而什么也不发，是 `HttpClient::build` 里的一个条件（阶段 1 / M5b 计划「执行期修正记录」第 49 ～ 51 行，「延后事项」第 25 行）。
- `[Host]` 下贴进的分享链接现在是 `E0017`，原来加载成一条什么也匹配不到的别名；一份有这样一行的配置在那一行删掉之前不再加载：第 55 行。
- `ConfigSummary.managed` 带着托管配置的 URL，现在没有东西显示摘要，放上 API 之前必须先去掉：第 45 行。
- Snell v6 要不要照已发表的逆向资料实现（可行性报告请所有者表态）：第 56 行。
- clap 在自己的报错里引用解析不了的 `--geoip-url` / `--geoip-asn-url` 取值，取值里可以带 MaxMind 的授权密钥：第 57 行。
- `update-interval=0` 有两种读法——规则集与域名集上读作没写，策略组上是加载错误 `E0018`——，要不要一致、一致到哪一边：第 99 行。
- 阶段 1 补遗与需求文档的阶段列：第 100 ～ 103 行与第 105 行（本节第 1 项）。
- 阶段 1 补遗（M5a）留下的（每条的来龙去脉在阶段 1 / M5a 计划的对应行里）：
  - 设了而为空的环境变量（`RURGE_LISTEN_HTTP=`）现在是错误（退出 2，`no address given`），与 `--listen-http ''` 相同——决定代理在哪里监听的设置，拒绝启动比悄悄照 Profile 监听稳妥；另一种读法是当作没设：执行期修正记录第 10 行。
  - `external` 策略的 `local-port` 的检查看不到 rurge 的缺省监听——一份不写任何监听键的 Profile 监听 `127.0.0.1:6152` 与 `:6153`，写 `local-port=6153` 的 `external` 策略能加载、没有诊断，启动后经这条策略的每个连接都是 rurge 连自己——，改法的草图在那一行；要不要在阶段 3 之前做：延后事项第 10 行。
  - `rurge service install` 不把 `--listen-http` / `--listen-socks5` 带进安装的服务：延后事项第 11 行。
  - Windows 的 `DEVICE_NAME` 取 DNS 主机名而不是大写的 NetBIOS 名（`COMPUTERNAME`），Profile 里按大写名字比较的会落空：执行期修正记录第 18 行。
  - Surge 对"同一资源既当 `RULE-SET` 又当 `DOMAIN-SET`"怎么办不知道，rurge 的做法（直接写出的是 `E0024`，嵌套的是 `W0032`）是自己定的，手边有 Surge 时核对：延后事项第 9 行。
  - `--listen-socks5` 写错取值时的原因 `address must be an IP literal like 0.0.0.0:6152` 与 `E0013` 共用一句，示例端口是 HTTP 的缺省端口：延后事项第 34 行。
  - 满额集合的内存（数字见「做了而没找到用例」一节）："可控"的判据、NFR-01 的前提，以及要不要像 Surge 那样把大集合的索引放到磁盘上（手册 `rules/ruleset.html`："sets with more than 1000 domain entries use an on-disk database instead of an in-memory tree"）：延后事项第 24 行。
  - 规则集文件行尾的注释照 Profile 的规则读：手册没有写 Surge 这样读集合文件，是从两份常用列表推断的，手边有 Surge 时拿一行 `IP-ASN,4134 // 说明` 的列表核对一次：执行期修正记录第 57 行。
  - 一个集合里几行同时命中同一条会话时报成子规则的是哪一行，以及两句提示出现的时机（逐条求值的行现在全部先走完），是 rurge 自己定的，Surge 报不报、报哪一行不知道：执行期修正记录第 53、60 行。
  - 域名集里读不了的行不再被告警引用，只给行号：照"凭据不进日志与错误文本"当场改的（`09f1a7a`），所有者若宁可看到那一行的文字，是一行的改动；规则集里类型认得而取值不对的行（`IP-CIDR,<不是网段的文字>`）仍引用那个取值，与同一条规则写在 Profile 里相同（M8c 的裁定），要不要连它也不引用由所有者定：执行期修正记录第 66 行。
  - 集合文件的行上的未知参数没有人报告（`no-resolve` 拼成 `no-resolv` 就丢了它而不出声），启动之后才载入的集合跳过了行也不出声：延后事项第 31、32 行，留给阶段 1 补遗（M5b）写计划时定做不做、怎么报。
  - 审计判为"做了"的条款没有人复核过，这一次查出一条（FR-RULE-05）只做了一部分；阶段 1 称齐备之前要再对一遍需求，每条"做了"指到一条会因它而红的用例：延后事项第 28 行。
  - 几条启用的 `FINAL` 的读法：手册的几句话（`rules/final.html`、`profile/format.html`）合起来只有一种读法能同时成立——匹配在第一条启用的 `FINAL` 处结束，策略与选项取最后一条的——，rurge 今天让写在两条 `FINAL` 之间的规则仍被评估；要改成的那种读法是从手册推断的，没有在 Surge 上观察过，手边有 Surge 时核对；阶段 1 补遗（M5b）改它：延后事项第 36 行。 2026-10-13 的更新：已照手册改（阶段 1 补遗（M5b），提交 `b5d4ef8`、`919ee16`）：匹配在第一条启用的 `FINAL` 处结束，策略与选项取最后一条的；写在两条 `FINAL` 之间和最后一条之后的规则不再评估（`W0019` 数它们，此前的每条 `FINAL` 另有一条 `W0021`），`GET /v1/rules` 不列它们，只被它们引用的规则集不再登记、下载（用例 `matching_ends_at_the_first_final_and_the_last_one_gives_the_policy` 与 `the_rules_after_the_first_final_are_not_listed`）。这个读法是推断的，从来没有在 Surge 上观察过：手册的三句话——`rules/final.html`："A FINAL rule always matches, so any rules written below it never take effect." 与 "If several enabled FINAL rules are present, the last one is the effective one."，`profile/format.html`："In the `[Rule]` section, a `FINAL` rule immediately terminates rule matching."——只有这一种读法能同时成立。错了的代价：Surge 其实会评估的一条夹在两条 `FINAL` 之间的规则，在 rurge 里不生效，`W0019` 会说有几条、在哪里；不改的代价是 Surge 不评估的规则在 rurge 里生效。只有带两条或更多启用的 `FINAL`、且它们之间夹着规则的 Profile 受影响。手边有 Surge 时核对：阶段 1 / M5b 计划「延后事项」第 22 行，「执行期修正记录」第 3 ～ 9 行。
- 推送是所有者的；两个提交说明的排版有误（`21ecb85`、`d309f33`），推送前是否 reword：第 26 行；约三十份较早的计划引用了一个含 Windows 用户名的本机 `rustfmt` 路径，要不要改写历史计划里的路径：第 95 行。
- 知悉，不需要决定的五件事。一、2026-10-09 上午（07:30 ～ 11:00），这次执行用的账号的每周模型用量上限用尽过一次，派出去的子代理都停住，上限复位后接着做；此后每个角色（实现、评审、审计）都用能胜任的最弱的模型，较弱的模型没有一次做对的部分要多一轮修正（第 96 行，「执行期修正记录」第 7 行）。二、任务 10 的实现者（一个子代理）为了查明一个测试为什么失败，用 `rustc` 编过一个只依赖标准库的 5 行小程序并运行过一次（向本机回环地址发一个 UDP 包），它自己在报告里说了这件事；这违反"不运行不是由本仓库构建的二进制"这条约束的字面，源码全是它自己写的，没有东西离开本机；此后给子代理的约束写明，探测要写成仓库里的测试、用 `cargo test` 跑（第 97 行，「执行期修正记录」第 46 行）。三、阶段 1 补遗（M5a）执行中，磁盘被聚焦构建写满过一次（`target/` 涨到 141 GiB，只剩 5.3 GiB 可用）：cargo 按所选的包统一依赖特性，每换一种 `-p` 的选择就另编一份依赖树；此后聚焦运行一律用 `--workspace --lib` / `--bin rurge` / `--test <名字>` 的形式，不用 `-p`，与全量门禁共用同一份构建（阶段 1 / M5a 计划「执行期修正记录」第 8 行）。四、API 的一处性质：不读请求正文的端点（`/v1/stop`、`/v1/dns/flush`、`/v1/profiles/reload`、`/v1/profiles/check`）和在读正文之前就被拒绝的请求，只凭请求头就答复并关掉连接，正文晚于请求头到达时客户端可能读不到应答，尽管请求已经执行（在 Windows 上量到，别的系统没有量）；阶段 1 补遗（M5b）会修，在那之前，往这四个端点发正文的脚本可能看到连接错误（阶段 1 / M5a 计划「延后事项」第 35 行，`docs/api/phase1.md` 的「鉴权与错误」）。五、一个测试里的启动有一次没有在 20 秒内结束，原因没有人知道，之后同样的负载下跑了 810 次没有再出现；再出现时辅助函数现在会说出是哪一次启动、子进程打印过什么（阶段 1 / M5a 计划「延后事项」第 37 行）。 2026-10-13 的更新：第四件已修（阶段 1 补遗（M5b），提交 `42ab1e3`、`1f1938a`）。API 的监听器现在分步关连接：服务端放手一条连接时先关写端，再读掉并丢弃对端还在发的字节，至多 1 秒、256 KiB，然后才关；正文晚于请求头到达的客户端读得到那个应答。不读请求正文的端点现在是三个——`POST /v1/dns/flush`、`POST /v1/profiles/reload`、`POST /v1/stop`；`POST /v1/profiles/check` 不在其中，它读正文，在正文到齐之前不答复——，另有在读正文之前就被拒绝的请求（`401`、`403`、`404`、`405`）：这七种应答在一台 Windows 11 x86-64 的开发机上量过，改动之前对一个把请求头与正文分开写的客户端读不到，之后读到（用例 `a_late_body_does_not_cost_a_dns_flush_its_answer`、`a_late_body_does_not_cost_a_wrong_key_its_401` 等）。还没有覆盖的：超过 256 KiB 之后才读应答的客户端仍可能丢应答（同一台机器上，发 300 KiB 之后立刻读或 0.5 秒后读，50 次里 50 次得到应答；发 8 MiB 之后 1.5 秒才读，10 次里 10 次什么也没有、一个 reset）；64 个收尾的位置由所有对端共用；一个已得到应答却不关连接的空闲客户端使每次停止多等至多 1 秒；Linux 与 macOS 没有量过，自动化用例在那两个系统上断言的是同一件事，但它们在改动之前红不红不知道（阶段 1 的手工清单各有一行）。见 `docs/api/phase1.md` 的「鉴权与错误」；阶段 1 / M5b 计划「执行期修正记录」第 81 ～ 87 行，「延后事项」第 20、23、72、73 行。

### 4. 脱敏后的视图

项目所有者定的规矩是"凭据不进日志、错误文本、API 输出与 `Debug`"。到阶段 2 结束时它是这样的：标记用例 `crates/rurge-config/tests/secrets.rs`（一份每个凭据字段都带标记的配置，加载之后的 `Debug`、诊断与脱敏输出里都找不到标记）通过；不带 `sensitive=1` 的 `GET /v1/profiles/current`、原样保留的行的 `Debug` 与 `policies/detail` 显示的，是一张"藏什么"的清单（`crates/rurge-config/src/redact.rs`）应用到一行上的结果，这一行按解析器读它的样子来读。这个视图经过两次对抗性复审、三轮修正：每一次复审都在普通配置里找到了仍然露出秘密的行，第二位复审者的判断是，对这种格式，这样的清单做不成可靠的；第三轮之后控制者代项目所有者裁定不再追，之后没有第三次复审，最后一个提交（`087f399`）也没有评审——控制者读了 diff、跑了全量门禁。所以它是尽力而为，不是"没有秘密会出现"的保证。已知没有覆盖的写法列在计划「延后事项」第 58 ～ 77 行（二十项，例如位置口令里带 `=`、`{password: hunter2}` 这样的 Clash 流式映射、没有分隔符的 `ca-passphrase hunter2`、落进它不属于的节的行），那些种类的行在视图里仍会露出它们的秘密；整分支终审只读了产品代码，它的结论只管产品代码。另一种做法是只显示已知无害内容的视图——认得的节头、已知键加已知无害形状的取值、策略行的名字 / 类型 / 服务器 / 端口与已知的非秘密参数，其余连注释在内一律 `***`——，它会遮住每一条注释和每一行 rurge 不认识的内容，读起来差很多，也离 Surge 更远（Surge 自己的端点遮口令字段）。换不换是所有者的决定（第 51 行），M8c 没有做，也没有把它写成计划。权衡时要知道：能取到脱敏视图的人拿着 API key，能用 `sensitive=1` 取到原样的配置；视图护着的是一张截图、一次粘贴、一份贴出去的报告，不是对 API 的客户端藏秘密。
