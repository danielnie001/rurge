# 测试：命令、只供测试的后端与测试约定

`CLAUDE.md` 的「常用命令」与「测试约定」是本文的要点；改一条测试规矩，两处一起改。

本文原是 `CLAUDE.md` 的「常用命令」一节，2026-10-26 原样挪来：命令行一行不改，三个只供测试的后端各一段，测试约定按原文的先后分成几段、按句换行，文字一字未改。文中的"状态一段"今天在 `docs/status.md`，"命令一节"就是本文。

## 命令

```bash
cargo test --workspace                          # 全部测试
cargo test -p rurge-config <name>               # 单个 crate / 用例
cargo test -p rurge-api                         # HTTP API 集成测试（鉴权、封禁、阶段 1 端点）
cargo test -p rurge-proto                       # 出站协议库：TLS 层、p12、http / socks5 出站对回环假上游（rurge_proto::testing）、trojan 出站与 WebSocket 层
cargo test -p rurge-proto flushed               # http / socks5 出站的握手每一步写完先 flush 再等应答（下层是会缓冲的一跳时，请求不必等到下一次写才发出）；握手期间代理关了连接的一句话与下一跳带话的失败原样通过的用例在 `cargo test -p rurge-proto http`
cargo test -p rurge-proto vmess                 # vmess：KDF / 头密封 / 分块的向量，出站对回环假服务端（FakeVmess）；UDP 载体在两个任务里轮询 `LazyHead` 的两半（读任务在拨号时起，写在 `send_to` 的任务里），第一个数据报的写被堵过了宽限时 `send_to` 照样及时完成（`a_first_datagram_whose_write_is_blocked_past_the_grace_is_still_sent`，按时钟量）
cargo test -p rurge-proto lazy_head             # 惰性请求头：两半在不同的任务里轮询时互相唤醒（两格的 `Both`），读端欠着的 flush 由它下一次读或写端完成的 flush / shutdown 了结，一条用真实的管道在两个任务里复现 CI 第二次运行第 1 类的用例
cargo test -p rurge-proto anytls                # anytls：padding 方案解析、会话层，SYNACK 看门狗（复用的会话上新流 3 秒内没有应答就关会话；会话的第一条流、版本 1 的对端不受看管，UDP 流照样起表），出站对回环假服务端（FakeAnyTls）
cargo test -p rurge-engine --test outbounds     # 经真实 http / socks5 出站的端到端用例（回环假上游）
cargo test -p rurge-engine --test outbounds_shadow_tls   # 经 Shadow TLS 的端到端用例（回环假服务端 + 夹具自己的伪装站点）
cargo test -p rurge-engine --test subscriptions   # 订阅：构建时同步载入、热重建与出站复用、经导入 / 派生成员出站、空组兜底
cargo test -p rurge-engine --test auto_groups     # 自动组：拨号触发的测试轮次、三种选法、evaluate-before-use、测试会话、临时覆盖与重载
cargo test -p rurge-policy probe                # 探针：两次 HEAD、HTTPS、不保持连接时的退化（回环 TestServer）
cargo test -p rurge-policy assemble             # 成员装配：顺序、过滤 / 前缀 / 修饰、全局重名、中继派生、组环
cargo test -p rurge-policy smart                # smart：打分、排序与重试列表、站点记忆、使用计数、抽样（时间由调用方注入）；失败计分：同一个失效站点一小时内只罚一次（"看"与"记"同一次加锁）、N 个不同的失效地址仍罚 N 次
cargo test -p rurge-engine --test smart         # smart 端到端：换成员与单次时限、首字节 / 3 秒无响应 / 上游先断的回报、抽样轮、覆盖与测试端点、重载清理、跟随出站模式下内部 DNS 会话（TCP 与 QUIC 载体）的回报（含载体发不出去时立刻计一次失败）、一阵会话打到同一个失效站点只罚一次、发不出去的名字不记入任何成员的失败、UDP 流在同一个失效地址上失败只罚一次
cargo test -p rurge-proto-ssh                   # ssh：私钥解码、主机密钥比对、会话与通道、断线重建、空闲断开与保活，出站对回环假服务端（FakeSsh）
cargo test -p rurge-engine --test outbounds_ssh   # 经 ssh 出站的端到端用例：目标名交给服务器解析、Keystore 私钥与指纹、经 Shadow TLS、经 SSH 的测速
cargo test -p rurge-proto-wireguard             # wireguard：路由表、client-id、协议栈（内存里的对端）、隧道与流、隧道内 DNS、生命周期（隧道只有一种结束方式 `Ending`、等待者的两句话、意外结束的 warn、协议栈的锁中毒与设备任务 panic、共用的隧道归最新的配置）、原生测速，出站对回环假对端（FakeWgPeer）
cargo test -p rurge-engine --test outbounds_wireguard   # 经 wireguard 出站的端到端用例：回显、[Host] 解析、经 underlying-proxy（socks5）的隧道与底层不载 UDP 时的失败、重载沿用与替换、握手测速与经隧道的 URL / UDP 测速、经隧道的 UDP
cargo test -p rurge-proto-wireguard --release throughput -- --ignored --nocapture   # WireGuard 吞吐基准（经回环假对端双向回显 64 MiB；不作门禁）
cargo test -p rurge-engine --release --lib relay_throughput -- --ignored --nocapture   # 转发循环吞吐基准（四个栈 × 四种缓冲 × 8 轮，上行再下行在同一条会话上；不作门禁；整张表约 26 ～ 27 分钟，RELAY_BENCH_MIB=<n> 缩小每轮的字节数）
cargo test -p rurge-engine --release --test bench -- --ignored --exact <name> --nocapture --test-threads=1   # 阶段 4 / M5b 的基准（B1 ～ B5 与 Q1，十六个忽略的用例；`--test bench` 是引擎的第二处 `-p` 例外，同 `relay_throughput`；不作门禁；每个单独一个进程（B5 读进程的峰值，一个进程只量一项）；跑之前看放 `target/` 的盘剩 20 GiB 以上；`HTTP_BENCH_QUICK=1` 在调试构建里缩小规模、只看跑不跑得通；读数在 `docs/status.md` 的阶段 4 / M5b 一节）
cargo test -p rurge-engine --lib relay          # 转发循环：每次写后恰好一次 flush、半关闭前先 flush、停着的写不被别的事插队、flush 永不完成时 stop 仍能打断、空闲超时、一个方向出错时另一个随之结束
cargo test -p rurge-engine --test udp          # UDP 流水线：SOCKS5 UDP ASSOCIATE → DIRECT / REJECT / socks5（含链）、全锥、QUIC 阻断、不支持 UDP 的策略、关联结束与流数上限
cargo test -p rurge-engine --test udp_tls_family   # 经 trojan / anytls（全锥）与 vmess（每个目标一条连接、对称）的 UDP 端到端用例（回环假服务端）
cargo test -p rurge-policy udp_probe           # UDP 测试：A 记录的问题、计时、静默与不载 UDP
cargo test -p rurge-proto shadowsocks           # shadowsocks：KDF 与分块的向量、SS 2022 头与身份头、UDP 包与防重放窗口，出站对回环假服务端（FakeShadowsocks，含两种 obfs；11 种流式方法的 TCP 与 UDP）
cargo test -p rurge-proto shadowsocks::stream   # ss 的流式方法：AES-CFB / AES-CTR（含 128 位进位）/ RC4 / ChaCha20 / Salsa20 的已知答案、任意切分下的续接、密钥流到头时出错而不 panic、IV 的收发（rc4 没有 IV）、停着密文时的 flush / shutdown、数据报的封与解
cargo test -p rurge-proto obfs                  # simple-obfs：http 请求头与 tls ClientHello 的布局、应答的读取，对回环假服务端的往返
cargo test -p rurge-engine --test outbounds_shadowsocks   # 经 ss 出站的端到端用例：各种方法（含流式方法的 TCP 与 UDP）、两种 obfs、时钟偏差、手册不再列出的方法名的 REJECT、经 underlying-proxy、UDP 与 udp-port、全锥、重载沿用与替换
cargo test -p rurge-proto snell                 # snell：Argon2id 与帧的已知答案、请求头、复用池与失效重试、UDP 数据报，出站对回环假服务端（FakeSnell：v1 ～ v5，含 obfs http / tls）；v2 的恒复用、v3 的 UDP、"没有应答"的三句文本（关闭 / 重置 / salt 已到）
cargo test -p rurge-proto snell::chunked        # snell v1 ～ v3 的分块记录流：两种密码的已知答案、任意切分、至多 0x3FFF 的块、零长度块与同一条流上的下一个请求、长度的保留位、v4 的头立刻解不开
cargo test -p rurge-engine --test outbounds_snell   # 经 snell 出站的端到端用例：v1 ～ v5、reuse 与 v2 的恒复用、obfs http / tls、拒绝与 psk 错、不写 version 的行对别的版本的服务端、服务端重置、v6 的 REJECT、经 underlying-proxy、UDP（v3 起；v1 / v2 不载）、全锥、重载沿用与替换
cargo test -p rurge-proto h2                    # HTTP/2 族：会话池（流控、max-streams、GOAWAY、空闲关闭、extended CONNECT）、h2-connect 的 TCP 与 CONNECT-UDP（capsule、masque 路径），出站对回环假服务端（FakeH2Proxy）；trust-tunnel 的用例用过滤词 trust_tunnel
cargo test -p rurge-engine --test outbounds_http2   # 经 h2-connect / trust-tunnel 的端到端用例：凭据与拒绝、max-streams、trust-tunnel 不载 UDP、CONNECT-UDP、没有 extended CONNECT、经 underlying-proxy、重载沿用与替换；trust-tunnel 的 h3=true（HTTP/3、max-streams 按每条 QUIC 连接、重载、与 Shadow TLS 的加载错误）
cargo test -p rurge-net quic                    # QUIC 共用部分：UDP 层（直连、tos、ecn=off、链式载体——载体此刻发不了时等而不丢、载体失败时拨号以它的原话失败——、端口跳跃）、拨号与传输参数（含不保活的 Transport::DNS）、错误文本（载体记下发送失败并交给接收方、载体结束时的原话、载体自己因 reset 而结束时以 ConnectionAborted 交出的用例用过滤词 packet_datagram）、经载体的连接从 1350 字节的包起步而直连的不（`only_a_connection_over_a_carrier_starts_from_the_carried_size`）、载体报出的 ConnectionReset 不算它的失败（`a_reset_is_not_the_carriers_failure`）；自写的 HTTP/3 客户端（QPACK 静态表与 Huffman、帧、SETTINGS / GOAWAY、可取消安全的写、extended CONNECT 与 HTTP Datagram、content-length 比对、被拒与 GOAWAY 的信号）对回环 H3Server
cargo test -p rurge-proto-quic                  # QUIC 会话（单飞、空闲关闭、失效重试、跟随底层策略、每条连接的流数上限与有上限时的失效重试）与 hysteria2（认证、TCP、UDP 分片与重组、Salamander / Gecko 与 BLAKE2b-256 的已知答案），出站对回环假服务端（FakeHysteria2）
cargo test -p rurge-engine --test outbounds_hysteria2   # 经 hysteria2 出站的端到端用例：TCP 与 UDP、口令错误、Hysteria-UDP: false、两种混淆、端口跳跃、经 underlying-proxy（socks5）与底层组换成员、组中继里跳端口的成员被略去、测速、重载沿用与替换
cargo test -p rurge-proto-quic --release throughput -- --ignored --nocapture   # Hysteria 2 吞吐基准（经回环假服务端双向 64 MiB，明文与 Salamander；不作门禁）
cargo test -p rurge-proto-quic tuic             # tuic：v5 / v4 的命令与地址、导出密钥的认证与 BLAKE3 token 的已知答案、TCP（v4 的应答）、UDP 的两种模式（v5 的分片与重组、写任务）与 Dissociate、心跳、关闭码与认证失败、失效连接的重试、端口跳跃，出站对回环假服务端（FakeTuic）
cargo test -p rurge-engine --test outbounds_tuic   # 经 tuic / tuic-v5 出站的端到端用例：TCP、UDP 的两种模式（v5 分片、v4 放不下就丢）、口令与 token 错误、端口跳跃、经 underlying-proxy（socks5）、测速、重载沿用与替换
cargo test -p rurge-proto-quic masque           # masque：经典 CONNECT 的隧道与每个 CONNECT 的 Basic 凭据、CONNECT-UDP（每个目标一条流、HTTP/3 数据报与流上收到的 capsule、能力检查、超长的包、畸形的 2xx、空闲 FIN、GOAWAY 与换连接、路由比较）、被拒（H3_REQUEST_REJECTED）或在 GOAWAY 界限之上的请求换连接重试、读时推进写了一半的帧、每条连接 256 MiB 的窗口下读端停住的隧道不拖住其余的（`tunnels_nobody_reads_leave_the_others_their_window`）、经载体的连接退回 1200 字节仍通（`a_tunnel_gets_through_a_path_that_cannot_carry_the_first_size`），出站对回环假服务端（FakeH3Proxy）
cargo test -p rurge-proto-quic trust_tunnel     # trust-tunnel 的 HTTP/3：经典 CONNECT 与 user-agent、headers、407、max-streams 按每条 QUIC 连接（会话的流数上限另见 session::tests::under_a_cap）、没人读的隧道不占光连接的接收窗口（窗口 ⌈max-streams × 8 MiB × 8/7⌉、至少 20 MiB：`the_last_tunnel_goes_on_beside_all_the_others_unread`）、GOAWAY、重启后的重试，出站对回环假服务端（FakeH3Proxy 扮 TrustTunnel endpoint）
cargo test -p rurge-engine --test outbounds_masque   # 经 masque 出站的端到端用例：TCP 与 CONNECT-UDP、口令错误、服务端不支持 CONNECT-UDP、超长的包只丢那一个、端口跳跃、经 underlying-proxy（socks5）、URL 测速与 UDP 测试、重载沿用与替换
cargo test -p rurge-dns doq                     # DoQ：查询报文（ID 0、padding 到 128 字节的倍数、去掉 keepalive）、每个查询一条流、协议错误关连接、取消、空闲与重启后的重连、悄悄失效的连接、证书校验（含 IP 字面量）、bootstrap 与地址轮换（每个地址一份时间），对回环假服务端（FakeDoq）
cargo test -p rurge-dns doh3                    # DoH3：POST 与请求头、应答的判据（状态、内容类型、正文长度）、GOAWAY 与被拒（H3_REQUEST_REJECTED）之后在新连接上重问、取消、悄悄失效的连接，对回环假服务端（FakeDoh3）；两者共用的 QUIC 会话（后台任务里的单飞拨号与它的时限、连接关闭即放手、一建成就不能用的连接、失效重问、悄悄失效的判定）用过滤词 upstream::session，上游 URL 的写法（带用户名口令的被拒、https:// 的名字与文本不带口令、它的 @ 只能在主机之前）用 upstream::tests
cargo test -p rurge-dns upstream::doh::         # DoH（不含 DoH3）：没写路径的 https:// 上游发往 /dns-query（写了 / 的照旧发往 /）、Age 从应答每条记录的 TTL 里减去（按列表的第一个成员，OPT 不动，走不到最后一条记录的报文原样交出）；DoH3 的 Age 用例在 upstream::doh3::
cargo test -p rurge-dns fanout                  # 并发查询：重发、部分结果、空应答、过了截止时间还有轮次没发时以超时结束而不空转（暂停的时钟 + 内存里的上游 MockDns::upstream()；不带截断与 TCP 重试）；多线程运行时的用例只断言错误的种类
cargo test -p rurge-engine --test pipeline      # 流水线端到端，含跟随出站模式：DoT / DoH 连接与 DoQ / DoH3 的 QUIC 载体作为内部会话走规则（经 SOCKS5 的 UDP、DIRECT 与带 socket 设置的 DIRECT 别名、REJECT 与以域名配置的代理改走直连、不载 UDP 的代理照 udp-policy-not-supported-behaviour 办：缺省让该上游失败，DIRECT 时改经 DIRECT；载体打不开或打开后发不出去时以代理的原话失败、带 via <策略>: 前缀；Windows 上载体收到 ICMP 端口不可达不算失败的会话）；每秒一次的流量采样机器睡过之后不补发错过的秒（the_traffic_sampler_does_not_make_up_for_missed_seconds）
cargo test -p rurge --test cli -- crashed does_not_load run_keeps_the_backup run_says_so a_log_file_that_cannot_be_opened   # 配置读不了 / 有错误 / --log-file 打不开时 rurge run 先恢复上一次崩溃留下的系统代理再以 2 退出（经文件后端 RURGE_SYSTEM_PROXY_BACKEND=file:<路径>）：拿不到数据目录的锁或有活着的实例时不碰备份、不创建数据目录、state.json 读不了时不改名、恢复了而备份没能清掉的那一句；真实的后端没在这条路径上跑过
cargo test -p rurge-inbound ipv4                # IPv4 映射地址（::ffff:a.b.c.d）按 IPv4 处理：SOCKS5 的目标、HTTP 的 CONNECT 与绝对 URI、双栈监听上 TCP 客户端自己的地址、UDP 数据报头；引擎里的流用 `cargo test -p rurge-engine --test udp ipv4_mapped` 与 `cargo test -p rurge-engine --lib ipv6_form`（出站以 IPv6 形式报回来源时找得到流），比较出站报回的来源的另外三处在 `rurge-net` 的 `packet_datagram`、`rurge-policy` 的 `udp_probe` 与 `rurge-proto` 的 `from_relay`
cargo test -p rurge-net resource                # 外部资源管理器：下载与缓存、退避、update-interval=0 读作没写（规则集与域名集；策略组上是 E0018，见 rurge-config 的 values_that_cannot_be_used_are_errors）、没有缓存时的 304 算失败的抓取、自动刷新关闭的 URL 任务只等刷新请求与管理器消失（每分钟看一次间隔）、URL 里的用户信息不进日志；阶段 1 补遗（M5b）起还有种类与状态、磁盘上的副本及其摘要（`cache.rs`：对不上的一对文件不是条目）、带检查的条目、可等待的更新（`update_now`）、采用与撤销（`adopt`、`retire`）、与手里相同的 200 不升版本
cargo test -p rurge-platform command            # 外部命令（kwriteconfig、gsettings、networksetup、schtasks）失败的报错文本：命令行参数里最后一个 @ 之前换成 ***
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族、TUIC v4 与 HTTP/3 族之外的全部协议，含 hysteria2 的无混淆 / Salamander / Gecko 与 tuic-v5）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss，含九种流式方法）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 的 HTTP/2 与 h3=true、h2-connect 的 TCP，只在 Linux / macOS）、官方 tuic 0.8.5 的 tuic-server（只测 tuic，只在 Linux；只认环境变量）、masque-server v0.13.0（只测 masque，只在 Linux）、AdGuard dnsproxy v0.86.0 与 CoreDNS v1.14.7（测 DoQ / DoH3 上游）、mihomo v1.19.32（测 snell v1 ～ v3、ss 的 chacha20 与这两种协议前面的 simple-obfs）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试（共 63 个用例需要参照二进制，其余是夹具的单元用例、`Failures` 的 Drop 守卫用例与 `masque.rs` 里节制传送的自测 `the_paced_transfer_ends_with_the_bytes_it_began_with`，不需要）；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_TUIC_SERVER / RURGE_TEST_MASQUE_SERVER / RURGE_TEST_DNSPROXY / RURGE_TEST_COREDNS / RURGE_TEST_MIHOMO / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
cargo test -p rurge-config --test secrets       # 凭据不外泄：一份每个凭据字段都带标记的配置，加载后的 Debug、诊断与脱敏输出里都找不到标记（新加的凭据字段或写法写进这份配置）；脱敏输出是一张"藏什么"的清单，尽力而为，这份用例过了不等于没有秘密会出现（见状态一段的 M8c）
cargo test -p rurge-config redact               # 脱敏（redact.rs）的单元用例：照节解析器的顺序读一行、整行键与位置凭据、URL 里的用户信息、被注释掉的行与行尾的注释、注释里的链接、不会 panic 的任意文本；已知没有覆盖的写法不在其中（计划「延后事项」第 58 ～ 77 行）
cargo test -p rurge-rules pre_match             # 预匹配：规则模式下先把带 pre-matching 的规则不解析地评估一遍，命中者的 reason 是 pre-matching 且轨迹里只多一步 pre-match，预匹配里没命中的规则留在自己的位置上（要先解析的 IP 规则在那里命中），直连与全局模式不看；经流水线与 CLI 的用例用同一个过滤词（--test pipeline、--test cli）；阶段 3 / M3c 起，DNS 查询与 SYN 两个阶段的预匹配经新入口 `RuleEngine::pre_match(session, layer)`（取代 `pre_match_domain` / `pre_match_ip`），它的用例也带这个过滤词，单独选见 `engine::tests::pre_match::` 那一行
cargo test -p rurge-rules sub_rule              # 子规则命中只属于命中的那条规则（AND / OR / NOT 里命中过集合而整条规则没命中时，不留给后面命中的规则）；日志行与请求记录的 notes 用 cargo test -p rurge-engine --test pipeline -- log_line logged_and_noted（捕获 DEBUG 日志，session= 等于记录的 id，并带 rule= 与 policy=）
cargo test -p rurge-config -- one_resource a_resource_is_reported the_same_kind the_report_names are_uses is_a_use is_not_a_use quotes_no_part   # 同一资源不可同时作两种集（加载期）：E0024 在 [Rule]（含逻辑规则的子规则）、[Host]、内联集里直接写出的两种身份上，报在后一处并说出另一处的位置，文本不引用那个资源的任何部分；同一种身份写两次不报，a.list 与 ./a.list 是同一个，被 #!IOS-ONLY 或不满足的 #!REQUIREMENT 关掉的行不算一处使用
cargo test -p rurge-rules -- the_role cannot_reuse   # 同一资源不可同时作两种集（运行期）：集合文件里嵌套出现的另一种身份是 W0032、不匹配也不建条目，配置自己写出的身份总是压过它；rurge check 不打开集合文件，看不到这一种，直接写出的 E0024 经 cargo test -p rurge --test cli two_kinds_of_set
cargo test -p rurge-platform sysinfo            # 平台变量：纯函数（System Version … (Build …) 的拼法、区域名、占位型号）与 macOS / Linux 的读取（对假的命令、文件与环境变量，三个平台都编译、都测）、Windows 的注册表读取（只在 Windows 上）；cli 一侧用 cargo test -p rurge --bin rurge cli::tests（读不到的退回原来的取值，--platform 只给本机的平台真实的版本）与 cargo test -p rurge --test cli -- check_reads_the_system_version check_does_not_give_this_systems_version；macOS 与 Linux 上的真实读取本机编译不到，由 CI 编译、运行（运行 39022550224 起）
cargo test -p rurge-config -- windows_only each_tag system_tag windows_and_linux_tags   # #!WINDOWS-ONLY / #!LINUX-ONLY：SIMPLIFIED 的两项，五个标记的三种写法各只在自己的系统上生效，名字后面紧接字母的不是标记，脱敏与解析共用同一份清单
cargo test -p rurge --bin rurge -- override listen_option   # --listen-http / --listen-socks5 的单元用例（整项替换、错误里没有取值、落在 external 策略的 local-port 上被拒）；取值的读法用 cargo test -p rurge-config listen_value，进程级用例用 cargo test -p rurge --test cli -- listen_
cargo test -p rurge --test cli -- its_profile recorded cannot_write_state_json   # state.json 的 current_profile：启动完成时写下 -c 的绝对路径，已记着同一份就不重写，没起来的启动不写，写不进只记一条 ERROR、启动照常
cargo test -p rurge-config diagnostic           # 诊断的建议：每个诊断码在 codes::hint 的表里做一个决定（缺省建议 / 每处自带 / 不设并写理由），every_code_is_decided_once 在新码没有决定时变红；建议是 &'static str，写进日志用 Diagnostic::headline()；CLI 一侧用 cargo test -p rurge --test cli -- check_says_what_to_do printed_with_its_hint
cargo test -p rurge-engine --test pipeline -- record lookup remote_address forgets_the_address a_dns_session   # 请求记录的监听端口 / 远端地址 / DNS 耗时：直连与经代理（含链、smart 组换成员）各记各的、被拒绝的与 UDP 载体没有远端地址、并发的会话互不串、内部 DNS 会话，以及会话日志行与记录的 id 对得上；`a_dns_session` 选中十个 `a_dns_session_…` 用例：九个是跟随出站模式下 DNS 会话的去向（代理以域名配置、`wireguard` 的 endpoint 是域名、链底是 REJECT 而改走直连，经以地址配置的代理，不等 `evaluate-before-use` 的第一轮，三种出站模式下各交给谁），一个是协议标签按 URL 的方案、不看端口（`a_dns_session_is_labelled_by_its_scheme_whatever_the_port`），其中两个也钉住内部 DNS 会话的 `remoteAddr`：经以地址配置的 `http` 代理时是代理的地址，因代理以域名配置而改走直连时是 DNS 服务器的地址；Trail 本身用 cargo test -p rurge-net trail，UDP 流用 cargo test -p rurge-engine --test udp -- remote_address lookup
cargo test -p rurge-api geoip                   # GET /v1/geoip：两个库各给各的字段、没有项与没有库时是 null、两个日期是库自带的构建时间、IPv4 映射的地址按 IPv4 查、400 与 401；GeoDb::describe 的单元用例用 cargo test -p rurge-rules describe
cargo test -p rurge-rules set::tests            # 集合：什么进哪个索引（域名索引 / IP 前缀树 / IP-ASN 的号码表 / 逐条求值的行）、什么时候先要名字的地址、几行同时命中时报成子规则的是哪一行
cargo test -p rurge-rules set_format::tests     # 规则集 / 域名集文件的一行怎么读：行尾注释（只有规则集文件）、开头的字节顺序标记、被跳过的行（域名集的原因不引用那一行的文字）与条目上限
cargo test -p rurge-rules --release --test memory -- --ignored --exact a_domain_set_of_a_million_entries --nocapture   # 一百万条的集合占多少内存（不进门禁；另两条是 a_rule_set_of_a_million_entries 与什么也不建的 nothing_is_built_is_the_baseline；每个用例单独一个进程，从进程外读峰值工作集，Linux 上读 VmHWM；数字在 M2 设计 Q3 与验收文档）
cargo test -p rurge --test cli -- run_leaves_the_profile   # 启动、重载、切组、停止之后，Profile 与它包含的文件的字节和修改时间都不变（FR-CFG-02）；sighup_reloads_the_profile 只在 Unix 上编译，本机没有编译过，由 CI 的 Linux 与 macOS 编译、运行（运行 40283632986 起三个平台全绿）
cargo test -p rurge-engine --test profile_sources   # Profile 的来源（阶段 1 补遗（M5b））：跟踪、同一次变化只报一次、"没有变化"的应答结束本地文件的优先（`outdated`）、`strict` 到期与暂停的判断（没有检查记录时从本地文件的时间算；`interval` 大到加不进时刻就永不到期）、链接只显示到哪里；单元用例用 cargo test -p rurge-engine --lib profile::
cargo test -p rurge-engine --test profile_updates   # 采用之前的判定（采用 / 不采用 / 等待）、留着的版本一起采用、一页登录页不当作 Profile（通配节下的也不；自己只有通配节的下载文本等它 include 的文件）、启动时的引导、向其余来源问更新、托管配置的副本不从本地文件建出任何东西、重载失败时跟上它点名而从没下载过的来源（`follow_missing`：不暂停任何东西、不动在用的一代、不联网时不跟，文件不再点名的由下一次判定放掉）
cargo test -p rurge-engine --test pipeline -- suspended suspension   # 暂停：HTTP 的 CONNECT 与明文请求、SOCKS5、DNS 服务器的 TCP 连接与 QUIC 载体在规则之前被拒，已经打开的会话照常，重载不动暂停；UDP 流与自动组的测速各用 --test udp suspended、--test auto_groups suspended
cargo test -p rurge-engine --test external_resources   # 外部资源的两个端点背后：列表（会话被拒时 `suspended` 与原因落在哪一项）、按键与 all 的更新、60 秒的限、新版本等它提到的来源时那个键的答案、换代时的补问（catch_up）、集合里没用上的行的三个数与 WARN；API 一层用 cargo test -p rurge-api --test api -- external_resources profile_check
cargo test -p rurge --test cli -- remote_sources   # 起真实二进制的场面：启动时等下载、实例锁、等待之后的退出、从第一个会话起的暂停与运行中开始的暂停、主循环的两个分支、读副本的命令、被改坏的副本重新下载、经 API 要求的更新被采用并重载、运行中换了链接的托管配置不重启就被下载并换上
cargo test -p rurge-api --test api late_body   # API 的分步关连接：不读正文的端点与四种拒绝在正文晚到时仍读得到应答，读正文的端点等正文；停止等什么用 --test api -- serve_waits serve_stops；单元用例用 cargo test -p rurge-api linger；真实的守护进程用 cargo test -p rurge --test cli -- run_stops rurge_stop_stops
cargo test -p rurge-config --lib -- managed remote reach copy wildcard   # 托管配置的生效文本（副本与本地文件谁生效、换地址的链、间隔的下限）、远程 include 的展开与缺节（通配节下连一个节都没有的下载正文也算）、Reach（各触及范围能做什么；托管配置的副本不 include 本地文件）
cargo test -p rurge-net http                    # 内部 HTTP 客户端：URL 里的用户名口令作 Basic 认证（百分号解码、标为敏感、调用方自己的 Authorization 优先）、重定向的每一种 Location 的写法带不带凭据、Response 的 Debug 只显示来处与大小
cargo test -p rurge-rules registry              # 集合注册表：集合文件里没用上的行的 W0003 与 WARN（worth_a_warning）、重编译一次一个（compile_lock）；--wait 的 settle 与 sets_behind 用 cargo test -p rurge-engine --lib stack
cargo test -p rurge-rules first_final           # 匹配在第一条启用的 FINAL 处结束、策略取最后一条的：同一个过滤词在 rurge-config（W0019 / W0021）、rurge-api（--test api，GET /v1/rules 不列第一条之后的规则）、rurge（--test cli）里各有用例
cargo test -p rurge-dns aaaa                    # AAAA 抑制按查找计数：一个接一个地查，第五个名字的查找关掉 AAAA；五个同时的同名查找只计一次；AAAA 应答、补问、过期条目的刷新各自怎么计；WARN 每次停发说一次；flush 之后从头数起
cargo test -p rurge-dns library                 # 系统解析库：.local 与 [Host] 的 server:system 走库（SystemDns::library_lookup）、地址按地族裁剪、时限、应答不进缓存、日志不带名字与地址；[Host] 键末尾的点用 cargo test -p rurge-dns hosts 与 cargo test -p rurge-config --lib -- roots_dot trailing_dot a_dot_after
cargo test -p rurge-config -- custom positional_method   # 旧的 `custom` 类型读作 `ss`：端口之后的位置依次是加密方法、口令、模块的链接（读了就丢），命名参数压过位置上的，主配置里每行一条 `I0001`，不加引号的口令带 `=` 是 `E0018`（整个口令加引号就读得出），位置上写错的方法名不被引用；导入的行、经引擎的连接与 `rurge check` 用同一个过滤词 custom（`rurge-policy`、`--test outbounds_shadowsocks`、`--test cli`）
cargo test -p rurge-rules -- geoip_unknown unknown_does_not unknown_lines unknown_starts tells_unknown   # `GEOIP,UNKNOWN` 与 `IP-ASN,UNKNOWN`：库已载入而说不出（`GeoResult::Unknown`）才命中，库没有载入时不命中，集合里与别的逐条求值的行按顺序判，库载入之后正在运行的规则引擎不必重载就开始命中；解析用 cargo test -p rurge-config unknown_is_a_value，`rurge check` 用 cargo test -p rurge --test cli check_accepts_ip_asn_unknown
cargo test -p rurge-dns tcp_url                 # `tcp://` 的 DNS 服务器只写主机与端口：带路径、查询串或片段的是 `W0026`，那一个服务器不用，Profile 照常加载
cargo test -p rurge-engine --test outbounds_wireguard tos   # `wireguard` 行上的 `tos`：隧道的 UDP 包带策略的 `tos`、握手的发起包照旧 0x88，同一个节上 `tos` 不同的两个策略共用一条隧道、谁也不顶掉谁；配置一侧用 cargo test -p rurge-config the_tos_of_a_wireguard_line_is_kept；线上的 DSCP 位没有抓包看过
cargo test -j 8 --workspace --lib -- every_cell_of_the_quic_verdict block_quic_reads_four_values_and_two_spellings an_unknown_block_quic_value_is_e0018   # `block-quic` 的四个取值 × 策略自己的参数 × 直连与否 × 载不载 UDP（64 格，`every_cell_of_the_quic_verdict` 与 P7 的表逐格比对）；`adaptive` 是第四个取值（`block_quic_reads_four_values_and_two_spellings`），不认识的取值是 `E0018`（`an_unknown_block_quic_value_is_e0018`）
cargo test -p rurge-engine --test outbounds several_addresses   # `use-local-host-item-for-proxy`：`[Host]` 给一个名字列了几个地址时，每个会话随机交一个给代理（假上游从不去拨那几个地址，只记下被要求的目标）
cargo test -p rurge-engine --test pipeline -- labelled_by_its_scheme follows_the_outbound_mode   # 跟随出站模式下 DNS 会话的协议标签按 URL 的方案、不看端口（`tcp://` 是 DNS、`tls://` 是 DOT、`https://` 是 DOH、`quic://` 是 DOQ、`h3://` 是 DOH3，`[Host]` 的 `server:` 同样），三种出站模式下 DNS 会话各交给谁；解析器把每种上游交给自己的连接器用 cargo test -p rurge-dns each_kind_of_upstream，引擎的五个面用 cargo test -p rurge-engine --lib the_five_faces
cargo test -p rurge-config value::            # 一行怎样拆成各项：读法表、整个在引号里的一项是值、引起来的值只去一次引号、穷举六个字符以内的全部字符串、一行几十万个同一种字符读得完
cargo test -p rurge-config -- a_region_ends positional_credentials_are_counted   # 脱敏的区域与按解析器的字段数遮位置凭据
cargo test -p rurge-config -- a_text_with_an_equals_sign an_unknown_parameter_where_a_password_stands a_positional_credential_with_an_equals_sign a_legacy_custom_password   # 整个在引号里的一项是值、带 `=` 的文字不是名字、位置凭据处的未知参数不点名
cargo test -p rurge --test cli -- line_splitting   # rurge check 的端到端：带撇号与不配对括号的行、引起来的含 `=` 的口令
cargo test -p rurge-config -- subnet ssid_setting network::   # 阶段 3 / M1 配置层：子网表达式的求值表（每种表达式 × subnet-exp-wifi-always-match × Wi-Fi 是不是主网卡 × SSID 拿不到）、subnet 组的成员表与没写 default 的 E0018、[SSID Setting] 一行的读法（表达式写到第一个前面是空白的认识的键为止，SSID 里可以有空格；含 = 的 URL 是列表项而不是参数；off；没有参数的行；不认识的键不被引用；表达式写错只在能被引用时才引用）与几行同时命中取第一行、NetworkCell
cargo test -p rurge-config --test secrets -- ssid_setting   # [SSID Setting] 的 DNS URL 带用户名口令的标记，和写坏了的几行（键拼错、键前面是逗号、键后面是冒号）不让口令进诊断与 Debug
cargo test -p rurge-platform netenv   # 平台层读网络（阶段 3 / M1）：八个文本解析器对夹具（netsh 的英文、Windows 11 与被翻译的写法，没有位置权限时的输出，/proc/net/route 与 ipv6_route，nmcli 的转义，iw dev link，route -n get default，networksetup -listallhardwareports，ipconfig getsummary 的 <redacted>），三个平台的读取器对假机器（FakeHost；三个平台都编译、都测；问 nmcli 带 LC_ALL=C.UTF-8 与空的 LANGUAGE，macOS 的硬件端口表读失败时沿用上一张），变化的提示（路由消息头的过滤与合并、Windows 与 Linux 的指纹轮询、每 30 秒读一次 Wi-Fi），文件后端；reading_this_machine_does_not_panic 真读一次本机（只读，断言在任何机器上都成立的不变量）；带时限的命令执行在 cargo test -p rurge-platform command 里
cargo test -p rurge-rules -- subnet network ssid   # SUBNET 规则按当前网络匹配：六种表达式、SSID 拿不到时不命中、逻辑规则里的 SUBNET、预匹配、同一个规则引擎在网络变了之后结果跟着变、求值进行中换网络仍按开头的那份、没有网络时永不命中
cargo test -p rurge-policy -- subnet network a_condition_whose   # subnet 组按条件选、default、没有网络时用 default、临时覆盖的设置与清除（换网络时清除，定义不变的重载保留）；换网络时 url-test / fallback / load-balance 的轮次作废而结果留着、evaluate-before-use 再等一轮、换网络那一刻正在跑的一轮不算、smart 不动、每个出站收到通知
cargo test -p rurge-proto-wireguard a_network_change_told   # 经 Outbound::network_changed 通知的 wireguard 出站给每个 peer 换新的载体
cargo test -p rurge-engine --test network   # Engine::network_changed 的下游（缓存清空与 AAAA 复位、正在用的那一代的解析器、subnet 组跟着变）与 [SSID Setting] 的 DNS 覆盖在建一代时生效（解析器的上游名单与它真的问谁）、覆盖在 customize 钩子之前套用
cargo test -p rurge-engine -- a_subnet_group_follows a_subnet_rule_follows a_rebuilt_registry_still_sees a_dial_waits_for_a_round_that_began the_override_of_a_network an_override_replaces the_debug_of_an_override   # 经真实 HTTP 监听器与假上游的端到端：subnet 组与 SUBNET 规则随引擎的网络变，订阅更新重建的注册表仍看得到网络，等第一轮的拨号等的是新网络上的那一轮；stack.rs 里 DnsOverride 的单元用例
cargo test -p rurge-api select_on_a_subnet_group   # POST /v1/policy_groups/select 对 subnet 组设置、清除临时覆盖，不写 state.json，非成员是 400
cargo test -p rurge --bin rurge network::   # 网络任务（cli/network.rs）：一阵变化先告知第一个再告知最后一个，安静一秒之后的变化立刻告知，窗口里又回到原样的变化不再告知，DNS 服务器或地址单独变也算变化，读失败什么也不告知，读到什么都不知道的网络（没有主网卡也没有 Wi-Fi）不告知、沿用上一个（丢失每段只说一次），读取至多每秒一次（一阵提示至多两次读取、读取期间来的提示不丢），第一次读最多等 1 秒（读得慢的读完再当作变化告知），隐藏选项与变量的取值规则
cargo test -p rurge --test cli -- the_network:: a_lookup_asks_the_servers_the_network_gives rule_match_reads_the_network subnet_rule run_starts_an_external_program   # 起真实二进制、网络来自文件后端的场面：换网络之后 subnet 组的当前选择变（经 API 读）、DNS 覆盖生效（GET /v1/dns 的 upstreams）且重建不重读 Profile、接管暂停对系统代理的文件后端（进入时恢复原设置、暂停期间 POST 只记下想要的状态、离开时按它办——包括进入时恢复失败、暂停期间 POST false 的，离开时关掉——、重载加上或拿掉 suspend 的行、网络文件说什么都不知道时暂停不结束）、为 DNS 覆盖重建的一代在标准错误上打印 W0026、rule match 与 dns lookup 读文件里的网络、rurge check 不再说 SUBNET 永不匹配、未知的 RURGE_NETWORK_BACKEND 以 2 退出、隐藏的 --network-backend 不在 --help 里且压过变量、日志 INFO 里没有 SSID、external 拉起的嵌套 rurge run 带 --network-backend
cargo test -p rurge-config process_name_values   # 阶段 3 / M2 配置层：`PROCESS-NAME` 值的分类（文件名、以 `/` 开头的路径、盘符加 `:\` 或 `:/` 的路径、`\\` 开头的 UNC、以 `/` 或 `\` 结尾的前缀、带通配符的、只有一个 `/` 的）
cargo test -p rurge-rules process   # `PROCESS-NAME` 在规则引擎里：`NeedsProcess` 在 `AND` / `OR` / `NOT` 与集合逐条求值的行里怎样传、要进程优先于要解析（`AND` 里进程不命中时解析器一次也没被问）、一次评估至多问一次进程、`SessionInfo.process` 有值时不问、预匹配不问、Windows 与别的平台的比较（`process_comparison_follows_the_platform`）、只知道名字的进程、内部集 `SYSTEM` 的进程行只在 macOS 上编进集合（别的平台上走到它的会话一次也不问进程，集合的文字照旧）
cargo test -p rurge-platform procinfo   # 平台层找进程（阶段 3 / M2）：与平台无关的部分对假的表来源（新鲜度的几种情形、一次读取服务一批查询、两次读取至少隔 20 毫秒、UDP 的未指定地址、IPv4 查询找映射形式、来源不是本机时不读表、本机地址列表的刷新、`is_local` 的三种回答）；Windows 四种行的解析、Linux 四个文件的解析（两种字节序）与 inode 扫描对内存里的 `/proc`、macOS 的走法对拼出的缓冲，三个平台的这些在每个宿主上编译、都测；Windows 读表的循环（`read_table`）对假的系统调用的用例只在 Windows 上编译、跑（函数和用例都在 `#[cfg(windows)]` 里）；各平台查测试进程自己的回环 TCP 连接与 UDP 套接字（`this_process_owns_its_loopback_sockets`，只读，按平台编译，本机只编译并跑 Windows 的）；`a_lookup_with_many_connections_open` 是忽略的基准（只在 Windows 上，加 `-- --ignored --nocapture`，开 500 条回环连接）
cargo test -p rurge-platform --test unsafe_functions   # 放开 unsafe 的函数名单（`LISTED`，七个）与扫描器自己的六条自测
cargo test -p rurge-inbound process   # 进程槽：只落定一次、等待的一方在落定时醒来而不等到期限、不丢唤醒（`a_settle_racing_a_waiter_is_never_missed`）、期限先到是 `None`、已落定的槽不论期限都答、`Debug` 不显示名字与路径
cargo test -p rurge-engine --test process   # 引擎里的进程查找（假的 `OwnerLookup`，经真实的 HTTP 与 SOCKS5 监听器，不用暂停的时钟）：规则命中、规则用不到进程的会话不等、规则等查找、到期按"不知道"判并在 `notes` 里说一句、第一次到期的 WARN 只有一行、不是本机的来源不查、内部会话不查、UDP 关联只查一次、关联开始 200 毫秒之后才开的流不再等、会话结束之后才查到的进程也在记录里、一阵并发会话都完成、失败的查找还回许可、查找的 DEBUG 行与进程不进 INFO；`owner.rs` 的四条单元用例用 `cargo test -p rurge-engine --lib owner`
cargo test -p rurge-api -- a_record_without_a_process every_field_of_a_record   # 请求记录的 `pid` 与 `processPath`：有值与 `null`、只有名字的进程、记录建立之后才查到的进程读记录时看得见
cargo test -p rurge --bin rurge -- the_process_backend_values the_owner_lookup   # `RURGE_PROCESS_BACKEND` 的取值（没设、`off`、`OFF` 不行、别的）与引擎的问题到平台的问题、平台的回答到引擎的回答这两次翻译
cargo test -p rurge --test cli -- processes check_does_not_say_a_process_rule   # `rurge check` 不再说 `PROCESS-NAME` 永不匹配；`run::processes`：未知的 `RURGE_PROCESS_BACKEND` 以 2 退出、`off` 时不查、起真实的二进制查测试进程自己（文件名、完整路径、目录前缀三种写法各命中一次；去掉辅助函数缺省设的 `off`）
cargo test -j 8 --workspace --lib -- tests::tcp::   # 阶段 3 / M3a，`rurge-tun`（下面直到 `tests::bench::` 的各行同：经整个工作区按名字选 `--lib` 里的用例，不用 `-p`；末尾的 `::` 让别的 crate 里名字带 `tests::tcp`、`tests::syn` 的用例不被选中）。最小的一条 TCP 路：经假 `Dialer` 的往返（v4 与 v6）、每个方向 1 MiB（也在最小的缓冲上、MTU 9000 上）、半关、拨号失败与被拒的会话得到重置、握手完成之前不拨号、保活与超时（保活 100 毫秒、超时 3 秒，这个二进制因此至少跑 6 秒）、`stop`、MTU 小于 1280 被拒
cargo test -j 8 --workspace --lib -- tests::filter::   # 进协议栈之前的过滤与计数：`classify` 的每一行（`malformed`、`fragment`、`martian`、`unsupported`，检查的次序、IPv4 总长度之后的字节、端口 0）、坏包不伤任何东西、每种原因第一次一行 WARN（不带地址，`martian` 不说）、丢弃的 DEBUG 行带读到的地址、进片的队列满了丢并计 `queue_full`
cargo test -j 8 --workspace --lib -- tests::syn::   # SYN 的去处与上限：同一个目的端点的几个 SYN 各得自己的连接、丢了的 SYN-ACK、套接字与半开的上限、半开超时、rurge 自己的地址（应答器地址的 53 端口除外：那里建套接字、由应答器答，见 `tests::dns::tcp::`，用例改发 54 端口）与池不认识的 fake 地址回 RST（`unknown_fake`）、RST 3 秒 100 个、`rst_for`、停止时回 RST、同时带 RST 或 FIN 的 SYN 不算新的；到应答器 53 端口的 TCP 连接有 `dns_tcp_max` 的上限（超出的 SYN 回 RST、计 `busy`、不占套接字与半开的位置，只有应答器 53 端口的连接占位，协议栈不收的 SYN 把位置还回去，并发的占位不超上限）
cargo test -j 8 --workspace --lib -- tests::hazards::   # smoltcp 0.12 的隐患：SACK-permitted 不回给应用、乱序数据不带 SACK 块、窗口边缘的守卫（缓冲里还有空位才丢；缓冲满时零窗口探测得到 ACK）、窗口缩放位数为 0、停在窗口 0 的上传恢复
cargo test -j 8 --workspace --lib -- tests::rebuild::   # 片的重建：栈 panic 或锁中毒时整片重建、连接被复位、预算与计数还回、新的种子、WARN 恰好一行、接下一个 SYN 的当中 panic 时占着的预算与位置都还回
cargo test -j 8 --workspace --lib -- tests::lifecycle::   # 流的一生：优雅关闭不留套接字、读得慢的应用不丢字节、应用中途重置结束会话、两千条连接一条接一条之后全部回零（上游是内存里的回显 `Script::MemoryEcho`）、丢掉的流留 `linger` 再复位、同一条连接第二次被丢什么也不做、FIN 与 RST 到转发循环是 EOF 与错误
cargo test -j 8 --workspace --lib -- tests::outcomes::   # 握手之后的拒绝、停止与设备出错：`REJECT-DROP` 拿着再关（拿完读掉应用还在发的字节）、其余拒绝回 RST、停止时每片的最后一轮把复位交给写任务（`STOP_GRACE` 1 秒）、停止不取消拨号、设备出错说且只说一行 WARN、第一个结局不变、设备坏了之后 `ending()` 不等正在进行的拨号（`ending_answers_before_a_dial_under_way_comes_back`，M4a 计划「执行期修正记录」第 100 行）
cargo test -j 8 --workspace --lib -- tests::budget:: tests::fixture:: tests::bench::   # 缓冲预算一步取齐，应用端夹具与内存设备本身的用例，`ScriptedTcp` 与基准的读数；`tests::bench::` 也选中三个基准本身（B1、B2 与 M3c 的 b3），它们是忽略的用例：`--list` 列得出，普通的一次运行跳过它们（要跑它们见下面三行）
TUN_BENCH_FAR=memory cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b1_single_stream_throughput --nocapture   # 阶段 3 / M3a 的 B1，忽略的用例、不作门禁，与 B2 一起是整个计划里唯一用 `-p` 的地方（只为不把整个工作区编成 release）：单流吞吐，每行一个 MTU（1500 / 9000）× `tx_max`（256 KiB / 1 MiB）× 方向，另有夹具自己的上限；`TUN_BENCH_FAR=memory` 时远端是内存里的管道，判据看这些行（每个方向不低于 0.7 Gbit/s 或夹具上限的 80%，四个计数为 0）；不设时远端是回环 TCP，只作那一跳的上限、不下判语；`TUN_BENCH_MIB` 改每行传多少 MiB（缺省 64）
cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b2_ten_thousand_connections --nocapture   # B2：一万条空闲连接加一百条活跃连接，每种设置单独一个进程：缺省取 `TunOptions::default()` 与 MTU 1500，`TUN_BENCH_SHARDS`（个数）、`TUN_BENCH_MIN` 与 `TUN_BENCH_TX_MAX`（KiB）、`TUN_BENCH_BUDGET`（MiB）、`TUN_BENCH_MTU`（字节）改它；打印进程的峰值（Windows 由测试自己起 `powershell` 读 `PeakWorkingSet64`，Linux 读 `VmHWM`，别处不读）、测试一侧自己的内存各项，和两个读数：峰值减去全部测试一侧的项（标着 "the gate"，对着 429,000,000 字节）与只减去准的应用端套接字缓冲的保守读数
cargo test -p rurge-tun --release --lib -- --ignored --exact tests::bench::b3_fake_answer_latency --nocapture   # b3：fake 应答的延迟，忽略的用例、不作门禁，与 B1、B2 同是用 `-p` 的地方：十万个不同的名字、一个线程，每个 A 查询计时 `Responder::answer` 加造应答包（`packet::udp_packets`），不含设备、写任务和引擎的规则评估，钩子是什么也不记的空钩子，所以是整条路的下限；打印中位数、p99 与最大值（连同最大的那个是第几个查询）和 NFR-01 的 0.2 毫秒，不断言；一台 Windows 11 x86-64 的开发机上 release：中位数 0.9 微秒、p99 3.2 微秒、最大 5.8 毫秒（最大值落在池的 `HashMap` 翻倍处，第 57,345 个名字——读代码得出的解释，没有改池去验证）
cargo test -j 8 --workspace --lib -- fakeip   # 阶段 3 / M3b，fake-IP 池（`rurge_dns::fakeip`，含 `file.rs` 里 `fake-ip.txt` 的读写；这一行起的十二行都经整个工作区按名字选，不用 `-p`，`--lib` 选单元用例、`--test` 选集成测试）：编号与 v4、v6 地址互转（只认两种写法，IPv4 映射的不算）、名字规范化（小写、去末尾的点、`[a-z0-9_-]` 的标签、253 字节）、先游标后最久没用到的、整个池用完之前不回收、有租约的编号从不回收（租约的结束算一次使用）、全部有租约时 `Exhausted`、文件的往返与严格的读法（读不了的原因不引用文件的内容）、读不了的文件留着不动到第一次保存、`clean 0` 的文件隔离接下来的 2048 个编号 120 秒（时间由调用方传，用例不等 120 秒）、文件里没有名字的编号与它们叠加隔离、保存任务至多每 10 秒写一次且停下时写 `clean 1`、保存失败是一条不带名字的 WARN、`leased()`（有租约的地址有几个，一个地址上的两个租约算一个）与 `named()`（有地址的名字有几个）只数、不显示名字
cargo test -j 8 --workspace --lib -- tests::options::   # `TunOptions::check`：缺省值通过、每个字段的范围（`keep_alive`、`udp_idle`、`udp_dns_idle`、`dns_tcp_idle`、`echo_timeout` 在 1 毫秒到 24 小时，`udp_dns_idle` 不得大于 `udp_idle`，`timeout` 大于 `keep_alive`，队列、`dns_queue`、`udp_bytes`、`dns_tcp_max` 与上限不为 0）、越界的字段按字段的次序报第一个、边界、`budget` / `max_sockets` / `max_half_open` 为 0 通过、`drop_hold` 一类的时间不检查、`Tun::start` 对不合格的选项 panic
cargo test -j 8 --workspace --lib -- tests::fake::   # 发往 fake 地址的 TCP：会话按名字拨号（`dst_host` 是名字、`local` 是应用连的端点，v4 与 v6）、连接开着时它的地址不被回收给别的名字（租约，连接结束再还）、池里没有的 fake 地址与没有池时回 RST 并计 `unknown_fake`、什么也不拨
cargo test -j 8 --workspace --lib -- tests::udp::   # UDP 会话与回包：往返（v4 与 v6）、发往 fake 地址的数据报按名字走并从那个 fake 端点回来、第三方的回包保留自己的来源、名字作来源时经池换回 fake 地址、每个应用源端点一个会话、队列 256 个、`admit_udp` 答 `Busy` 的丢弃计 `busy`、两个方向都空闲 `udp_idle` 才结束（只有下行也算）、目标表至多 1024 个与空闲的目标早放租约、大于 MTU 的回包按 IPv4 / IPv6 分片且重组后逐字节相同（标识各不相同）、被拒的目标回 ICMP 管理上禁止（`REJECT` 3 秒 100 个、`REJECT-NO-DROP` 不限、`REJECT-DROP` 不回，引用最近一个包的首部）、发往 rurge 自己地址的 UDP 回 ICMP 且不限频（应答器地址的 53 端口除外：那里是应答器，见 `tests::dns::udp::`；用例 `udp_to_rurge_addresses_is_prohibited_unlimited` 发往 54 端口）、未知的 fake 地址丢弃计 `unknown_fake`、写任务的队列满了的 ICMP 丢弃计 `queue_full`、停止时等着写任务队列空位的发送随之结束、会话结束时放掉目标的租约即使引擎还拿着客户端、排队的数据报共用的字节预算（`tests::udp::budget::`：放不下的计 `queue_full`、会话结束时还回、第一个数据报放不下时不开会话）、停止结束每个会话
cargo test -j 8 --workspace --lib -- tests::icmp::   # ICMP 回显：真实地址经 `Pinger`（v4 与 v6，保留应用的标识、序号与负载）、fake 地址先解析（同地址族优先）再转发且应答来自 fake 地址、rurge 自己的地址本地应答、`icmp-forwarding = false` 一律丢弃（计 `unsupported`，rurge 自己的地址也是）、不认识的 fake 地址与解析不出的名字不转发、迟迟没有应答的放弃（用例里 `echo_timeout` 取 300 毫秒）、在途至多 `max_echoes`（超出计 `busy`，名额不论怎样结束都还）、`Pinger` 失败不应答、应答大于 MTU 时分片、写任务的队列满了本地应答丢弃计 `queue_full`、停止结束在途的回显、失败只在 DEBUG 里说
cargo test -j 8 --workspace --test tun   # 经真实引擎的端到端（`Engine::start_tun`，内存设备，应用端是另一个 smoltcp 栈，两道防线见测试约定）：TCP 到 fake 地址（v4 与 v6）走名字并进请求记录（`listener` 是 `tun`）、TCP 到文档地址照规则交给 `socks5` 策略、被拒的 TCP 回 RST、UDP 到 fake 地址往返并从那个 fake 端点回来、被拒的 UDP 流与被 `block-quic` 挡掉的 QUIC 回 ICMP 管理上禁止、ping fake 地址发往名字的地址（Profile 写 `icmp-forwarding = false` 重载之后不再应答）、进程查找按会话的形状、`GET /v1/traffic` 的 `Tun` 桶、停止结束卡在上游上的转发、池的文件在数据目录里（停下时 `clean 1`，同一个数据目录上的第二个引擎沿用）、引擎停止接受时 TUN 随之停止、读不了的池文件留着不动且只 WARN 一次、上一次不干净只说一次 INFO、`associate` 在 `closed` 取消时返回且不再拿着客户端、这一代的 DNS 设置（`allow-dns-svcb`、`hijack-dns`、`always-real-ip`、`[Host]`）进 `TunSettings`（`the_tuns_settings_carry_the_dns_settings_of_the_profile`）
cargo test -j 8 --workspace --lib -- which_flow_a_reply_answers   # 引擎 UDP：一个回包应答哪条流（`Routes::flow_for` 的三档——载体发往的目标恰好等于回包的来源、同端口的域名流、随便一条——，恰好一条时才把 `answers` 告诉客户端，死了的流不算）；端到端的对应用例是 `--test udp` 里的 `a_reply_says_which_flow_it_answers`
cargo test -j 8 --workspace --test udp -- a_refused_flow_tells   # 引擎 UDP：被拒的流对每个被丢的数据报都告诉客户端（`UdpClient::refused`，按拒绝的种类：`REJECT` 一族的规则、`block-quic`、不载 UDP 的策略），拨不通、发不出去与暂停的流什么也不告诉（它们 5 秒后重试）
cargo test -j 8 --workspace --lib -- query_follows   # 进程查询按会话的形状（`owner::query_for`）：HTTP、SOCKS5（含 UDP 关联的控制连接）与虚拟网卡的 TCP 会话发 TCP 的查询，没有 `local` 的监听器会话退回 `0.0.0.0:<in_port>`，虚拟网卡的 UDP 会话只按本地端点，没有 `local` 的虚拟网卡 TCP 会话不带 `remote`，`Forward` 与 `Internal` 不查
cargo test -j 8 --workspace --lib -- compares_the_far_address   # 平台层找进程：TCP 的行连远端地址一起比（同一个本地端点连着两台主机上同号的端口时不认错行，查询里的未指定地址只比端口）
cargo test -j 8 --workspace --lib -- unread_bytes   # TIME-WAIT 里没读的数据（片一级，合成时间）：`unread_bytes_outlive_the_time_wait`（rurge 先 `shutdown`，应用发数据和 FIN，流不读，过了宽限与 smoltcp 的 10 秒，读到全部字节再读到 `Ok(0)`，预算的份额留到读完）与 `unread_bytes_across_the_end_of_the_receive_buffer_are_kept_in_order`（没读的字节在环形缓冲的两头）；这一组的其余几条（丢掉的流还份额、超时的连接读完字节后是复位、复位的连接什么也不留、同一个四元组上的新连接不被旧的流读到）在 M3a 那一行的 `tests::lifecycle::` 里
cargo test -j 8 --workspace --lib -- stop_ends_a_relay   # 停止结束卡在上游上的转发（`tests::outcomes::stop_ends_a_relay_stuck_on_its_upstream`：上游是 `Script::Hold`、应用写 1 MiB 把上行堵住，`stop()` 之后 `join()` 在界限内答 `Stopped`，转发记成 `Err("cancelled")`，应用的写以 `ConnectionReset` 结束）；引擎一侧的端到端是 `--test tun` 里的 `stopping_the_tun_ends_a_relay_stuck_on_its_upstream`
cargo test -j 8 --workspace --lib -- tests::dns::   # 阶段 3 / M3c，应答器与它的 UDP / TCP 一头（`crates/rurge-tun/src/dns/`，70 个；M3c 的各行同 M3a、M3b 的写法：经整个工作区按名字选，不用 `-p`，b3 除外）。`answer`（23 个）：判定的次序——读请求的三种拒绝、class、canary、SVCB 与 HTTPS、对 A 与 AAAA 的预匹配、地址族、`always-real-ip`、没有池、fake 地址，其余类型转发——与相邻几步的先后（各一条），0x20 大小写混排的名字拿同一个地址且问题段逐字节回显，池耗尽答 SERVFAIL 并只 WARN 一行；`forward`（10 个）：转发至多 256 个、其余当场 SERVFAIL 而 fake 应答不等转发、转发中途被丢掉的 `answer` 还名额、UDP 的限度夹在 512 与 1232 之间且超出置 TC、TCP 超过 65,535 字节答 SERVFAIL、每个查询只取一次设置、每个查询一行 DEBUG 而名字不在 INFO 以上；`udp`（19 个）：发往应答器地址 53 端口的查询与 `hijack-dns` 劫持的查询由应答器答且来源是被问的端点，别的端口回 ICMP，不是查询的包与 IPv6 的照常作会话，去处的次序，重载下一个包就看到，连发 1,000 个查询、应答器的队列满了计 `queue_full`、应答放不进写任务的队列时丢并计数，只问 53 端口的会话 `udp_dns_idle` 就结束（发过别的端口之后不再算，扫除按 `udp_idle`），停止时被上游扣住的查询随之结束；`tcp`（18 个）：一条连接上按次序答、截断的 UDP 应答经 TCP 拿到整份、空闲 `dns_tcp_idle` 关、转发等应答不算空闲、乱码与长度 0 与没发全的查询关、关的时候读掉应用还在发的字节、应用不读应答就复位、不关自己那一半的应用到时被放手、UDP 与 TCP 共用一个应答器、应答器的别的 TCP 端口回 RST、被劫持的服务器的 TCP 照常作连接、停止时被上游扣住的查询随之结束、应用发一个查询和长度 0 再过 200 毫秒才读，读到应答再读到流的结束而不是复位（R25 在 DNS 一侧的确定用例：`serve_dns` 在 `shutdown` 之后立刻放手时 20 次里 20 次红）、同时服务的连接至多 `dns_tcp_max` 条（超出的 SYN 回 RST 并计 `busy`，应用关掉一条之后新的连接进来并被应答）、握手不完成的连接在半开超时后还位置；停止与每条关连接的用例都断言位置已还
cargo test -j 8 --workspace --lib -- tests::prematch::   # 阶段 3 / M3c，SYN 层的预匹配（10 个）：被拒的 SYN 回 RST、`REJECT-DROP` 的 SYN 被丢而应用什么也收不到、SYN 层的会话是这条连接（P11 的字段，与拨号时问规则的会话是同一个）、预匹配的 RST 经 3 秒 100 个的限频、被拒的 SYN 不留 fake 地址的租约、协议栈没有收下的 SYN（MSS 为 0 的）把租约还回去、先问规则再问协议栈、丢掉的 SYN 一行 DEBUG 而不计数、停止 / 上限 / rurge 自己的地址 / 应答器的别的端口 / 不认识的 fake 地址的 SYN 不问规则、应答器的 53 端口的 SYN 不问规则
cargo test -j 8 --workspace --test tun_dns   # 阶段 3 / M3c，经真实引擎的 DNS 与预匹配端到端（23 个；`Engine::start_tun`，内存设备，接线在 `tests/common/tun.rs`，两道防线见测试约定）：经应答器问到的名字拿 fake 地址、连过去、引擎拨名字并进请求记录；DNS 层的预匹配答空应答并只记一次（`dst` 没有 `:0`，INFO 只有一行带名字），`REJECT-DROP` 不答、`REJECT-NO-DROP` 答 `198.18.0.244` 且不升级，`REJECT` 与 `REJECT-TINYGIF` 第 50 次起静默，命中集合里的一行时 `notes` 多一句 `Sub-rule matched`；SYN 层的预匹配只 RST 它点名的端口，连 fake 地址的端口也拒；出站模式取活的，换代之后同一条规则再记一条，不该记的拒绝不查进程；经 HTTP / SOCKS5 监听器来的 fake 地址换回名字（CONNECT、SOCKS5 的 TCP 与 UDP 流），池里没有的与 rurge 自己的地址得 `fake address not known`，没有 TUN 时照普通地址，不在 rurge 地址空间里的地址与池无关，TUN 停止之后不再换回，两条流同一目标时回包保留真实的来源
cargo test -j 8 --workspace --test pipeline -- refusals_at_the_dns_layer repeated_rejects   # 自动升级的计数：DNS 层的预匹配拒绝与会话的拒绝经同一个方法 `Escalation::refusal` 数在一起（同一个名字 49 次查询加一个会话就是第 50 次，此后两处的下一次都是 `REJECT-DROP`；把 DNS 层的拒绝记在另一个键下时会话那一次得 `Reject`，用例变红）；单元用例 `engine::tests::a_refusal_is_counted_by_the_destination_of_its_session` 钉住方法本身（`REJECT` 与 `REJECT-TINYGIF` 合起来数、别的种类不数、窗口过了就落下去），`prematch::tests::the_interval_is_five_minutes` 钉住"同一代同一条规则 5 分钟记一次"的五分钟
cargo test -j 8 --workspace --lib -- fanout::tests::forward_ fanout::tests::each_send resolver::tests::forward_ mock_dns_answers_other_types   # 阶段 3 / M3c，`rurge-dns` 的原样转发（20 个；暂停的时钟与内存里的上游 `MockDns::upstream()`，不碰网络）：`forward_raw` 第一个 NOERROR 或 NXDOMAIN 胜出、别的码留着、都答了才交回、没有应答时超时、对沉默的上游重发、不是对这次发送的应答不收、问题只差大小写的应答收下、每次发送换新的 ID；`Resolver::forward` 带着查询的 ID 交回应答、照 `[Host]` 选上游（`server:` 的服务器、`server:system` 的系统服务器，没有系统服务器时失败）、不进缓存、不计 AAAA 抑制、OPT 降到 1232、读不了的查询失败、一次转发一行 DEBUG 而名字不在 INFO 以上
cargo test -j 8 --workspace --lib -- message::tests:: upstream::udp::tests::   # 阶段 3 / M3c，`rurge-dns` 的报文辅助与 UDP 上游的 ID（33 个，含此前就有的）：只读报文头与问题（名字小写、去掉末尾的点）、`read_request` 的三种拒绝、`answer` 逐字节回显问题且只写所问地址族、`answer_truncated`、`answer_header_only`、`cap_udp_size` 只降不升、`FORMERR` 与 `NOTIMP` 的往返；UDP 上游给每个在途的查询一个不冲突的 ID（两个同 ID 的查询同时在途各得自己的应答，被丢掉的查询还 ID，65,536 个都占着时失败，截断的应答照原样改走 TCP）
cargo test -j 8 --workspace --lib -- engine::tests::pre_match::   # 阶段 3 / M3c，预匹配的新入口 `RuleEngine::pre_match(session, layer)`（`crates/rurge-rules/src/engine/tests/pre_match.rs`，11 个）：DNS 层要连接才知道的条件（端口、对名字的 IP 类规则、`PROTOCOL`、`USER-AGENT`、`URL-REGEX`）答"不知道"而规则不命中，`NOT` 不把"不知道"翻成命中，`OR` 的另一支不靠连接就成立与 `NOT` 套在名字使之不成立的 `AND` 上是命中（任何端口都一样），集合里要连接的行让集合不命中而域名行照常命中，`SRC-IP` 与 `SUBNET` 在 DNS 层照常判定，`DOMAIN` 一族与 `HOSTNAME-TYPE` 不受影响，命中记一次并带命中的集合里的行，`evaluate` 永远看不到 `NeedsConnection`
cargo test -j 8 --workspace --lib -- route::   # 阶段 3 / M4a，`rurge-platform` 的路由集与三个平台的步骤（42 个；全部是数据，哪个宿主上都编译、都测，没有一行执行程序）：`RouteSet::compute` 的八个基础前缀、排除与包含的洞、合并与 `RouteSet::diff`、rurge 自己的空间从不被剥、`external` 的地址是洞、`ipv6-vif` 关了就不用 IPv6 的项，以及 `linux` / `macos` / `windows` 三个文件把要做的动作翻成 `Step`（逐字的命令行与脚本、一个地址族一份、`netsh -f` 的 `key=value`、脚本每行以换行结尾且没有回车、DNS 的接管与交还）
cargo test -j 8 --workspace --lib -- tests::dispatch:: queue_bytes a_shard_rebuilt_twice_in_a_minute   # 阶段 3 / M4a，任务 1 在 `rurge-tun` 里的三条（5 个）：进片的队列按字节计（超出 `queue_bytes` 丢并计 `queue_full`、片取走之后字节还回）、每个 TUN 一个 `RandomState` 选片、片重建的 WARN 每片每分钟一行（其余 DEBUG）、`queue_bytes` 不得为 0
cargo test -j 8 --workspace --test tun -- start_tun_refuses a_second_tun_on_one_engine an_association_returns_when_closed   # 阶段 3 / M4a，任务 2 在引擎里的三条（4 个）：`start_tun` 拒绝 `TunOptions::check` 拒绝的选项与 MTU 小于 1280 的设备且什么也不开（没有 `fake-ip.txt`），一个引擎同时只一个 TUN（第一个结束后第二个能起），以及"引擎放开客户端"以 `STOP_GRACE`（1 秒）为界
cargo test -p rurge --bin rurge -- cli::enhanced   # 阶段 3 / M4a，可执行文件里增强模式的管理器、三个后端、撤销动作与"想要的"（42 个）：打开的次序、失败的回退与倒着撤销、备份先于它撤销的那一步、备份写不进时不做那一步、读不懂的备份不动、关闭的次序与等 TUN 3 秒、IPv6 的三种情形、`follow` 只做差与出口表始终是超集、路由的 IPv6 一半失败时记下什么与按 IPv6 没开往下做（`Ok(false)`）、设备坏了报哪一次打开、无备份的崩溃把 `features.enhanced_mode` 写回 `false`、平台后端与 `off` 什么都答"不可用"、文件后端的动作与 `fail` / `fail_undo` / `device_fails_after_ms`
cargo test -p rurge --test cli -- run::enhanced_mode   # 阶段 3 / M4a，增强模式的端到端（20 个，经 `RURGE_ENHANCED_BACKEND=file:` 与真实的 `rurge run`）：启动时开与关（标准输出的次序）、经 API 开关与形状、失败时退 1 与 API 回 500、关闭失败之后仍是使用者要的关（进出暂停也不再打开）、`kill` 之后再起的恢复、`ipv6-vif = always` 与"IPv6 配不上"、设备坏了关掉、接管暂停的两个方向、重载只改变了的、重载开 / 关 IPv6、`ipv6-vif = auto` 随网络、`follow` 失败照开着、两个辅助函数缺省 `off`、未知的 `RURGE_ENHANCED_BACKEND` 以 2 退出
cargo test -j 8 --workspace --lib -- quic::initial   # 阶段 3 / M5，QUIC 识别的单元用例（`rurge-net`）：RFC 9001 与 RFC 9369 附录 A.2 的客户端 Initial（1200 字节，十六进制照抄；常量上面的注释写着 SHA-256，供抄写时核对；抄错一个字节会让 AEAD 的标签不过、用例变红）、构造器造的每一个版本（v1 / v2 / 草案 29 ～ 32，以及 DCID 的 0、1、21 字节）、包头筛不过的（版本、类型、长度、包号偏移）、前缀与头 64 个字节里的每一位翻转、QUIC 位清零的 Initial（`a_cleared_quic_bit_is_still_one`）、quinn 截的真客户端 Initial（只在回环上，每一步有时限）
cargo test -j 8 --workspace --test udp -- quic_on_any_port block_quic_blocks_quic_on_any_port a_lookalike stun_flows   # 阶段 3 / M5，引擎：所有端口上的 QUIC 与 STUN 标签（`quic.test:8443`、`stun.test:9000`）、443 以外的 QUIC 被 `block-quic` 挡下、仿 Initial 的普通 UDP、STUN 流的标签与 `PROTOCOL,UDP`
cargo test -j 8 --workspace --lib -- quic_adaptive a_client_tcp_session_tells_the_table   # 阶段 3 / M5，自适应表：第一次挡、1 秒内 TCP 算回落、窗口过了放行、安静窗口、第 201 个名字淘汰最久没用的、状态落定的两行 DEBUG；客户端 TCP 会话通知表的是 `engine::tests::a_client_tcp_session_tells_the_table`（名字里没有 `quic_adaptive`，所以单列）；`Internal` 的会话不通知靠的是它走 `dial_internal`、不经 `dial`（结构保证，没有单独的用例）
cargo test -j 8 --workspace --lib -- ip_rewrite a_single_address_is_a_slash_32_or_a_slash_128 no_section_is_an_empty_table   # 阶段 3 / M5，`rurge-config`：`[IP Rewrite]` 的读法（次序、主机位、单个地址、大小写、坏行 `W0012`、空节；后两条的名字里没有 `ip_rewrite`，所以单列）
cargo test -j 8 --workspace --lib -- tests::rewrite   # 阶段 3 / M5，`rurge-tun`：三种动作、`W0012` / 位置 / 计数 / 重载、两个限频共用、`reflect` 的包、`reflected` 不写日志、`reject` 的 IPv6 引用与扩展首部（`reject_quotes_an_ipv6_packet_up_to_the_minimum_mtu`、`refusal_reads_past_no_ipv6_extension_header`（含截短的分片首部））
cargo test -j 8 --workspace --lib -- tests::writer   # 阶段 3 / M5，写任务的两条队列：优先批先写、一条队列关着、两条都关才结束、`queue_full`、`STOP_GRACE`、`writer_queue` 是两条各自的容量
cargo test -j 8 --workspace --lib -- tests::udp:: tests::icmp:: tests::dns::udp::   # 阶段 3 / M5，发送一方选队列：开了进优先、关了进普通（TCP 与 ICMP 总在普通）
cargo test -j 8 --workspace --lib -- wireguard   # 阶段 3 / M5，`rurge-config`：手册 6.11 的服务端写法（`listen-port` / `accept-incoming` 的 `W0029`、不带 `endpoint` 的 peer、两者混写、没有 `listen-port` 时缺 `endpoint` 仍是 `E0023`）
cargo test -j 8 --workspace --test outbounds_wireguard -- a_policy_on_a_section_without_peers   # 阶段 3 / M5，引擎：没有 peer 的节上的策略立刻失败，错误说明原因
cargo test -j 8 --workspace --test pipeline -- a_dns_session_matches_rules_by_the_servers_name an_ip_cidr_rule   # 阶段 3 / M5，内部 DNS 会话按服务器的名字匹配、`IP-CIDR` 仍按地址；单元的三条用 `--lib` 里的名字 `without_an_engine_a_named_server_is_dialled_at_its_address`、`connector_dials_the_resolved_address_and_names_it`、`a_named_quic_server_reaches_its_connector_with_its_name`
cargo test -j 8 --workspace --lib -- prefix::   # 阶段 4 / M1a，`rurge-http` 的前缀流（4 条）：前缀先于底下的流被读到、读的缓冲比前缀小时分批给出、写与关写端直通、缓冲读完之后 `fill` 读到流的末
cargo test -j 8 --workspace --lib -- tls::tests::   # 阶段 4 / M1a，`rurge-http` 的 ClientHello（10 条）：真的 rustls 客户端产生的一条、任意切分、跨记录、大于一个记录、超过 64 KiB、SNI 大写与没有 SNI、ALPN、损坏的字节不 panic、长度对不上
cargo test -j 8 --workspace --lib -- sniff::tests::   # 阶段 4 / M1a，`rurge-http` 判断与嗅探的本体（14 条）：方法记号、`PRI`、小写与 21 个字符、只来了一半、判的次数（一个字节一个字节发、一串小记录）、`sniff` 的五种结局（超时用调短的时限）、前缀在嗅探之后照旧完整
cargo test -j 8 --workspace --lib -- hold::   # 阶段 4 / M1a，`rurge-inbound` 的停留（4 条）：读到 EOF、读出错、`drop_hold` 到、停止立刻放手，以及 `linger` 读的那一段
cargo test -j 8 --workspace --lib -- sniffable   # 阶段 4 / M1a，`rurge-inbound` 两个监听器的先答后交（5 条）：SOCKS5 与 `CONNECT` 先答、再交给 `serve`，停留与转发的用例；`rurge-tun` 的 `tests::outcomes::serve::a_sniffable_connection_is_served_with_the_tuns_holds` 也带这个词，所以这一条过滤词会多一条
cargo test -j 8 --workspace --lib -- outcomes::serve   # 阶段 4 / M1a，`rurge-tun`（3 条）：可嗅探的会话整个交给 `serve`，拿到的 `ServeOpts` 是 TUN 的 `drop_hold` / `linger` 与停止令牌，缺省 `serve` 真的拨了号，停止到得了
cargo test -j 8 --workspace --test sniff   # 阶段 4 / M1a，经引擎的嗅探端到端（21 条；`EngineShared.sniff_wait` 300 毫秒，只有一条用缺省的 15 秒、不调短（`a_stop_during_the_wait_ends_a_tun_session_at_once`：让停止落在等待之中，不等满））：三种入口上的 SNI 与协议、`PROTOCOL` 在拨号之前、三个设置、预匹配与暂停、服务器先说话、前缀不丢、计数
cargo test -j 8 --workspace --lib -- engine::tests::   # 阶段 4 / M1b，`rurge-http` 的引擎本体（30 条）：每个请求的流程、守卫、应答正文的包装、逐跳的头、拒绝与失败、504 与两个页开关、借来的连接上的重发、升级与隧道里的 `CONNECT`；这个过滤词一共选中 102 条：除了 `rurge-http` 的 30 条，还连带 `rurge-engine` 的 9 条与 `rurge-rules` 的 63 条（`engine::tests` 51 条，`engine::tests::pre_match` 12 条）
cargo test -j 8 --workspace --test http   # 阶段 4 / M1b，经引擎的明文 HTTP 端到端（36 条）：四种入口、每条客户端连接的池与借来的连接、字节只数上游一侧、WebSocket、chunked、`Expect: 100-continue`、重发的两种情形、终止与停止、承载连接的记录、记录与会话日志的新字段、`requests` 的计数
cargo test -j 8 --workspace --lib -- mitm   # 阶段 4 / M2a：这个过滤词一共选中 45 条（`-- --list` 数，一台机器上的一次运行）——`rurge-config` 12 条（`mitm::tests::` 10 条：九个键、两张名单、`W0001` / `W0005` / `W0029` 与 `E0017` 与 `Debug`；另两条名字里有 `mitm`：`config::tests::the_mitm_section_is_typed_and_no_longer_deferred` 与 `hostlist::tests::mitm_hostname_example_and_special_tokens`）、`rurge-http` 30 条（`mitm::authority::tests::` 19 条：六条检查与 `W0039` 的各种坏法、材料指纹、叶证书的参数与换槽、缓存与并存、`Debug`；`mitm::decrypt::tests::` 11 条：何时解密、服务端配置与握手、三种警报、`Watch`、可嗅探的扩大）、`rurge-engine` 3 条（`mitm::tests::` 2 条：限频的两个边界；与 `outbounds::tests::load_checked_reports_an_unusable_mitm_ca`）
cargo test -j 8 --workspace --lib -- state::tests   # 阶段 4 / M2a，`rurge-engine` 的 `state.json` 第 2 版（9 条）：第 1 版的四个开关读作没设过、第 2 版是阶段 3 读得懂的、原子写与 `.broken`、`enhanced_backup` 的往返
cargo test -j 8 --workspace --test mitm   # 阶段 4 / M2a，经引擎的 MITM 端到端（29 条）：三种入口的解密与记录、同一条连接上的两个请求、不受信的源站与 `skip-server-cert-verify`、排除项与名单与来源、SOCKS5 上的 `127.0.0.1`、重载（同材料 / 另一张 CA / 坏了的 CA）、开关关着与只给 `h2` 的客户端、可嗅探的扩大、停止落在握手里与握手之后、不信任与证书固定的日志与限频、经虚拟网卡的行、证书固定与停止、WebSocket、明文不受影响、解密的请求经 HTTP 代理隧道
cargo test -j 8 --workspace --test api -- the_mitm   # 阶段 4 / M2a，`rurge-api` 的三条（3 条）：开关的读 / 切 / 写进 `state.json`、`GET /v1/mitm/ca` 的头与 DER 与 `?x-key=`、没有可用的 CA 时 404
cargo test -j 8 --workspace --test cli -- a_daemon_restores_the_mitm_switch_from_state_json   # 阶段 4 / M2a，可执行文件的一条（1 条）：起守护进程之后 `GET /v1/features/mitm` 读得到 `state.json` 里记着的值
cargo test -j 8 --workspace --lib -- version::   # 阶段 4 / M2b，`rurge-http` 的版本转换（5 条）：`:authority` 与 `Host`、拆开的 `cookie` 合成一个、`TE: trailers` 的去留、连接专用的头、转过去再转回来的幂等
cargo test -j 8 --workspace --lib -- responses::   # 阶段 4 / M2b，`rurge-inbound` 的三种应答的形状（3 条；M2b 加的是 `not_implemented_is_a_501_with_the_message`，HTTP/2 的 `CONNECT` 答 501 用的那一个）
cargo test -j 8 --workspace --lib -- engine::tests::   # 阶段 4 / M2b 之后这个过滤词一共选中 133 条（一台机器上的一次运行）：`rurge-http` 61 条（M1b 的引擎本体与 M2b 的 HTTP/2：客户端的服务端与执行器、在途与空闲、`CONNECT` 的 501、共享的上游、字节、被拒的流、终止与看门狗、GOAWAY 之后的一秒）、`rurge-engine` 9 条、`rurge-rules` 63 条（`engine::tests` 51 条、`engine::tests::pre_match` 12 条）
cargo test -j 8 --workspace --lib -- mitm   # 阶段 4 / M2b 之后这个过滤词一共选中 48 条：`rurge-config` 12 条、`rurge-http` 33 条（`mitm::authority::tests::` 19 条、`mitm::decrypt::tests::` 14 条，M2b 加了三条 `h2` 的）、`rurge-engine` 3 条
cargo test -j 8 --workspace --test mitm   # 阶段 4 / M2b 之后 36 条（M2a 的 29 条 + M2b 的 7 条端到端）：四种版本组合、`h2 = true` 下只给 `h2` 的客户端、每条流各自己的规则、并发的流共用一条上游连接、`cookie` 到 HTTP/1.1 源站、HTTP/1.1 客户端的 WebSocket 对讲 h2 的源站、`CONNECT` 的 501 与一条记录
cargo test -j 8 --workspace --lib -- mitm   # 阶段 4 / M2c 之后这个过滤词一共选中 61 条：M2b 之后的 48 条，加 `mitm::generate` 7 条、`mitm::api` 5 条与 `mitm::authority` 的 `an_api_that_cannot_start_is_named` 1 条。按 crate 分：`rurge-config` 12 条、`rurge-http` 46 条（`mitm::authority::tests::` 20、`mitm::decrypt::tests::` 14、`mitm::generate::tests::` 7、`mitm::api::tests::` 5）、`rurge-engine` 3 条
cargo test -j 8 --workspace --test mitm   # 阶段 4 / M2c 之后 37 条：M2b 之后的 36 条，加 `api_tls_is_the_last_ca_a_generation_had` 1 条
cargo test -j 8 --workspace --lib -- mitm::generate   # 阶段 4 / M2c，`rurge-http` 的新 CA（7 条）：P-256、回拨 2 天、10 年、带 SKI 的 CA；p12 是 3DES 加密、HMAC-SHA1 的 MAC、读得回；`[MITM]` 节能被 `Authority::load` 读进来；用它签一张叶证书、在回环上握一次手；两张 CA 不同；`Generated` 的 `Debug` 里没有材料；PEM 是标准文字（每行 64 个字符）
cargo test -j 8 --workspace --bins -- cli::mitm   # 阶段 4 / M2c，可执行文件的 `cli::mitm`（1 条）：`write_new` 有一个文件写不成时删掉这一次写出的，本来就有的路径不动
cargo test -j 8 --workspace --test cli -- mitm_ca_   # 阶段 4 / M2c，`rurge mitm ca generate` / `export` 的端到端（这台机器上 4 条：四个文件与不打印口令、不覆盖也不建目录、`export` 打出证书、`export` 对用不了的 CA 报错；第 5 条 `mitm_ca_generate_keeps_the_secrets_private` 带 `#[cfg(unix)]`，Windows 上不编译，只有 CI 编译与运行）
cargo test -j 8 --workspace --lib -- mitm::api   # 阶段 4 / M2c，`rurge-http` 的 API 的 TLS（5 条）：叶证书按 SNI 或本地地址签、模板只给 `http/1.1` 且不收 0-RTT、信另一张 CA 的客户端被拒、API 与解密的连接共用叶证书的缓存、`client_config` 给的不是证书时是错误
cargo test -j 8 --workspace --test mitm -- api_tls   # 阶段 4 / M2c，`rurge-engine` 的一条（1 条）：`Engine::api_tls()` 是最后一个生效过的 CA（重载换 CA、重载让 CA 用不了之后各问一次）
cargo test -j 8 --workspace --lib -- tls::   # 阶段 4 / M2c，API 的 TLS 监听器的单元用例（8 条，都在 `rurge-api` 的 `tls::tests::` 里）；这个过滤词一共选中 115 条（一台机器上的一次运行）：`rurge-api` 8、`rurge-config` 15、`rurge-http` 10（M1a 的 ClientHello 与两张根存储）、`rurge-net` 2、`rurge-proto` 80（各出站协议的 TLS 传输）
cargo test -j 8 --workspace --test api -- tls   # 阶段 4 / M2c，`rurge-api` 的两条（2 条）：API 经 TLS 服务时拿到的是 MITM CA 签的证书；`serve_tls` 没有 CA 时照样绑上、什么也不答
cargo test -j 8 --workspace --test cli -- run::mitm   # 阶段 4 / M2c，`http-api-tls` 的两条端到端（2 条）：`status` 与 `stop` 经 HTTPS 够得到 API（`--remote` 仍是明文、连不上）、没有 CA 时 API 不起而 rurge 照常跑
cargo test -j 8 --workspace --bins -- an_https_endpoint_needs_a_usable_ca   # 阶段 4 / M2c，命令行的 `Endpoint`（1 条）：`http-api-tls` 把 Profile 里那张 CA 的证书放进 `Endpoint`，CA 用不了时在任何请求之前就报错；`what_a_reload_says_about_the_api` 钉住重载时关于 API 的每一句（`cargo test -j 8 --workspace --bins -- what_a_reload_says_about_the_api`）
cargo test -j 8 --workspace --lib -- dialect   # 阶段 4 / M3a，`rurge-config` 的 `dialect`（7 条）：九种 ICU 写法在类外的改写与别的照抄、类里的成员与三种拒绝、改写之后的式子意思与 ICU 一致、`Template` 照 NSRegularExpression 展开、`Word` 与 `Template` 的 `Debug` 不显示文字
cargo test -j 8 --workspace --lib -- rewrite   # 阶段 4 / M3a：这个过滤词一共选中 58 条（一台机器上的一次运行，`-- --list` 数）——`rurge-config` 14 条（`rewrite::tests::` 7 条：两个节的读法与每一种诊断、`header-replace` 换成一行、框架头的 `W0041`、值的两种写法、`Debug`；`dialect::tests::translate_rewrites_the_icu_escapes`、`redact::tests::rewrite_lines_are_shown_as_written`、`config::tests::rewrite_sections_are_read_and_no_longer_deferred` 各 1 条；`[IP Rewrite]` 的 4 条）、`rurge-http` 21 条（`rewrite::tests::` 4 条、`rewrite::url::tests::` 9 条、`rewrite::header::tests::` 6 条，另加 `engine::tests::` 里名字带 `rewrite` 的 2 条）、`rurge-policy` 1 条、`rurge-rules` 1 条、`rurge-tun` 21 条（阶段 3 / M5 的 `tests::rewrite::`，与 M3a 无关）
cargo test -j 8 --workspace --lib -- headers_are_rewritten not_rewritten head_is_rewritten redirect_is_answered rewrite_reject header_mode_sends retargeted changed_host_is_written_back deleting_ request_whose_framing response_whose_framing plain_connection_uses_the_rewrite pool_keeps_plain backtracking_rule origin_keys host_that_is_no_authority   # 阶段 4 / M3a，`rurge-http` 的引擎里 M3a 加的 18 条：它们没有共同的词，这 16 个过滤词正好选中这 18 条（一条不多、一条不少）。M3a 之后 `--lib -- engine::tests::` 一共选中 151 条（`rurge-http` 79、`rurge-engine` 9、`rurge-rules` 63）
cargo test -j 8 --workspace --test rewrite   # 阶段 4 / M3a，经引擎的端到端（23 条）：手册的两页黄金用例（`header`、`302`、`307`、`reject`、"Matching details"、Header Rewrite 页的四种动作与省略方向、`header-del` 与 `header-add` 连用、改框架头而失败）、开关、被拒的请求不重写、`302` 带网关、计数跨重载，以及解密的 `https://` 请求、解密的 HTTP/2 请求改写到明文、`http://` 改写成 `https://`、明文与 TLS 不共用连接、经转发的代理、`Host` 的写回与删除、嗅探到的连接上的绝对形式、`Proxy-Authorization`、回溯超限、头的值与查询串不进备注与日志、六个计数
cargo test -j 8 --workspace --lib -- body   # 阶段 4 / M3b：这个过滤词一共选中 90 条（一台机器上的一次运行，`-- --list` 数）——`rurge-http` 34 条（`rewrite::body::tests::` 的 23 条是任务 3、4 的全部单元用例，另有 `engine::tests::` 里名字带 `body` 的 11 条）、`rurge-config` 9 条（`dialect::tests::` 里 `BodyPattern` 的 5 条、`rewrite::tests::` 2 条、`config::tests::` 1 条、`keystore::tests::` 1 条）、`rurge-net` 27 条（大多是名字里带 `nobody` 的，与正文无关）、`rurge-engine` 7 条、`rurge-dns` 4 条、`rurge-policy` 3 条、`rurge-proto-quic` 2 条、`rurge-tun` 2 条、`rurge-platform` 与 `rurge-rules` 各 1 条；`body` 也命中 `nobody`，所以 90 条不是 Body Rewrite 的条数
cargo test -j 8 --workspace --lib -- map_local   # 阶段 4 / M3b：20 条（一台机器上的一次运行，`-- --list` 数）——`rurge-config` 11 条（`rewrite::tests::` 8 条、`config::tests::` 2 条、`reach::tests::` 1 条）、`rurge-http` 9 条（`rewrite::map_local::tests::` 5 条、`engine::tests::` 4 条）
cargo test -j 8 --workspace --test rewrite   # 阶段 4 / M3b 之后 50 条：M3a 的 23 条，加 M3b 的 27 条（任务 6 的 3 条与任务 7 的 24 条）
cargo test -j 8 --workspace --lib -- capture::   # 阶段 4 / M4a，`rurge-http` 的 `capture` 模块（22 条，其中 2 条带 `#[ignore]`、量内存的两条见本组最后两行）：一条的账与记账、单条与总量两种截断、跨会话的挤掉、丢掉与预留在放下时还回去、清除与四个数、按 ID 找、`put`、`Limits` 的降低、两个正文的解开（多层、没有编码、截断、`br` 一类）、存进去的头不与 hyper 的读缓冲共享，以及 `started_rfc3339`
cargo test -j 8 --workspace --lib -- captured   # 阶段 4 / M4a，`rurge-http` 引擎测试模块里抓包的 16 条（名字里都带 `captured`，`capture::` 选不到它们）：两个取样点各自的样子、借来的连接、改写过的请求正文、被拒绝的、Map Local、101 与 `CONNECT`、没有应答的、客户端走掉、HTTP/2 上重置、截断过的正文、trailer、HTTP/2 客户端的版本
cargo test -j 8 --workspace --test capture   # 阶段 4 / M4a，`rurge-engine` 的端到端（15 条）：开关与 `always-capture`、`FINAL`、逻辑规则、直连模式、关掉之后、`state.json`、日志、拒绝、监听器的口令、上游代理的凭据、解密、重载。这一条不写过滤词：写 `-- capture` 只选中 12 条，另外 3 条的名字里没有这个词（`a_logical_rule_names_the_session_of_its_line`、`entries_outlive_a_reload`、`the_session_a_rule_names_comes_before_default`）
cargo test -j 8 --workspace --test api -- capture   # 阶段 4 / M4a，`rurge-api` 的四个（4 条）：开关的读 / 切 / 写进 `state.json`、状态、会话列表、一个会话的摘要（由新到旧）
cargo test -j 8 --workspace --test api -- har   # 阶段 4 / M4a，`rurge-api` 的 HAR（5 条，名字里没有 `capture` 这个词）：旧到新、正文的文字与 Base64、没有应答、遮住与空的 `cookies`、查询串与 `CONNECT`
cargo test -j 8 --workspace --test api -- entry decode clearing odd_session   # 阶段 4 / M4a，`rurge-api` 的另外五条（5 条）：一条的全部与四个遮住的头、ID 与 `404`、`?decode` 的三种情形、清除一个会话或全部、名字奇怪的会话
cargo test -j 8 --workspace --bin rurge -- sizes_are_bytes_with_an_optional_binary_suffix the_body_size_is_lowered_to_the_capture_size   # 阶段 4 / M4a，可执行文件的两个单元用例（2 条）：大小的写法与两种错误、单条大于总量时降到总量
cargo test -j 8 --workspace --test cli -- capture   # 阶段 4 / M4a，命令行的端到端（4 条）：两个参数从选项或环境变量来、单条被降低的那一行 WARN、写错的参数以 2 退出、经 API 打开开关之后导出 HAR
cargo test -j 8 --workspace --lib -- capture::memory::an_empty_store_is_the_baseline --ignored --exact --nocapture   # 阶段 4 / M4a，量内存的基线（1 条，`#[ignore]`；缺省的 `cargo test` 与上面那两行都不跑它）：只建一份空的存储、读出峰值工作集
cargo test -j 8 --workspace --lib -- capture::memory::a_full_store_at_the_default_limits --ignored --exact --nocapture   # 阶段 4 / M4a，量内存的装满（1 条，`#[ignore]`）：按缺省的两个上限装满、读出峰值工作集与 `status()` 的各个数；与基线各自单独跑，两条不并在一起（两次读的是两个进程各自的峰值，见 `docs/api/phase4.md` 的「量到的内存」）
cargo test -j 8 --workspace --lib -- observe::memory::an_empty_request_log_is_the_baseline --ignored --exact --nocapture   # 阶段 4 / M4b，量记录大小的基线（1 条，`#[ignore]`；缺省的 `cargo test` 不跑它）：只建一份空的 `RequestLog::new(100_000)`、读出峰值工作集
cargo test -j 8 --workspace --lib -- observe::memory::a_full_request_log_of_http_requests --ignored --exact --nocapture   # 阶段 4 / M4b，量记录大小的装满（1 条，`#[ignore]`）：把 100 000 条像 HTTP 请求的记录装进 `RequestLog`、读出峰值工作集与 `len()`；与基线各自单独跑，数在 `docs/api/phase4.md` 的「量到的一条记录」
# 阶段 3 / M5 重跑的基准读数（一台机器上的一次运行、**不是 CI**；release，`TUN_BENCH_FAR=memory`；命令仍是上面 B1 / B2 / b3 那三条）：B1 八个量出来的行都过 0.7 Gbit/s 的线（最低的一行 4.065，是线的 5.8 倍），四个计数都是 0，中位数上行 8.03、下行 4.84 Gbit/s（两个 MTU 混在一起，只作量级）；同设置（MTU 1500、`tx_max` 256 KiB）的 5.717 / 5.172 是上一次的 79.7% 与 94.7%。B2 门槛读数（峰值减去全部测试一侧的项）403,671,140 字节（403.7 MB / 385.0 MiB），在线 429,000,000 字节以内、余量 25.3 MB；保守读数（只减去准的应用端套接字缓冲）445,333,604 字节（445.3 MB），超线 16.3 MB——这个读数照旧归项目所有者，这里只记数。b3 中位数 0.9 微秒、p99 2.9 微秒，最大 4542.2 微秒（4.5 ms，落在第 28,673 个查询上，是线的 22.7 倍，两处都是池的 `HashMap` 的翻倍处，读代码与算术得出）。细节与上一次不同的原因见计划「执行期修正记录」第 61 ～ 63 行
cargo insta test -p rurge-config --review       # 语料库快照有变化时审阅
cargo clippy --all-targets -- -D warnings       # 零警告（CI 设 RUSTUP_TOOLCHAIN=1.89.0，与本机的 MSRV 工具链一致；更新的 clippy 会多出 lint）
cargo fmt --all
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform；--data-dir 连同已缓存的订阅与已下载的 Profile 来源——托管配置的副本、远程 include——一起检查，不联网，也不创建或写数据目录）
cargo run -p rurge -- rule match -c config.conf example.com --explain   # 离线测试规则匹配
cargo bench -p rurge-rules                      # criterion 基准（域名 / IP 索引、规则引擎；集合的两组：-- no_resolve 是十万行带 no-resolve 的 IP 行，-- asn 是五千行 IP-ASN；不是门禁，数字在阶段 1 / M5a 计划的执行期修正记录第 54、61 行）
cargo run -p rurge -- dns lookup -c config.conf example.com --trace    # 离线 DNS 解析（--server 覆盖上游）
cargo bench -p rurge-dns                        # criterion 基准（DNS 缓存命中）
cargo run -p rurge -- run -c config.conf --log-level info   # 前台运行 HTTP / SOCKS5 代理（Ctrl-C 退出）
cargo run -p rurge -- run -c config.conf --watch --log-file rurge.log   # 加配置热重载（SIGHUP / --watch）与滚动日志文件
cargo run -p rurge -- run -c config.conf --system-proxy   # 前台运行并把系统代理指向 rurge，退出时恢复；崩溃后在下次启动时恢复
cargo run -p rurge -- status -c config.conf --data-dir ./data   # 查看运行中实例的状态（reload / stop 同样支持 -c 或 --remote/--key；托管配置生效时 http-api 写在数据目录里的副本中，三个命令用 --data-dir 找到它，缺省是平台的数据目录）
cargo run -p rurge -- service install -c config.conf --user --dry-run   # 打印开机自启的安装计划（去掉 --dry-run 才真正写入 / 执行）
```

## 只供测试的后端

`RURGE_SYSTEM_PROXY_BACKEND=file:<path>` 把系统代理的读写重定向到一个 JSON 文件，是仅供自动化测试使用的后端（未知取值会报错退出），不要在正常使用中设置它。

`RURGE_NETWORK_BACKEND=file:<path>` 把对网络的读取改成读一个 JSON 文件（形状同 `rurge_platform::netenv::Snapshot`：`primary`、`wifi`、`dns`、`addresses`，每个字段都可以不写；文件不在或读不懂时是"什么都不知道"；每 100 毫秒看一次文件有没有变），是仅供自动化测试使用的后端（未知取值会报错、以 2 退出，从不退回真实的网络），不要在正常使用中设置它。它到不了 `external` 策略拉起的程序（那里没有任何以 `RURGE_` 开头的变量），所以 `rurge run`、`rule match`、`dns lookup`、`dns cache` 另有隐藏的选项 `--network-backend <file:path>`（同一个变量，不在 `--help` 里，在命令行上取代变量）。

`RURGE_PROCESS_BACKEND=off` 让 `rurge run` 不去查会话来自的进程（`shared.owners` 留空，请求记录里没有进程，`PROCESS-NAME` 一律按不知道判），是仅供自动化测试使用的开关（没设时查本机，恰好 `off` 才关，`OFF` 不行；别的取值会报错、以 2 退出——先恢复上一次崩溃留下的系统代理，在绑定任何端口之前——，从不退回真实的查找），不要在正常使用中设置它，它也不是给使用者的"关掉查找"的开关（M2-D3）。它不是命令行选项，所以不在 `--help` 里，也没有对应的隐藏选项；`rurge check`、`rule match`、`dns lookup` 不读它（它们不查进程）；`external` 策略拉起的程序拿不到它（那里没有任何以 `RURGE_` 开头的变量），嵌套的 `rurge run` 照常查本机，查找只读系统的表，不影响用例。

`RURGE_ENHANCED_BACKEND` 把增强模式要动的那套系统换掉（`crates/rurge/src/cli/enhanced/backend.rs`），是仅供自动化测试使用的后端，不要在正常使用中设置它。不设时是平台后端：**三个平台都是平台后端的代码**（`rurge_platform::enhanced::system`：Linux 用 `ip` / `resolvectl`，macOS 用 `ifconfig` / `route` / `networksetup`，Windows 用 `netsh -f` 与 `ipconfig`，Linux 上需要时改 `rp_filter`；设备是 `/dev/net/tun`、utun 与 Wintun）。三个平台的代码都在 CI 上编译、缺省用例通过（运行 40611702392，提交 `3a76b80`；Linux 与 macOS 专有的代码——`tun/linux.rs`、`tun/macos.rs` 与挑平台的 `cfg`——是第一次被编译），但缺省的用例不建网卡、不改系统设置；三个平台都没有在真机上跑过（阶段 3 / M4d；CI 一句是 M4 推送之后的读回）。三个取值：

- `RURGE_ENHANCED_BACKEND=file:<路径>`：把系统当成一个 JSON 文件（`crates/rurge/src/cli/enhanced/file.rs`）。文件里当前开着的各项是 `open`（设备开着没有）、`ipv6`（网卡有 IPv6 的一半没有）、`routes`（装着的前缀，字符串数组，排序并去重）、`dns`（系统 DNS 被接管没有）、`adjusted`（系统自己的调整做着没有），做过的动作按次序在 `actions` 里，每条一个字符串：`open`、`configure`、`configure_ipv6`、`unconfigure_ipv6`、`routes +<加了几条> -<减了几条>`、`take_dns`、`give_dns_back`、`adjust`、`close`、`undo <撤销动作的名字>`（`undo take_dns`、`undo adjust`、`undo rp_filter <值>`、`undo mac_dns <服务>`）。文件里不认识的键原样留着，所以用例可以在里面写下面三个。**失败的动作不记**，它对应的字段也不改：`actions` 里只有真的做成了的。
  - `"fail": "<动作名>"`：让那一个动作失败（错误 `<动作名> failed (test)`），只对着一个名字。`fail: routes` 只让"加"的那一半失败（`add` 非空时），关闭时减的那一半照做；`fail: configure_ipv6` 不算失败：`configure` 与 `configure_ipv6` 答"没有 IPv6"（打开与重载照常往下、管理器记一行 WARN），`configure` 那一步照样记一条 `configure`，`configure_ipv6` 什么也不记。
  - `"fail_undo": "<撤销动作的名字>"`：让那一条撤销失败（错误 `<名字> undo failed (test)`），字段不改、不记。
  - `"device_fails_after_ms": <毫秒>`：打开时读一次；过了这么久，设备的下一次读以 `the device failed (test)` 失败，TUN 随之以设备坏了结束（增强模式整个关掉，`GET` 变假）。
  - 设备什么也不运（从不送包进来，写来的包丢掉），MTU 1500，名字 `rurge-test`。文件的写不是原子的（每步重写整个文件、不是改名），所以读它的用例不要在守护进程还写的时候读：等 `GET` 的状态、等 API 的回复，或等这一次的整段动作都出现在文件里。
- `RURGE_ENHANCED_BACKEND=off`：打开即失败，错误 `the enhanced mode backend is off (RURGE_ENHANCED_BACKEND=off)`。起 `rurge` 二进制的辅助函数缺省就是这个值（见下面的测试约定）。
- 别的取值：``error: unknown RURGE_ENHANCED_BACKEND value `<取值>` (expected off or file:<path>)``，以 2 退出，退出之前先恢复上一次崩溃留下的系统代理（与 `RURGE_NETWORK_BACKEND` 同一个位置、同一个理由）。

这个变量到不了 `external` 策略拉起的程序（那里没有任何以 `RURGE_` 开头的变量），嵌套的 `rurge run` 因此用平台后端；它不是命令行选项，不在 `--help` 里，也没有对应的隐藏选项。

## 测试约定（全文）

### M8c 起的通用约定

测试约定（M8c 起）：测试只用回环地址和端口 0。
唯一的例外：要先于主人绑定就写进配置的端口，或必须一直关着的端口，取自 20000–29999 并现场试绑一次（`closed_tcp_port()` / `closed_udp_port()` 在 `crates/rurge-engine/tests/common/mod.rs`，`free_port()` 在 `tests/external/tests/common/mod.rs`，`free_tcp_port()` 在 `crates/rurge/tests/cli.rs`，参照服务端的端口用 `tests/interop/src/lib.rs` 的 `free_port()`、`free_udp_port()`、`free_tcp_and_udp_port()` 与 `free_ports()`；20000–29999 在 Linux（32768–60999）、macOS 与 Windows（49152–65535）缺省的临时端口区间之外，系统不会把这一段分给别的套接字——这台开发机的区间不是缺省的，见 `CLAUDE.md`「这台开发机」），不用 `bind(0)` 再放掉（放掉的号会被并行用例的 `bind(0)` 拿走），也不用"绑了不听的 TCP socket"当关着的端口（Linux 与 Windows 拒绝连接，macOS 默默丢掉 SYN，连接会挂住）；"一个没有应答的地方"是测试自己持有的监听器，不是某个固定端口；数"服务器被问了几次"的用例用不重发的 `FanoutOpts`；对守护进程做事之前先等它说准备好了；"断言什么都没发生"的等待是仅有允许的固定等待，注释要写它等的是什么、余量多大；经自动组拨号会在后台触发一轮测试，所以拨号之后的断言要在那一轮何时结束时都成立，数成员失败次数的用例让成员不参加轮次（`crates/rurge-engine/tests/common/mod.rs` 的 `UNTESTED`），新写的这类用例把 `NO_TEST`（`crates/rurge-engine/tests/common/mod.rs`，`http://127.0.0.1:9/`）指到关着的端口再跑一遍；抓日志的用例用 `LogCapture`（`crates/rurge-engine/tests/common/log_capture.rs`）：`start()` 把捕获装在调用它的线程上，用例要跑在当前线程运行时（普通的 `#[tokio::test]`），否则别的工作线程上的日志行抓不到；`crates/rurge/tests/cli.rs` 里，守护进程在打印 `listening on` 之前就退出时，用例带着它已写到标准错误的内容失败；测试里手写的 HTTP 客户端把每个请求——请求头与正文——在一次写里发出（`crates/rurge/tests/cli.rs` 的 `api_call` 与 `http_get_with` 就是这样），只有专门测"正文晚到"的用例（`crates/rurge-api/tests/api.rs` 里 `a_late_body_does_not_cost_…` 那一组，`crates/rurge/tests/cli.rs` 的 `api_call_with_a_late_body`）分几次写：API 的监听器自阶段 1 补遗（M5b）起分步关连接，正文晚于请求头到达的客户端读得到应答，这条规矩因此只是最简单的写法，不再是为了绕开那个性质（此前不读正文的端点凭请求头就答复并关掉连接，在 Windows 上这样的客户端丢掉它还没读的应答）。

