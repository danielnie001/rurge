<div align="center">

<img src="docs/assets/rurge-icon.svg" width="128" height="128" alt="rurge 图标">

# rurge

**中文** | [English](README_en.md)

</div>

---

### 简介

rurge（**Ru**st + Su**rge**）是一个用 Rust 编写的跨平台网络代理工具，目标是逐步复刻 [Surge](https://nssurge.com/)（macOS / iOS）的全部功能，并做到：

- **原生兼容 Surge 配置格式**：直接使用 `.conf` 配置、`.sgmodule` 模块、`RULE-SET` / `DOMAIN-SET` 规则集、`policy-path` 策略订阅和托管配置（`#!MANAGED-CONFIG` 与远程 `#!include` 在 `rurge run` 运行期间下载到数据目录、按 `interval` 更新，与 Surge 的差异登记在[兼容性清单](docs/surge-compatibility-matrix.md)的 1.3 节，见「当前状态」）；
- **兼容 Surge 脚本 API**：现有 `http-request` / `http-response` / `cron` / `event` / `dns` / `rule` / `generic` 脚本无需改动即可运行；
- **兼容 Surge HTTP API**：现有 Dashboard 与自动化工具可以直接对接；
- **跨平台核心优先**：Windows / Linux / macOS 共用同一个命令行守护进程，桌面 GUI 放在后期阶段。

### 当前状态

> **2026-10-26：CI 第一次三个平台全绿**（运行号 40283632986，提交 `ac0a98a`；此前的十一次运行都失败）。下面各段里“CI 没有绿过”“修好没有要再推送一次才知道”一类的说法写于这次运行之前，以这一句为准：上一次运行读出的六类失败，修了之后这一次都通过（第六类，Windows 上 `MockDns` 找不到端口，原因仍不知道）；阶段 3 / M2、M3 的提交也第一次在三个平台上跑过并通过。手工验收清单仍一项也没有做过。

> **阶段 1 的里程碑 M1 ～ M4b 与随后的两个补遗已合并，代理端到端可用**。阶段 1 补遗（M5b）做了过去没有做的三项：托管配置（`#!MANAGED-CONFIG`）在 `rurge run` 运行期间按 `interval` 下载到数据目录（rurge 不改写你的文件，下载到的是副本），先检查再采用——加载不了的与不是 Profile 的（比如强制门户的页面）不采用，上一份照旧生效——，`strict` 到期未更新成功时拒绝新会话（API 照常），更新后自动重载；远程 `#!include` 同样下载、缓存、刷新；外部资源可经 `GET /v1/external_resources` 查看、经 `POST /v1/external_resources/update` 立即更新。下载的链接里写了用户名口令（`https://user:pass@host/…`）时，rurge 把它们作为 Basic 认证发出（手册没有写明 Surge 发不发；`http://` 的链接上它们是明文；登记在兼容性清单 3.1 节 `RULE-SET` 一行）。下载到的文本不因为被 include 而多出权力：它不启动本机的程序，也不点名本地的规则集、域名集与 `policy-path` 文件（托管配置的副本也不 include 任何本地文件），所以托管配置的副本里的本地 `#!include`、`external` 策略、本地集合与 `policy-path` 与手册的写法不同（手册没有这样的限制；没有在 Surge 上核对）；这些差异登记在 [docs/surge-compatibility-matrix.md](docs/surge-compatibility-matrix.md) 的 1.3 节与 10.4 节。同一次补遗里，写在第一条启用的 `FINAL` 之后的规则不再评估（读法是按手册的三句话推断的，没有在 Surge 上观察过），API 的连接分步关闭，使正文晚于请求头到达的客户端读得到应答（在一台 Windows 11 x86-64 的开发机上量过，Linux 与 macOS 没有量）。阶段 1 的需求（需求文档第 4 节里所属阶段含阶段 1 的 49 行，共 416 句）已经逐句对着代码读过一遍，每一句的结论和证据在 [docs/acceptance/phase1-acceptance.md](docs/acceptance/phase1-acceptance.md)。结果是三个数，要一起看：**2 句没有做完**，都在优先级 P0 的需求行上（规则集文件里一行上的 `extended-matching`、“大集合内存可控”没有判据；逐句读的时候是 3 句，第三句是手册 IP 页的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`，手册基线重对时做了）；**37 句的阶段栏写着阶段 1，而兼容性清单或某份设计把它们登记在后面的阶段**，其中 34 句今天不按需求的字面做、等阶段 3 ～ 6，另 3 句登记在阶段 2、今天有代码；**10 句有意做成了别的样子，已登记**（逐句读的时候是 14 句：DNS 连接的 DoH、DoT、DNS 三个协议标签与 `PROTOCOL` 规则匹配那 4 句，手册基线重对时随标签改按 URL 的方案补上了）。改需求文档的阶段栏还是改登记，由项目所有者定。这一遍读的是代码和用例的断言，补了用例，补上了一些小缺口，每条结论另有人抽查；判为“做了”的句子指到一条断言了这一句的用例，这不等于别处没有缺口。它没有覆盖：阶段 1 的手工验收清单（一行也没有做过）、CI（没有绿过；那一遍读的时候所有用例只在一台 Windows 11 x86-64 的开发机上跑过，CI 上的第二次运行见阶段 2 一段）、阶段 2 的验收（见下）；真实的系统解析库对 `.local` 名字做什么，在任何平台的真实网络上都没有观察过。2026-10-10 的逐条核对还查出十余处只做到解析或没有做的条款，已由阶段 1 补遗（M5a）补上（2026-10-11）：`pre-matching` 规则优先、子规则命中日志、同一资源不可同时作两种集的检查、`GET /v1/geoip`、平台变量取自操作系统（macOS 与 Linux 的读取在 CI 的运行 39022550224 里编译并通过了一次，用例大多对着假的命令与文件，真实的 Mac 上读到什么还没有核对）、`--listen-http` / `--listen-socks5`、`rurge check` 的建议、请求记录的监听端口 / 远端地址 / DNS 耗时、`state.json` 里的当前 Profile 记录、`#!WINDOWS-ONLY` / `#!LINUX-ONLY`，连同规则集文件的行尾注释与集合里 `IP-ASN` 行按号码查；清单与证据见 [docs/acceptance/phase2-acceptance.md](docs/acceptance/phase2-acceptance.md) 的「需求逐条对照代码」一节。已有的是（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行——清单现在还有对真实服务商的托管配置与远程 `#!include`、真实的 `strict` 和真实的 `.local` 名字几项，也都没有做过——，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）功能上已完成、验收尚未完成：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），手册列出的 11 种流式旧方法在 M8b 实现（TCP 与 UDP；它们不校验数据、已被弃用，加载时告警）；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 在 M8b 实现（没写 `version` 即 v1；UDP 自 v3 起），只有 v6 按 REJECT 处理；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP）；M7（QUIC 族）已完成：M7a（QUIC 地基与 Hysteria 2）已完成——`hysteria2` 的 TCP 与 UDP（QUIC 数据报，全锥）、Salamander 与 Gecko 混淆、`port-hopping`、`download-bandwidth`（只上报给服务端）与 `ecn`，可经 `underlying-proxy` 连出去；M7b（TUIC）已完成——`tuic-v5`（TUIC v5）与 `tuic`（TUIC v4，tuic 0.8.5 的协议）的 TCP 与 UDP（服务端接受 QUIC 数据报时走数据报，v5 放不下的包分片，否则每个包一条单向流；全锥）、`port-hopping` 与 `ecn`，可经 `underlying-proxy` 连出去；M7c（HTTP/3 族与 DoQ / DoH3）已完成——`masque` 的 TCP（HTTP/3 上的普通 CONNECT，每个 CONNECT 都带 Basic 凭据）与 UDP（CONNECT-UDP，HTTP/3 数据报，每个目标一条流、对称型）、`port-hopping` 与 `ecn`，`trust-tunnel` 的 `h3=true`（HTTP/3，`max-streams` 按每条 QUIC 连接计），`quic://`（DoQ）与 `h3://`（DoH3）加密 DNS 上游，开了 `encrypted-dns-follow-outbound-mode` 时 DoQ / DoH3 的连接也按规则走；M8（收尾与验收）除 M8a 的任务 6 的后续（再推送一次、看修正管不管用）之外已完成：M8a 让 CI 把失败的用例发成公开可读的注解，并让凭据不进日志、诊断与 `Debug`（脱敏后的配置视图是一张“藏什么”的清单，尽力而为，不是没有秘密会出现的保证），四个开关的环境变量接受 `1` / `0`、`yes` / `no`、`on` / `off`；M8b 做了上面说的流式旧方法与 Snell v1–v3，VMess 的旧握手不实现（没写 `vmess-aead=true` 的行照样用 AEAD 握手），`W0007` 改指 rurge 不实现的协议与变体（`tailscale`、Snell v6、手册不再列出的 `ss` 方法名）；M8c 处理了历次计划留下的遗留事项（QUIC 的接收窗口与 `masque` 经 `underlying-proxy` 时的起步包长、载体的错误、DoH 的缺省路径与 `Age`、AnyTLS 的 SYNACK 看门狗、WireGuard 隧道的结束与重建、`smart` 的失败计分、IPv4 映射地址、配置加载不了时也恢复系统代理、解析器空转的修复、`CORE_VERSION` 22），写了阶段 2 的验收清单 [docs/acceptance/phase2-acceptance.md](docs/acceptance/phase2-acceptance.md)；[可行性报告](docs/reports/2026-10-08-phase2-feasibility-snell-v6-quic-tailscale.md)（Snell v6、Snell 的 QUIC Proxy Mode、Tailscale）是 M8 开头同设计一起写的。**阶段 2 的验收没有完成**：三平台 CI 还没有绿过。最近一次运行是 `7bd6589`（2026-10-17，运行号 39022550224）：Linux 跑完了全部测试二进制，5 条失败；macOS 与 Windows 都停在 `rurge-proto` 的单元测试二进制里（各有三条用例没有结束，用满了测试一步的 60 分钟），所以只跑到排在它前面的各包，`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-rules` 这四个包的测试在这两个平台上没有跑到。对参考实现的互操作用例的结果，这是第一次读得到：装了参照服务端的平台上，除五条之外都通过（这是那一次运行的结果，CI 仍没有绿过）。读出的六类失败——`LazyHead` 的两半交给两个任务轮询时丢了写端的唤醒（生产里 VMess 的 UDP 载体也这样轮询它，这一处修正对它同样起作用）；互操作用例收 UDP 回包的缓冲只有 64 字节；比较回包的来源时没有把 IPv4 映射的地址归一；masque-server 每条 TCP 隧道只排 1 MiB 客户端的字节（参照服务端的限制，用例改为节制上行，登记在兼容性清单）；macOS 上 UDP socket 的缺省发送缓冲 9216 字节装不下测试发的大数据报；Windows 上 `MockDns` 找不到端口（原因没有证据）——前五类已经修了，第六类换了找端口的做法并补了诊断（`f8d0389`、`4fc1bad`、`cae06c3`、`26be5e2`、`c048315`、`2519b3f`、`0deba9f`、`f9377a8`、`94cfd9e`）；修好没有，以及那四个包的测试在这两个平台上的结果，都要再推送一次才知道，所以 M8a 的任务 6 只做了第一轮。手工验收清单 [docs/acceptance/phase2-manual.md](docs/acceptance/phase2-manual.md) 还没有执行。`subnet` 策略组随阶段 3 / M1 做了（见下一段）；`tailscale` 策略不实现，现在经本机 tailscaled 的 SOCKS5 以 `socks5` / `external` 策略接入。手册基线重对（2026-10-14）已完成：把那天取的整本手册（87 页；Surge Mac 6.10.0 / iOS 5.23.0，2026-10-13 发布）逐句对着兼容性清单读了一遍，项目对照的手册从此是这一版（读的结果与怎样读的见 [docs/reports/2026-10-14-manual-baseline-recheck.md](docs/reports/2026-10-14-manual-baseline-recheck.md)）；2026-10-28 的复查把它前移到 Surge Mac 6.11.0 / iOS 5.24.0（83 页不变、4 页改过，见[复查报告](docs/reports/2026-10-28-manual-6.11-recheck.md)）。读出的缺口里小而且能在回环上验证的补上了十一处——`GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`、`[IP Rewrite]` 是已知的节、旧的 `custom` 策略类型读作 `ss`、`tcp://` 的 DNS 服务器只写主机与端口、`wireguard` 行上的 `tos`、逻辑规则里的永不匹配告警、`use-local-host-item-for-proxy` 随机取地址、跟随出站模式时 DNS 会话的协议标签按 URL 的方案、`block-quic = all-proxy`、`subnet` 组的告警，和给八处已有行为补的用例——，兼容性清单改了 123 处（当时 565 行，阶段 3 / M1 之后 566 行）。读的是手册的文字：没有在 Surge 上观察过任何行为，读成“一致”的 2,439 条没有人逐条复核，阶段 3 及以后的行上手册多说的细节还没有收。随后的一轮改了一行怎样拆成各项：引号只在一个值开始的地方开启引用，括号只在配对时分组，带撇号或不配对括号的名字（`Tom's HK`、`Proxy (US`）加载得了；整个在引号里的一项是值，位置上含 `=` 的口令引起来就读得出；一个值中间成对的引号不再护住逗号（`password=ab"c,d"`），要护住就把整个值引起来；差异登记在兼容性清单 1.1 节。两份手工清单各加了回环上验不了的项（`wireguard` 的 `tos` 在线上的 DSCP 位、一行真实的旧 `custom` 策略、真实的 GeoLite2 库之下的 `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`），也都没有做过。

> **阶段 3（增强模式 / TUN）的 M1（网络环境与子网）、M2（进程识别）与 M3（虚拟网卡的内核）已完成——写这一段时 M3 只在内存里的设备上做完、测完，`rurge run` 还不接虚拟网卡；三个平台的设备与系统一侧是其后 M4 接上的（见下面两段）。**写这一段时看得到的只有几处小的：M3c 的两处——`rurge dns lookup --trace` 一类输出里 FORMERR 与 NOTIMP 现在写名字（原来写 `RCODE1`、`RCODE4`），UDP 上游给在途的同 ID 的第二个查询换一个空着的 ID（原来第二个顶掉第一个的等待者）——和 M3b 的三处：`GET /v1/traffic` 多了恒为 0 的 `listener.tun`，进程查找的 DEBUG 行的字段 `listen_port` 换成了 `remote`，`rurge run` 的 TCP 进程查找连远端地址一起比；细节见 [docs/api/phase3.md](docs/api/phase3.md)。阶段 3 的总设计与 M1、M2、M3 的细化设计、实施计划已写好（`docs/superpowers/specs/`、`docs/superpowers/plans/`）。M1 不需要虚拟网卡，做的是对今天经系统代理与代理端口进来的流量就有用的那一半：rurge 读出本机当前的网络（主网卡、默认网关、网卡类型、Wi-Fi 的名字；三个平台各读各的，Windows 与 Linux 每 2 秒比一次，macOS 听路由套接字），网络变了就清 DNS 缓存、让 `url-test` / `fallback` / `load-balance` 重新测速、让 WireGuard 换新的载体；`subnet` 策略组按当前网络选成员（可经 `POST /v1/policy_groups/select` 临时覆盖，换网络时清除），`SUBNET` 规则按当前网络匹配；`[SSID Setting]` 能在指定的网络上换 DNS 服务器，或暂停对系统代理的接管（`suspend=true`；增强模式的那一半随 M4）。**行为变化**：没有写 `default` 的 `subnet` 组现在是加载错误（此前按空组兜底）。Wi-Fi 的名字在没有位置权限的 Windows、macOS 14 起的守护进程里等处读不到，那时 `SSID:` / `BSSID:` 条件不命中、不报错，`TYPE:` 与 `ROUTER:` 照常。Linux 与 macOS 的读取在开发机上只对着假的机器测过；它们的用例在 CI 的运行 39022550224 里在 Linux 与 macOS 上编译、运行，通过了一次（其中一条在运行它的机器上真读一次本机的网络设置，只断言不变量）；手工验收清单 [docs/acceptance/phase3-manual.md](docs/acceptance/phase3-manual.md) 一项也没有做过；与 Surge 的差异登记在 [兼容性清单](docs/surge-compatibility-matrix.md)（2.3、3.1、3.4、5.1、5.2、9.4 节等），API 与日志行见 [docs/api/phase3.md](docs/api/phase3.md)。M2（进程识别）也已完成：`PROCESS-NAME` 规则现在按发起会话的进程匹配（Windows 读系统的 TCP / UDP 连接表与进程的映像路径，Linux 读 `/proc/net` 并按 inode 找进程，macOS 读 `sysctl` 的 PCB 表与 `proc_pidpath`；只读系统的表，同样不需要虚拟网卡——今天经系统代理与代理端口从本机进来的连接就有进程可查），请求记录与 `GET /v1/requests/recent`、`active` 多了 `pid` 与 `processPath` 两个键；规则走到 `PROCESS-NAME` 而进程还没查到时至多等 200 毫秒，到期按“不知道”判（会话的 `notes` 里有一句说明）；来自局域网里别的设备、虚拟机、WSL2 或容器的连接在本机没有对应的套接字，进程是“不知道”。**读法的变化**：`PROCESS-NAME` 的值以盘符（`C:\`、`d:/`）或 `\\` 开头的现在是路径（此前一律当作文件名、永不匹配）；Windows 上文件名与路径不分大小写、不去掉 `.exe`（手册只讲 Mac，这是 rurge 的读法）；内部集 `SYSTEM` 里 `trustd` 与 `netbiosd` 两条进程规则只在 macOS 上生效。进程的名字、路径与 PID 只出现在 DEBUG 日志、请求记录与 API 里。Linux 与 macOS 的真实读取在开发机上只对着夹具测过，第一次 CI 运行时核对；手工验收清单的 M2 一节一项也没有做过；差异登记在 [兼容性清单](docs/surge-compatibility-matrix.md)（3.1、3.5、9.4、10.4 节），API 与日志行见 [docs/api/phase3.md](docs/api/phase3.md)。M3（虚拟网卡的内核）做的是 `rurge run` 起虚拟网卡之后要用的那一半，在一块**内存里的设备**上做完、测完：新 crate `rurge-tun`（三个平台是同一份代码）把进来的 IP 包分到用户态的 TCP 协议栈（smoltcp，按四元组分片）、按应用源端点的 UDP 会话、ICMP 回显和 DNS 应答器，TCP 连接与 UDP 数据报经引擎现有的流水线按规则与策略走；fake-IP 池（`198.18.0.0/15` 与 `fd00:6152::/64`，名字与地址的对应存在数据目录的 `fake-ip.txt` 里）让发往 fake 地址的连接按它背后的名字走。**DNS 应答器**（`198.18.0.2` 的 53 端口，UDP 与 TCP，加上 `hijack-dns` 劫持的服务器）对 A 与 AAAA 答 fake 地址（`always-real-ip` 豁免的答真实地址），SVCB 与 HTTPS 按 `allow-dns-svcb` 拒绝，Firefox 的 canary 域答 NXDOMAIN，其余类型照 `[Host]` 的 `server:` 转发给上游；`pre-matching` 的 `REJECT` 一族在 DNS 与 SYN 两层提前拒绝（DNS 层没有连接，带端口、目的地址的条件读作"不知道"，不会因 `NOT` 被翻成拒绝一切；同一主机 30 秒内第 50 次起自动升级为 `REJECT-DROP`；同一条规则 5 分钟只在请求记录里出现一次）；经 HTTP / SOCKS5 监听器来的、目的是 fake 地址的会话也换回名字。**写这一段时没有做的**：真实的设备、地址、路由、系统 DNS 的接管与防环（M4）——它们在 M4 做了（`--enhanced-mode`），三个平台的代码都在 CI 上编译、缺省用例通过（2026-10-28，运行 40611702392，提交 `3a76b80`），但设备与系统改动没有在真机上运行过；明文 HTTP 请求经监听器带着 fake 地址来，换回名字之后请求行与 `Host` 仍是 fake 地址（阶段 4 的 HTTP 引擎再改）。开发机上的数字（一台 Windows 11 x86-64 的开发机，release 构建，内存里的设备）：单流吞吐每个方向约 5 ～ 7 Gbit/s（门槛 0.7；这是 M3a 时量的，M3c 没有复跑），一万条空闲连接加一百条活跃连接时 rurge 一侧约 402 MB（NFR-01 的 300 MB 按"达到目标的 70%"折成 429 MB 的线；保守的读法超线约 15 MB，预算要不要调留给项目所有者），fake 应答在内核里的延迟（不含设备）中位数不到 1 微秒（NFR-01 要小于 0.2 毫秒）；真实设备上的数字、真实的应用（浏览器、`dig`、`nslookup`、带 0x20 的解析器）问应答器会怎样，没有观察过。CI 没有跑过 M2、M3 的提交；手工验收清单的 M3 一节（19 项，要等 M4）一项也没有做过；差异登记在 [兼容性清单](docs/surge-compatibility-matrix.md)（4.1、6.4、9.1 节等），API 与日志行见 [docs/api/phase3.md](docs/api/phase3.md)。接下来是 M4（平台接入：三个平台的虚拟网卡、地址与路由、系统 DNS 的接管与防环）——M4、M5 与 M6 都做完了，见下面两段。

> **阶段 3 / M6（收尾与验收）也做完了（2026-10-29），阶段 3 的需求逐句对过代码了。** 做的是：把阶段 3 的需求切成 141 句（十张表），一句一行地对着手册、代码与用例的断言读一遍，结论写在 [docs/acceptance/phase3-acceptance.md](docs/acceptance/phase3-acceptance.md)；总设计第 14 节八条验收标准逐条写出 L1（内存设备与假实现上的用例）、L2（特权用例，没有运行过）、L3（手工项）与 CI 四种证据与还缺什么；手工清单核了一遍（172 项：W 50、M 39、L 42、P 41，按节 M1 37、M2 23、M3 19、M4 55、M5 19、M6 19，文件开头加了「怎么用」与第 14 节八条的索引表），兼容性清单里阶段栏含 3 的 81 行核了一遍（开工时是 80 行，核对时按 M5 的设计把 2.1 节 `block-quic` 的阶段栏改成 `2 / 3`，多出这一行）；对外的文档对齐。**结果是三个数与第四个数，要一起看**：**没有做完（已登记）5 句**（P0 没有，P1 一句——FR-IN-10 的 `tfo-behaviour`；非功能需求 4 句：NFR-01 两句、NFR-07 一句、NFR-11 一句）；**需求文字有问题 4 句**（P0 四句：`[IP Rewrite]` 的网关模式两句、FR-IN-09 的 `network-changed` 脚本、FR-IN-07 的 `$request.processPath`）；**有意不同（已登记）3 句**（P0 两句、P1 一句）；**本机没有能红的用例 3 句**（三个平台的 TUN 设备那一层）。**阶段 3 的功能需求不齐备**（那一句 P1 没有做完），所以「齐备」一词不用；**齐备也不等于验收通过**：手工验收一项也没有做，L2 的四条特权用例没有运行过，M5 与 M6 的提交 CI 没有见过（最近一次被 CI 见过的运行是 `3a76b80` 上的 40611702392），八条验收标准里没有一条能因为内存设备上的用例全绿就写成“通过”。读的代码是 `7c5753e`，之后补的用例在其后的三个提交里（`5e00564`、`fcf9464`、`37616a0`）；全量门禁那次：86 个结果行、4723 通过、0 失败、13 忽略（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）。差异登记在兼容性清单，API 与日志行见 [docs/api/phase3.md](docs/api/phase3.md)。

> **阶段 3 / M4（平台接入）也做完了（2026-10-27）：三个平台的增强模式都有了代码。** Windows 用 Wintun（要一个 WireGuard LLC 签名的 `wintun.dll` 0.14 或更新，从 <https://www.wintun.net/> 取，放在 `rurge.exe` 旁边或用 `--wintun-dll` 指出；取与 `rurge.exe` 同一架构的那一份，x86-64 是压缩包里的 `bin/amd64`；增强模式要以管理员运行），Linux 用 `/dev/net/tun`（root），macOS 用 utun（root）；`--enhanced-mode`（环境变量 `RURGE_ENHANCED_MODE`）与 `POST /v1/features/enhanced_mode` 打开与关闭，不跨重启保留。照代码会做的是：建虚拟网卡、配地址与 MTU 1500、装路由（八个 IPv4 前缀、八个 IPv6 前缀加 `64:ff9b::/96`，减去回环、链路本地、组播与广播（Windows 上另减 `240.0.0.0/4`，Windows 不往这一段发包）；`ipv6-vif` 的三种取值，`tun-excluded-routes` / `tun-included-routes` 与 `external` 的 `addresses` 都算在其中）、接管系统 DNS（Linux 只在 `/etc/resolv.conf` 是 systemd-resolved 的存根时，macOS 逐个启用的网络服务，Windows 只改虚拟网卡那一份）并在关闭或崩溃之后的下一次启动把它们还回去（macOS 的 DNS 与 Linux 的 `rp_filter` 进 `state.json` 的备份）、Linux 上按需把 `net.ipv4.conf.all.rp_filter` 放宽到 2、防环（rurge 自己的连接绑出口的网卡，Windows 另加 `IP_UNICAST_IF` / `IPV6_UNICAST_IF` 与虚拟网卡自己的转发与 weak host 设置）、外部程序一律 DIRECT（Windows 经 Job Object）。`ping` 经虚拟网卡、接管暂停的增强模式一半、与另一个全隧道 VPN 前缀相撞时打开失败，也都是代码里的事。**状态**：三个平台的代码都在 CI 上编译、缺省的用例通过（2026-10-28，运行号 40611702392，提交 `3a76b80`；Linux 与 macOS 专有的代码——`tun/linux.rs`、`tun/macos.rs` 与挑平台的 `cfg`——是第一次被编译），但缺省的用例不建网卡、不改系统设置，设备与系统改动在三个平台上都没有真的运行过；**三个平台都没有在真机上验证**；手工验收清单（[docs/acceptance/phase3-manual.md](docs/acceptance/phase3-manual.md)）的 M4 一节（W19 ～ W41、M17 ～ M30、L18 ～ L32、P29 ～ P31）一项也没有做过；阶段 3 的需求还没有逐句对过代码，所以阶段 3 不称齐备。与其他 VPN 共存、非直连的 VPN 路由、没有 resolved 的 Linux、Windows 的多宿主与浏览器的 DoH 等限度登记在 [兼容性清单](docs/surge-compatibility-matrix.md) 的 9.1 节与 6.3 节，API 与日志行见 [docs/api/phase3.md](docs/api/phase3.md) 的「M4」～「M4d」几节。

> **阶段 3 / M5（识别、阻断与 `[IP Rewrite]`）也做完了（2026-10-28），阶段 3 的六个里程碑只剩 M6（收尾与验收）。** 做的是：UDP 流在**所有端口**上识别 QUIC（先过包头筛，再用自己的 Destination Connection ID 的 Initial 密钥解密确认；认 v1、v2 与草案 29 ～ 32，不认 gQUIC 与草案 28 以前）与 STUN；`block-quic` 的 `adaptive`（先挡，1 秒内等到同名主机的客户端 TCP 算能回落、QUIC 一直挡住，否则从那时起放行；每个引擎至多记 200 个名字、按最近用到淘汰）与手册 6.11 的 `auto`（**缺省行为变了**：`DIRECT` 不挡、不载 UDP 的代理挡、其余自适应；此前代理一律挡、直连不挡）；`[IP Rewrite]` 第一次生效（`reflect` 只对调两个地址、端口与其余字节不动；`reject` 对不带 ACK 的 SYN 回 RST、对其余回 ICMP“管理上禁止”、RST、ICMP 差错（含 ICMPv6 重定向）、非首片、带分片首部之外的 IPv6 扩展首部的包与首部读不出的 TCP 什么也不回；`drop` 悄悄丢弃；命中的包不进协议栈、规则与请求记录，计数与日志见 API 文档）；`udp-priority` 落到写回虚拟网卡的两条队列上（UDP 的回包不排在 TCP 后面）；开着 `encrypted-dns-follow-outbound-mode` 时 DNS 服务器自己的连接带主机名，按主机名写的域名规则第一次能匹配它（经代理时，代理收到的仍是引导解析出的地址）；手册 6.11 的 WireGuard 服务端写法（`listen-port`、不带 `endpoint` 的 peer）能加载，并说清这一版不接受连入；三个基准重跑（一台 Windows 开发机、release：B1 八个量出来的行都过 0.7 Gbit/s 的线，最低的一行 4.065 Gbit/s；B2 门槛读数 403.7 MB（线 429 MB）、保守读数 445.3 MB、超线 16.3 MB（归项目所有者）；b3 中位数 0.9 微秒、p99 2.9 微秒、最大 4.5 毫秒）。**状态**：M5 的提交都在本地、CI 没有见过（最近一次被 CI 见过的运行是 `3a76b80` 上的 40611702392）；`[IP Rewrite]` 只在内存设备上证明过，真机上送回的包怎样被操作系统回应要看手工验收；`udp-priority` 的效果没有观察过；手工验收清单（[docs/acceptance/phase3-manual.md](docs/acceptance/phase3-manual.md)）的 M5 一节（W42 ～ W47、M31 ～ M36、L33 ～ L38、P32）这一轮才写，一项也没有做过；**阶段 3 仍不称齐备**（需求还没有逐句对过代码）。差异登记在兼容性清单的 1.2、2.1、3.1、4.3、4.6、6.1、6.2、7.2 与 9.10 节，API 与日志行见 [docs/api/phase3.md](docs/api/phase3.md) 的「M5」一节；下一步是 M6（阶段 3 的需求逐句对照代码、验收文档与手工验收）。

完整的需求、模块划分、平台差异和分阶段路线图见 [docs/requirements.md](docs/requirements.md)；
Surge 配置项 / 规则 / 参数 / API 的逐项兼容清单见 [docs/surge-compatibility-matrix.md](docs/surge-compatibility-matrix.md)。

### 计划特性

| 模块           | 内容                                                                                                                                                          | 阶段  |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| 配置与 Profile | Surge`.conf` 解析、`[General]` 全部选项、托管配置自动更新、`.sgmodule` 模块、Requirement 表达式、Keystore（`.conf` 解析与校验、Requirement 表达式（四个平台变量取自操作系统——macOS 与 Linux 的读取在 CI 的运行 39022550224 里编译并通过了一次，真实的 Mac 上读到什么待核对——，`#!WINDOWS-ONLY` / `#!LINUX-ONLY` 两个扩展简写可用，阶段 1 补遗（M5a））与 Keystore 已实现，阶段 1 / 2；托管配置的自动更新（先检查再采用，`strict` 到期未更新成功时拒绝新会话）与远程 `#!include` 已实现，阶段 1 补遗（M5b），与 Surge 的差异登记在兼容性清单 1.3 节，见「当前状态」；`.sgmodule` 模块在阶段 5）                                               | 1 / 5 |
| 入站           | HTTP / HTTPS 代理、SOCKS5、局域网共享与认证、系统代理设置（HTTP / SOCKS5 监听、Basic 认证、`proxy-restricted-to-lan` 已实现，M3a；三平台系统代理开关、`skip-proxy` 转换、退出 / 崩溃恢复已实现，M4b）                                                                                                     | 1     |
| 规则系统       | 域名 / IP / GEOIP / IP-ASN / HTTP / 进程 / 源与端口 / 协议与网络 / 逻辑 / 脚本 / 规则集 / FINAL（规则引擎 / 规则集 / GeoIP 已实现，M2a；`pre-matching` 优先、子规则命中日志、同一资源不可同时作两种集的检查已实现，阶段 1 补遗（M5a）；写在第一条启用的 `FINAL` 之后的规则不再评估，读法是按手册推断的、没有在 Surge 上观察过，阶段 1 补遗（M5b）；`SUBNET` 规则按当前网络匹配已实现，阶段 3 / M1；`PROCESS-NAME` 按发起会话的进程匹配已实现，阶段 3 / M2；`PROTOCOL` 的 `QUIC` 与 `STUN` 在所有端口上识别已实现，阶段 3 / M5） | 1 / 3     |
| DNS            | 普通 DNS、DoH / DoT / DoQ / DoH3、本地映射、劫持、fake-ip、always-real-ip（普通 DNS / DoH / DoT / `tcp://` / `[Host]` / 系统 hosts 已实现，M2b；DoQ / DoH3 已实现，阶段 2 / M7c；开了 `encrypted-dns-follow-outbound-mode` 时 DNS 服务器自己的连接带主机名、按主机名写的规则能匹配它，阶段 3 / M5）                | 1 / 3 |
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP（TCP）与 SOCKS5（含 `socks5-tls`，TCP 与 UDP）上游及 `underlying-proxy` 链已实现，阶段 2 / M1、M5a；Trojan（TCP 与 UDP；WebSocket 传输）已实现，阶段 2 / M2a、M5b；VMess（AEAD 握手，TCP 与 UDP；可叠加 TLS / WebSocket）与 AnyTLS（TCP 与 UDP，会话复用）已实现，阶段 2 / M2b、M5b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP 与 UDP，用户态隧道）已实现，阶段 2 / M4b、M5c；外部程序（TCP 与 UDP，三平台）已实现，阶段 2 / M4c、M5a；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v1–v5（TCP，v3 起有 UDP；`reuse`、obfs）已实现，阶段 2 / M6b、M8b；Snell v6 与 `tailscale` 策略不实现；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c；Hysteria 2（TCP 与 UDP、Salamander / Gecko 混淆、端口跳跃）已实现，阶段 2 / M7a；TUIC v5 与 v4（TCP 与 UDP、端口跳跃）已实现，阶段 2 / M7b；MASQUE（TCP 与 CONNECT-UDP、端口跳跃）与 Trust Tunnel 的 HTTP/3（`h3=true`）已实现，阶段 2 / M7c） | 2     |
| 策略组         | select / url-test / fallback / load-balance / smart / subnet、策略引入与订阅、延迟测试（`select` 组的选择经 API 读取与切换已实现，阶段 2 / M1；策略引入与订阅、组级 `underlying-proxy` 已实现，阶段 2 / M3a；url-test / fallback / load-balance、延迟测试与临时覆盖已实现，阶段 2 / M3b；smart 已实现，阶段 2 / M3c；subnet 组按当前网络选成员、没写 default 是加载错误，已实现，阶段 3 / M1）                                                                        | 2 / 3     |
| 增强模式       | 虚拟网卡（Wintun / tun / utun）、UDP、路由包含与排除、虚拟网卡的内核（用户态的 TCP / UDP / ICMP、fake-IP 池、DNS 应答器、预匹配在 DNS 与 SYN 两层、经监听器来的 fake 地址；在内存里的设备上做完、测完，阶段 3 / M3）、进程识别（`PROCESS-NAME` 按发起会话的进程匹配、请求记录带进程，已实现，阶段 3 / M2）、子网设置（网络环境探测与子网设置——`[SSID Setting]` 的 DNS 覆盖与接管暂停——已实现，阶段 3 / M1）、三个平台的设备与系统一侧（虚拟网卡、地址与 MTU、路由、系统 DNS 的接管与交还、Linux 的 `rp_filter`、防环、权限；有代码，阶段 3 / M4；三个平台的代码都在 CI 上编译、缺省用例通过（运行 40611702392），但设备与系统改动没有真的运行过，三个平台都没有在真机上验证）、识别与阻断（所有端口上的 QUIC 与 STUN、`block-quic` 的 `adaptive` 与 6.11 的 `auto`）、`[IP Rewrite]`（`reflect` / `reject` / `drop`）、`udp-priority` 的两条写回队列、手册 6.11 的 WireGuard 服务端写法（阶段 3 / M5；只在内存设备与单元用例里验证过，没有在真机上验证）；验收：需求逐句对照过代码（141 句，三个数与第四个数见「当前状态」）、第 14 节八条验收标准的证据与两份清单的核对（阶段 3 / M6），手工验收一项也没有做                                                                                      | 3     |
| HTTP 处理      | MITM（HTTPS 解密）、URL / Header / Body 重写、Map Local、请求查看与抓包                                                                                       | 4     |
| 脚本与模块     | JavaScript 引擎、完整 Surge 脚本 API、模块系统、信息面板                                                                                                      | 5     |
| API 与工具     | Surge 兼容 HTTP API、Web Dashboard、Logbook、延迟 / 基准测试、CLI（阶段 1 端点与 `rurge reload/stop/status` 已实现，M4a；`rurge service install/uninstall`（基础版）已实现，M4b；外部资源的 `GET /v1/external_resources` 与 `POST /v1/external_resources/update` 已实现，阶段 1 补遗（M5b））                                    | 6     |
| 高级网络       | 网关模式、DHCP 服务器、端口转发、内置 Snell / MTProto 服务器                                                                                                  | 7     |
| 桌面 GUI       | 跨平台桌面客户端、URL Scheme                                                                                                                                  | 8     |

### 路线图

现在：阶段 2 收尾（CI 在 2026-10-26 第一次三个平台全绿；手工验收清单还没有执行），阶段 1 / M5a、阶段 1 / M5b、手册基线重对与随后的一行怎样拆成各项已完成——项目对照的手册现在是 Surge Mac 6.11.0 / iOS 5.24.0 的手册（2026-10-28 的复查：目录里照旧 87 页，其中 83 页与 2026-10-14 的基线逐字节相同、4 页改过，见 [复查报告](docs/reports/2026-10-28-manual-6.11-recheck.md)），整本手册已经对着兼容性清单读过一遍，那一遍读出的一行怎样拆分的问题也改掉了；阶段 3 的 M1（网络环境与子网）、M2（进程识别）、M3（虚拟网卡的内核，只在内存里的设备上）、M4（平台接入：三个平台的虚拟网卡、地址与路由、系统 DNS、防环）、M5（识别、阻断与 `[IP Rewrite]`）与 M6（收尾与验收）已完成；M4 的三个平台都没有在真机上验证，M5 只到内存设备与单元用例，阶段 3 的需求逐句对照过代码（141 句）但功能需求不齐备（P1 有一句没有做完），手工验收一项也没有做，M5 与 M6 的提交 CI 没有见过；接下来是阶段 4（HTTP 引擎）。

1. **阶段 0** 需求与文档（已完成）
2. **阶段 1** 核心骨架：配置解析、HTTP / SOCKS5 入站、DIRECT / REJECT、规则引擎、DNS、CLI、HTTP API 骨架、系统代理、服务安装（M1 ～ M4b 已合并，代理端到端可用；HTTP API 阶段 1 端点与 `rurge reload/stop/status` 已完成，M4a；系统代理与 `rurge service install/uninstall`（基础版）已完成，M4b；阶段 1 / M5a 补上了十来处只做到解析或没有做的条款；阶段 1 补遗（M5b）做了托管配置的更新、远程 `#!include` 与外部资源的手动更新，并把阶段 1 的需求逐句对了一遍代码——结果与它没有覆盖的东西见「当前状态」和 [docs/acceptance/phase1-acceptance.md](docs/acceptance/phase1-acceptance.md)；阶段 1 的手工验收清单一行也没有做过；CI 在 2026-10-26 第一次全绿）
3. **阶段 2** 出站协议全集、策略组、策略订阅（功能已完成，验收未完成——三平台 CI 在 2026-10-26 第一次全绿（运行号 40283632986，提交 `ac0a98a`；上一次运行 39022550224 读出的六类失败修了之后这一次都通过，第六类的原因仍不知道；互操作用例对真实的二进制在这次运行里都通过）；手工验收清单还没有执行：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成；M7a（QUIC 地基与 Hysteria 2）已完成；M7b（TUIC）已完成；M7c（HTTP/3 族与 DoQ / DoH3）已完成，M7 至此完成；M8（收尾与验收）已完成，M8a 任务 6 的修正在 2026-10-26 的运行里通过）
4. **阶段 3** 增强模式（TUN）、fake-ip、进程识别（已开始：总设计已写好，六个里程碑里 M1 网络环境与子网——`subnet` 组、`SUBNET` 规则、`[SSID Setting]`——与 M2 进程识别——`PROCESS-NAME` 按发起会话的进程匹配——与 M3 虚拟网卡的内核——用户态 TCP / UDP / ICMP、fake-IP 池、DNS 应答器、预匹配在 DNS 与 SYN 两层，只在内存里的设备上——已完成；M4 平台接入——三个平台的虚拟网卡、地址与路由、系统 DNS 与防环——也已完成（有代码，三个平台都没有在真机上验证，手工验收清单的 M4 一节一项也没有做过）；M5 识别、阻断与 `[IP Rewrite]`——所有端口上的 QUIC 与 STUN、`block-quic` 的 `adaptive` 与 6.11 的 `auto`、`[IP Rewrite]`、`udp-priority`、内部 DNS 会话带主机名、手册 6.11 的 WireGuard 服务端写法——也已完成（只在内存设备与单元用例里验证过，提交在本地、CI 没有见过，手工验收清单的 M5 一节一项也没有做过）；M6 收尾与验收也已完成（需求逐句对过 141 句、八条验收标准的证据、两份清单的核对、对外的文档；功能需求不齐备，P1 有一句没有做完，手工验收一项也没有做，M5 与 M6 的提交 CI 没有见过）；接下来是阶段 4（HTTP 引擎））
5. **阶段 4** HTTP 引擎：MITM、重写、Map Local、抓包
6. **阶段 5** 脚本引擎与模块系统
7. **阶段 6** HTTP API 全量兼容、Web Dashboard、测试与诊断工具
8. **阶段 7** 网关 / DHCP / 端口转发 / 内置服务器
9. **阶段 8** 桌面 GUI

近期非目标：iOS / tvOS 版本、Surge Ponte（依赖 iCloud）、Apple 平台专属界面；Tailscale 的原生集成已经评估过（[可行性报告](docs/reports/2026-10-08-phase2-feasibility-snell-v6-quic-tailscale.md)）：现在经本机 tailscaled 的 SOCKS5 以 `socks5` / `external` 策略接入，原生支持等官方 Rust 库成熟后复评。

### 快速开始（计划中的形态）

> `rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge run`（HTTP / SOCKS5 代理，DIRECT / REJECT，以及阶段 2 的全部出站协议上游（见上面的特性表）——TCP 类的可叠加 Shadow TLS，含 `underlying-proxy` 链）已可用；`select` / `url-test` / `fallback` / `load-balance` / `smart` / `subnet` 组已可用。HTTP API 与 `rurge reload` / `stop` / `status` 已可用（见 [docs/api/phase1.md](docs/api/phase1.md)，阶段 2 新增端点见 [docs/api/phase2.md](docs/api/phase2.md)）。`rurge run --system-proxy` 可以把系统代理指向 rurge，退出时恢复、崩溃后在下次启动时恢复；`rurge service install | uninstall [--user] [--dry-run]` 可以注册 / 移除开机自启（systemd / launchd / Windows 计划任务）。macOS 经 `networksetup` 设置，通常需要管理员账户；是否需要 `sudo` 尚未在真机验证（见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)），失败时 rurge 原样报出工具的错误。开增强模式（`--enhanced-mode`）三个平台都要提权：Windows 上要以管理员运行，并准备一个 WireGuard LLC 签名的 `wintun.dll`（0.14 或更新，从 <https://www.wintun.net/> 取；要取与 `rurge.exe` 同一架构的那一份，x86-64 是压缩包里的 `bin/amd64`），放在 `rurge.exe` 旁边或用 `--wintun-dll <PATH>`（环境变量 `RURGE_WINTUN_DLL`）指出；Linux 与 macOS 上要 root。`rurge run` 另支持 `--idle-timeout`、`--request-log-size`、`--watch`（配置热重载）、`--log-file`（按天滚动）、`--empty-group-reject`（没有成员的策略组拒绝而不是直连）、`--listen-http` / `--listen-socks5`（整项替换配置里的监听地址）等 rurge 专有运行时选项，只经命令行参数 / 环境变量提供，不写入 Surge 配置文件。Profile 里写了 `#!MANAGED-CONFIG` 或远程 `#!include`、而数据目录里还没有下载到它们的内容时，`rurge run` 启动时先等第一次下载（标准错误上有一行说明，至多 60 秒；`--no-network` 时不等）；`rurge check`、`rurge rule match`、`rurge dns lookup` 与 `rurge reload` / `stop` / `status` 只读已经下载到的内容，从不为 Profile 联网（`reload` / `stop` / `status` 另有 `--data-dir`）。