### 阶段 1 补遗（M5b）起：失败时也留在回环、`rurge_command()`、改一行看它变红

阶段 1 补遗（M5b）起另有三条：一、测试在**失败**时也不得离开回环、离开自己的临时目录——Profile 里写了主机名的用例另给第二道防线（一条按地址的规则、一行把名字钉到 `127.0.0.1` 的 `[Host]`，或一个会拒绝的连接器），规则失灵时会话仍留在回环；没有哪条用例把回环之外的地址（文档地址也算）写成可能被拨的目标；资源管理器自己的测试把客户端建在一个拒绝一切名字的解析器上（`crates/rurge-net/src/resource/mod.rs` 的 `manager()` 与 `NoNames`）——带主机名的链接在那里只能登记、不能解析，真的下载的用例用回环上的 `TestServer`，新写的建管理器的测试照这样做；二、起 `rurge` 二进制的用例一律经 `crates/rurge/tests/cli.rs` 的 `rurge_command()` 或 `rurge_process()`：它们把缺省数据目录与配置目录所读的变量（`LOCALAPPDATA`、`APPDATA`、`XDG_DATA_HOME`、`XDG_CONFIG_HOME`）指到构建自己的临时目录下的一个草稿目录，并从子进程的环境里去掉 `RURGE_DATA_DIR` 与 `RURGE_LOG_FILE`（要用它们的用例在构造函数之后自己设；`HOME` 有意不动，所以在 macOS 上忘了 `--data-dir` 的用例仍够得到缺省目录，那里仍要给 `--data-dir`；`RURGE_SYSTEM_PROXY_BACKEND` 也有意不动，它由起守护进程的辅助函数设置，去掉的话忘了文件后端的用例会够到真的系统代理）；三、做"改一行代码看用例变红"的验证之前，先说清这一改会让哪些会话、查找、下载与文件去哪里，可能离开回环或写到用例的临时目录之外的，不做。

### 阶段 1 补遗（M5b）的计划与设计里的四条

阶段 1 补遗（M5b）的计划（「全局约束」）与设计（第 14 节第 29 条）里还有四条，这里各记一句：在线跑起来的引擎或守护进程要把 GeoIP 的两个下载地址指到回环（引擎用例的 `geo_urls`，命令行的 `--geoip-url` / `--geoip-asn-url`），否则它会去取公网的库；拉起 `rurge` 的用例一律给 `--data-dir`（或 `RURGE_DATA_DIR`），指到用例自己的临时目录（`check`、`rule match`、`dns lookup` 与三个控制命令都读数据目录里 Profile 来源的副本，而缺省的平台数据目录是开发机上真实的那一个）；会让一个工作线程停住的用例在用例自己的线程上按系统时钟等，不等运行时的定时器（tokio 1.53.1 的多线程调度器里握着驱动的那个工作线程不把驱动交出去）；新写的计时用例提交前单独循环跑二十次以上。

### 系统解析库

调真正的系统解析库的用例数到五条，都只问 `localhost`：经解析器的 `the_platform_library_answers_localhost_with_loopback_addresses`（`crates/rurge/src/cli/runtime.rs`），和 `crates/rurge-proto` 里把代理服务器写成 `localhost` 的四条（`http::tests::https_wraps_the_proxy_connection_in_tls`、`http::tests::a_proxy_that_wants_a_client_certificate_says_so`、`socks5::tests::socks5_tls_wraps_the_session_in_tls`、`trust_tunnel::tests::the_sni_is_the_server_name_or_the_sni_parameter`，经 `rurge_net::connector::SystemResolve`）。
这个数来自两次临时的探针（都已还原）：第一次看解析器的那个接口，连 `cli.rs` 起的二进制在内；第二次看 `SystemResolve`，只看用例自己的进程；互操作用例在开发机上立刻返回、只在 Unix 上编译的用例没有编译，两次都不在其内。
新的用例不许把别的名字交给系统解析库。