```bash
# 构建
cargo build --release

# 校验配置
rurge check -c config.conf

# 离线测试某个请求会命中哪条规则、落到哪个策略（无需启动守护进程）
rurge rule match -c surge.conf www.example.com --explain

# 按配置的 DNS 设置离线解析一个域名
rurge dns lookup -c surge.conf www.example.com --trace

# 运行
rurge run -c config.conf

# 运行，并把系统代理指向 rurge（退出时恢复；崩溃后在下次启动时恢复）
rurge run -c config.conf --system-proxy

# 先看清注册开机自启会做什么（去掉 --dry-run 才真正安装）
rurge service install -c config.conf --user --dry-run
```

配置文件直接采用 Surge 格式：

```ini
[General]
loglevel = notify
dns-server = 223.5.5.5, 119.29.29.29
encrypted-dns-server = https://dns.alidns.com/dns-query
http-listen = 127.0.0.1:6152
socks5-listen = 127.0.0.1:6153
skip-proxy = 127.0.0.1, 192.168.0.0/16, 10.0.0.0/8, localhost, *.local
exclude-simple-hostnames = true
http-api = password@127.0.0.1:6171
http-api-web-dashboard = true

[Proxy]
Proxy-A = ss, example.com, 8388, encrypt-method=chacha20-ietf-poly1305, password=secret
Proxy-B = trojan, example.org, 443, password=secret, sni=example.org

[Proxy Group]
Auto = url-test, Proxy-A, Proxy-B, interval=600, tolerance=100
Select = select, Auto, Proxy-A, Proxy-B, DIRECT

[Rule]
DOMAIN-SUFFIX,example.com,Select
RULE-SET,SYSTEM,DIRECT
RULE-SET,LAN,DIRECT
GEOIP,CN,DIRECT
FINAL,Select,dns-failed
```