### 阶段 3 / M1 起：网络环境

阶段 3 / M1 起另有两条：一、`rurge_command()` 与 `rurge_process()` 缺省把 `RURGE_NETWORK_BACKEND` 指到构建自己的临时目录下的一个不存在的文件（每次调用一个，`crates/rurge/tests/cli.rs` 的 `unknown_network()`），所以起守护进程或离线命令的用例看到的网络是"什么都不知道"，从不读本机真实的网络，真实的网络变化也不会在用例中途清 DNS 缓存、作废测速；要模拟网络的用例自己写那个文件（先写临时文件再 `rename`：文件后端每 100 毫秒读一次，`std::fs::write` 先截断再写，会读到空文件）并在构造函数之后把变量指过去；二、`external` 策略拉起的程序拿不到任何以 `RURGE_` 开头的变量，所以用例里作为 `external` 程序起的嵌套 `rurge run`，要在策略的 `args` 里带隐藏的 `--network-backend file:<一个不存在的文件>`（`run_starts_an_external_program_and_stops_it_on_exit` 就是这样），否则它会去读本机的网络。

### 阶段 3 / M2 起：进程识别

阶段 3 / M2 起另有一条：`rurge_command()` 与 `rurge_process()` 缺省设 `RURGE_PROCESS_BACKEND=off`（`crates/rurge/tests/cli.rs`），所以起守护进程的用例不查进程（不多出阻塞线程、读表与 DEBUG 行）；查真实进程的那一条端到端用例（`a_process_rule_matches_the_test_process`）在构造函数之后 `env_remove("RURGE_PROCESS_BACKEND")`；`external` 策略拉起的嵌套 `rurge run` 拿不到任何 `RURGE_` 开头的变量，它的会话照真的查（只读系统的表；它的 Profile 里没有 `PROCESS-NAME` 规则，没有谁等它），不需要带什么选项；碰到进程查找的用例不用暂停的时钟（`spawn_blocking` 不算运行时的工作，暂停的时钟会在阻塞线程干活时自动前进，让 200 毫秒的期限提前到），会在阻塞线程上挂住的假查找在用例持有的闸被丢掉时放开（tokio 的运行时退出时等全部阻塞任务结束）；进程识别的用例只读系统的表，从不改系统设置、不建网卡、不起本仓库之外的程序。

### 阶段 3 / M3a 起：`rurge-tun`

阶段 3 / M3a 起另有一条：`rurge-tun` 的用例只拨用例写给假 `Dialer` 的回环地址（写了别的就 panic），所以没有一条的连接能离开回环；起 TUN 的辅助都经 `src/tests/support.rs` 的 `tun_dialing`（`Tun::start` 只在这一处调，M3b 加参数只改它）；要一连开关上千条回环 TCP 连接的用例，上游用内存里的（`Script::MemoryEcho`），不用回环上的服务器——两千条连接在开发机上用光过临时端口（`os error 10048`，单独连着跑 12 次红 9 次）；跑 TUN 核心的用例不暂停 tokio 的时钟（核心读 `std::time::Instant`，夹具跟 tokio 的时钟，混着暂停会空转），计时的用例在真实的时间里跑：能调的取短的选项（保活 100 毫秒、超时 3 秒、`linger`、`drop_hold`），停止的宽限 `STOP_GRACE`（1 秒）是真等的，用例给它的余量 `MARGIN` 是 3 秒。要片或写者被挡住才成立的用例，前提在各任务的任何先后下都要成立：读设备的任务从不等队列（预算装不下就丢），片一轮至多取 `INGRESS_PER_WAKE` 个，写者一轮从每个队列至多取 `writer_queue` 批，所以预算要装得下比片被挡住之前能取的更多的包；`tests::dispatch` 的那一条另有一份跑在当前线程的运行时上，让读设备的任务跑在片前面，旧的预算在那里三次都红在与 CI 相同的那一行（CI 运行 41402787066 在 macOS 上撞上过，M4b 记录第 75 ～ 80 行）。

### 第二次 CI 运行起：UDP 的缓冲

第二次 CI 运行（`7bd6589`）起另有两条：一、要发大于 9216 字节的 UDP 数据报的测试 socket，先把发送缓冲调高（`socket2::SockRef::set_send_buffer_size`，例如 256 KiB；macOS 上 UDP socket 的缺省发送缓冲是 9216 字节，即 `net.inet.udp.maxdgram`，XNU 里的 `udp_sendspace`，更大的数据报以 `EMSGSIZE` 失败），例子在 `crates/rurge-engine/tests/common/mod.rs` 的 `udp_associate`；二、从 `PacketSocket` 收包的缓冲给 64 KiB（那是它 `recv_from` 的约定，小了在 Linux 与 macOS 上被截断、在 Windows 上以 `WSAEMSGSIZE` 失败），例子是 `tests/interop/tests/common/mod.rs` 的 `udp_roundtrip`。

### 阶段 3 / M3b 起：经引擎的 TUN 用例

阶段 3 / M3b 起另有三条：一、经引擎的 TUN 用例（`crates/rurge-engine/tests/tun.rs`）守 M3b 计划 P17 的两道防线：规则以 `SRC-IP,198.18.0.1/32,REJECT` 与 `SRC-IP,fd00:6152::1/128,REJECT` 收尾（`guarded()`），在 harness 追加的 `FINAL,DIRECT` 之前，所以应用发出而没有规则接住的会话被拒，到不了 `FINAL`；名字由 harness 的 MockDns（或 `[Host]`）钉到 `127.0.0.1`，再由一条显式的规则交给 `DIRECT`，应用连的是它经 `TunHandle::pool()` 取到的 fake 地址；真实 IP 的 TCP 只用文档地址（`192.0.2.0/24`、`2001:db8::/32`），经一个 `FakeSocks5`（`Socks5Script { connect_to: Some(回环上的 echo) }`，不论被问到哪里都连到那个 echo）；发往真实 IP 的 UDP 只交给 `REJECT` 一族；ICMP 只经假 `Pinger`（`FakePinger`，什么也不发出去）。
二、塞进 TUN 的每个 UDP 包与 ICMP 回显请求都带正确的校验和（`classify` 验它们，不对的计 `malformed`）：用 `rurge_tun::testing::packets`（`udp`、`echo_request`）造；数 ICMP 消息条数的用例把 `writer_queue` 设到至少 400（接收任务与 `UdpClient::refused` 不等写任务的队列，满了就丢并计 `queue_full`；`crates/rurge-tun/src/tests/udp/refused.rs` 的 `counting_icmp()`）。
三、`TunHandle::session_tasks()` 数的是不属于 TUN 自己的任务——会话、回显和应答器的每个查询的任务（R10），不数片、写任务、读设备的任务与应答器的队列任务——，只在 TUN 还在跑时读：阶段 3 / M3c 的任务 0 起它开头有 `debug_assert!`，停止之后调它在调试构建里 panic（发行构建里 panic 或少算）；停止之后读任务数用 `live_tasks()`；两个都是 `cfg(test)` 的。

### 阶段 3 / M3c 起：TUN 与 DNS

阶段 3 / M3c 起另有几条：一、`TunOptions::udp_dns_idle` 不得大于 `udp_idle`（`check` 拒绝），所以把 `udp_idle` 调到 10 秒以下的用例要同时把 `udp_dns_idle` 调到不大于它的值（`crates/rurge-tun/src/tests/udp/sessions.rs` 里的几处就是这样），想测只问 53 端口的会话更早结束的用例反过来单调 `udp_dns_idle`；二、连发一阵查询的用例把 `writer_queue` 设到比这一阵大（应答器在 UDP 上的应答 `try_send` 给写任务，满了就丢并计 `queue_full`，R12；TCP 上的应答写进连接，由分片送出、等空位，不丢；`a_burst_of_queries_is_answered` 用 1,100）。**阶段 3 / M5 起写任务有两条队列**（`udp-priority`）：UDP 的应答（含它分出的片）进优先队列；TCP 的段与重置、ICMP 的消息、回显应答与 `[IP Rewrite]` 送回的都是普通队列，`writer_queue` 是**各自**的容量——上面“设到比这一阵大”要对两条队列都成立才不会丢；三、`rurge-tun` 的用例里应答器的转发只去 `FakeDns`（`rurge_tun::testing`，什么也不发出去），`rurge-dns` 里转发的用例只问回环上的 `MockDns` 或内存里的上游（`MockDns::upstream()`），依赖时间的走暂停的时钟；四、经引擎的 DNS 用例（`crates/rurge-engine/tests/tun_dns.rs`，接线在 `tests/common/tun.rs`）守 M3c 计划 P24 的两道防线：TUN 一侧经应答器或池问到的名字由 harness 的 `MockDns` 钉到回环，规则照 M3b 的 `guarded()` 收尾；监听器一侧要测换名字的用例，先用一条显式的规则放过那个名字（`DOMAIN,target.test,DIRECT`），紧跟 `IP-CIDR,198.18.0.0/15,REJECT,no-resolve` 与 `DEST-PORT,<上游端口>,REJECT`，换名字失灵时会话被拒，到不了 harness 追加的 `FINAL,DIRECT`；五、要看到应答器的 TCP 连接占了几个位置（`dns_tcp_max`）用 `TunHandle::dns_connections()`（只在测试构建里；位置在分片收下 SYN 时占、会话返回时还，握手不完成的在半开超时后还），要看池里有几个地址有租约、几个名字有地址用 `FakeIpPool::leased()` 与 `named()`，不要读池的 `Debug` 文字去数。

### 阶段 3 / M4a 起：增强模式的测试后端

阶段 3 / M4a 起另有几条：一、`rurge_command()` 与 `rurge_process()` 缺省设 `RURGE_ENHANCED_BACKEND=off`（`crates/rurge/tests/cli.rs`，说明在 `rurge_command` 上一段），所以没有一条起 `rurge` 二进制的缺省用例能走到平台后端——它一接到动作就会去改这台机器的路由与系统 DNS；`mod run` 的 `rurge_run_with` 另去掉 `RURGE_ENHANCED_MODE`（它不继承开发机上的这个变量）；二、要用文件后端的用例在构造函数之后自己设 `file:<路径>`（`crates/rurge/tests/run/enhanced_mode.rs` 的 `run_with_the_file_backend` 与 `run_enhanced` 就是这样），路径在用例自己的临时目录里，文件不在时后端把它当作"什么也没做过"；三、**文件后端的写不是原子的**（每个动作重写整个文件，不是先写临时文件再改名），所以读它的用例不要与守护进程的写抢：等 `GET /v1/features/enhanced_mode` 的状态、等 API 的一个回复（`POST /v1/profiles/reload` 的回复在这一次的 `follow` 之后才发），或等这一次的整段动作都出现在 `actions` 里（拿定动作条数之后 `wait_until` 比一整段），等到了再读别的字段——任务 8、9 的修正轮各在这一点上红过一次（M4a 计划「执行期修正记录」第 69、70、80、81 行）；四、池的文件 `fake-ip.txt` 的 `0600` 用例（`the_pool_file_is_its_owners_alone`）是 `#[cfg(unix)]` 的，只在 CI 的 Linux 与 macOS 上编译、运行；五、可执行文件这个 crate 里没有 `LogCapture`，所以"关闭等 TUN 超过 3 秒那一行 WARN"没有用例钉住（M4a 计划「执行期修正记录」第 47 行）：`closing_waits_for_the_tun_three_seconds_at_most` 只断言 `disable` 返回、`close` 被调用，以及在暂停的时钟上恰好等到 3 秒。

### 阶段 3 / M4b 起：出口的扫描、运行时用例与 ICMP

阶段 3 / M4b 起另有几条：

一、扫描用例 `crates/rurge/tests/sockets.rs` 的 `every_outbound_socket_is_made_in_one_place` 读 `crates/*/src` 下每一个 `.rs`（`tests/interop`、`tests/external` 这类只供测试的成员不读）。跳过测试代码：路径里有名为 `tests` 或 `testing` 的目录、或文件名是 `testing.rs` 的整个跳过（它们由 `#[cfg(test)]` 或 `testing` 特性门住）；文件里从第一个内联测试模块（`#[cfg(test)]` 紧跟 `mod <名字> {`，中间可以隔着属性、注释与空行）那一行起到文件末尾跳过，那之后在第 0 列开始的条目（`fn`、`pub …`、`impl`、`struct`、`enum`、`union`、`const`、`static`、`use`、`type`、`trait`、`mod`、`extern`、`macro_rules!`，前面可以有 `pub` / `pub(…)` / `async` / `unsafe` / `default` 与 `fn` 之前的 `const`；`extern` 是条目的关键字，不是修饰）由用例报错、带文件与行号（仓库的约定：测试模块在文件末尾）；`#[cfg(test)]` 紧跟 `mod <名字>;`（另一个文件里的测试模块）不结束文件。每行只看 `//` 之前的部分（文档注释整行跳过）。找的写法共 15 种：`UdpSocket::bind(`、`TcpStream::connect`、`TcpSocket::new_v4(`、`TcpSocket::new_v6(`、`Socket::new(`、`Socket::new_raw(`、`TcpListener::bind(`、`build_http(`、`lookup_host(`、`DirectConnector::new(`、`NoopSocketHook)`（`Arc::new(NoopSocketHook)` 与带路径的 `Arc::new(rurge_net::socket::NoopSocketHook)` 都算）、`Endpoint::client(`、`libc::socket(`、`socket::socket(`（`nix::sys::socket::socket(` 按标识符边界也算它的命中）、`resource_client(`。命中的边界是“前一个字符不是标识符字符”：`TosSocket::new(` 不算（`rurge-net` 的 `quic/socket.rs`），`socket2::Socket::new(` 算。加一处建套接字、问系统解析库或建不带钩子的连接器的地方，就在 `LISTED` 里加一行（文件、写法、理由写它是哪一个函数）并跑一遍这条用例；去掉一处就删一行；M4c / M4d 加设备的套接字时同样（macOS 的 utun 控制套接字）。名单与找到的写法**逐处**比对（不按（文件，写法）去重）：同一个文件里同一种写法的第二处也要一行，所以“多一处、少一处都变红”按处成立。一条照实的限制：一行只读 `//` 之前的部分，所以字符串里的 `//`（例如一个网址）之后也不读。

二、运行时用例 `crates/rurge-engine/tests/egress.rs` 用一个记录的钩子（`EgressLog`：记下每个被问到的地址，可以答 `Ok(None)` 或答 `Err`），七类上游各一次（`every_kind_of_dns_upstream_asks_the_hook_before_it_dials`）、流水线的回退（`the_pipeline_fallback_asks_the_hook`）、资源下载（`resource_downloads_ask_the_hook`）、Profile 来源（`profile_sources_ask_the_hook`：`ProfileSources::production` 拿带钩子的 `resource_client_with`，一行 `#!include` 指到回环的 `TestServer`，等副本落到数据目录之后看钩子被问过回环地址）、钩子答 `Err` 时什么也送不出去（`an_egress_error_stops_each_kind_before_anything_is_sent`）。所有服务器都是用例自己在回环上持有的、不应答的监听器或 UDP 套接字，不依赖本机没有的参照服务端（出口在建套接字之前问，每一类查一次就够看到问没问）。`BOUND` 取 2 秒：一条查找的时限，看的不是查找的结局（一个不应答的服务器把查找留到解析器 30 秒后放弃）；`QUIET` 取 300 毫秒：那是“断言什么都没发生”的等待的余量（钩子答 `Err` 时查找立刻结束，不会有东西到）。

三、`OwnResolve` 的退路（系统解析库）在用例里是假的（M4b 的计划期决定 P9），所以 M4b 没有新增调真正的系统解析库的用例。

四、`crates/rurge/src/cli/pinger.rs` 的 `pings_the_loopback` 对 `127.0.0.1` 与 `::1` 各发一次回显（5 秒时限），建不出套接字时打印原因并返回（Windows 不是管理员、Linux 的 `ping_group_range` 不允许）。**这台开发机上它没有跳过**：原始套接字开得起来，两个地址族都真的来回了一次、载荷相等（循环 20 次全过，M4b 记录第 45 行）；Linux / Android 的数据报套接字与它退到原始套接字的那一路、macOS 的数据报套接字、“开不了套接字所以跳过”这条路在这台开发机上都走不到；CI 的运行 40611702392 在 Linux 与 macOS 上跑过这条用例、通过了，走的是其中哪一条路没有读到（任务日志要登录才能读）。macOS 的数据报套接字按标识符与序号对号那一条也没有在 macOS 上观察过（同一行与第 46 行）。