### 文档

- [需求文档（PRD）](docs/requirements.md)
- [Surge 兼容性清单](docs/surge-compatibility-matrix.md)
- [阶段 1 需求逐句对照代码](docs/acceptance/phase1-acceptance.md)（阶段 1 的 416 句需求每一句的结论与证据；结果是三个数，要一起看，见「当前状态」）
- [阶段 2 验收清单](docs/acceptance/phase2-acceptance.md)（九条标准的证据与缺口；需求逐条对照代码的结果）
- 各阶段的设计文档与实施计划将放在 `docs/superpowers/specs/` 与 `docs/superpowers/plans/`

### 开发

- 工具链：Rust stable（通过 `rustup` 安装），`cargo fmt`、`cargo clippy`、`cargo test`
- 工作流：每个阶段先写设计文档，再写实施计划，再按计划实现并附测试
- 贡献前请先阅读需求文档，并在 Issue 中讨论较大的改动

### 免责声明

- rurge 是独立的开源项目，与 Surge 的开发者 NSSurge 没有任何关系；「Surge」是其所有者的商标，本项目仅用于描述兼容性。
- 本项目不提供任何代理服务器或网络服务，请遵守所在地区的法律法规使用。
- MITM（HTTPS 解密）等功能只应用于自己拥有或获得授权的设备与流量。
- GEOIP / IP-ASN 规则默认使用 MaxMind 的 GeoLite2 数据库：This product includes GeoLite2 data created by MaxMind, available from <https://www.maxmind.com>。

### 许可证

[MIT](LICENSE)