五、本轮新增用例的过滤词（`cargo test -j 8 --workspace --lib -- <过滤词>`，可执行文件的用`--bins`，集成用例用 `--test <文件名>`）：`rereading_the_system_servers_rebuilds_the_host_upstreams`（`rurge-dns`）；`only_a_route_built_from_an_egress_table_says_it_is_one` 与 `a_direct_connector_routes_to_the_egress_table_its_hook_names`（`rurge-net`）；`platform_sockets_say_which_egress_table_they_answer_from`、`a_follow_that_writes_the_same_table_keeps_the_one_in_the_slot` 与 `a_follow_that_writes_another_table_rebuilds_the_resolvers_upstreams`（可执行文件，`--bins`）；`a_connection_dialled_under_another_egress_table_takes_no_new_requests`（`rurge-proto-quic`）；`an_egress_table_change_underneath_dials_every_peer_anew`（`rurge-proto-wireguard`；它等的是250 毫秒一次的 `TICK`，提交前循环 20 次）；`an_ipv4_address_in_its_mapped_form_is_decided_as_an_ipv4_address`（`rurge-platform`）；`profile_sources_ask_the_hook`（`--test egress`）；`the_programs_pid_is_only_logged_at_debug`（`rurge-proto`）；扫描用例的两条纯函数用例是 `an_item_after_the_tests_is_reported` 与 `a_connector_without_a_hook_is_found_however_it_is_written`（`--test sockets`）。

### 阶段 3 / M4c 起：平台的系统一侧与设备

阶段 3 / M4c 起另有几条：

一、**系统一侧的用例不碰系统**。`LinuxSystem`（`crates/rurge-platform/src/enhanced/linux.rs`）与 `MacSystem`（同目录的 `macos.rs`）都不带 `cfg`（M4c 计划 P2）：程序经 `CommandRunner` 给进来、文件经一个根目录（或 `resolv.conf` 的路径）给进来，所以那两个模块的全部用例在这台 Windows 开发机上也编译、也跑。假执行器是 `crate::command::testing` 的 `FakeRunner`（`reply` 写应答、`fail` 写失败、`calls` 回执行过的命令，键都是 `cmd.join(" ")`，没列的命令回空输出）；根目录是用例自己的 `tempfile::TempDir`，用例把 `etc/resolv.conf`、`proc/sys/net/ipv4/conf/*/rp_filter`、`run/systemd/resolve/resolv.conf` 放进去。命令与程序名一律按绝对路径断言（`/usr/sbin/ip`、`/usr/sbin/resolvectl`、`/usr/sbin/networksetup`、`/sbin/route` 一类），因为生产代码不查 `PATH`。抓日志的用例用 `LogCapture`（`crates/rurge-platform/src/log_capture.rs`，照 `rurge-tun` 的同一份写），都是普通的 `#[test]`：那两个函数是同步的，没有运行时。

二、**没有一条缺省的用例开设备**（M4c 计划 P19）：`TunDevice::open` 与真实平台的 `System::open` 不被任何缺省用例调用，在这台开发机上它们也是带 `cfg` 的、编不到。在 Linux / macOS 的 CI 上它们是真的：以普通用户跑会得到 `EPERM`，但谁以 root 跑测试，就真的建了网卡。以后加用例的人照这一条：要测打开之后的事，用文件后端或不经 `system()` 的 `Unsupported`，不调这两个。

三、**夹具是手写的**（M4c 计划 P20）：`enhanced/macos.rs` 的 `tests` 里有五份工具输出（`-listallnetworkservices`、`-getdnsservers`、`-listnetworkserviceorder`、`ipconfig getpacket`、DHCP 的一整包），每份开头一行注明它不是从 Mac 上取的、出自哪里。`-listnetworkserviceorder` 的格式凭记忆写，没有可引用的样本（P20），留给 M4d 的手工清单核对。

四、**L2 特权用例**（计划的任务 9）：新的工作区成员 `tests/privileged`（包名 `rurge-privileged-tests`，`publish = false`，不是任何 crate 的依赖），要跑的东西、要求与它们改宿主上的什么见 `tests/privileged/README.md`。门控是 `RURGE_TEST_PRIVILEGED=1`，只在一次性的 Linux 虚拟机里、以 root 跑；没有它就每条用例打印一行 `skipped: …` 并返回，什么也不建。**这些用例写好之后没有在任何地方运行过。**

- 地址与夹具（协调者的裁定 R17）：两个用例自建的命名空间 `rl2-a-<pid>-<n>`（“本机”）与 `rl2-b-<pid>-<n>`（“互联网”），veth 的 `10.231.0.2/30` / `10.231.0.1/30`，a 的默认路由经它；服务端在 b 的回环上另加的 `10.231.1.1/32` 才是用例的目标——它在 rurge 装进网卡的基础前缀里、不在 a 的任何直连路由里，只经默认路由可达，所以应用发给它的流量必然进虚拟网卡，而 rurge 自己的连接绑在 veth 上就绕得开（发给 `10.231.0.1` 的会走内核为 veth 装的 `/30` 直连路由，永远不进网卡，什么也证明不了）。`Lab` 在 `/etc/netns/rl2-a-<pid>-<n>/` 下写一份 `resolv.conf`（`ip netns exec` 把它绑到 a 的 `/etc/resolv.conf` 上），用例结束时删掉那个目录；`/etc/netns/` 原来没有时由用例建出来、用完留着。
- 一个 Lab 一组自己的名字、一次只有一个 Lab（协调者的裁定 R20）：`<n>` 来自进程内的计数器（三条用例跑在同一个进程的线程上，只用进程号会撞），`static ONE_AT_A_TIME: Mutex<()>` 让它们一个接一个（`ip netns add` 的第一次还要把 `/run/netns` 挂成挂载点）。
- 起 `rurge` 时去掉环境里每一个以 `RURGE_` 开头的变量，并把 `XDG_DATA_HOME` / `XDG_CONFIG_HOME` 指到 Lab 的临时目录下的两个目录（协调者的裁定 R21）：用例以 root 跑，root 的家目录不是测试文件的地方；而从开发机漏进来的 `RURGE_ENHANCED_BACKEND=off` 一类会让它根本不开增强模式。
- `l2-helper` 的 DNS 报文编解码在 `tests/privileged/src/dns.rs`，六条单元用例在三个平台上都跑；`tests/privileged/tests/linux_netns.rs` 是 `#![cfg(target_os = "linux")]`，别的平台上是 0 条用例。

五、两份名单的数目：`crates/rurge/tests/sockets.rs` 的 `LISTED` 现在 25 行（M4c 加的是 macOS 的 utun 控制套接字那一行），`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 现在 9 个函数（`set_iff` 与 `utun_connect`）——加一处建套接字的地方、加一个放开的函数时就地改，那两条扫描用例会逐处报错。

六、**CI 的 clippy 错误在注解里**（计划的任务 1）：`.github/workflows/ci.yml` 的 clippy 一步有名字、输出 `tee` 到 `$RUNNER_TEMP/clippy.log`，“Report the failed tests”按 `steps.clippy.outcome` 选读哪一份日志，报告脚本把 `error[E….]:` 与 `error: ` 两类行发成 `::error title=compile error::` 注解。M4c 的 Linux / macOS 代码只有 CI 编译，而任务日志要登录才能读；这一改让编译错误在公开可读的注解里就能看到（M4c 记录第 10 行）。

### 阶段 3 / M4d 起：Windows 的设备与读回

阶段 3 / M4d 起另有几条：

一、**有真的 `wintun.dll` 的用例由 `RURGE_TEST_WINTUN_DLL` 放开**，缺省不设：设了它的那一条（`crates/rurge-platform/src/tun/windows/wintun.rs` 的 `a_real_wintun_dll_passes`）校验签名者（必须是 `WireGuard LLC`）并把模块加载起来，只读、只在用例自己的进程里；没设时它打印原因并返回，不失败。缺省的 `cargo test` 与 CI 都不设它；执行计划的自动化会话也不设它、不下载 `wintun.dll`（M4d 计划的「全局约束」）。**本机一次也没有真的跑过它**，真的 `wintun.dll` 的签名者由手工清单核对（`docs/acceptance/phase3-manual.md` 的 W36）。

二、**Windows 的读回夹具是手写的**（M4d 计划 P17）：`crates/rurge-platform/src/route/windows.rs` 的 `ROUTES_EN` / `ROUTES_ZH` / `ADDRESSES_EN` / `ADDRESSES_ZH` / `DNS_EN` / `DNS_ZH` / `MTU_EN` / `MTU_ZH` 与 `crates/rurge-platform/src/enhanced/windows.rs` 的 `ADDRESSES_V4` 一族，每份旁边写着 “not taken from a machine”。解析只认前缀、序号、地址与数字，不认文字，所以夹具里的字写错了也不影响结论；中文系统上真的 `netsh` 输出由手工清单的 W32 核对（英文与中文各一台）。

三、**`WindowsSystem` 的用例只用假的执行器与临时目录**：程序经 `CommandRunner`（`crate::command::testing::FakeRunner`），脚本写进用例自己的 `tempfile::TempDir`，一条命令也不真的执行（M4d 计划的「全局约束」）。缺省的 `cargo test` 里不改系统设置、只碰用例自己的东西的系统调用只有四样：校验测试二进制自己的签名（`WinVerifyTrust`，不联网）、从 System32 载入 `kernel32.dll` 看缺入口时的错误、回环套接字上的 `IP_UNICAST_IF`（设选项，不是只读）、测试自己起的子进程上的 `IsProcessInJob`。

四、**没有一条缺省的用例开 Windows 的设备**（同 M4c 那一条，M4d 计划 P19）：`TunDevice::open` 与真实平台的 `System::open` 不被任何缺省用例调用；Windows 的 CI 机器以管理员运行，谁调了 `TunDevice::open`，就真的装了驱动、建了网卡。要测打开之后的事，用文件后端或 `Unsupported`。

五、**`--wintun-dll` 的那一条只在非 Windows 上运行**：`crates/rurge/tests/cli.rs` 的 `the_wintun_dll_option_is_a_usage_error_elsewhere` 是 `#[cfg_attr(not(windows), test)]` 加 `#[cfg_attr(windows, allow(dead_code))]`，在这台开发机上只编译、不运行（Windows 上这个选项是寻常的那个，带着它起的 `rurge run` 会成功并一直跑下去）；第一次运行是 Linux / macOS 的 CI（M4d 记录第 75、76 行）。`wintun_dll_applies` 自己的单元用例（`crates/rurge/src/cli/run.rs` 的 `wintun_dll_is_only_for_windows`）三个平台都跑。

六、两份名单的数目（阶段 3 / M4d 之后）：`crates/rurge/tests/sockets.rs` 的 `LISTED` 仍是 25 行（M4d 的生产代码不新建套接字，Wintun 不是套接字，`unicast_if` 设在已有的套接字上），`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 现在 23 个函数（M4d 加了 `unicast_if`、`in_job` 与 `tun/windows/wintun.rs` 的十二个）——加一处建套接字的地方、加一个放开的函数时就地改，那两条扫描用例会逐处报错。

### 阶段 3 / M5 起：识别、`[IP Rewrite]` 与两条队列

阶段 3 / M5 起另有几条：

一、**QUIC 与 STUN 的测试向量有出处**。两个客户端 Initial 照 RFC 9001 附录 A.2 与 RFC 9369 附录 A.2（从 rfc-editor.org 的正文取，2026-10-28）：各 1200 字节，DCID `8394c8f03e515708`、包号 2、Length `0x449e`；十六进制照抄成 `crates/rurge-net/src/quic/initial.rs` 的 `RFC9001_CLIENT_INITIAL` 与 `RFC9369_CLIENT_INITIAL`；两个常量上面的注释写着各自的 SHA-256，供抄写时核对；**用例只断言 `is_client_initial` 为真，不断言长度、也不断言摘要**：抄错一个字节会让 AEAD 的标签不过，`the_rfc_9001_client_initial_is_one` / `the_rfc_9369_client_initial_is_one` 因此变红（`a_changed_payload_byte_is_not_one` 与 `no_bit_flip_in_the_first_64_bytes_makes_one` 另外钉住这一点）。STUN 的判据照 RFC 8489 第 5、6 节：魔数 `0x2112A442`、首两位为 0、长度是 4 的倍数且恰好等于数据报（`crates/rurge-engine/src/sniff.rs` 的 `binding()` 造消息）。要造一个真的 QUIC Initial 的用例用 `rurge_net::quic::testing::client_initial`（各个 crate 的用例共用）；quinn 只在一条用例里截一个真的客户端 Initial（`a_real_quinn_client_initial_is_one`），只在回环上、每一步有时限（设计第 14 节），超时就把它当失败，不重试。

二、**[IP Rewrite] 的用例从文字造表**（M5 记录第 8 行）：`rurge-tun` 的用例用 `rurge_config::text::parse_str` 取出 `[IP Rewrite]` 一节，交给 `rurge_config::ip_rewrite::parse`，不为用例加公开的构造函数（照 `src/tests/dns/mod.rs` 从文字造 `HostList` 的做法）；文档地址（`192.0.2.0/24` 一类）只作为表的行与内存设备上包的地址，不当可能被拨的目标。

三、**写任务的两条队列各自有容量**（M5 记录第 41 ～ 44 行）：`TunOptions.writer_queue` 是两条队列各自的容量（不是合起来的）；要填满一条队列的用例在 `writer::run` 被 spawn 之前把批放好（`Writer::start` 多一个填队列的闭包），不靠“spawn 之后它还没醒”这个窗口；回包等空位的用例自带五秒的时限（`reply_to_the_app`），免得卡住不回来；`gated_without_priority()`（关着 `udp-priority`、其余同 `gated()`）用在“回包与 ICMP 在一条队列里看次序”的用例上。停止的宽限 `STOP_GRACE`（1 秒）对两条队列都算，两条都关了才结束。

四、**内部 DNS 会话带名字的用例**（M5 记录第 46、50、51、52 行）：上游写成名字时只有引导问得到它，`boot.query_count(…)` 用 `>= 1`（引擎的引导用缺省 `FanoutOpts`，1 秒没回答就重发；数“被问了几次”用不重发的 `FanoutOpts` 是 `CLAUDE.md` 的约定，`engine_from_profile` 设不了它）；`up` 的计数仍是 `0`；整个查找用例自己有五秒的时限。规则不能被交给引擎的解析器（自己解析自己会挂住）；把 `route_internal` 临时改成 `choose_policy` 时两条引擎用例都会在五秒上超时，这是 M5 记录第 53 行的变异结果。引擎还没接上时拨地址的用例（`dns_pipeline::tests::without_an_engine_a_named_server_is_dialled_at_its_address`）盯住回落连接器只收到地址。

五、**自适应的表不靠墙钟**（P8）：表的单元用例用显式的时刻（`Instant` 由调用方传）；“客户端 TCP 会话通知了表”放在 crate 里的单元用例，预置一项一小时之后才过期的“试探中”；不写“第一条 QUIC 被挡、紧接着一个 TCP、第二条仍被挡”的集成用例——两步之间超过 1 秒时它会时红时绿（设计第 20 节第 1 条）。引擎的两条端到端用例（`adaptive_on_a_direct_alias_blocks_the_first_quic_flow`、`auto_blocks_quic_through_a_proxy_that_carries_no_udp`）在解析之前就定下目标（`d.test` / `noudp.test`），照 `per_policy_block_quic_follows_the_terminal_policy` 的写法。

六、**计时用例提交前单独循环二十次以上**：`[IP Rewrite]` 的两组（`tests::rewrite` 与引擎的 `ip_rewrite_in_the_profile`）、写任务的两组（`tests::writer` 与 `writer_queue: 1` 的那几条）、带名字的 DNS 会话的五条（M5 记录第 49、54 行），各单独跑过二十次（M5 记录第 34、40、45、49、54 行）；日志在工作区（git 忽略）。

七、**M5 没有平台代码**（设计第 1 节）：不用 `RURGE_TEST_PRIVILEGED` 与 `RURGE_TEST_WINTUN_DLL`，缺省的 `cargo test` 不建网卡、不改系统设置；M5 的用例在一台 Windows 11 x86-64 的开发机上跑过，CI 运行 40798658216（`32238be`，2026-10-29）在三个平台上跑过并通过（那一次运行在 macOS 上失败的是一条阶段 2 的互操作用例，见 `docs/status.md`「M6 推送之后的 CI」）。三个基准是唯一用 `-p` 的地方，读数在命令一节 M5 那一条注释里。

### 阶段 4 / M1a 起：嗅探

阶段 4 / M1a 起另有几条（计划 `docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`，下面写“M1a 记录第 N 行”指它末尾的「执行期修正记录」；M1a 的提交都在本地，CI 没有见过）：

一、**用例绑不了、也不该拨回环的 80 与 443**（计划 P14）。经 SOCKS5、`CONNECT` 与虚拟网卡到 80、443 的会话（还有 `force-http-engine-hosts` 让别的端口可嗅探的那些），规则一律把它们送到一个 SOCKS5 策略，策略指向 `rurge_proto::testing::FakeSocks5`，`Socks5Script::connect_to` 是回环上那个真服务器（`TlsFixture`、`rurge_proto::testing::echo_server()` 或“服务器先说话”的 `TestServer`）：不论被问到哪里都连到回环，看哪个 `FakeSocks5` 被问、问的是什么就知道哪条规则命中。没有规则接住的会话会落到 harness 追加的 `FINAL,DIRECT`、拨回环的 80 / 443，那个端口不一定开着，所以**不许有这样的会话**；写了主机名的 Profile 另给第二道防线（监听器的用例里加一行 `[Host]` 钉到 `127.0.0.1`，虚拟网卡的用例用 `guarded`）。例子在 `crates/rurge-engine/tests/sniff.rs` 的 `two_policies` 与 `socks5_connect`。

二、**`EngineShared.sniff_wait`**（`crates/rurge-engine/tests/common/mod.rs`）：只供测试调短嗅探的等待，缺省是 `rurge_http::sniff::SNIFF_WAIT`（15 秒，`EngineShared::new` 给的），不进命令行与环境变量；交给 `harness_with` 时用 `EngineShared { sniff_wait: Duration::from_millis(300), ..EngineShared::default() }`（`sniff.rs` 的 `sniffing()`）。端到端用例用 300 毫秒；**用缺省 15 秒、不调短的只有一条**：`a_stop_during_the_wait_ends_a_tun_session_at_once`。它不等满 15 秒：时限长，停止才一定落在等待之中，它证明的是“停止结束了等待”，所以不能用调短的时限。没有用例等满缺省的 15 秒；等满时限的路径只在调短的时限上跑过。

三、**讲“拨号之后才答”的现有用例改用不嗅探的端口**（计划 P9，设计第 14 节第 6 条）：经 SOCKS5、`CONNECT` 与虚拟网卡连 80、443 的现有用例里，断言应答码、错误页、被拒时的关闭，或客户端不先发字节就等结果的，目标端口改成不嗅探的（443 → 8443，80 → 8080），写到这个端口的地方（`dst`、规则）随之改，断言不改，改的地方加一行英文注释（例如 `// 8443 is not sniffed (phase 4 M1 design 4.1): the answer still waits for the dial`）。改了哪六处见 M1a 记录第 12 行与第 29 行（`tests/outbounds.rs` 两条、`tests/pipeline.rs` 两条、`tests/process.rs` 一条、`tests/tun.rs` 一条）；客户端先发字节的用例照过。新行为（先答后读）由 `crates/rurge-engine/tests/sniff.rs` 的用例钉住。

四、**带等待的嗅探用例提交前各自单独循环跑二十次以上**（全过才算数）：`rurge-http` 的 `sniff::tests`（任务 2 的 13 条，修正第 1 轮之后是 14 条）、`rurge-inbound` 的 `hold::tests` 四条与 SOCKS5 的 `a_dropped_sniffable_session_is_held_after_the_answer`、`the_default_serve_cancels_the_session_at_the_stop`（任务 4）、`rurge-tun` 的 `tests::outcomes::serve` 三条（任务 5）、整个 `sniff` 二进制（任务 6 时 5 条、任务 7 时 21 条，各循环一轮），以及修正轮里改过等待的那一条（`extended_matching_rules_a_tls_session_by_its_sni`）。日志在工作区（git 忽略），命令行见 M1a 记录第 19、21、25、27、30、31、33、34 行。

五、`CLAUDE.md`「测试约定」里没有要随之改的句子：M1a 的规矩都是上面几条的具体做法（第二道防线、有界等待、循环跑二十次），那几节已经写着。

### 阶段 4 / M1b 起：明文 HTTP 引擎

阶段 4 / M1b 起另有几条（计划 `docs/superpowers/plans/2026-10-29-phase4-m1b-http-engine-plan.md`，下面写“M1b 记录第 N 行”指它末尾的「执行期修正记录」；M1a 与 M1b 的提交都在本地，CI 都没有见过）：

一、**空闲时限的夹具 `harness_timed`**（`crates/rurge-engine/tests/common/mod.rs`，计划 P15）：`runtime_timed` 与 `harness_timed` 让引擎里请求头、等应答头与正文的时限取 `RuntimeOptions.idle_timeout`；要等时限到的用例（504、停住的正文、池的空闲、客户端不读）用它调短（300 ～ 500 毫秒），现有的用例经 `runtime_edited` 仍是 600 秒，不变。

二、**数连接用 `TestServer::connections()`**（`rurge_net::testing`，`rurge-net` 的 `testing` 特性，计划 P16）：接受循环里每接受一条连接加一，数的是“服务器被连了几次”，不靠日志或时间；`TestServer` 不支持升级、流式的正文与停住，要这些的用例用自己的回环监听器（`streaming_origin`、`chunked_echo`、`cut_off_origin` 一类，都在 `crates/rurge-engine/tests/http.rs` 的测试模块里）。

三、**“一条 `CONNECT :80` 隧道一条记录”的用例改成每个请求一条**（计划 P11 的两张表）：M1b 起 `CONNECT` 隧道与 SOCKS5 里的明文请求进引擎，一个请求一条记录、一次拨号；`crates/rurge-inbound` 的转发用例搬到引擎（`the_listener_rewrites_a_plain_request_for_the_origin` 一族），七条删掉；`crates/rurge-engine/tests/sniff.rs` 的三条照计划改了写的字节与读到的开头。改了哪几处见 M1b 记录第 15 ～ 18 行、第 42 行与第 58 行。

四、**断言钩子结果的用例用单线程运行时**（普通的 `#[tokio::test]`，M1b 记录第 61 行）：`SessionHandle::finish` 照挂上的先后跑钩子，记录的钩子先于 `smart::watch`；两个工作线程时用例可能恰好在两个钩子之间读书，产品代码退回去了也可能是绿的。

五、**“客户端不读”的用例写 256 MiB**（M1b 记录第 62 行）：Linux 回环的缓冲能自动调到几 MiB、macOS 约 4 MiB，4 MiB 可能整个塞进两端的缓冲、请求就做完了；源站只在对端肯收时才写，不占内存。

六、**kill 与停止之前等源站收到请求**（M1b 记录第 63、74 行）：`TestServer` 先记下请求再按 `set_delay` 等，所以 `origin.requests().len() == 1` 才是“请求已经到了上游”；记录在拨号之前就有，等“有一条活动的记录”会让 kill 落在拨号或发请求时。

七、**代理端口上的 fake 地址用 `listener_rules`**（`crates/rurge-engine/tests/common/tun.rs`，M1b 记录第 60 行）：那个夹具的规则有三行——`DOMAIN,target.test,DIRECT` 把名字放过去；之后 `IP-CIDR,198.18.0.0/15,REJECT,no-resolve` 拒掉 rurge 的整个 fake 地址段；`DEST-PORT,<上游端口>,REJECT` 拒掉上游的端口。没被反查成名字的地址被后两行拒掉，到不了夹具末尾的 `FINAL,DIRECT`；所以即使反查那一步、或“反查不到就拒”那一步退回去了，请求也不会被拨到回环之外（`tun_dns.rs` 开头的说明也是这个意思）。

八、**规则里回环之外的地址段只写 `REJECT`**（M1b 记录第 79 行）：`IP-CIDR,10.0.0.0/8,REJECT` 逼求值去查地址，但不能写成可能被拨的目标。

九、**带等待的引擎用例提交前各自单独循环跑二十次以上**（全过才算数）：`crates/rurge-engine/tests/http.rs` 整个二进制（36 条）、`--test sniff`、`crates/rurge-http` 的 `engine::tests`（那个过滤词一共选中 102 条：`rurge-http` 30、`rurge-engine` 9、`rurge-rules` 63）与 `pool::tests`；`--test api` 里新加的那一条（`requests_carry_the_method_the_url_and_the_status_code`）也单独循环过。日志在工作区（git 忽略），命令行见 M1b 记录第 23、27、34、35、43、48、59、78 行。

### 阶段 4 / M2a 起：MITM

阶段 4 / M2a 起另有几条（计划 `docs/superpowers/plans/2026-10-30-phase4-m2a-mitm-http1-plan.md`，下面写“M2a 记录第 N 行”指它末尾的「执行期修正记录」；M1a、M1b 与 M2a 的提交都在本地，CI 都没有见过）：

一、**测试从不把任何 CA 装进系统或浏览器的信任库**（总设计 P4-D26、设计第 17 节，M2-D24）：不运行 `certutil`、`security add-trusted-cert` 这类命令，也不写调用它们的代码。测试的客户端是 rustls 的客户端，根只有测试自己的 MITM CA。

二、**测试的 CA 从哪里来**：引擎与 API 的用例用 `TlsFixture::with_ca_name("rurge test MITM CA", &[])` 的 `ca_p12_base64(..)`（`crates/rurge-proto` 的 `testing`），源站的证书来自另一个 `TlsFixture`；PKCS#12 用 `rurge-proto` 的 `testing` 里的 `p12_base64` 与 `p12_cert_base64`（M2a 记录第 25、40 行）。两张 CA 与 PKCS#12 都在内存里生成，不读、不写用例目录之外的证书文件。

三、**期望“不信任”的用例，信的 CA 与签发的 CA 的 subject 要不同**（M2a 记录第 27、40 行）：webpki 先按 subject 找信任锚再验签名，同名时是 `BadSignature`（客户端发 `decrypt_error`），不是 `UnknownIssuer`（`unknown_ca`）。`TlsFixture::new` 的 CA 都叫 `rurge test CA`（`rurge-net` 的 `quic/dial.rs` 有用例靠同名，`new` 不能改），另起名字用 `with_ca_name`。

四、**引擎的 MITM 用例用 `tests/mitm.rs` 的 `harness_mitm(..)`**（计划写的 `harness_trusting` 是 `tests/common/mod.rs` 给出站用例的），源站的根经 `EngineShared.roots` 交给引擎。**根不能是空的存储**：空的存储建不出上游的校验器，`Mitm::build` 失败，那一代没有 `Mitm`，用例会在 MITM 没生效的情况下绿；用不上根的用例也给一份非空的（如 `mitm_ca().roots()`），并断言 `mitm_ca_der().is_some()`（M2a 记录第 55 行 ①，覆盖第 54 行 ④）。

五、**OpenSSL 的五组夹具**（`crates/rurge-http/src/mitm/fixtures.rs`）由协调者用本机的 OpenSSL 3.2.4 生成；以后要新的，照 **M2a 记录第 23 行**的命令（`openssl pkcs8 -topk8 -nocrypt …` 才写出 PKCS#8；计划 P6 原文的 `pkey -outform DER` 与 `genpkey -outform DER` 写的是 SEC1 / PKCS#1，不对）。Pi 不运行 OpenSSL。

六、**时钟**：`Authority` 的时钟可以注入（计划 P7、M2a 记录第 26 行）；夹具与有效期的单元用例把现在钉在 2027-01-01；**与 rustls 客户端握手的用例用真实时钟**（客户端按系统时间校验叶证书，钉了时间会 `CertificateExpired`，M2a 记录第 32 行）。`TlsFixture` 的证书是 rcgen 的缺省有效期。

七、**对端不发 `close_notify` 就关闭时 rustls 报 `UnexpectedEof`**（M2a 记录第 35、39 行，延后第 2 行）：测试的 TLS 源站回显完先 `shutdown()` 再放手。任务 8 的 `switching_echo_over_tls` 起初漏了这一步，客户端读到 `UnexpectedEof`，一度被当成 rurge 不发 `close_notify`（M2a 记录第 54 行 ⑤、第 55 行 ②）。

八、**要一个 rustls 不收的名字，用 64 个字符的标签**（M2a 记录第 33 行）：rustls-pki-types 1.15.1 的 `DnsName` 收下划线。

九、**日志断言一律“以级别开头且含整句”**（`tests/mitm.rs` 的 `lines_with` 与六个句子常量 `UNTRUSTED_NAMED`、`UNTRUSTED_VIRTUAL`、`PINNING_NAMED`、`PINNING_VIRTUAL`、`FAILED_NAMED`、`FAILED_VIRTUAL`），不找“同一行里有两个词”；计数一律与整个 `MitmCounts` 比（M2a 记录第 49 行）。

十、**客户端的 TLS 1.3 握手在它发出 Finished 时就返回**，引擎的 `accept` 可能还没读完（M2a 记录第 49 行 ④）：握手之后要做的停止、重载、数计数，先等 `decrypted == n`。

十一、**要“停在握手里”：先读到引擎回的第一个字节、且是 `0x16`，再停**（M2a 记录第 44 行）：写完 ClientHello 就停可能落在嗅探上（嗅探本来就不留记录），用例遮不住要盖的分支。

十二、**经虚拟网卡的 `AppTcp` 没有 `shutdown()` 就丢掉会给会话发重置**；“不信任”的客户端经虚拟网卡时，警报由用例自己写明文 `unknown_ca`（rustls 0.23.43 在 TLS 1.3 里还没解密过对端任何消息时收明文警报；tokio-rustls 0.26.4 只把排好的警报写一次）（M2a 记录第 47、48 行）。真的 rustls 客户端经虚拟网卡发来的警报这条路没有用例（M2a 延后第 4 行）。

十三、**限频的两个边界**（一小时、1024 个键）只有 `crates/rurge-engine/src/mitm.rs` 的两条单元用例（M2a 记录第 45 行），集成用例碰不到（M2a 延后第 5 行）。

### 阶段 4 / M2b 起：MITM 的 HTTP/2

阶段 4 / M2b 起另有几条（计划 `docs/superpowers/plans/2026-10-30-phase4-m2b-mitm-http2-plan.md`，下面写“M2b 记录第 N 行”指它末尾的「执行期修正记录」；M1a、M1b、M2a 与 M2b 的提交都在本地，CI 都没有见过）：

一、**客户端一侧的 HTTP/2 不经 TLS，在 `tokio::io::duplex` 上用先验知识讲**（计划 P18）：`serve_connection` 拿到的已经是解密之后的流，所以用例无需证书就能讲 HTTP/2。客户端的构造在 `crates/rurge-http/src/engine.rs` 的测试模块里：`rig_h2`（`Conn` 的 `h2` 为真，目标是一个 `TestServer` 或自己的源站）、`h2_client`（`hyper::client::conn::http2::Builder::new(TokioExecutor::new())` 的握手，连接的 future 用 `tokio::spawn` 驱动）、`h2_request`（`Full<Bytes>` 的空正文）与 `served_within`（给服务任务一个时限）。

二、**要读服务端的 SETTINGS 时才用 h2 的客户端**（计划 P18）：`the_h2_server_offers_100_streams_and_no_extended_connect` 用 `h2::client::handshake`，因为要断言的是“服务端宣告了什么”（`max_concurrent_send_streams() == 100`、`is_extended_connect_protocol_enabled()` 为假），hyper 的客户端不把它交出来。

三、**会 GOAWAY、会拒绝流、会中途重置的源站用 h2 的服务端**（计划 P18；`crates/rurge-proto/src/testing/h2proxy.rs` 是现成的写法）：`OriginScript::{GoAwayAfterOne, RefuseAfterOne, ResetMidBody}`、`H2Origin { addr, connections, ended, reset }` 与 `h2_origin(&fixture, script)`，TLS 用 `fixture.acceptor_with_alpn(false, &[b"h2"])`，`h2::server::handshake` 之后 `accept()` 一直跑到 `None`（它同时驱动连接）才放手并 `notify_one`。用 `notify_one` 而不是 `notify_waiters`：通知可能早于用例来等，前者存一个许可。应答要带 `content-length`，否则 `h2` 的 `Client::response()` 读不到正文。别的源站用 `TestServer`（它经 TLS 时按协商到的 ALPN 讲；`RecordedRequest.version` 把它记成 `"HTTP/2.0"`）。

四、**窗口调到只停住一个流**（计划 P7、M2b 记录第 23 行）：`an_h2_client_that_stops_reading_one_stream_fails_only_that_request` 把客户端的流窗口调成 1 MiB、连接窗口 16 MiB：第一个流不读、占着那 1 MiB 不释放，第二个流的 4 MiB 还放得下，所以停住的只有那一个，看门狗只结束它。

五、**“先有数据、再重置”的用例要用信号排先后，不能靠固定等待**（M2b 记录第 50、52 行）：h2 0.4.19 的 `send_reset` 会把这条流还排在队列里的帧（HEADERS 与 DATA）丢掉（`h2-0.4.19/src/proto/streams/send.rs:254-268`）；而 hyper 的 `PipeToSendStream` 交出一段 DATA 之后会在同一次 poll 里接着读正文（`hyper-1.11.1/src/proto/h2/mod.rs:142-258`），上游的重置这时如果已经到了，读到的就是 `Failed`，hyper 随即以 `INTERNAL_ERROR` 重置客户端一侧的流、客户端一个字节都读不到。所以 `OriginScript::ResetMidBody` 驱动到用例告诉它“那一段已经读到了”（`H2Origin.reset`）才 `send_reset`，源站那边用 `tokio::select!` 等这个信号或 `conn.accept()`（`accept` 驱动连接，把应答头与那一段写出去），用例读到那一段之后 `notify_one`，不固定等多少毫秒。

六、**`TestServer::set_delay` 只对 `set` 过的路径起作用**（M2b 记录第 29、45 行）：没 `set` 的路径立刻答 404。要让一个请求停住，先 `set(路径, 正文)` 再 `set_delay`，否则用例等的是“源站立刻答完了”。`TestServer` 也不读请求正文，先答完的话 hyper 会以 `NO_ERROR` 重置请求一侧，数 `up` 的用例要先把源站按住。

七、**丢掉 hyper 客户端一个请求的应答 future 会发 `RST_STREAM(CANCEL)`**（M2b 记录第 30 行），不论请求有没有正文：h2 的 `StreamRef` 的 `drop` 走 `drop_stream_ref` → `maybe_cancel`（`h2-0.4.19/src/proto/streams/streams.rs:1631`、`:1687-1705`）。“客户端重置一个流”的用例靠这一条。

八、**判原因码用 `h2_reason_of`，不要假定它有**（M2b 记录第 52 行）：它顺着 `std::error::Error::source()` 找 `h2::Error`，返回 `reason()`；`h2::Error` 也装 I/O 错误，那样的 `reason()` 是 `None`，写成“没有原因码就当作 `INTERNAL_ERROR`”会让断言因为错的理由通过。钉住原因码的六条用例：`a_killed_h2_stream_is_reset_and_the_others_go_on`、`a_killed_h2_stream_whose_body_flows_is_reset_and_its_upstream_let_go`、`an_h2_client_that_stops_reading_one_stream_fails_only_that_request`、`a_rejected_h2_request_without_a_page_resets_only_its_stream`、`a_dropped_h2_request_is_held_then_reset`、`an_upstream_h2_stream_reset_mid_body_reaches_the_client_as_internal_error`。

九、**带等待的 HTTP/2 用例提交前各自单独循环跑二十次以上**（全过才算数）：任务 1 ～ 3 的新用例各一圈、`engine::tests` 整个一圈，任务 5 的七条各一圈外又整个 `--test mitm` 一圈；日志在工作区（git 忽略），命令行见 M2b 记录第 31、32、46、47、51、52、56 行。

### 阶段 4 / M2c 起：CA 工具、`http-api-tls` 与命令行的 HTTPS

阶段 4 / M2c 起另有几条（计划 `docs/superpowers/plans/2026-10-31-phase4-m2c-ca-tools-api-tls-plan.md`，下面写“M2c 记录第 N 行”指它末尾的「执行期修正记录」；M1a、M1b、M2a、M2b 与 M2c 的提交都在本地，CI 都没有见过）：

一、**CA 只在用例自己的临时目录或内存里生成**（P16）：`rurge mitm ca generate` 的 `--out` 一律指到 `tempfile::tempdir()`；`rurge-http` 的单元用例用 `mitm::testing` 的夹具或 `generate_at`；命令行的用例用 `generate` 写进临时目录（`crates/rurge/tests/run/mitm.rs` 的 `generated_conf`）。它打印的安装命令一个字也不执行。

二、**`rurge-http::mitm::testing::NOW` 是 2027-01-01T00:00:00Z，在今天之后**（M2c 记录第 10 行）：`generate_at(NOW)` 生成的 CA 的 `notBefore` 是 `NOW − 2 天`，用真时钟读回是“还没生效”。所以**用真时钟读回的用例用 `generate()`**（`the_conf_is_a_mitm_section_that_loads`、`a_generated_ca_serves_a_handshake`），钉住日期的用例才用 `generate_at(NOW)`。

三、**两张同名的 CA 不是“不信任”**：webpki 先按 subject 找信任锚、再验签名，所以信的 CA 与签发的 CA 同名时，失败原因是 `BadSignature`（客户端发 `decrypt_error`），不是 `UnknownIssuer`（`unknown_ca`）。`generate` 的 CA 叫 `rurge MITM CA <日期>`，同一天生成的两张也同名；`TlsFixture` 的 CA 都叫 `rurge test CA`。要“另一个 CA”、失败的原因也要对的用例给它另一个名字：`crates/rurge-http/src/mitm/api.rs` 的 `a_client_trusting_another_ca_is_refused` 用 `rurge test other CA`，`TlsFixture` 用 `TlsFixture::with_ca_name`。

四、**API 的 TLS 用例的客户端只信测试的那张 CA**：`crates/rurge-api/src/tls.rs` 的 `Rig` 与 `crates/rurge-api/tests/api.rs` 的 `the_api_is_served_over_tls_with_the_mitm_ca` 都从用例自己的 CA 建 `client_config`，根只有它；`a_client_trusting_another_ca_is_refused` 是反面那一条。`serve_tls` 自己不查有没有 CA（P11）：`serve_tls_without_a_ca_answers_nothing` 证明没有 CA 时它照样绑上、每一次握手都失败、从不讲明文。

五、**握手的名额与时限用 `with_limits` 调小**：`TlsListener::with_limits(inner, source, stop, max, time)` 是给用例的入口，缺省的 `new` 用 `TLS_HANDSHAKES`（64，同 `LINGER_MAX`）与 `TLS_HANDSHAKE_TIME`（30 秒）。`handshakes_beyond_the_limit_are_closed` 把名额调成 1、`a_handshake_that_does_not_finish_in_time_is_closed` 把时限调成 300 毫秒——不要写等 30 秒的用例。

六、**端到端里 `status -c` / `stop -c` 要 Profile 里写着的端口**：`rurge status` 从 Profile 里读 API 的地址，所以用例的 Profile 要写 `free_tcp_port()` 取的端口（`crates/rurge/tests/run/mitm.rs`）；`crates/rurge/tests/cli.rs` 的 `API_GENERAL` 是 `127.0.0.1:0`，那条命令连不上它（M2c 记录第 35 行 ②）。

七、**诊断看流**：`rurge check` 把诊断写在**标准输出**（有位置的带 `<文件>:<行>`，有 `hint` 的下面另起一行 `  hint: …`，按位置排序、没有位置的排在最前），`rurge run` 写在**标准错误**（不带位置，同样排过序），日志行在**标准输出**（M2c 记录第 44 行）。抓 WARN 的用例读标准输出——`crates/rurge/tests/cli.rs` 的 `output_until_it_is_running` 就把标准错误丢掉了。

八、**带等待的 M2c 用例提交前各自单独循环跑二十次以上**（全过才算数）：任务 5 的五条握手用例一圈、`--test api` 的两条一圈、任务 5 修正轮 1 的五条又一圈、`--test cli` 的 `run::mitm` 两条一圈（各 20 次）。日志与命令行在工作区（git 忽略）：`task-5-loop-tls.log`、`task-5-loop-api.log`、`task-5-fix-1-loop-tls.log`、`task-6-loop-mitm.log`。

### 阶段 4 / M3a 起：URL Rewrite 与 Header Rewrite

阶段 4 / M3a 起另有几条（计划 `docs/superpowers/plans/2026-10-31-phase4-m3a-url-header-rewrite-plan.md`，下面写“M3a 记录第 N 行”指它末尾的「执行期修正记录」；M1a、M1b、M2a、M2b、M2c 与 M3a 的提交都在本地，CI 都没有见过）：

一、**手册的黄金用例照手册的原文逐字写规则**（P19）：`crates/rurge-engine/tests/rewrite.rs` 里手册的那七条（任务 7 的第一批）照 `http/url-rewrite.html` 与 `http/header-rewrite.html` 的例子行一字不改；要连上游的那些把**测试源站的端口写在请求里**（`http://www.google.cn:{源站端口}/`），式子 `^http://www\.google\.cn` 照样命中、替换之后端口留着。与手册的出入写在用例的注释里，不改手册的规则（设计第 16 节第 3 条、第 22 节第 8 条；M3a 记录第 64 行）。

二、**用例里出现的每个名字各一行 `[Host]` 钉到 `127.0.0.1`**：换目标之后的新主机、`Location` 里的、规则里写着而这一次请求不会去的都算（M3a 记录第 64、66 行）。第二道防线是共用夹具的 `dns-server`（回环上的 `MockDns`），夹具在规则之后自带 `FINAL,DIRECT`，用例不另写 `FINAL`（P19）。

三、**不该拨号的请求，把策略指向 `Guard`**（P19）：`tests/rewrite.rs` 的 `Guard` 是测试自己持有的回环监听器（非阻塞的 `accept()`，`connections()` 数到过几条），Profile 把它写成 HTTP 代理策略；用例结束前断言 `connections() == 0`。**请求本来就指向测试自己的源站时不必另起守卫**：源站的 `connections() == 0` 是同一件事（M3a 记录第 57 行）。

四、**网关的地址只经 `EngineShared.network` 给**：`302` 的 `Location` 与网关只是文字、从不被拨，用例里的网关用 `127.0.0.2`；给网络经 `EngineShared::network.store(…)`，不手建 `NetworkCell`（M3a 记录第 58 行）。

五、**建引擎的用例经 `harness_with` / `harness_trusting` 给 `TlsFixture` 的根，不用 `harness(p)`**（P19，M3a 记录第 20、32、56 行）：`harness(p)` 的 `roots` 是 `None`，`build_rewrite` 会去读系统的信任库；给空的根同样不行——那是 rustls 的 `NoRootAnchors`，这一代就没有重写，用例会在重写没生效的情况下绿。只建 `Rewrite` 的单元用例用 `crate::mitm::testing::generated` 或 `engine.rs` 测试模块里的 `roots_of`。

六、**URL Rewrite 的替换只换配上的那一段、尾巴照接**（设计第 5.5 节，M3a 记录第 45 行）：写规则与期望的 URL 时照这个算——`^http://target\.test/a https://localhost:{端口}/secure` 配 `http://target.test/a` 得 `https://localhost:{端口}/secure`；而 `^http://target\.test https://localhost:{端口}/` 配同一个请求得 `https://localhost:{端口}//a`（没配上的 `/a` 接在后面），源站答 404。

七、**换到 `https` 而目标不讲 TLS 时，握手要等 `REWRITE_HANDSHAKE` 的 30 秒**（M3a 记录第 46 行）：用例不这样换；非要换的话用 accept 之后立刻关的源站（`closing_origin()`），否则读应答的时限先到。

八、**`TestServer` 记下的 `RecordedRequest.path` 是 `uri().path()`、不带查询串**（M3a 记录第 64 行）：要断言查询串到了源站，用 `tests/rewrite.rs` 的 `RecordingOrigin`（它把每个请求的头整个记下）。

九、**解密的端到端用例的源站不在 443 上**：把它的端口写进 `[MITM]` 的 `hostname`（`origin.test:{端口}`），不用假的 SOCKS5 把端口凑到 443——换目标之后的拨号走选定的策略，假的 SOCKS5 连的是它自己写死的目标，到不了新的主机（M3a 记录第 65 行；`tests/mitm.rs` 的 `a_written_port_makes_its_port_sniffable` 是同一做法）。

十、**手写的上游代理收到 `CONNECT` 记下之后立刻关连接**（`tests/rewrite.rs` 的 `RecordingProxy`）：否则对它的 TLS 握手要等 `REWRITE_HANDSHAKE` 的 30 秒。

十一、**带等待的 M3a 用例提交前各自单独循环跑二十次以上**（全过才算数）：任务 5 的 14 条一圈、应答一侧的 2 条一圈、修正轮 1 的 2 条一圈；任务 6 的 `tests/rewrite.rs` 4 条、`--test api` 的 1 条与 `--test cli` 的 1 条各一圈；任务 7 的两批（7 条与 12 条）各一圈。日志与命令行在工作区（git 忽略）：`task-5-loop-a.log`、`task-5-loop-b.log`、`task-5-fix-1-loop.log`、`loop-task-6a.log`、`loop-task-6b-api.log`、`loop-task-6b-cli.log`、`loop-task-7a.log`、`loop-task-7b.log`。

### 阶段 4 / M3b 起：Body Rewrite 与 Map Local

阶段 4 / M3b 起另有几条（计划 `docs/superpowers/plans/2026-10-31-phase4-m3b-body-rewrite-map-local-plan.md`，下面写“M3b 记录第 N 行”指它末尾的「执行期修正记录」，写“延后事项第 N 行”指同一份计划的另一张表；M1a、M1b、M2a、M2b、M2c、M3a 与 M3b 的提交都在本地，CI 都没有见过）：

一、**大正文在用例里生成，上限用真的**（P9、P11）：32 MiB 与 10 MiB 的边界用例（`crates/rurge-engine/tests/rewrite.rs` 的 `the_request_limit_is_thirty_two_mib` 与 `the_response_limit_is_ten_mib`）在内存里写重复的 ASCII，不写磁盘上的大文件；超过上限而不该读的请求正文只发头。验机制的单元用例用小上限（第 46 行第 (2) 条：`crates/rurge-http/src/rewrite/body.rs` 的 `Limit { bytes: 4, text: "4 bytes" }`），解压炸弹的输入当场压出来；回溯超限的式子用 `BodyPattern::with_backtrack_limit(.., 1000)` 造（第 46 行第 (1) 条）。

二、**Map Local 的文件只在 `tempfile` 的临时目录里**（`Harness.dir` 就是 Profile 所在的目录，手册的相对路径 `data/map-local.json` 接在那里）；URL 的数据只经回环上的 `TestServer` 与在线的一代（`crates/rurge-engine/tests/rewrite.rs` 的 `online_harness`，经 `common` 的 `online_runtime_with`）。等下载好或失败用 `wait_until` 看外部资源列表里那一项的 `ready` 或 `error`，不用固定的等待（第 67 行）。

三、**抓日志的用例用 `LogCapture`，断言的写法照 `tracing` 缺省格式器**（第 74 行第 ⑤ 条）：字段的值是 `Debug`，带引号、反斜杠成双，例如 `pattern="^http://b\\.test:\\d+/"`、`host="b.test"`。数 WARN 的用例也用它——一条规则一小时至多一行，`warnings_come_at_most_once_an_hour` 是 `rurge-http` 的单元用例。

四、**断言一组头的名字时排序之后比、不断言次序**（第 46 行第 (3) 条）：`HeaderMap::keys()` 的次序不是插入的次序。

五、**Map Local 的答复“已经开始”以客户端读到应答头为准，不看 `handle.bytes()`**（第 53 行第 (1) 条）：它没有上游连接，下行字节始终是 0。

六、**测试客户端读整个 chunked 正文以 10 秒为限，`fill` 每次读以 5 秒为限**（第 65 行第 ③ 条）：`crates/rurge-http/src/engine.rs` 测试模块的 `Client::response_chunked` 与 `response_any` 给整份正文套 10 秒，`fill` 限的是一次读（5 秒）；`crates/rurge-engine/tests/rewrite.rs` 的 `answer_bytes` 是前一种的同一写法。

七、**trailer 只在两端都讲 HTTP/2 时断言**（第 20、60 行）：客户端经 `rig_tls_h2`、源站是 `h2_origin` 的 `OriginScript::Trailers`（`crates/rurge-http/src/engine.rs` 测试模块的 `an_unchanged_response_keeps_its_bytes_and_trailers`）；HTTP/1.1 的那一半只断言正文的字节与头不变、照旧 chunked 分帧，不断言 trailer——到 HTTP/1.1 客户端的 trailer 一个也不转。

八、**要断言源站收到的请求，或要源站答一段指定的字节（chunked、压缩过、发到一半停住），用 `crates/rurge-engine/tests/rewrite.rs` 的 `ScriptedOrigin`**（第 73 行、第 74 行第 ③ 条）：`spawn` / `spawn_tls`，`heard()` 是每个请求的头与去掉分帧的正文，`connections()` 数它接过几条连接；TLS 的源站只给 `http/1.1`，用例才读得回它收到的东西。只读头的用 `RecordingOrigin`（第 76 行订正了 P20 里说用 `RecordingOrigin` 的那一句）。

九、**超过 10 MiB 的应答原样流过：只断言客户端收到的字节**（照它收到的分帧取正文，`answer_parts`），不断言分帧（第 64 行把它留给任务 7；就是 `the_response_limit_is_ten_mib`）。

十、**手写的请求连 32 MiB 的正文也一次写出**（第 76 行第 ① 条、第 77 行）：`CLAUDE.md` 那一条没有为大正文开的例外；超过上限的请求只发头。

十一、**“源站没有被连过”用 `connections()` 断言，读在记录落定之后**（第 76 行第 ②、③ 条，第 77 行）。

十二、**讲 HTTP/2 的客户端**（第 74 行第 ①、② 条，第 75 行）：请求的 `:authority` 带源站的端口（引擎照它拨号，不带就拨 443）；长度不知道的正文与头里说了长度的正文各用一个自己的 body 类型，`is_end_stream` 照实写（缺省是 `false`，引擎会把没有正文的请求当成“没有长度”）；连接任务要 `tokio::spawn`。

十三、**gzip 的字节由用例当场写**（第 74 行第 ④ 条）：`gzipped` 与 `crc32`（stored 块），`rurge-engine` 的测试没有 `flate2` 开发依赖。

十四、**手册的 `[Body Rewrite]` 行把 `/` 紧接在主机后**（第 73 行）：测试源站的端口插在中间，带斜杠的式子写 `:\d+`（M6 写进验收文档）。

十五、**`rurge-net` 的单元测试要关着的端口时用 `rurge_net::testing` 的 `closed_udp_port` / `closed_tcp_port`**（第 69 ～ 71 行；同 `rurge-engine` 测试 `common` 里的那两个），不要取 0 号端口、放掉再用；全工作区还没改的五处在延后事项第 1 行。任务 10（第 97 ～ 100 行）改了那五处与补扫出的三处，共八处；`rurge-proto` 的单元测试从那时起也经开发依赖里带 `testing` 特性的 `rurge-net` 用这两个函数。

十六、**带等待的 M3b 用例提交前各自单独循环跑二十次以上**（全过才算数）：任务 5 的 Map Local 3 条（`rurge-http` 的 `engine::tests::` 里的 `a_local_file_nobody_reads_is_let_go`、`kill_ends_a_local_file_answer` 与 `a_file_that_becomes_shorter_ends_the_answer_with_an_error`）一圈、请求正文的 3 条一圈、应答正文的 4 条一圈；任务 6 的 `map_local_urls_are_listed_as_data`、`map_local_url_data_that_never_came_answers_502` 与 `the_same_map_local_url_is_one_resource` 三条各 20 次（共 60 次）；任务 6a 的整个 `rurge_net` 测试二进制 20 次（每次 336 条全过）；任务 7 的 10 条各 20 次、修正轮 1 的 2 条各 20 次。日志与命令行在工作区（git 忽略）：`loop-task-5b-map-local.log`、`loop-task-5c-body-request.log`、`loop-task-5d-body-response.log`、`loop-task-6.log`、`loop-task-6a.log`、`loop-task-7.log`、`loop-task-7-fix1.log`。任务 10 改过的九条各 20 次、`crates/rurge/tests/cli.rs` 的 `run::remote_sources::the_link_to_the_managed_profile_is_shown_as_where_it_goes_and_nothing_more` 10 次：`loop-task-10.log`。

### 阶段 4 / M4a 起：抓包

阶段 4 / M4a 起另有几条（计划 `docs/superpowers/plans/2026-11-01-phase4-m4a-capture-plan.md`，下面写“M4a 记录第 N 行”指它末尾的「执行期修正记录」，写“延后事项第 N 行”指同一份计划的另一张表；M1a、M1b、M2a、M2b、M2c、M3a、M3b 与 M4a 的提交都在本地，CI 都没有见过）：

一、**抓包的内存是量出来的，不是门**（P15，M4a 记录第 76、81 行）：两条带 `#[ignore]` 的用例在 `crates/rurge-http/src/capture/memory.rs`，**调试构建**、`rurge-http` 的**库内**用例，各单独跑一次——`cargo test -j 8 --workspace --lib -- capture::memory::<名字> --ignored --exact --nocapture`，不用 `--release`。峰值工作集由操作系统读：`peak_resident_bytes`、`parse_count`、`parse_vm_hwm` 与它们的用例照抄自 `crates/rurge-tun/src/tests/bench/peak.rs`（Windows 上起一个 `powershell` 读本进程的 `PeakWorkingSet64`，Linux 读 `/proc/self/status` 的 `VmHWM`）。它与 `crates/rurge-rules/tests/memory.rs` 的做法不同：那里是 **release** 的**集成测试**，这里是调试构建的库内用例。为什么：`--release` 要把整个工作区另编一份（`target/` 的盘位，见 `CLAUDE.md`「这台开发机」），而条目、`HeaderMap` 与 `Bytes` 在两种构建里分配的大小相同，量的是“装满”与“基线”两次运行的差；库内的用例用得上 `Pending`，走的是真实的按块记账（P15）。**这是量、不是门**：缺省的 `cargo test` 不跑这两条，也没有一条用例断言它们的数。请求记录的大小也这样量（`crates/rurge-engine/src/observe/memory.rs`，M4b 计划 P12）：`observe::memory::an_empty_request_log_is_the_baseline` 与 `a_full_request_log_of_http_requests` 两条各自单独跑一次，数在 `docs/api/phase4.md` 的「M4b　请求记录的完整」一节。

二、**抓包的用例用小的上限，等条目存进去用有时限的轮询**（P14，M4a 记录第 20、24 行）：验机制的单元用例用几十、几百字节的 `Limits`，正文的两个真实上限（100 MiB 与 5 MiB）只在说缺省值与量内存的那几条里出现。引擎与端到端用例用 `common` 的 `wait_until` 等 `capture().session(..)` 不空（5 秒、每 10 毫秒看一次），命令行的用例轮询 API 5 秒、每次隔 50 毫秒；“**什么也不存**”是记录结束之后固定等 200 毫秒（`crates/rurge-engine/tests/capture.rs` 的 `QUIET`，注释写它等的是什么、余量多大——回环上存一条只要几微秒）。

三、**`[Rule]` 的 `FINAL` 写在最后**（M4a 记录第 22 行）：写在前面会让它之后的第一行得一条 `W0019`（`rule(s) after FINAL never take effect`），与用例无关的诊断会混进来。

四、**`rurge-engine` 测试的 `common::Profile` 总在 `rules` 之后补一行 `FINAL,DIRECT`**（M4a 记录第 44 行）：引擎编到第一个 `FINAL` 为止、把最后一个放在表尾，写进 `rules` 的 `FINAL` 到不了引擎。要在 `FINAL` 上带参数的用例用 `harness_edited` 改最后一条规则的参数，不要另写一行 `FINAL`。

五、**别的 crate 的用例要往存储里放自己造的条目，经 `rurge-http` 的 `testing` 特性开的 `CaptureStore::put`**（计划 P2）：`rurge-api` 的开发依赖带着 `features = ["testing"]`，生产代码不开它——`put` 不在任何生产路径上。`rurge-api` 的测试另有一个 `flate2` 开发依赖，用来当场压 gzip（M4a 记录第 49 行）；`rurge-engine` 的测试仍没有它，照旧当场写 gzip 的字节（「阶段 4 / M3b 起」第十三条）。

六、**要断言启动时那一行 WARN 的用例自己起进程、从第一行读起**（M4a 记录第 63、69 行）：命令行的 WARN 写在两行 `listening on` 之前，而 `spawn_command` 读到两行 `listening on` 就返回、把之前的行丢掉；夹具 `lines_until_the_api` 自己起进程、有时限地（20 秒）逐行读到 `api on http://` 那一行为止，交回 `(Daemon, 之前的行, 端口)`——它交回的 `Daemon` 的 `http` 与 `socks` 是 0，不要拿它当 `spawn_command` 用。

七、**断言 clap 拒绝写错的参数的用例写成不带平台条件的 `#[test]`，有时限地等进程退出**（M4a 记录第 20 行）：`bad_capture_sizes_are_refused` 用 `try_wait` 等 20 秒，超时就 `kill`、`wait` 再 `panic!`；不用会一直等下去的 `output()`——值要是被错收下，守护进程会一直跑。断言的是退出码 `2` 与标准错误里 `parse_size` 的两句原文。

八、**两张 CA 不同名**（P14）：同名的 CA 让握手报 `BadSignature`，不报 `UnknownIssuer`（`TlsFixture::with_ca_name`）。

九、**循环跑二十次时每一次记下 `test result:` 那一行**（M4a 记录第 24 行）：只记退出码不够——过滤词没对上时退出码也是 0。`cmd /c "… & echo %ERRORLEVEL%"` 里的 `%ERRORLEVEL%` 在 `cmd` 读这一行时就展开，记下的总是 0：要用 `cmd /v:on` 与 `!ERRORLEVEL!`，或在 PowerShell 里同步调用之后读 `$LASTEXITCODE`；`Start-Process` 不能同时给 `-NoNewWindow` 与 `-WindowStyle`（进程起不来，每次都等到时限）（M4a 记录第 75 行，第 80 行订正 ①）。

十、**`GET /v1/rurge/capture/sessions/{name}/har` 的应答是分帧的流**（M4a 记录第 64 行）：`Body::from_stream` 让 hyper 用 `Transfer-Encoding: chunked` 分帧，而 `crates/rurge/tests/cli.rs` 的 `api_call` 交回分帧之后的原文；命令行的用例解析 HAR 之前先去掉块头（`dechunked`）。

十一、**`tests/cli.rs` 里起守护进程的 `async` 用例，阻塞的调用都放进 `spawn_blocking`**（M4a 记录第 69 行 ②、第 70 行）：`spawn_command`、`api_port`、`api_call`、`http_get` 都不写在运行时的工作线程上；要等就按系统时钟等（`std::thread::sleep`），不在 `async fn` 里等 `tokio::time::sleep`。这是上面「阶段 1 补遗（M5b）」那一条（本文第 266 行：会让一个工作线程停住的用例在用例自己的线程上按系统时钟等，不等运行时的定时器）的一个实例，不是新规矩。

十二、**`rurge-http` 引擎测试的 `Fake::dial` 连上之后在 trail 上留下地址**（M4a 记录第 34 行 ②）：`handle.trail().set_peer(peer)`，照真的 `ConnUpstreams::dial` 经 `trail().follow(..)` 留的那样；断言新拨的连接上条目的 `remote` 的用例靠它（`Fake` 原来只在借来的连接上留地址）。

十三、**测“应答之前重置”的 HTTP/2 用例用收下请求、什么也不答的源站**（M4a 记录第 35 行，第 40 行订正它说的原因）：`holding_origin(b"")`，不用当场就写应答头与五个字节的 `stalled_origin`——后者让重置与应答赛跑，跑二十次里十次左右应答先到，条目要等 `Tail` 被丢下才存得进去，5 秒的等待就等不到。用例要的是重置落在应答之前。

十四、**`rurge-api` 的用例要的 `HeaderMap`、`Method`、`StatusCode`、`Version` 从已有的开发依赖 `hyper` 的重新导出拿**（M4a 记录第 51 行）：axum 0.8.9 有 `pub use http;`，生产代码写 `axum::http::HeaderMap`，不为用例加 `http` 依赖。钉住“毫秒向下取整”的用例给一个不整的时长（35.999 毫秒、12.999 毫秒）：改成四舍五入时它会红（M4a 记录第 53 行 ③、第 54 行）。

十五、**进程内跑引擎的端到端用例的客户端一律是 tokio 的套接字**（M4a 记录第 72 行）：`crates/rurge-engine/tests/capture.rs` 的 `get_with`、`post_with`、`send_with` 照 `common` 的 `plain_get` 写（`async`、读的时限 5 秒、整个请求——头与正文——在**一次写**里发出）。

十六、**别的用例文件要 `[MITM]` 的夹具时从 `tests/rewrite.rs` 照抄**（P14，M4a 记录第 72、79 行 ④）：`mitm_section`、`mitm_ca`、`client_config`、`handshake`、`read_head`、`read_response`、`decrypted_get`、`harness_mitm`、`decrypted`，每个前面一句英文注释写来处与为什么不挪进 `common`（别的文件也在用它们的原样）。`tests/capture.rs` 抄来的 `RecordingProxy` 记**整个请求头**，不只请求行（M4a 记录第 79 行 ⑤）——看监听器的口令那一条问的就是头。

十七、**断言被拒绝的请求的应答（状态码、拒绝页）的用例给 `show-error-page-for-reject = true`**（M4a 记录第 79 行 ③）：缺省关着时拒绝不答话、条目的应答是 `None`，就没有应答头可以比。

十八、**看“监听器的口令不进条目”的用例红，要三处一起放开**（M4a 记录第 73 行，第 80 行订正派工）：监听器入站的 `remove`（`crates/rurge-inbound/src/http.rs` 的 `req.headers_mut().remove(header::PROXY_AUTHORIZATION)`）、`slot.arm` 闭包里的头与 `slot.leaving` 的头——`leaving` 那一处的 `Keep` 同时决定发出去的请求与条目的头（设计第 6.1 节的第 1 个取样点）。只放开前两处时用例仍是绿的。

十九、**“存储里没有一个头还引用交进来的缓冲”用 `Bytes::is_unique` 钉**（M4a 记录第 81 行）：头值做成一块 `Bytes` 的切片交进去，`commit` 之后丢掉用例自己的克隆再断言；用例是 `the_stored_headers_hold_none_of_the_buffer_they_were_read_from`。

二十、**量内存的两条各自单独跑**（`--exact`），读的是**整个进程**的峰值（M4a 记录第 76、81 行）：两条并在一起跑时，基线读到的会是装过满的那个进程的峰值。

二十一、**带等待与并发的 M4a 用例提交前各自单独循环跑二十次以上**（全过才算数）：任务 1 的 `when_finished_and_finish_race_without_losing_or_repeating`（第 21 行）；任务 3 的五条——`a_client_that_leaves_mid_body_is_captured_as_failed`、`a_killed_request_leaves_no_reservation_behind`、`an_upgrade_is_captured_at_its_101`、`a_dropped_request_is_captured_before_its_hold_ends`、`an_http2_stream_reset_before_its_answer_is_captured_with_no_response`——各二十次（第 36 行），修正轮 1 补的 `an_http2_stream_reset_mid_body_is_captured_with_what_went_by` 二十次（第 40 行）；任务 4 的九条各二十次（第 43 行）；任务 7 的四条 `cli` 用例各二十次（第 62 行），修正轮 1 之后改过的两条再各二十次（第 70 行）；任务 8 的六条端到端用例各二十次（第 75 行）。结论凭**每一次的 `test result:` 那一行**，那一次记的退出码不作数（第 80 行订正 ①）。日志在工作区（git 忽略）：`loop-task-1.log`、`loop-task-3.log`、`loop-task-3-fix-1.log`、`loop-task-4.log`、`loop-task-7.log`、`loop-task-7-fix-1.log`、`loop-task-8.log`。

### 第 17 次 CI 运行起：同一个号上的 TCP 与 UDP、互操作用例失败时的说明

CI 运行 41411445901（第 17 次）起另有两条（M4b 计划 `docs/superpowers/plans/2026-11-02-phase4-m4b-records-plan.md` 的任务 11，下面写“M4b 记录第 N 行”指它末尾的「执行期修正记录」）：

一、**同一个号上要 TCP 监听器与 UDP 套接字都有的测试服务端，用 `rurge_dns::testing::tcp_and_udp_on_one_port()` 拿这一对**（`crates/rurge-dns/src/testing.rs`，M4b 记录第 82、83 行）：它至多试 64 轮、两种次序轮流（单数轮先 TCP 后 UDP，双数轮先 UDP 后 TCP），都不成时 panic 的文本说出两种各自最后的错误。不要 bind TCP 的 0 号、再在同一个号上 bind UDP 并 `unwrap`：系统给一种套接字的临时端口，同一个号在另一种上不保证能用——第 17 次运行的 Windows 上，`crates/rurge-engine/tests/egress.rs` 的两条在这一步得到了 `WSAEACCES`（10013），它们在第 13 ～ 16 次都通过。现在用它的有 `MockDns::start`、`crates/rurge-dns/src/upstream/udp.rs` 的两条单元测试与 `egress.rs` 的两处。参照服务端要的号照旧用 `tests/interop/src/lib.rs` 的 `free_tcp_and_udp_port()`（「M8c 起的通用约定」）。

二、**互操作用例失败时，读注解先找参照服务端的那一段**（M4b 记录第 82、84 行）：`tests/interop/src/lib.rs` 的 `Reference` 在用例失败时（`std::thread::panicking()`）由 `Drop` 先问子进程是不是自己退出了，杀、收之后用 `eprintln!` 说出这一点与它日志的最后至多 20 行（2000 个字符以内，`TAIL_LINES`、`TAIL_CHARS`），连同 panic 的那一句放得进 `.github/scripts/report-test-failures.py` 的一条注解（60 行、3000 字）；不在 panic 时什么也不多说。新的参照服务端都经 `Reference` 起，不另写；要在 `Drop` 里多说什么，写进 `post_mortem` 并给它用例（`Drop` 在展开途中再 panic 会中止进程，所以那里不 `unwrap`、不 `expect`、不按字节切）。`wait_ready` 与 `wait_for_output` 的 panic 本来带着整份日志，那时注解装不下后面这一段。这几行本机运行不到（这台机器上没有参照服务端），到 2026-11-02 只编译过。

### 阶段 4 / M5a 起：QUIC 的 ClientHello 与 `auto-quic-block`

阶段 4 / M5a 起另有几条（计划 `docs/superpowers/plans/2026-11-02-phase4-m5a-quic-hello-auto-quic-block-plan.md`，下面写“M5a 记录第 N 行”指它末尾的「执行期修正记录」；M5a 的提交都在本地，CI 都没有见过）：

一、**QUIC 的用例只喂数据报，不完成任何 QUIC 握手**（总设计第 14 节第 5 条，设计第 10 节）：数据报喂给 `rurge_net::quic::initial` 的 `read_first` 与 `HelloReader::read`（单元用例），或经 SOCKS5 的 UDP 关联（`crates/rurge-engine/tests/udp.rs` 与 `tests/mitm.rs`）与内存设备上的虚拟网卡（`tests/tun.rs`）发出去。quinn 的真客户端只在两条用例里当客户端，只连一个只收不答的回环套接字，截下前几个数据报就丢掉 `Connecting`：`a_real_quinn_client_initial_is_one`（分类没变）与 `a_real_quinn_client_hello_across_two_datagrams`（拼出的消息）。没有一条 QUIC 握手被完成过，也没有一条用例等服务器应答。

二、**数据报由 `quic::testing` 的构造器造，不建套接字、不读文件、不联网**（M5-D2，计划 P3）：`crates/rurge-net/src/quic/testing.rs`（`rurge-net` 的 `testing` 特性；引擎与 API 的开发依赖开着它）里的 `Frame` 六种（`Padding`、`Ping`、`Ack`、`Crypto`、`ConnectionClose`、`Raw`）、`Initial`（`seal`）、`long_packet`、`datagram`、`client_hello`（rustls 的 QUIC 客户端在内存里写出的真 ClientHello）、`big_client_hello`（一长串 ALPN 撑到一个 1200 字节的包装不下）、`hello_initial` 与 `hello_initials`、`chaos_frames` 与 `chrome_frames`（Chrome 的两种排法，只取 QUICHE 的事实），以及两份 RFC 样例（`RFC9001_CLIENT_INITIAL`、`RFC9369_CLIENT_INITIAL`）与 `unhex`。随机的部分用固定的种子（私有的 SplitMix64），断言的信息里带种子。

三、**持住的两个上限只在暂停时钟的单元用例里量，端到端不等它们**（设计第 5.2 节，计划 P5）：`HELLO_WAIT`（300 毫秒）与 `HELLO_DATAGRAMS`（16）没有测试用的旋钮（M5-D7）。边界由 `crates/rurge-engine/src/udp.rs` 的六条 `holding_…` 量（`#[tokio::test(start_paused = true)]`：299 毫秒时还持着、第二个数据报到时放、第 16 个时放、关联结束与队列关上时结束、读者放弃时带着原因结束）；端到端只有“只发第一个数据报”的那几例等它过去（`a_hello_that_never_completes_goes_out_after_300_ms`、`an_incomplete_hello_is_not_blocked`），等的是一个 300 毫秒的期限，有界。

四、**放行的 QUIC 流的数据报发往没有人收的回环端口，用例不断言说明为空**（设计第 16 节第 2 条，计划 P12）：这些用例的目标多是钉到 `127.0.0.1` 的名字的 443（那个号不归用例所有，不能在那里放一个收数据报的服务器），没有收的一端也照发。断言的是记录与计数（`status` 不是 `rejected`、`error` 里没有 `QUIC blocked by auto-quic-block`、`quic.blocked` 不变），**不断言说明为空**：Windows 上可能写上一个“连接被重置”一类的说明。要收回数据报的用例自己绑一个回环套接字（`tests/udp.rs` 的 `udp_sink()`，64 KiB 的缓冲、不答）。

五、**失败信息只写情形与长度，从不打印数据报与 ClientHello 的字节**（M5a 记录第 13、17 行）：比整个数据报或整个 ClientHello 的断言写成 `assert!(a == b, "…{} bytes, not {}", …)`，信息里只有情形、两边的长度与（随机的构造）种子。`HelloReader` 的 `Debug` 手写，只写版本、最大包号、段数与字节数，不写密钥（计划 P4 ⑦）。数据报的字节、ClientHello 的字节与密钥不进日志、错误文本或 `Debug`；打印长度、段数、包号与种子可以。

六、**任务 3 的六条端到端一起并行跑，在这台机器上会偶发 `STATUS_HEAP_CORRUPTION`**（M5a 记录第 18 行）：`crates/rurge-engine/tests/udp.rs` 的 `a_hello_across_two_datagrams_is_routed_by_its_sni`、`without_extended_matching_the_sni_does_not_match`、`a_hello_that_never_completes_goes_out_after_300_ms`、`an_unreadable_hello_is_noted`、`a_flow_whose_association_ends_while_held_leaves_no_record`、`the_rfc_sample_flow_has_its_sni` 一起（并行）跑 100 次，有 4 次以 `STATUS_HEAP_CORRUPTION`（0xC0000374）退出、没有失败的用例，都停在刚开跑时；每条单独各跑 50 次（共 300 次）与六条一起加 `--test-threads=1` 跑 60 次都是 0 次。这与 `CLAUDE.md`「这台开发机」第一条、阶段 2 / M8 设计第 3.5 节查明的征状相同（注入的网络钩子在几个线程同时开始用套接字时出错），记作环境，不为它改代码；撞上时单独重跑那一个测试二进制。


### 阶段 4 / M5b 起：基准与遗留

阶段 4 / M5b 起另有几条（计划 `docs/superpowers/plans/2026-11-03-phase4-m5b-bench-legacy-plan.md`，下面写“M5b 记录第 N 行”指它末尾的「执行期修正记录」；M5b 的提交都在本地，CI 都没有见过）：

一、**基准是量、不是门**：B1 ～ B5 与 Q1 十六个都是 `#[ignore]` 的用例，CI 的 `cargo test` 与门禁编译它们、不跑；没有“先红”，`HTTP_BENCH_QUICK=1` 在调试构建里缩小规模、只看跑不跑得通；守护读数解析的 `the_readings_are_parsed_from_what_the_system_prints` 不忽略，照常先红（先写、看它红、再实现到绿）。

二、**`--test bench` 是引擎里第二处用 `-p` 跑的**（第一处是 `relay_throughput`）：同 `rurge-tun` 的三个基准（命令一节的那三行）与命令一节里另几条带 `--release` 的基准（WireGuard 与 Hysteria 2 的吞吐、`rurge-rules` 的内存），只另编这个 crate 与它的依赖的一份 release。门禁照旧经整个工作区跑、不用 `-p`。

三、**内存的读数一个进程一个**：每个 B5 一开头查 `static MEASURED: AtomicBool`，同一个进程里跑到第二个 B5 就 panic（`one B5 measurement per process: run it alone with --exact`），所以每个基准单独跑（`--exact`、`--test-threads=1`）；峰值只涨不落，差用有符号的数（Windows 会修剪工作集，`now` 会变小）。

四、**遗留的端到端用例用真实的时钟**：`a_request_sent_again_waits_its_whole_idle_on_the_new_connection` 约 3 秒，常数是 `IDLE` 2,500 毫秒、`HOLD`/`DELAY` 1,500 毫秒，绿的一侧每处至少 1 秒余量，提交前单独循环 25 次全过（M5b 记录第 11、12 行）；它钉住重发之前那一处 `restart`：删掉它，第二个请求答 504（M5b 记录第 11 行的变异 ③）；`Idle` 第一次 `poll_frame` 才起表与 `Progress::restart` 本身由两条单元用例钉住（变异 ①、②）。两条单元用例（`idle_starts_its_clock_at_the_first_poll`、`restart_gives_the_resent_request_its_whole_wait`）用暂停的时钟，`rurge-http` 的开发依赖因此给 tokio 开了 `test-util`。

五、**测 TLS 会话恢复的用例，票据在握手之后才到，客户端要读一下才收得进**（M5b 记录第 16 行）：rustls 服务端在 TLS 1.3 上照样发票据（没有 `ticketer` 时走有状态的一支，把会话放进 `session_storage`），但票据在服务端收到客户端的 Finished 之后才发，客户端握完不读就收不进；rustls 客户端缺省的存储至多留 31 个服务器、挤掉最早的，名字多于它而依次握两遍时第二遍一个也恢复不了。这一条是协调者加的（P13 ① 没有）。
