# 阶段 4「HTTP 引擎」设计文档

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-29 |
| 状态     | 草案。已决事项（第 17 节 P4-D1 ～ D26）是在项目所有者"自己决定、继续做"的常设授权下定的，所有者可以改 |
| 对应需求 | [需求文档](../../requirements.md) 第 7 节「阶段 4」：FR-HTTP-01 ～ 09、10（完整）、11、13、14，FR-RULE-14；另有登记到阶段 4 的 FR-RULE-07 / 08 的拨号之前部分（第 1.2 节） |
| 兼容基线 | [Surge 兼容性清单](../../surge-compatibility-matrix.md) 第 1.2、1.6、2.1、3.1 ～ 3.5、7、10.3、10.4 节；手册文本取自 2026-10-29，与 2026-10-28 的文本逐字节相同（Surge Mac 6.11.0 / iOS 5.24.0） |
| 前置     | 阶段 1 ～ 3 的里程碑都已合并，阶段 3 的需求逐句对过代码（M6）；三个阶段的手工验收都没有做；CI 最近一次三平台全绿是 `41c35d4` 上的运行 40824893108 |
| 调研     | [`docs/reports/2026-10-29-phase4-manual-recheck.md`](../../reports/2026-10-29-phase4-manual-recheck.md)（阶段格含 4 的 64 行对着手册逐行重读）；[`docs/reports/2026-10-29-phase4-http-mitm-survey.md`](../../reports/2026-10-29-phase4-http-mitm-survey.md)（hyper 作代理、现成的 MITM 代理、嗅探、HTTP/2、抓包格式、上游连接池）；[`docs/reports/2026-10-29-phase4-certs-body-survey.md`](../../reports/2026-10-29-phase4-certs-body-survey.md)（伪造的叶证书、rcgen、`ca-p12`、rustls、正则、jq、正文解码） |

本文档是阶段 4 的总设计：定目标与范围、读手册得到的事实与需求文档的出入、技术选型、crate 边界、跨里程碑共用的做法、里程碑拆分、测试策略与验收。每个里程碑开工前再各写一份细化设计与实施计划（同阶段 2、3）；类型与函数的名字、API 的 JSON 形状、常数的取值、诊断码的编号在那里定稿，不在本文档展开。

三份调研都是只读的助手写的（读手册文本、公开网页与本机 cargo 注册表里的源码，什么也没有运行）。调研报告里多数网页是经摘要读到的，报告自己标了每一条的来源；本文档引用时照此对待：标【源码】的是读了本机锁定版本的源码，其余的数字与行为在用到之前对着原文或源码再核一遍。

## 1. 目标与范围

### 1.1 阶段目标

抓包、HTTPS 解密与全部重写能力（需求文档第 7 节）。落到使用上：`[MITM]` 配好 CA 之后，发往名单里主机的 HTTPS 被解密；经 HTTP 引擎的每个请求在请求记录里各占一条，带方法、URL 与状态码，各自匹配规则、各自走自己的策略——`URL-REGEX`、`USER-AGENT` 从此对经 SOCKS5、`CONNECT` 与虚拟网卡来的明文 HTTP 与解密之后的 HTTPS 都生效；`[URL Rewrite]`、`[Header Rewrite]`、`[Body Rewrite]`、`[Map Local]` 生效；抓到的请求头与正文能经 API 读出、导出成 HAR；`mitm`、`rewrite`、`capture` 三个开关运行中切换、重启之后保留。

### 1.2 范围内（引用需求文档编号）

| 领域 | 需求 |
| ---- | ---- |
| 会话入口与嗅探 | FR-HTTP-01 的 80 / 443 协议嗅探、`force-http-engine-hosts`、`always-raw-tcp-hosts` / `always-raw-tcp-keywords`；FR-RULE-07 / 08 的拨号之前部分（从 TLS ClientHello 提取 SNI、从 Host 与 `:authority` 提取主机名、识别协议）（阶段格写 1，清单与 M8c 计划延后第 105 行登记在阶段 4）；`extended-matching` 对真实会话生效，连同规则集文件里一行上的 `extended-matching`（清单 L314，阶段 1 两句没做完的其中一句） |
| HTTP 引擎 | FR-HTTP-01（HTTP/1.1 解析与转发、keep-alive、chunked、WebSocket 升级透传）、FR-HTTP-02（`h2=true` 时与客户端、上游用 HTTP/2，`:authority`） |
| MITM | FR-HTTP-03（`[MITM]` 九个键、叶证书、`client-source-address`、`skip-server-cert-verify`、证书固定的提示、`ca-p12` / `ca-keystore-name`、`rurge mitm ca generate`、`GET /v1/mitm/ca`）、FR-HTTP-04（`auto-quic-block`）、FR-HTTP-14（`http-api-tls`） |
| 重写 | FR-HTTP-05 ～ 09（URL / Header / Body Rewrite、Map Local、流水线的顺序） |
| 观测 | FR-HTTP-10（完整）、FR-HTTP-11（抓包）、FR-RULE-14（`always-capture`） |
| 开关 | FR-HTTP-13（`mitm`、`capture`、`rewrite` 运行时切换并持久化） |
| 规则 | FR-RULE-01 的 `USER-AGENT`、`URL-REGEX` 对引擎里的每个请求求值（阶段 1 起只对 HTTP 监听器转发的明文请求） |
| 遗留 | 前几个阶段登记到阶段 4 的事项（第 1.5 节） |

### 1.3 范围外

| 项 | 去向 |
| -- | ---- |
| 脚本（`[Script]`、`http-request` / `http-response` 脚本、`requires-body`、`max-size`、`full-header-mode`） | 阶段 5。本阶段在流水线里给它留位置（第 6.2 节），正文的缓冲与"超出上限就放过"的做法让脚本复用（手册：脚本的上限与 Body Rewrite 共用 32 MB，重对报告第 2 节第 5 条） |
| Dashboard 的抓包视图、CLI 的 `feature` 命令 | 阶段 6。本阶段给出 API |
| 把 CA 自动装进系统信任库 | 清单 L585 写的是 P2；它改系统设置，归项目所有者（第 17 节 Q5）。本阶段只给各平台的手工步骤 |
| HTTP/3 的解密 | 不做。`auto-quic-block` 让客户端回落到 TCP 上的 HTTP/2 或 HTTP/1.1 |
| WebSocket 的逐条消息查看与改写 | 手册没有这一项；本阶段只在升级之后透传字节 |
| 证书固定的主机自动绕开 | 手册只说记一条提示；本阶段照手册，不自动绕开 |
| `client-source-address` 的 MAC 地址写法 | 阶段 7（要网关模式，清单 L583 的阶段格是 4 / 7）；本阶段照常解析，MAC 项永不匹配 |
| HTTP/2 的明文先验（h2c prior knowledge） | 不进引擎，按原样转发（第 5.4 节）；Surge 是否认它，手册没有写 |
| ECH（加密的 ClientHello） | 客户端用 ECH 时 rurge 只看得到外层的 SNI（公共名）：规则与 MITM 名单按它比，这样的连接不解密（握手里拿不到内层的名字）；登记。手册没有写，调研没有覆盖 |

### 1.4 里程碑拆分

六个里程碑。顺序依据两点：**会话入口与明文 HTTP 引擎是其余一切的底座**，先做；**MITM 早于重写**，因为今天的流量大多是 HTTPS，重写在解密之后才有用，先有 MITM 的话重写一做完就能在 HTTPS 上验证。

| 里程碑 | 内容 | 需求 | 可验收的产出 |
| ------ | ---- | ---- | ------------ |
| **M1 会话入口与明文 HTTP 引擎** | 新 crate `rurge-http`；可嗅探的会话先答后读、嗅探先于规则（第 5 节）；ClientHello 的完整读取（SNI、ALPN）；`SessionInfo` 在规则之前带上 SNI、Host 与协议；`extended-matching` 生效与清单 L314 的补完；`force-http-engine-hosts` 与 `always-raw-tcp-*`；明文 HTTP/1.1 引擎（keep-alive、chunked、WebSocket 透传）；每个请求各自匹配规则；上游连接池；经 HTTP 监听器的绝对形式请求改走引擎（含 fake 地址的改写，R27）；每个请求一条记录（方法、URL、状态码、所用上游连接的 `remoteAddr`）。可嗅探的端口只有 80、443 与 `force-http-engine-hosts`，`[MITM]` 名单写明的端口随 M2 | FR-HTTP-01、FR-RULE-07 / 08 的拨号之前部分、FR-RULE-01 的 HTTP 类、FR-HTTP-10 的一部分 | 经 SOCKS5、`CONNECT`、内存设备上的虚拟网卡与 HTTP 监听器来的明文 HTTP 端到端：每个请求各自命中规则、复用上游连接；嗅探的三种结局；`extended-matching` 命中 SNI |
| **M2 MITM** | `[MITM]` 类型化（九个键）；`[MITM] hostname` 写明的端口进可嗅探的条件；CA 的载入（`ca-p12`、`ca-keystore-name`）；叶证书的签发与缓存；与客户端的 TLS、与上游的 TLS（校验与跳过）；HTTP/2 两侧；证书固定的提示；`client-source-address`；`rurge mitm ca generate` / `export` 与安装指引；`GET /v1/mitm/ca`；`http-api-tls`；功能开关的持久化（第一个是 `mitm`） | FR-HTTP-02、03、13（`mitm`）、14 | 回环上的 HTTPS 服务器经三种入口被解密，测试客户端只信测试 CA；h2 与 h1 的四种组合；握手之后不发请求的客户端触发提示 |
| **M3 重写与 Map Local** | 四个节的类型化与一行的切法（`value.rs` 与 `redact.rs` 同一个提交）；URL / Header / Body Rewrite；正文的缓冲、上限、解压；正则与 jq；Map Local（文件、URL、文本、Base64、`tiny-gif`）；`rewrite` 开关；请求记录里的备注 | FR-HTTP-05 ～ 09、13（`rewrite`） | 手册每个重写示例一条用例（第 15 节第 1 条）；32 MB / 10 MB 两个上限的用例 |
| **M4 抓包与请求记录的完整** | 抓包存储（总量与单条的上限、命名会话）、`always-capture`、`capture` 开关；`/v1/rurge/capture/…` 与 HAR 导出；FR-HTTP-10 的其余字段与前几个阶段登记的空缺（第 1.5 节） | FR-HTTP-10、11、13（`capture`）、FR-RULE-14 | 经 API 读出抓到的请求头与正文、导出的 HAR 能被解析；`always-capture` 在开关关着时照样抓 |
| **M5 `auto-quic-block`、基准与遗留** | QUIC Initial 的解密与跨包的 ClientHello 重组、按 `[MITM]` 名单阻断；HTTP 引擎的基准；遗留事项里落到本阶段而前四个里程碑没有收的 | FR-HTTP-04 | 用 RFC 9001 附录 A 的样例包与跨两个包的 ClientHello 的用例；基准数字 |
| **M6 收尾与验收** | 阶段 4 的需求逐句对照代码（照阶段 1、3 的做法）、验收文档、手工清单、兼容性清单、README | — | 第 15 节 |

M1 体量最大，细化设计时多半拆成两份计划：M1a（会话入口、嗅探、规则之前的 SNI 与协议、`extended-matching`）→ M1b（HTTP 引擎、连接池、记录、HTTP 监听器改走引擎）。M2 也可能把 `http-api-tls` 与 CA 工具拆出去。

### 1.5 前几个阶段登记到本阶段的事项

| 来源 | 事项 | 去向 |
| ---- | ---- | ---- |
| 阶段 3 / M4 设计 M4-D24（M3c 计划延后第 19 行，清单 R27） | 经监听器来的绝对形式明文请求里，主机是 fake 地址时保留着它 | M1：引擎把请求行与 `Host` 里的 fake 地址换回域名（第 6.8 节） |
| M8c 计划延后第 105 行 | FR-RULE-07 / 08：拨号之前取 SNI 与 Host、识别协议 | M1（第 5 节） |
| 清单 L314；阶段 1 验收的"没有做完"之一 | 规则集文件里一行上的 `extended-matching`：进了域名索引的行丢了标志，"一行带、整集都带"没有做 | M1 |
| 阶段 1 / M3a 计划执行期的记录（"HTTP 侧的 REJECT-DROP 不随客户端关闭提前结束（hyper 服务内观察不到），只能固定保持到超时；阶段 4 自有 HTTP 引擎后统一"） | HTTP 一侧的 `REJECT-DROP` 看不到客户端提前关闭 | M1 设法解决（第 6.7 节）；解决不了照实登记 |
| 阶段 1 / M5a 计划延后第 3、20、21、22 行 | `remoteAddr` 在复用连接与隧道的会话上缺；`dnsMs` 在一次解析结束时才有数 | M1：经引擎的请求借用池里的连接时也有 `remoteAddr`（池是 M1 建的，拨号时记下的地址随池里的连接存着，第 6.4、9.1 节）；其余（隧道上复用的会话、UDP 流、`dnsMs`）M4（FR-HTTP-10 完整） |
| 阶段 1 / M5b 计划延后第 31 行 | 把 Profile 文本变成本机路径的东西先问 `Reach` | M3：Map Local 的 `data=` 路径（第 8.5 节） |
| 阶段 3 / M5 计划延后第 6 行 | QUIC 里的 SNI、跨几个包的 ClientHello、按 SNI 匹配规则 | M5（第 11 节） |
| 手册基线重对计划延后第 8 行 | 2026-10-13 之后的测试版条目（跨多个包的 QUIC 主机名识别等）手册没写 | M5 照这一条做跨包重组；M6 重对一次发行说明 |
| 归项目所有者、与本阶段相交的 | M8c 计划延后第 52 行（`[MITM] # 注释` 的节头）、第 43 / 51 / 72 行（脱敏视图里重写节的头值）、第 105 行（阶段栏）；阶段 3 / M5 计划延后第 4 行（NFR-01 的读数）、第 16 行（gQUIC）；基线重对计划延后第 6 行（`CORE_VERSION`） | 不代定，列在第 17 节 |

## 2. 重读手册的结果，与需求文档的出入

### 2.1 重读的结论

阶段 4 开始时把清单里阶段格含 4 的 64 行对着手册逐行重读了一遍（报告 [`2026-10-29-phase4-manual-recheck.md`](../../reports/2026-10-29-phase4-manual-recheck.md)）：49 行一致，14 行"手册多说了"，**没有一行与手册相冲**，1 行（L860，`rurge mitm ca`）是 rurge 自己的东西。报告第 2 节是手册里有、清单里没有的 7 处，第 3 节是手册没有写、设计只能自己定的 25 处（3.1 ～ 3.25），本文档第 2.3 节逐处给出做法。这一遍同前两轮一样**没有观察过 Surge 的实际行为**。"手册多说了"的 14 行在实现它们的里程碑里改清单（第 18 节）。

### 2.2 照手册而与需求文档字面不同的

| 需求文档 | 手册 | 本设计 |
| -------- | ---- | ------ |
| 3.3 节流水线第 8 步"MITM 解密 → Header Rewrite → URL Rewrite → Body Rewrite → 脚本 → Map Local 短路"；3.2 节 `rurge-http` 依赖 `rurge-rules`、`rurge-policy`，职责里有"请求记录与抓包" | "After a policy is selected, HTTP requests and responses can be rewritten, mapped to local data, or processed by scripts."（`getting-started/how-surge-works.html`）；HTTP 类规则看一个请求（`rules/http.html`） | **每个请求先各自匹配规则、选定策略，再走重写**（第 6.2、6.3 节）；Map Local 排在 URL Rewrite 之后、Body Rewrite 之前（第 2.3 节 3.1）。规则与策略经引擎实现的 trait 到 `rurge-http`，`rurge-http` 不依赖 `rurge-rules`、`rurge-policy`；请求记录照旧在 `rurge-engine`，抓包在 `rurge-http`（第 4 节）。需求文档这两处随本文档订正 |
| FR-HTTP-03"SNI 命中忽略端口" | "For TLS or QUIC connections identified by the sniffed SNI, a bare entry matches regardless of the connection's actual port."（`http/mitm.html`） | 照手册：**只有不带端口的项**、只对靠嗅探到的 SNI 认出的 TLS 与 QUIC 不看端口（第 7.1 节）。需求文档随本文档订正 |
| FR-HTTP-07"响应 10 MB 直通" | "A response body may be modified only up to 1 MB on iOS and 10 MB on macOS."（`http/body-rewrite.html`） | 取桌面（macOS）的 10 MB，不改 |
| FR-HTTP-11"通过 API 与 Dashboard 查看" | 手册的 HTTP API 没有读抓包内容的端点（重对报告要点 8） | 端点是 rurge 专有的，放在 `/v1/rurge/capture/…`（需求文档 9.2 节 Q6 的决定）；容量走命令行参数与环境变量（FR-CFG-17） |
| FR-HTTP-03"`rurge mitm ca generate` 生成 CA 并给出各平台信任安装指引" | "The certificate is generated locally and saved only in your profile and the system Keychain."（`http/mitm.html`） | rurge 不改写配置文件（兼容性原则）、不改系统设置：生成的东西写进使用者指定的目录，配置的几行由使用者自己贴进 `[MITM]`（第 7.6 节）；清单 L585 已这样登记 |
| FR-HTTP-13"持久化" | 手册没有说开关是否跨重启保留 | 照需求文档持久化（第 10.1 节） |

### 2.3 手册没有写、由本设计定的

重对报告第 3 节的 25 处，在本设计里的归宿（"M*n*"表示细化设计里定稿并登记清单的里程碑）：

| # | 问题 | 做法 |
| - | ---- | ---- |
| 3.1 | Map Local 在流水线的哪一步 | 请求一侧依次：Header Rewrite → URL Rewrite（`302` / `307` / `reject` 在这里短路）→ Map Local（命中就短路）→ Body Rewrite → 请求脚本（阶段 5）→ 上游。Map Local 比的是 URL Rewrite 之后的 URL；它给出的应答不过响应一侧的重写（理由：Map Local 是"代替上游"，响应一侧的重写是"改上游给的东西"）。阶段 5 的请求脚本放在哪里由阶段 5 定（M3） |
| 3.2 | 规则怎样逐个请求求值；连接一层与请求一层的关系；解密之后的请求被拒绝 | 每个请求各自组一份会话信息求一次规则（第 6.3 节）；承载请求的连接不另求规则——解密与否由 `[MITM]` 名单定，不由规则定。解密之后的请求被 `REJECT` 一族拒绝时与明文请求一样：`show-error-page-for-reject` 开着给 403 错误页（在 TLS 里），关着就关连接；`REJECT-TINYGIF` 给那张 GIF；`REJECT-DROP` 停一会儿再关（M1） |
| 3.3 | 嗅探等多久、看多少字节、服务器先说话的协议 | 第 5.2、5.4 节：只嗅探"可嗅探"的会话；等第一批客户端字节至多 15 秒，TLS 读到完整的 ClientHello 为止（至多 64 KiB）；超时、客户端关闭写端、判断为别的协议，都按原样转发。服务器先说话的协议在这些端口上要晚 15 秒才通，`always-raw-tcp-*` 就是给它们的（手册："without sniffing or parsing"） |
| 3.4 | SOCKS5 与 `CONNECT` 何时答"成功" | 可嗅探的会话先答（SOCKS5 的成功应答、`BND` 为 `0.0.0.0:0`；`CONNECT` 的 200；虚拟网卡本来就先完成握手），之后拨号失败只能关连接——引擎里的单个请求失败另有 502 错误页；不可嗅探的会话照旧先拨号再答（第 5.3 节） |
| 3.5 | 叶证书与 CA 的参数、p12 的算法 | 第 7.2、7.3、7.6 节 |
| 3.6 | `h2 = true` 时两侧各用什么 | 第 7.4 节：对客户端在 ALPN 里给 `h2` 与 `http/1.1`，对上游同样两个都给，两侧各自协商、版本不同由引擎转换；`h2 = false` 时两侧只给 `http/1.1` |
| 3.7 | 上游证书不合法、又没有跳过校验时 | 这个请求失败：502 错误页（`show-error-page` 开着时），请求记录写原因；客户端那一侧的握手早已完成，不改成 TLS 错误（M2） |
| 3.8 | 没有 SNI 的 ClientHello | 用连接自己的目标去比名单、去签叶证书：`CONNECT` 的主机、SOCKS5 的域名、fake 地址反查到的域名，都没有时是 IP；目标是 IP 时叶证书带 IP 的 SAN；`<ip-address>` 一类的项照 IP 比（M2） |
| 3.9 | `client-source-address` 对本机流量取哪个地址 | 本机发起的会话——监听器上来自回环的、虚拟网卡上来自本机应用的——一律以 `127.0.0.1` 计（照手册"`127.0.0.1` should be included if you want to enable MITM for the current device"）；别的设备用它自己的地址；名单按顺序匹配，第一项决定（M2） |
| 3.10 | `always-raw-tcp-hosts` 不带端口的项 | 对所有端口（今天的 `HostList` 没有缺省端口时就这样读）；嗅探只发生在可嗅探的端口上，所以实际起作用的是 80、443 与名单写明的端口 |
| 3.11 | `always-raw-tcp-keywords` 分不分大小写 | 不分（会话里的域名已是小写，关键字比之前也转小写） |
| 3.12 | 同一主机既在 `always-raw-tcp-*` 又在 MITM 名单里 | `always-raw-tcp-*` 先于一切：命中的连接不嗅探、不解密、不进引擎 |
| 3.13 | URL Rewrite 的几个应答 | `reject` 同 `REJECT` 策略对明文请求的应答（3.2）；`302` / `307` 的应答只有 `Location` 与 `Content-Length: 0`；当前出口没有网关时含 `{{{GATEWAY_ADDRESS}}}` 的规则放弃并留备注；`header` 模式把 `http` 改成 `https` 时上游走 TLS；`wss` 与 `ws` 同样算合法（手册只写了 `ws`，`wss` 是它的 TLS 形式；登记）（M3） |
| 3.14 | 用 Host 与连接主机名重建的 URL 是否也用于别的三种 | 不用：照手册字面只在 URL Rewrite 一页（M3） |
| 3.15 | Header Rewrite 的细节 | 字段名不分大小写；`header-del` 删全部同名行；`header-replace` 换全部同名行的值；`header-replace-regex` 在每一行的值里替换全部匹配；改到 `Content-Length` 或 `Transfer-Encoding`（加、删、改）时这个请求失败：请求方向不发往上游，响应方向丢掉上游的应答，两种都给 502 错误页并写进记录；加载时对写明这两个名字的行另给一条告警（M3） |
| 3.16 | Body Rewrite 的细节 | 每个正则替换全部匹配（多行模式，`$1` 模板）；`^$` 照正则的字面（多行模式下匹配空行，空正文上匹配一次，所以能生成正文）；解压之后不再压回：去掉 `Content-Encoding`、重算 `Content-Length`；jq 产出多个值时依次以换行连接（同 `jq -c` 的输出）；正则与 jq 的方言见第 3.3、3.4 节（M3） |
| 3.17 | Map Local 的上限、URL 的缓存、`header` 里写了框架头 | 文件不设上限（从磁盘流式读出，不进内存）；URL 的数据经资源管理器下载与更新，沿用它的上限与间隔；`header` 里的 `Content-Type` 盖过按扩展名的补全，`Content-Length` 与 `Transfer-Encoding` 不采用（由引擎按数据算；同 Header Rewrite 不许改框架）（M3） |
| 3.18 | 抓包存什么、上限、命名会话、关掉之后 | 第 9.2 节（M4） |
| 3.19 | 开关是否跨重启保留 | 保留（FR-HTTP-13；第 10.1 节） |
| 3.20 | 一个请求一条记录，承载它们的连接是否另有一条 | 第 9.1 节：一个请求一条；承载的连接只在一个请求也没有走完时有一条（握手失败、握手之后不发请求、读不出请求） |
| 3.21 | HTTP/1.1 的细节 | keep-alive 两侧都有；管线化的请求串行处理（hyper 的做法）；不改写正文时 `Expect: 100-continue` 由 rurge 在开始读正文时答 `100 Continue`，不等上游（hyper 的做法，登记）；WebSocket 在 101 之后透传字节（第 6.6 节） |
| 3.22 | `http-api-tls = true` 而没有 CA | API 不启动，给告警（不退回明文：使用者要的是加密，退回明文会让密钥以明文过网络）（M2） |
| 3.23 | `ca-passphrase` 为空 | 接受（rurge 的平台没有那个限制；这样的配置 Surge 不收，rurge 收，登记）（M2） |
| 3.24 | 旧配置里的 `[Replica]` | 认得的节、忽略，给一条说明"Surge Mac 5.4.0 起已移除"的提示级诊断（M3） |
| 3.25 | 重写节一行的切法 | 按空白切；一项可以用 `"…"` 或 `'…'` 包住（去掉外面那对引号，里面的空白保留，没有转义，一项里不能有包住它的那种引号）；`key=value` 的值同样可以包住；`value.rs` 与 `redact.rs` 在同一个提交里改（CLAUDE.md「一行怎样拆成各项」）（M3） |

## 3. 技术选型

### 3.1 HTTP 的实现（P4-D4）

调研（HTTP / MITM 报告第 1、2 节）的候选：

| 方案 | 要点 | 对 rurge |
| ---- | ---- | -------- |
| hyper 1.x（锁定 1.11.1，已在依赖里） | HTTP/1.1 两侧的连接、HTTP/2 的封装、升级；`preserve_header_case` 两侧都有，但原始拼写**读不到**（映射类型是 `pub(crate)`）、头的**顺序保不住**（写出的顺序是 `HeaderMap` 的遍历顺序，交错的同名头被聚拢）；发不出 1xx（除了 101 与自动的 `100 Continue`）；请求里的折行头与非 UTF-8 的请求目标直接 400；trailers 要声明才转；body 是真背压【源码】 | 手册没有要求字节级的保真；这些限制登记进清单即可 |
| 用 `httparse` 自己写 HTTP/1 编解码 | 能保住顺序与拼写、能转 1xx | 请求走私的风险自己担：Cloudflare 的 Pingora 正是因为宽容而在 2026 年修了三个 CVE（调研第 1.12 节） |
| rama（维护 hyper、h2 的同步分叉） | MITM 的现成栈 | MSRV 1.96，高于项目的 1.89；示例自述不到生产水平 |
| hudsucker | 现成的 MITM 框架 | 只认 `GET ` 与 TLS 两种开头、叶证书只有 DNS 名、复用 CA 的私钥、总删 `Host`、上游池只按 scheme 与 authority 分（调研第 2.1 节）；rurge 要每个请求各走自己的策略，套不进它的上游 |

**决定**：用 **hyper 1.x**（服务端 `http1` 带升级、`http2`；客户端 `conn::http1` / `conn::http2` 加自己的连接池，第 6.4 节），不用 hyper-util 的 `legacy::Client`（它的池键只有 scheme 与 authority，装不下策略，第 6.4 节）。hyper 的限制在 M1 的文档任务里逐条登记进清单 7.1 节（第 6.9 节）。以后要字节级保真时，换掉的只是 `rurge-http` 里编解码的那一层。

### 3.2 TLS 与证书（P4-D8、D9、D10）

- **rustls 0.23 与 tokio-rustls 0.26**（已在依赖里）做两侧的 TLS；ClientHello 由 rurge 自己读完、记下读过的字节再交给 rustls（第 5.4 节），不靠 `LazyConfigAcceptor`——它读走的字节不还回来，判断为"不解密"时就回不去了（调研第 3.3 节）。
- **rcgen 0.14**（已在锁文件里，今天只在测试里用）**成为 `rurge-http` 的正式依赖**，加 `x509-parser` 特性（`x509-parser` 已在依赖树里，由 `p12-keystore` 带来）以便用 `Issuer::from_ca_cert_der` 从使用者的 CA 证书里读出 subject 与 SKI；ring 后端能生成 ECDSA P-256 / P-384 与 Ed25519 的密钥、能用 2048 ～ 4096 位的 RSA 私钥签名，不能生成 RSA 密钥（证书报告第 2.4 节【源码】）。
- **p12-keystore 升到最新的 0.4.x**（今天锁定 0.2.1，`rurge-proto` 的 `[Keystore]` 用它）：0.3.2 起对迭代次数、scrypt 内存与并行度设了上限、链里遇到环会停（此前会无界地长内存），0.3.0 起认得明文的 `keyBag` 与没有 `localKeyID` 的文件（证书报告第 3.5 节，读自更新说明）。`ca-p12` 可能来自托管配置（下载到的文本），没有上限的解析是一个拒绝服务的口子。升级会改到 `crates/rurge-proto/src/keystore.rs` 的几处调用，M2 的第一个任务做。
- **上游证书校验的根**：系统的根 ∪ `webpki-roots`（第 7.5 节）。

### 3.3 正则（P4-D14）

**沿用工作区直接依赖的唯一正则包 `fancy-regex`**（今天锁定 0.14.0；`URL-REGEX` 规则的 `rurge_config::rule::Pattern` 就是它）。手册没有点名 Surge 用哪个正则引擎（证书报告第 5.1 节：手册里一处也没有）；阶段 1 起的设计把"Surge 用 Apple 的 `NSRegularExpression`（ICU 的语法）"当作前提，那是推断——Surge 跑在 Apple 平台上，手册写的 `$1` 与"`^` `$` 逐行锚定"正好是它的模板与 `anchorsMatchLines`——没有证实。本阶段照旧以它为准，差异分三类处理（证书报告第 5.2 ～ 5.4 节，fancy-regex 一侧读的是本机 0.14.0 的源码）：

- **替换模板：自己展开**，照 `NSRegularExpression` 的模板：`$` 后读不超过分组数所需位数的数字，编号超出分组数的得空串，`$` 后不是数字时是字面的 `$`，反斜杠只转义 `$` 与它自己。fancy-regex 自己的展开在这里悄悄地不同：`$1abc` 被读成名为 `1abc` 的组（得空串），`\$` 与 `\\` 不认。三种重写共用这一个展开器（几十行），不用 fancy-regex 的 `expand`。
- **悄悄不同的写法**：`\h` / `\H`（ICU 是水平空白，fancy-regex 是十六进制数字）、`\v`、`\<` / `\>`（ICU 是字面的 `<` `>`，fancy-regex 是词边界），以及 `.` 与多行模式的 `^` `$` 对 `\r\n` 的处理（HTTP 正文常用 CRLF：`^foo=(.*)$` 在 ICU 下捕获不含 `\r`，在 fancy-regex 0.14 下含）。M3 在编译之前把前三种改写成等价的写法；CRLF 靠 fancy-regex 0.18 起的 `(?R)`（CRLF 模式）——**fancy-regex 升到 ≥ 0.18 在 M3 定**（0.19 起公开类型改成泛型，要改调用处），连同规则一侧的 `Pattern` 一起改，规则与重写不用两套读法。
- **编译不过的写法**：`\Q…\E`（M3 改写成转义的字面）、`\Z`（0.15 起有）、有界的变长后行断言（0.17 起有，要特性开关）；仍不支持的（`(?w)`、`\X`、`\N{…}` 等）这一行告警并跳过，登记。
- **每处替换都替换全部匹配**（`NSRegularExpression` 的替换接口替换全部匹配；三种重写取同一个读法）。回溯超过 fancy-regex 的上限（缺省 100 万步）时：规则一侧照阶段 1 的约定按不匹配，重写一侧跳过这条改写并在记录里留备注（让使用者看得见）。

### 3.4 jq（P4-D15）

用纯 Rust 的 **jaq**（`jaq-core` 3.1、`jaq-std` 3.0、`jaq-json` 2.0，都是 MIT，MSRV 不高于 1.70），不链接 C 的 libjq（没有维护中的绑定，`jq-rs` 停在 2019 年；要编译 C）。要在 M3 处理的几件（证书报告第 6 节）：

- **与 jq 的差异**：数字（jaq 的整数不丢精度）、对 `null` 切片、`reduce` / `foreach` 的多输出、`limit` 的负数等；jaq 的正则（`test`、`sub` 等）用的是 `regex-bites`（`\d` `\s` `\w` 只认 ASCII、没有 `\p{…}`），不是 jq 的 Oniguruma。手册唯一的 jq 例子（`test("^X-")`）不受影响。逐条比对手册示例与常见写法，差异登记。
- **线程**：`jaq_json::Val` 缺省不是 `Send`（`Rc`），`sync` 特性换成 `Arc`；编译好的过滤器是否 `Send + Sync` 要实际编译才知道。M3 的细化设计据此决定"加载时编译、各线程共享"还是"加载时只校验、用时再编译"。
- **求值不可中断，且可能递归过深**：jaq 没有时限的接口（只能在两个输出之间查时限），一个写坏的或恶意的表达式（`.sgmodule` 来自第三方）可以一直算下去；深的递归可能让栈溢出，**Rust 里栈溢出会结束整个进程**。所以 jq 不在异步的工作线程上求值，**M3 的细化设计必须给出让坏的表达式至多拖住有限的线程、不让进程崩掉的办法**（例如专用的、栈很大的少数线程，等到时限就放弃这次改写；核实 jaq 有没有递归深度的上限，没有的话考虑别的隔离），拿出办法之前 jq 一类的改写不合并。

### 3.5 解压（P4-D13）

只需要解压（改写之后不再压回，第 2.3 节 3.16；手册只说 "the body is decompressed first if necessary, and `Content-Length` is recalculated."（`http/body-rewrite.html`），没有说压不压回）。正文本来就整个缓冲着，同步地解到内存里就够，不要异步的适配层：

- `gzip`：`flate2`（已在依赖里）的 `MultiGzDecoder`（解全部的成员；`GzDecoder` 只解第一个）。
- `deflate`：RFC 9110 说是 zlib 封装，实际有服务器发裸的 deflate 流：先按 zlib 头的校验判断，不是 zlib 就按裸流解（mitmproxy 的做法；hudsucker 只认 zlib）。
- `br`：**`brotli-decompressor`**（只有解码，BSD-3-Clause / MIT，Dropbox 的 rust-brotli 一族）。
- `zstd`：**`ruzstd`**（纯 Rust，MIT，MSRV 1.87；比 C 的实现慢约 1.4 ～ 3.5 倍，正文至多 10 MB / 32 MB，可以接受）；浏览器已经发 `zstd`（Chrome 123、Firefox 126 起）。不用 C 绑定的 `zstd`（要 C 编译器）。

叠了几层的按相反的顺序逐层解，每一层都受同一个上限；有一层不认识就不改写、原样转发、留备注。

### 3.6 QUIC Initial 的解密（P4-D21）

用依赖里已有的 rustls 的 `quic` 模块（按 RFC 9001 由目的连接 ID 导出 Initial 的密钥，去掉包头保护、解密载荷），不另引入密码学的包；只认 QUIC v1 与 v2 的 Initial（同阶段 3 / M5 的识别），跨包的 CRYPTO 帧按偏移重组（第 11 节）。

### 3.7 依赖总表（P4-D24）

阶段 3 定的是"不引入新的包"；本阶段做不到：jq 与 `br`、`zstd` 的解码没有现成的包可用。原则：**新包只为做不出来或不该自己做的东西引入，许可在 NFR-11 的白名单里（MIT / Apache-2.0 / BSD / ISC / MPL-2.0 等）**，在提交说明与 `docs/status.md` 里逐个写出名字、版本、许可与传递依赖；所有者可以撤回（第 17 节 Q4）。

| 包 | 新 / 已有 | 用途 | 许可 |
| -- | --------- | ---- | ---- |
| hyper、hyper-util、h2、http、http-body-util | 已有 | HTTP 引擎 | MIT |
| rustls、tokio-rustls、rustls-native-certs | 已有 | 两侧的 TLS、系统的根 | Apache-2.0 / ISC / MIT |
| webpki-roots（锁定 1.0.9） | 已有（`rurge-net` 依赖它） | 编进二进制的 Mozilla 根，上游校验取并集（第 7.5 节） | **CDLA-Permissive-2.0，不在 NFR-11 的白名单里**；它在本阶段之前就在依赖里，本阶段多一处用法；列给所有者（第 17 节 Q4） |
| rcgen | 已有（今天只在测试里） | 叶证书、`rurge mitm ca generate` | MIT OR Apache-2.0 |
| p12-keystore | 已有，0.2.1 → 0.4.x（MSRV 1.85） | 读写 PKCS#12 | MIT OR Apache-2.0 |
| fancy-regex | 已有，0.14 → 是否升到 ≥ 0.18 在 M3 定（第 3.3 节） | 三种重写的正则 | MIT |
| flate2 | 已有 | gzip / deflate | MIT OR Apache-2.0 |
| jaq-core、jaq-std、jaq-json | 新 | jq 表达式 | MIT |
| brotli-decompressor | 新 | `br` | BSD-3-Clause / MIT |
| ruzstd | 新 | `zstd` | MIT |

新包的版本在 M3 定稿，许可与传递依赖在引入的提交里逐个写明（调研里的许可读自 crates.io 的摘要，引入时对着包里的许可文件再核一遍）。不引入 hudsucker、rama、`moka`（叶证书的缓存用锁文件里已有的 `lru`）、`har`（HAR 用 `serde_json` 自己写，几十行）、`mime_guess`（扩展名到类型的表自己写，第 8.5 节）、`async-compression`（解压是同步的，第 3.5 节）。`unsafe` 不变：`rurge-http` 是 `forbid`，本阶段不加放开的函数。

## 4. Workspace 与 crate 边界（P4-D22）

| crate | 本阶段的变化 |
| ----- | ------------ |
| `rurge-http`（新） | 平台无关，`forbid(unsafe_code)`：前缀缓冲与回放的流、ClientHello 与 HTTP 开头的判断、HTTP 引擎（每个连接一个 hyper 服务、每个请求的流水线）、上游连接池、MITM（CA、叶证书的签发与缓存、两侧的 TLS 配置）、四种重写与 Map Local、正文的缓冲与解压、抓包存储与 HAR、QUIC Initial 的解密与 SNI。依赖 `rurge-config`（各节的类型、`HostList`、`SessionInfo`）、`rurge-net`（`BoxedStream`、根证书、`quic` 的共用部分、测试服务器）、`rurge-inbound`（`SessionHandle`、`Dialer`、错误页）、`rurge-proto`（`RejectKind`、出站 TLS 的校验器、`[Keystore]` 的 p12 解码；不用其中的出站协议，拨号经 trait 由引擎做；`rurge-inbound` 本来就依赖它）；**不依赖** `rurge-engine`、`rurge-rules`、`rurge-policy`、`rurge-api`、`rurge-platform` 与可执行文件——规则、策略、拨号、记录、fake 地址的反查经它定义的 trait 由引擎实现（同 `rurge-tun` 的 `Hooks`） |
| `rurge-config` | 五个节（`[MITM]`、`[URL Rewrite]`、`[Header Rewrite]`、`[Body Rewrite]`、`[Map Local]`）类型化，从 `W0016` 的名单里去掉（M2 / M3）；一行的切法（`value.rs` 与 `redact.rs`，M3）；新的诊断码（各自在 `codes::hint` 里做决定）；`[Replica]` 认得、忽略（M3） |
| `rurge-rules` | 规则集里一行上的 `extended-matching`（清单 L314，M1）；`USER-AGENT`、`URL-REGEX` 不变（求值的对象多了引擎里的请求） |
| `rurge-inbound` | `Dialer` 加"把客户端的流整个交给引擎"的入口（第 5.3 节，有缺省实现，同 `admit_udp` / `associate` 的先例）；HTTP 监听器与 SOCKS5 监听器对可嗅探的会话先答后交；HTTP 监听器的绝对形式请求改交引擎，今天的 `forward()` 退役（M1） |
| `rurge-tun` | TCP 会话对可嗅探的会话走同一个入口（M1）；UDP 的 QUIC 交给 `auto-quic-block` 判断（M5） |
| `rurge-engine` | 实现 `rurge-http` 的 trait（每个请求的规则与策略、经策略拨号、记录、进程槽共享、fake 地址反查）；嗅探之后的分派（第 5.5 节）；`[MITM]` 的运行时状态（CA 与叶证书缓存随配置原子切换）；`plain_http` 的推断换成显式的模式（第 6.8 节）；`state.json` 第 2 版（第 10.1 节）；重载与 `network_changed` 之后池里旧的上游连接不再借出（第 6.4 节）；`block-quic=adaptive` 的表改在客户端连接交给引擎时听（第 6.4 节） |
| `rurge-proto` | `keystore.rs` 随 `p12-keystore` 升级改调用（M2）；`[Keystore]` 的 p12 解码给 MITM 的 CA 复用；`transport/tls.rs` 里的校验器（`Standard` / `Pinned` / `Insecure` 三种，今天是模块私有的）公开给 `rurge-http` 用（M2） |
| `rurge-api` | `/v1/features/{mitm,capture,rewrite}` 生效；`GET /v1/mitm/ca`；`/v1/rurge/capture/…`；请求记录的新字段；`http-api-tls` 的 TLS 接入（M2 / M4）。仍只依赖 `rurge-engine` 与 `rurge-config`：要用到的 `rurge-http` 类型经引擎转出 |
| `rurge`（bin） | `rurge mitm ca generate` / `export`；抓包容量的命令行参数与环境变量；`http-api-tls` 时 CLI 的客户端走 HTTPS 并信配置里的 CA |

需求文档 3.2 节 `rurge-http` 一行的依赖（`rurge-rules`、`rurge-policy`）与职责里的"请求记录"随本文档订正。请求记录（`RequestRecord`）照旧在 `rurge-engine` 的 `observe` 里：它是全部会话共用的，不是 HTTP 专有的；抓包存储是 HTTP 专有的，放在 `rurge-http`，引擎持有它的实例。

依赖方向：`rurge (bin) → { rurge-engine → { rurge-http → { rurge-inbound, rurge-proto, rurge-config, rurge-net }, rurge-tun, … }, rurge-api, rurge-platform }`；`rurge-http` 与 `rurge-tun` 互不依赖。阶段 5 的 `rurge-script` 依赖 `rurge-http`（需求文档 3.2 节），本阶段的流水线为它留接口位置，不定接口。

## 5. 会话入口与嗅探（M1，P4-D2、D3）

### 5.1 今天的样子

三种入口今天都是"先拨号、再转发"：HTTP 监听器的 `CONNECT` 先拨号、成功再答 200；SOCKS5 先拨号、再答成功或失败；虚拟网卡的协议栈在 SYN 时就完成应用的握手、随后拨号，失败回 RST。SNI 只在转发时从客户端的第一块数据里读（至多 8 KiB、不跨块重组），只写进请求记录，规则看不到；HTTP 监听器的绝对形式请求每个请求拨一次号、不复用上游连接、去掉 `Upgrade`。

### 5.2 可嗅探的会话

一个 TCP 会话**可嗅探**，当且仅当：

1. 不命中 `always-raw-tcp-hosts`（按目标主机与端口）与 `always-raw-tcp-keywords`（按目标主机）；并且
2. 下面三者之一：目标端口是 80 或 443；目标（主机与端口）命中 `force-http-engine-hosts`；目标端口是 `[MITM] hostname` 里某一项写明的端口（`:0` 的项让全部端口都算），并且 `mitm` 开着、有可用的 CA（M2 起）。

`force-http-engine-hosts` 管的是明文，直接拿连接的目标比：不带端口的项只对 80（"For example, for the `force-http-engine-hosts` parameter, a bare hostname is only effective for port 80."，`profile/host-list.html`），`host:0` 是这个主机的全部端口，`*:0` 是全部；它与三个开关无关（手册：被它强制进引擎的连接上 `URL-REGEX` 等 HTTP 规则生效，`rules/http.html`）。`[MITM] hostname` 只看端口不看主机：在读到 SNI 之前目标可能只是一个 IP，名单里写着主机名的项还比不了。三个开关都关着时 80 与 443 照样嗅探——规则要看 SNI 与协议（`extended-matching`、`PROTOCOL`），明文的请求照样进引擎、各自求规则。

不可嗅探的会话照今天的路走，什么也不变。

### 5.3 先答后读

可嗅探的会话由监听器先答"成功"，再把客户端的流整个交给引擎（`Dialer` 的新入口）：

- SOCKS5：成功应答，`BND.ADDR` / `BND.PORT` 写 `0.0.0.0:0`；
- `CONNECT`：200；
- 虚拟网卡：握手本来就先完成了，不变。

之后拨号失败只能关连接（虚拟网卡上回 RST，同今天）；走 HTTP 引擎的连接里，单个请求的失败给 502 / 504 错误页（第 6.7 节）。这改变了可嗅探的会话对拨号失败的表现：以前 SOCKS5 客户端拿到"连接被拒绝"一类的应答码、`CONNECT` 拿到错误页，以后拿到一个随即关闭的隧道。代价登记进清单；Surge 在 80 与 443 上嗅探，同样只能这样做（推断）。

### 5.4 读与判

引擎把客户端的流包进一个**前缀缓冲**：读进来的字节先留在缓冲里，判完之后原样交给下一步（按原样转发时先写给上游，解密时交给 rustls，进 HTTP 引擎时交给 hyper），一个字节也不丢。判断：

| 开头 | 判为 | 读到哪里为止 |
| ---- | ---- | ------------ |
| `0x16 0x03 0x00..0x04`，随后是握手类型 `0x01` | TLS：解析 ClientHello，取 SNI 与 ALPN；ClientHello 可以跨几个 TLS 记录、几次读（带后量子密钥交换的超过 1.5 KiB） | 完整的 ClientHello；超过 64 KiB 算作"别的" |
| 一个大写的方法记号（`A-Z`，可含 `-` 与 `_`，至多 20 个字符）后跟一个空格，且不是 `PRI ` | HTTP/1.x（随后由 hyper 解析；解析失败时 hyper 答 400 并关连接） | 方法记号与空格 |
| `PRI * HTTP/2.0` | 明文的 HTTP/2 先验，按原样转发（第 1.3 节） | 同上 |
| 其它 | 别的协议，按原样转发 | 第一个不符合的字节 |

**等待**：至多 15 秒等到能判（Envoy 的监听器过滤器缺省 15 秒；比 Chrome 闲置的预连接套接字的 10 秒长，那样的套接字过了 10 秒才发请求也不会被错判成"别的"），超时、客户端关闭写端都按原样转发。sing-box 的 300 毫秒更短，但预连接晚于它发请求时这个请求会绕过引擎——重写与 `URL-REGEX` 时有时无，是悄悄的错；15 秒的代价是服务器先说话的协议在这些端口上晚 15 秒才通，那是看得见的，并且 `always-raw-tcp-*` 就是为它们准备的。值在 M1 的细化设计里定稿，用例经只供测试的选项把它调短。

### 5.5 判完之后

| 判为 | 去向 |
| ---- | ---- |
| TLS，且第 7.1 节的条件都满足 | **解密**：与客户端握手，之后进 HTTP 引擎（`https`） |
| TLS，不解密 | **按原样转发**：会话信息带上 SNI 与协议 `HTTPS`，求规则、拨号、把缓冲里的字节先写给上游、再转发 |
| HTTP/1.x | **HTTP 引擎**（`http`） |
| 别的、超时 | **按原样转发**：协议是 `TCP` |

`force-http-engine-hosts` 命中的会话不再另外判断"像不像 HTTP"，只看开头是不是 TLS：不是 TLS 的一律进引擎（手册说这些连接被"forced into HTTP processing"，并且"never decrypts TLS"）；是 TLS 的照上表的前两行（只有 MITM 名单能让它解密）。

### 5.6 规则看见什么

按原样转发的会话求一次规则，此时会话信息里已经有：SNI（TLS 时）、协议（`HTTPS` / `TCP`）、目标（`CONNECT` 的主机、SOCKS5 的域名、fake 地址反查到的域名或 IP）。于是 `extended-matching` 对真实会话生效（今天在清单 L265 登记"不起作用"），`PROTOCOL,HTTPS` 在拨号之前就能命中（FR-RULE-08）。规则集文件里一行上的 `extended-matching` 一并补完：进了域名索引的 `DOMAIN` 与 `DOMAIN-SUFFIX` 行保留标志，一行带了、整个集合的域名行都带（手册："Note that if any domain rule inside the set file already carries the flag, it is applied to all domain rules of that set."，`rules/ruleset.html`）。

进 HTTP 引擎的会话不在连接一层求规则：每个请求各自求（第 6.3 节）。

预匹配（DNS 与 SYN 两层，阶段 3）不变，发生在这一切之前。出站模式（直连 / 全局 / 规则）只决定"策略怎么选"：直连与全局模式下照样嗅探、照样解密与重写，策略分别是 `DIRECT` 与全局策略。

## 6. HTTP 引擎（M1，P4-D4 ～ D6、D11、D12）

### 6.1 连接

每个进引擎的客户端连接一个 hyper 服务：明文与 ALPN 为 `http/1.1`（或没有 ALPN）的解密连接用 HTTP/1.1（带升级），ALPN 为 `h2` 的解密连接用 HTTP/2。HTTP/1.1 上的请求严格串行（hyper 的做法，管线化的请求留在缓冲里排队）；HTTP/2 上每个流是一个请求，并发上限在 M2 定（hyper 的缺省 200 偏高，调研第 1.10 节）。请求头的读取有时限（hyper 的 `header_read_timeout`，要装定时器才生效——入站已经装了，调研第 1.10 节）。

### 6.2 每个请求的流程

```
请求头读完
→ 组会话信息、求规则、选策略（第 6.3 节）      —— REJECT 一族在这里答复
→ Header Rewrite（请求）                     —— 改了框架头则失败（502）
→ URL Rewrite                               —— 302 / 307 / reject 在这里答复；header 模式改目标
→ Map Local                                 —— 命中则答复，不连上游
→ 要不要缓冲请求正文（有匹配的 Body Rewrite 规则时）
→ Body Rewrite（请求）                       —— 超过 32 MB 断开；chunked 与 Expect 的不改写
→ 〔请求脚本：阶段 5〕
→ 从连接池取上游连接，发出（第 6.4 节）
→ 响应头到达
→ Header Rewrite（响应）                     —— 改了框架头则失败（502）
→ 要不要缓冲响应正文（有匹配的 Body Rewrite 规则时）
→ Body Rewrite（响应）                       —— 超过 10 MB 原样流过
→ 〔响应脚本：阶段 5〕
→ 交给客户端；记录与抓包（第 9 节）
```

正文缺省**流式**转发（hyper 的 body 有真背压，调研第 1.8 节）；只有在**读完头的那一刻**就知道有某条 Body Rewrite 规则（阶段 5 起还有要正文的脚本）匹配这个 URL 与方向时才缓冲——缓冲与否必须在那一刻定（mitmproxy 的同一条教训，调研第 2.3 节）。`rewrite` 开关关着时，Header Rewrite、URL Rewrite、Body Rewrite 与 Map Local 一概跳过（手册："Map Local only applies to requests handled by the HTTP engine, and only when the Rewrite feature is enabled."，`http/map-local.html`）。

### 6.3 每个请求的会话信息与规则（P4-D5）

每个请求组一份会话信息，求一次规则、选一次策略：

- **目标、也就是拨号的地方，是连接的目标**（同今天：`CONNECT` 与 SOCKS5 给的主机、fake 地址反查到的域名、虚拟网卡上的真实 IP）。例外只有两个：经 HTTP 监听器来的绝对形式请求，目标是 URL 的主机与端口（客户端本来就是在对 rurge 这个代理说话，第 6.8 节）；URL Rewrite 的 `header` 模式换了目标（第 8.2 节）。不按 `Host` 改拨号的地方：按真实 IP 连接而 `Host` 写着一个名字的流量（HTTPDNS 一类、hosts 文件、`curl --resolve`）今天拨的是那个 IP，以后也是；`CONNECT x:8080` 里 `Host` 不带端口的请求照旧拨 8080。
- URL 是完整的 URL（`http://` 或 `https://` 开头，去掉 userinfo）：绝对形式取请求行，否则用 `Host` 或 `:authority` 重建，都没有时用连接的目标；`http_host`、`user_agent` 取自请求头；协议是 `HTTP` 或 `HTTPS`；SNI、来源地址、监听器、监听端口沿用连接的。`DOMAIN` 一类的规则因此照旧比连接的目标，带 `extended-matching` 的再比 SNI 与 `Host`（清单 L265 的语义不变）。
- 嗅探到的连接上（虚拟网卡、SOCKS5、`CONNECT` 的隧道里），客户端发来的绝对形式请求与 `CONNECT` 照原样转给连接的目标——那是客户端在对它自己的代理说话；`CONNECT` 得到 2xx 之后两侧透传字节（同第 6.6 节的升级）。
- **进程**：同一个连接上的请求共用连接的进程槽，不各自查找（CLAUDE.md「网络与进程」）。
- **规则的开销**：每个请求一次求值。规则集里 `USER-AGENT`、`URL-REGEX` 等行是逐条求值的（阶段 1 / M5a 计划延后第 26 行，去向"有用户报告再说"；与手册 "All other rule types are evaluated linearly." 一致），以前只有 HTTP 监听器转发的明文请求会走到它们，以后经引擎的每个请求都走到。M5 量一次引擎里逐请求求值的开销（阶段 1 的基准见清单 L312 指到的记录）。
- **fake 地址**：经 HTTP 监听器的绝对形式请求里，主机是 fake 地址时先反查回域名（R27，第 6.8 节）；别的入口上连接的目标在引擎接手之前就已经反查过（阶段 3）。

### 6.4 上游连接池（P4-D6）

自己写，放在 `rurge-http`。理由：hyper-util 的 `legacy::Client` 的池键只有 scheme 与 authority（调研第 6.1 节【源码】），而 rurge 的上游连接属于一条出站链，同一个主机走两个策略的请求不能共用连接；它还要等连上才知道是 HTTP/1.1 还是 HTTP/2（hudsucker #142 的根源）。

- **池属于一条客户端连接**：上游的连接只借给同一条客户端连接上的请求，随它关闭（mitmproxy 也是这样）。不跨客户端连接共享，理由：按连接认证的方案（NTLM、Negotiate）会串到别的客户端；阶段 7 的网关模式下会跨设备；生命周期简单，不要全局的空闲表。代价是不同的客户端连接各自连上游，与不经 rurge 时一样多；M5 的基准量一次，需要时再议。
- **键**：出站链的标识（解析到底的策略链，连同配置的代次）、scheme、主机、端口、TLS 的参数（SNI、是否校验、提供的 ALPN）、转发的方式（直接对源站说 HTTP，还是以绝对形式交给 HTTP 代理，第 6.8 节）。
- HTTP/1.1 的连接一次只借给一个请求；请求或应答的正文没读完、或任一侧带 `Connection: close` 的连接不放回。HTTP/2 的连接共享，按 `is_ready` / `is_closed` 与对端的并发上限取用，收到 GOAWAY 的不再分配新的流。
- 每条客户端连接至多留几条空闲的上游连接（M1 定）。配置重载与出口表变化（`Engine::network_changed`）之后，之前建的连接不再借出——引擎里的每个请求是一个会话，池里的连接活得比它长，CLAUDE.md「出站套接字只有一处建法」要求这样的套接字在出口表变了之后重建。
- **两步取连接，记账照旧**：都经引擎实现的 trait——先"为这个请求选路"（规则与策略；`REJECT` 一族在这一步答复），池里没有可借的再"按这条路拨号"。今天拨号时才做的记账（把记录标成已连上、smart 组的 `book.used` 与 `smart::watch`、`Trail` 记下的对端地址与 DNS 耗时）照旧只在真拨号时、在请求自己的任务里做（`Trail` 的作用域是任务局部的，交给别的任务就丢）；借到池里的连接时，引擎把这个请求的记录标成已连上、写上那条连接拨号时记下的远端地址（第 9.1 节）。smart 组对借来的连接上的请求怎样记，M1 的细化设计读 `smart` 的代码定。`block-quic=adaptive` 的表听的是客户端的 TCP 会话（阶段 3 / M5），照旧在客户端连接进来时告诉它一次，不按请求。
- 上游的 TCP 连接开 `TCP_NODELAY`（rama #1224 的教训：没开时 16 KiB 以上的应答在延迟确认上卡约 40 毫秒）。建连照旧经引擎与 `rurge-net` 的统一建法（CLAUDE.md「出站套接字只有一处建法」），池不自己建套接字。
- 非幂等的请求（`POST` 等）不在连接失效之后自动重发；幂等的请求在"借到的空闲连接一写就断"时重发一次（hyper-util 的 `retry_canceled_requests` 的语义）。

### 6.5 头的处理

- 逐跳的头照 RFC 9110 第 7.6.1 节去掉（`Connection` 与它列出的名字、`Keep-Alive`、`Proxy-Connection`、`TE`（除了 `trailers`）、`Transfer-Encoding`、`Upgrade`（升级的请求除外）），hyper 在 HTTP/1.1 一侧不替代理做这件事（调研第 1.9 节）。`Proxy-Authorization`：经 HTTP 监听器的请求上是给 rurge 的，监听器消费、不往上游转；嗅探到的连接上是给客户端自己的那个代理的，原样转发（第 6.3 节）。
- 不加 `Via`、`X-Forwarded-For`（Surge 的手册没有写它加）。
- HTTP/2 与 HTTP/1.1 之间：`:authority` 与 `Host` 互补、HTTP/2 的多个 `cookie` 合成一个、请求目标的形式（源站形式 / 绝对形式）由引擎定（调研第 1.11 节）。
- 头名的拼写：HTTP/1.1 到 HTTP/1.1 时两侧都开 `preserve_header_case`、改写时把消息的扩展整体带过去，原拼写能原样转出；新加的头一律小写。记录与抓包里的头名是小写（读不到原拼写，第 6.9 节）。

### 6.6 WebSocket（P4-D12）

带 `Upgrade` 的请求照普通请求走完流水线（规则、Header Rewrite、URL Rewrite 都管它），上游答 101 之后，两侧各取升级之后的流（hyper 的 `upgrade::on`，必须在拆开请求之前取；升级之后已经读进缓冲的字节要一起交出，调研第 1.5 节），之后按字节双向转发，计入这个请求的记录。不宣告 HTTP/2 的扩展 `CONNECT`（RFC 8441）：上游不支持时浏览器不会回落（rama #932 的教训），不宣告时浏览器为 WebSocket 另开一条 HTTP/1.1 的连接（推断）。

### 6.7 错误与时限

- 拨号失败、上游连不上：502；上游超时：504；都带错误页（`show-error-page` 开着时，阶段 1 的错误页），记录写原因。
- `REJECT-DROP`：要停到时限或客户端先关为止。阶段 1 / M3a 记下的是"hyper 服务内观察不到"客户端的关闭；引擎的连接是 rurge 自己交给 hyper 的，M1 的细化设计找办法（例如由包着客户端流的那一层报告读到 EOF），找不到就照旧固定停到时限，照实登记。
- 客户端一侧 keep-alive 的空闲时限、等上游应答头的时限在 M1 定，与今天的转发的空闲时限对齐。

### 6.8 经 HTTP 监听器来的绝对形式请求

今天 HTTP 监听器自己处理（`forward()`：每个请求拨一次号，策略是 HTTP 代理时以绝对形式转给它，`Dialed.forward` 带上要加的头）。以后监听器把每个请求交给引擎，走第 6.2 节的同一条流程，监听器只剩下鉴权、解析代理请求与 `CONNECT`。

- `SessionInfo` 上"`listener == Http` 且 `url` 有值就是监听器在转发明文请求"的约定（`crates/rurge-config/src/session.rs` 第 74 ～ 79 行的注释）在本阶段作废：引擎里每个请求都有 URL，"以绝对形式交给 HTTP 代理"改成引擎显式给出的转发方式（池键的一项），只对 `http://` 的请求、策略是 HTTP 代理时用。
- 请求行与 `Host` 里的主机是 fake 地址时，换回它对应的域名再求规则与转发（R27；阶段 3 / M4 设计 M4-D24 把这件事留给本阶段）。

### 6.9 hyper 的限制（登记清单 7.1 节）

M1 的文档任务把下面各条登记为与 Surge 可能不同的地方（Surge 怎么做不知道，没有观察过）：头的顺序（同名的值聚在一起，名字按第一次出现的顺序）；记录与抓包里头名的原拼写；1xx 不转发（101 除外；`100 Continue` 由 rurge 自己答）；请求里的折行头、非 UTF-8 的请求目标答 400；非 ASCII 的原因短语转发时变空；trailers 只在声明过时转发；请求被短路（Map Local、拒绝）而客户端还在上传时连接会被关（调研第 1.8 节）。

## 7. MITM（M2，P4-D7 ～ D11、D20）

### 7.1 何时解密

一个判为 TLS 的会话，下面几条都成立才解密：

1. `mitm` 开关开着；
2. `[MITM]` 有可用的 CA（第 7.2 节）；
3. `client-source-address` 允许这个来源（第 2.3 节 3.9；不写等于全部）；
4. 生效的 `hostname` 名单（`hostname` 去掉 `hostname-disabled` 里的项）命中：拿 SNI 比（没有 SNI 时拿连接的目标，第 2.3 节 3.8）；名单按顺序匹配，第一项决定，`-` 开头的项排除；**不带端口的项在有 SNI 时不看端口**，没有 SNI 时只对 443；
5. 客户端的 ALPN 没有、或含 `http/1.1`、或含 `h2` 且 `h2 = true`——否则 rurge 给不出客户端能用的协议，按原样转发（DEBUG 一行）。

不解密的 TLS 会话按原样转发（第 5.5 节）。

### 7.2 CA（P4-D9）

- 来源：`ca-keystore-name` 指向的 `[Keystore]` p12 条目优先，否则 `ca-p12`（Base64 的 PKCS#12）与 `ca-passphrase`；解码复用 `rurge-proto` 的 `[Keystore]` 解码（升级后的 `p12-keystore`）。条目变了的重载换 CA，已缓存的叶证书作废。
- 出问题——解不开、口令不对、算法不认识、里面没有私钥、证书不是 CA、密钥类型 rcgen 签不了——时：一条告警（写明是哪一种），**MITM 不生效**，配置的其余部分照常（同手册对非法名单项的态度"produces a profile warning instead of a parse error"）。
- `p12-keystore` 0.2.1 读不了的写法（证书报告第 3.3、3.4 节）：没有 `localKeyID` 的私钥、明文的 `keyBag`（0.3.0 起能读）；SHA-1 与 SHA-256 以外的 MAC（0.3.2 起能读）；PBES2 配 HMAC-SHA1 的伪随机函数——这一种卡在底层的 `pkcs5` 没有打开 `sha1-insecure` 特性，OpenSSL 1.1 用 `-keypbe AES-256-CBC` 写出的文件会落在这里（推断）。M2 升级之后逐种重核，能靠打开底层包的特性补上的补，仍读不了的登记。
- CA 证书的 subject 里有多值的 RDN 或重复的属性类型时，rcgen 重建出的签发者名字与 CA 证书的 subject 不再逐字节相同，链会断（证书报告第 2.6 节【源码】）：这样的 CA 当作"出问题"，告警里写明原因。

### 7.3 叶证书（P4-D8）

| 项 | 取值 | 理由 |
| -- | ---- | ---- |
| 密钥 | **每张叶证书各自新生成一把 ECDSA P-256** | 生成便宜（ring）；不复用 CA 的密钥，就不会踩"同一签发者同一序列号"（Firefox 的 `SEC_ERROR_REUSED_ISSUER_AND_SERIAL`）与"叶与 CA 共用 SKI"（Windows Schannel）两个坑；rcgen 缺省的序列号由公钥算出，复用密钥时全部相同（证书报告第 1.4、2.5 节） |
| 序列号 | 16 个随机字节，最高位清零 | 同上；RFC 5280 的 20 字节上限 |
| 名字 | SAN：主机名是 DNS 名，IP 是 IP 地址；CN 同一个名字（64 字符以内时） | 各家客户端只看 SAN；按 IP 访问的站点要 IP 的 SAN（hudsucker #125 的教训） |
| 扩展 | EKU serverAuth；KeyUsage digitalSignature；AKI 取自 CA 的 SKI；叶不写 SKI；不写 BasicConstraints | 同 mitmproxy 与 rcgen 的缺省（叶不写 SKI 是为 Schannel） |
| 有效期 | `notBefore` 回拨 2 天（时钟不准的设备）；总长 47 天，且不晚于 CA 的 `notAfter` | Apple 对用户的根也卡 825 天；Chromium 的 398 / 200 / 100 / 47 天只管"已知的根"，但 Android 上有被当成已知根的报告（证书报告第 1.1 节）；叶证书每次运行现签、在内存里续期，短一些没有代价，47 天是已公布的最严的一档 |
| 链 | 叶 + CA | 客户端只装了根时也能建链 |
| 缓存 | 按名字（不含端口）缓存，容量 1000 条（M2 定），剩余寿命不足三分之一时重签；重载换了 CA 时清空 | 不按端口分：同一主机不同端口的证书一样 |

TLS 会话的恢复：各个解密连接的服务端配置共享同一份会话缓存（rustls 缺省每个配置 256 个），恢复要求 SNI 相同（证书报告第 4.1 节）。

### 7.4 与客户端的握手（P4-D7、D11）

**先与客户端握手、再按请求连上游**（不先连上游、不照抄上游证书与 ALPN）：

- 理由：每个请求各自选策略（第 6.3 节），握手时还不知道请求走哪条出站；先连上游（mitmproxy 的缺省 `eager`）要先选一次连接一层的策略，第一个请求再按自己的规则可能换到另一条出站，等于白连一次。代价：叶证书只写 SNI 一个名字（不照抄上游证书的 SAN——反正只写一个名字也不会让浏览器在 HTTP/2 上合并连接，证书报告第 4.1 节）；上游的证书错误以请求失败的样子出现（第 2.3 节 3.7）。
- ALPN：`h2 = true` 时服务端列表是 `h2`、`http/1.1`，否则只有 `http/1.1`；客户端没给 ALPN 时不选（按 HTTP/1.1）。客户端给的 ALPN 里没有这两个时不解密（第 7.1 节第 5 条）。
- 握手失败：客户端发来证书类的警报（`unknown_ca`、`bad_certificate`、`certificate_unknown`）时 WARN 一行"客户端不信任 rurge 为这个主机签的证书"并提示检查 CA 是否装好、是否信任（这几种警报服务端分不清"没装 CA"与"证书固定"，证书报告第 4.3 节）；握手中途断开 INFO 一行。都写进这条连接的记录（第 9.1 节）。
- **证书固定的提示**：握手完成、一个请求也没发就关闭（读到 `close_notify` 或直接断开）时，WARN 一行"`<主机>` 很可能用了证书固定；不想解密它就在 `hostname` 里写 `-<主机>`"——照手册"If a client completes the TLS handshake but disconnects without sending a request, Surge logs a hint that the host is likely protected by certificate pinning."（`http/mitm.html`）。不自动把它排除。

### 7.5 与上游（P4-D10）

- 每个请求经连接池取连接（第 6.4 节）；对上游的 SNI 是客户端给的 SNI，客户端没给时是连接的目标（是 IP 时不发 SNI），URL Rewrite 的 `header` 模式换了目标时是新的主机；上游的证书按这个名字校验。ALPN 照第 7.4 节的同一张列表，两侧各自协商、版本不同由引擎转换。
- 校验：缺省按证书链与名字校验，根是**系统的根 ∪ `webpki-roots`**。今天的 `rurge_net::tls::root_store()` 只在读到 0 个系统根时才补 `webpki-roots`；Windows 的根存储是按需填充的，直接读存储会漏掉没被用过的公共根（证书报告第 4.2 节），浏览器在那台机器上能连的站点经 rurge 解密之后会因"不认识的签发者"失败。所以解密的上游取并集，阶段 2 的出站 TLS 不动（要不要换成调用系统校验接口的 `rustls-platform-verifier`，归所有者，第 17 节 Q4）。
- `skip-server-cert-verify = true` 时不校验链与名字，握手里的签名照常校验（`rurge-proto` 已有的 `Verifier` 的 `Insecure` 模式）。
- 上游要客户端证书时：`[MITM]` 没有客户端证书的键（清单 L588），握手失败、请求失败，记录写原因；要靠 `-<主机>` 排除。

### 7.6 `rurge mitm ca generate` / `export`（P4-D9）

- `rurge mitm ca generate --out <目录>`：生成 ECDSA P-256 的 CA（rcgen 的 ring 后端生成不了 RSA 密钥），有效期 10 年，`BasicConstraints CA:TRUE, pathLen 0`、`KeyUsage keyCertSign, cRLSign`、带 SKI，CN 写明 rurge 与生成的日期。写出三个文件：CA 证书（PEM 与 DER，公开）、PKCS#12（私钥，**只属主可读**，NFR-03）、一段配置（`[MITM]` 的 `ca-p12` 与 `ca-passphrase` 两行，含私钥，只属主可读）。"只属主可读"在 Unix 上是 `0600`；Windows 上没有权限位，文件随所在目录的权限（同 fake-IP 映射文件的做法，`crates/rurge-dns/src/fakeip/file.rs`），输出里提醒把目录设成只有自己能读，要不要另设 ACL 由 M2 的细化设计定。口令随机生成（Surge 要求非空）。**不往标准输出打印私钥**（凭据的规矩）；打印的是三个文件的路径与各平台的安装步骤（Windows `certutil -user -addstore Root`、macOS `security add-trusted-cert`、Linux 各发行版的系统库与 NSS 库、Firefox、Android 与 iOS 的说明），**rurge 自己不装**（不改系统设置）。
- p12 的算法：**3DES 加密、HMAC-SHA1 的 MAC**——macOS 15 之前的钥匙串（因而那些系统上的 Surge，推断）只认这一族，OpenSSL 3 读它不需要 legacy（证书报告第 3.2、3.6 节；都没有对真实文件试过，M2 用例里用 rurge 自己读回一次）。
- `rurge mitm ca export -c <配置>`：从配置里读出 CA 证书（不含私钥）写成 PEM 或 DER，同 `GET /v1/mitm/ca`。
- 写目录之前检查目标文件不存在（不覆盖），不存在的目录不自动建。

### 7.7 `GET /v1/mitm/ca`

DER 编码的 CA 证书，不含私钥（手册）；没有可用的 CA 时 404。密钥可以放在查询参数里（`?x-key=`，阶段 1 起就认），浏览器直接下载。

### 7.8 `http-api-tls`（P4-D20）

API 的监听器外面套一层 TLS：每条连接按 SNI（没有时按这条连接进来的本地地址）用 MITM 的 CA 现签一张叶证书（同第 7.3 节，同一个缓存）——照手册"Surge will use the CA certificate of MITM to generate the server certificate for the corresponding access address."（`tools/http-api.html`）。没有可用的 CA 时 API 不启动（第 2.3 节 3.22）。`rurge status` / `reload` / `stop` 等命令看到 `http-api-tls = true` 时走 HTTPS，信配置里的 CA（命令行本来就读同一份配置）。

## 8. 重写与 Map Local（M3，P4-D13 ～ D16）

### 8.1 共同的

- 四个节与 `[Map Local]` 在加载时类型化：正则在加载时编译，编译不过的行告警并跳过（不是加载错误）；方向、动作、类型不认识的行同样告警并跳过。一行的切法见第 2.3 节 3.25。
- 匹配的对象是请求的完整 URL（第 6.3 节）；URL Rewrite 另外试 Host 与连接主机名重建的两种 URL（手册"Besides the request-line URL, Surge also tries matching with the URL rebuilt from the `Host` header value and from the underlying connection's hostname."，`http/url-rewrite.html`）。
- 重写可以串联，按配置里的顺序生效；URL Rewrite 的 `header` 模式只用第一条匹配的（手册）。每个起了作用的改写在这个请求的记录里留一条备注（"注明重写"，手册"the request detail view shows the notes for applied rewrites and scripts"），跳过的（不是 UTF-8、不是 JSON、超过上限）同样留备注。
- 下载到的配置文本里的重写规则照常生效（它们不读本机文件）；只有 Map Local 的文件路径要先问 `Reach`（第 8.5 节）。

### 8.2 URL Rewrite

`header`：把匹配的部分替换成模板（`$1` 等），得到的 URL 必须是合法的 `http` / `https` / `ws` / `wss` URL，否则丢弃这条改写；改写之后 `Host` 随新 URL 改，目标换成新 URL 的主机与端口——**策略不重选**（"After a policy is selected…"），新目标经已经选定的策略连接。`302` / `307`：答复重定向，`Location` 是替换之后的 URL，`{{{GATEWAY_ADDRESS}}}` 换成当前出口的默认网关（取自网络环境的快照，阶段 3 / M1）。`reject`：第 2.3 节 3.13。

### 8.3 Header Rewrite

`http-request` / `http-response`（方向可以省，省了是请求），四种动作照手册与第 2.3 节 3.15。改写在拿到头之后、决定缓冲正文之前做，所以 Body Rewrite 看到的是改过的头。

### 8.4 Body Rewrite（P4-D13）

- **缓冲**：读完头时有匹配的规则才缓冲（第 6.2 节）。请求正文至多 32 MB，超过就**断开这条连接**（手册）；响应正文至多 10 MB，超过就**放弃修改、把已经缓冲的与其余的原样流过**（手册的 macOS 数）。上限按线上的字节算；解压之后的大小同样以这两个数为上限，超过时同样放弃修改（防解压炸弹）。
- 不改写、原样转发并告警的：请求带 `Transfer-Encoding: chunked` 或 `Expect: 100-continue`（手册）；HTTP/2 上没有 `content-length` 的请求正文（与 chunked 同类：长度事先不知道）。
- **解压**：按 `Content-Encoding` 解开 `gzip`、`deflate`、`br`、`zstd` 再改写；改过之后去掉 `Content-Encoding`、重算 `Content-Length`，不压回（第 2.3 节 3.16）。没有改动时原样转出原来的字节（连同压缩）。
- **正则**：正文必须是合法的 UTF-8，否则跳过这条规则并留备注（手册）；多行模式（手册："The `^` and `$` anchors match at the beginning and end of each line of the body."），行尾按 CRLF 读（第 3.3 节）；替换全部匹配，模板照第 3.3 节展开；同一个请求命中的几条依次作用。URL Rewrite 与 Header Rewrite 的正则不开多行（URL 与头值都是一行）。
- **jq**：正文必须是合法的 JSON，否则跳过并留备注；表达式在加载时编译，编译不过的告警并跳过（手册"An invalid jq expression produces a warning and the rule is skipped."）；输出为空时保留原文（手册）；多个输出值第 2.3 节 3.16；输出是紧凑的 JSON（不缩进）。求值的隔离与时限照第 3.4 节，超时或出错时跳过并留备注。
- 改写之后的正文为空也照样发出（`Content-Length: 0`）。

### 8.5 Map Local（P4-D16）

- 位置：第 2.3 节 3.1；要 `rewrite` 开关开着。
- `data-type`：`file`（缺省）、`text`、`tiny-gif`、`base64`；`data` 照手册。`file` 的相对路径相对配置文件所在的目录，绝对路径在三个平台都接受（手册只说 macOS 能用绝对路径，Surge 的另外两个平台是 iOS 与 tvOS；登记）；路径来自下载到的文本（托管配置的副本、远程 include 的文本）时**先问 `Reach`**，不许读本机文件的就告警并跳过这一行（CLAUDE.md「下载到的文本」）。`data` 是 URL 时经资源管理器下载与缓存（新的一类外部资源，FR-CFG-14）。
- `header`：`a:b|c:d` 与 Base64 两种（"If the value contains no `:`, Surge treats it as base64."）；`status-code` 200 ～ 999，以外的告警并跳过这一行。
- `Content-Type`：`file` 按扩展名查一张自带的表（json、html、js、css、png、jpg、gif、webp、svg、txt、xml、mp4 等常见的几十种），查不到是 `application/octet-stream`（手册）；其余三种照手册。
- 文件在请求到达时读（文件改了不必重载），读不到时 502 并写进记录。

## 9. 请求记录与抓包（M1、M4，P4-D17、D18）

### 9.1 请求记录（P4-D17）

- **一个 HTTP 请求一条记录**（手册"appear as individual HTTP requests"）：加上方法、URL（去掉 userinfo）、状态码、请求与应答的正文字节数；`remoteAddr` 是这个请求用的上游连接的远端地址（池里借来的连接同样有，M1 就做，解决阶段 1 / M5a 延后第 3 行在 HTTP 上的那一半）；耗时分出取连接、等应答头、收正文三段（FR-HTTP-10 的"各阶段耗时"）；备注里写改写、Map Local、放过的原因（FR-HTTP-10 的"备注"）。
- **承载请求的连接**（解密的连接、进引擎的明文连接）本身**只在一个请求也没有走完时**有一条记录：握手失败、握手之后不发请求（证书固定的提示也写在这里）、读不出请求、超时。它的字节数不另计（各请求的记录里有正文的字节数；握手与头的开销不计入任何一条，登记）。
- 一个请求的记录用它自己的 `SessionHandle`，进程槽与连接共享（第 6.3 节）；"终止活动请求"对单个请求的记录有效：终止的是这个请求（HTTP/2 上是重置这个流，HTTP/1.1 上是关掉这条连接）。
- 其余 FR-HTTP-10 的字段（来源的设备）属阶段 7；记录的容量照旧是 `--request-log-size`（缺省 1000）。HTTP 的记录比连接多得多，容量够不够在 M4 量一次。

### 9.2 抓包（P4-D18）

- **存什么**：开着抓包（`capture` 开关，或这个请求命中的规则带 `always-capture`）时，每个经引擎的请求存请求行、请求头、请求正文、状态行、应答头、应答正文、各段耗时与记录的 ID；WebSocket 只存升级的请求与应答（第 1.3 节）。正文存线上的字节（压缩着的照原样），读出时可以要求解压。
- **上限**：总字节数与单条正文的上限，都是 rurge 专有的运行时选项，只走命令行参数与环境变量（FR-CFG-17；名字在 M4 定，照 `--request-log-size` / `RURGE_REQUEST_LOG_SIZE` 的写法）；缺省值在 M4 量过内存之后定。单条正文超过上限时只存开头一段并标"截断"；总量超出时丢掉最早的条目。存储只在内存里（进程退出就没了）。
- **命名会话**：`always-capture=<名字>` 抓到的归进那个名字的会话；开关抓到的归进缺省会话。一个请求两者都满足时归进规则给的名字。开关关掉之后已经抓到的保留，直到被挤掉、被清除或进程退出。
- **API**（rurge 专有，`/v1/rurge/capture/…`；形状在 M4 定，写进 `docs/api/phase4.md`）：列出会话、列出某个会话的条目（摘要）、读一条（头与 Base64 的正文）、导出某个会话为 HAR 1.2、清除。
- **HAR**：用 `serde_json` 照 HAR 1.2 的字段写（调研第 5.1 节）：拿不到的耗时写 `-1`；`headersSize` 拿不到原始字节（第 6.9 节），写 `-1`；二进制正文 Base64、`encoding` 写 `base64`。
- **凭据**：这是用户流量的内容，不是 rurge 持有的凭据；但抓包经 API 输出，与 CLAUDE.md「凭据」一条怎样相处归所有者（第 17 节 Q1）。**在所有者回答之前**：`Authorization`、`Proxy-Authorization`、`Cookie`、`Set-Cookie` 四个头的值在 API 输出与 HAR 里遮住（写 `***`），存储里保留原值；正文照存照给（抓包的意义在这里）。

## 10. 功能开关、API 与命令行

### 10.1 功能开关（P4-D19）

- `mitm`、`rewrite`、`capture`：`GET` 读真实状态、`POST` 切换，切换立即对新的连接与请求生效（已经解密的连接不因为关掉 `mitm` 而中断，此后的新连接不解密）。`scripting` 照旧读作关、`POST` 答 501，到阶段 5。
- **持久化**：写进 `state.json`，下次启动照用（FR-HTTP-13）。`state.json` 今天的 `features` 里已经有这四个布尔字段，阶段 1 ～ 3 的代码从来不能把它们设成真，旧文件里的 `false` 不是使用者的选择——所以 **`state.json` 升到第 2 版**，这四个字段改成"没设过 / 开 / 关"三种，读第 1 版的文件时把它们当作没设过。
- **缺省**（没设过时）：`mitm` 开、`rewrite` 开、`capture` 关。手册没有写缺省；理由：配置里写了 `[MITM]` 与重写节就是要它们生效，抓包要占内存、会存正文，要使用者自己打开。

### 10.2 API

| 端点 | 本阶段 |
| ---- | ------ |
| `GET` / `POST /v1/features/{mitm,rewrite,capture}` | 生效（第 10.1 节） |
| `GET /v1/mitm/ca` | 新（第 7.7 节） |
| `GET /v1/requests/recent`、`active`，`POST /v1/requests/kill` | 记录多了 HTTP 的字段（第 9.1 节） |
| `/v1/rurge/capture/…` | 新（第 9.2 节） |

形状写进新建的 `docs/api/phase4.md`；日志行在哪一级说什么也写在那里（同 `docs/api/phase3.md`）。

### 10.3 命令行

`rurge mitm ca generate` / `export`（第 7.6 节）；抓包上限的两个参数（第 9.2 节）；`rurge check` 给出新节的诊断。`rurge rule match` 已有的 `--url`、`--user-agent`、`--sni`、`--http-host`、`--protocol` 照旧模拟一个请求（引擎里的每个请求就是这样一次求值）。

## 11. `auto-quic-block`（M5，P4-D21）

- 在 rurge 看得到 QUIC 包的地方（虚拟网卡的 UDP、SOCKS5 的 UDP 关联）：一条 UDP 流的第一批包里认出 QUIC v1 / v2 的 Initial（阶段 3 / M5 已经在所有端口上认），由目的连接 ID 导出密钥、去掉包头保护、解密载荷，从 CRYPTO 帧里按偏移重组 ClientHello（可以跨几个包，至多若干个包、若干毫秒与 64 KiB，M5 定），取出 SNI。
- **SNI 在规则之前写进会话信息**（阶段 3 / M5 计划延后第 6 行的"按 SNI 匹配规则"）：同 TCP 上的 TLS（第 5.6 节），`extended-matching` 对 QUIC 的流同样生效。为此这条流的规则求值要等到 ClientHello 凑齐或放弃为止；等待的上限与凑不齐时按没有 SNI 求值，M5 定。
- 第 7.1 节的条件（`mitm` 开、有可用的 CA、来源允许、名单按 SNI 命中——不带端口的项不看端口）都成立且 `auto-quic-block` 为真（缺省）时阻断这条流（丢包，同 `block-quic` 的阻断），客户端回落到 TCP（手册）。
- 解不开、凑不齐 ClientHello 时不阻断。不认的版本（gQUIC、草案 28 以前）照阶段 3 / M5 的决定不认（所有者的事项，第 17 节）。

## 12. 配置的解析、诊断与脱敏（P4-D23）

- `[MITM]`（M2）与四个重写节、`[Map Local]`（M3）从"解析了但不生效"的名单（`W0016`，`crates/rurge-config/src/deferred.rs`）里拿掉，类型化；语料库快照 `corpus__corpus__kitchen-sink.snap` 里那一行随之改。
- 新的诊断（编号在细化设计里定，今天下一个空着的是 `E0025`、`W0039`、`I0005`）：每个码在 `codes::hint` 的表里做一个决定（CLAUDE.md「诊断」）；"不适用 / 没有效果"一类的告警先问 `ParamReader::has_unrejected`。
- 凭据：`ca-p12`、`ca-passphrase` 已在 `SECRET_KEYS` 里，`[Keystore]` 的 p12 条目已经藏；本阶段没有新的带凭据的键。重写节与 `[Map Local]` 的行在脱敏视图里照写的显示——这是阶段 1 的裁定（`docs/api/phase1.md`），头值里可能有凭据，要不要藏归所有者（M8c 计划延后第 43、51、72 行，第 17 节 Q2）。M3 改一行的切法时 `value.rs` 与 `redact.rs` 同一个提交改。
- `[MITM] # 注释` 这样的节头怎么读归所有者（M8c 计划延后第 52 行，第 17 节 Q3）；本阶段不改节头的解析。

## 13. 错误处理与可观测性（P4-D25）

- 日志与错误以英文为主（NFR-10）。每个请求一行会话日志，级别同今天的会话（`crates/rurge-engine/src/engine.rs` 的 `log_session`：完成的在 DEBUG，被拒与失败的在 INFO），字段同今天的会话日志行再加方法；路径（`scheme://主机[:端口]/路径`）只在 DEBUG 的行里，**不写查询串、不写 userinfo**（查询串里常有令牌）；请求头与正文的内容在任何级别都不进日志。承载请求的连接自己的那一行照第 9.1 节：只在一个请求也没有走完时有。
- MITM 的握手失败与证书固定的提示照第 7.4 节的级别；CA 出问题在加载时告警、运行中不再每个连接重复。
- 进程与网络的信息照旧只在 DEBUG（CLAUDE.md「日志的分级」）。
- 计数（在 `GET /v1/traffic` 或新端点里，M1 定）：嗅探的结局（TLS / HTTP / 别的 / 超时）、解密的连接、握手失败、请求、缓冲放弃（超限、不是 UTF-8、不是 JSON）。

## 14. 测试策略（P4-D26）

**沿用的规矩**（CLAUDE.md「测试约定」）：只用回环与端口 0；失败时也不离开回环与临时目录；有界等待；起 `rurge` 的用例经 `rurge_command()` / `rurge_process()` 并给 `--data-dir`；缺省的 `cargo test` 不改系统设置——**测试从不把任何 CA 装进系统或浏览器的信任库**：测试客户端是 rustls 的客户端，只信测试自己生成的 CA。

1. **单元**（`rurge-http`）：前缀缓冲的回放；ClientHello 的跨记录、跨读重组（用 rustls 客户端产生真实的 ClientHello，切成任意块喂进去）；HTTP 开头的判断；名单与条件（第 7.1 节）；叶证书的字段（用 `x509-parser` 读回）；每种重写的语义；正文的上限与解压（含解压炸弹）；jq 与正则；HAR 的字段。
2. **端到端**（引擎）：回环上的明文与 TLS 测试服务器（`rurge_net::testing::TestServer` 与它的 TLS 版、`rurge_proto::testing::TlsFixture`）；客户端经 HTTP 监听器的绝对形式与 `CONNECT`、SOCKS5、内存设备上的虚拟网卡（阶段 3 的夹具）进来；断言每个请求的记录、规则、连接复用（服务器数连接）、改写的结果、错误页。HTTP/2 两侧的四种组合；WebSocket 的回显；`Expect: 100-continue`；32 MB 与 10 MB 两个边界（正文用生成的字节，时间与内存有界）。
3. **手册示例的黄金用例**：四个重写页与 Map Local 页上的每个示例一条（验收第 1 条）；示例里的主机名经一行 `[Host]` 钉到 `127.0.0.1`，另加一条按地址的规则作第二道防线（CLAUDE.md「测试约定」）。
4. **嗅探的等待**：15 秒的时限经只供测试的选项调短；"什么都没发生"的等待照规矩写注释。
5. **`auto-quic-block`**：RFC 9001 附录 A 的样例包（解密的正确性）、自己用 rustls 的 QUIC 接口构造的跨两个包的 ClientHello；只把包喂给判断函数与内存设备，不收发真实的 QUIC。
6. **CA 工具**：`rurge mitm ca generate --out <临时目录>`，读回 p12（3DES / SHA1）、用它签一张叶证书并完成一次回环上的握手；文件权限在 Unix 上断言是 `0600`（Windows 上照第 7.6 节）。
7. **基准**（忽略的用例，不作门禁）：经引擎的明文 HTTP/1.1 吞吐与每请求延迟、解密之后的同样两项、1,000 条规则下每请求求值的开销、叶证书签发的耗时（M5）。

每个任务结束的门禁不变：`cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace`。

## 15. 阶段 4 验收标准（对应需求文档第 7 节）

1. **手册中每个重写示例有对应测试**——第 14 节第 3 条的黄金用例全部通过；用例的名单与手册示例逐个对应，写进验收文档。
2. **主流站点 MITM 解密成功且证书固定站点给出提示**——自动的部分：回环上的 TLS 服务器经三种入口被解密、HTTP/2 与 HTTP/1.1 两侧、握手之后不发请求的客户端触发提示；"主流站点"只能在手工验收里做（三个平台的真实浏览器与命令行工具、真实的站点，`docs/acceptance/phase4-manual.md`），没有做的照实写。
3. **Body Rewrite 与 jq 用例通过**——第 14 节第 1 ～ 3 条里 Body Rewrite 与 jq 的用例。
4. **大小限制行为与手册一致**——32 MB 断开、10 MB 原样流过、chunked 与 `Expect` 不改写，三条边界用例。
5. **抓包可通过 API 读取**——经 `/v1/rurge/capture/…` 读出头与正文、HAR 能被解析。
6. 阶段 4 的需求逐句对照代码（照阶段 1、3 的做法），几个数照实写。
7. 三平台 CI 绿；没有证据的照实写，不写成通过。

## 16. 风险

| 编号 | 风险 | 影响 | 应对 |
| ---- | ---- | ---- | ---- |
| A | 先答后读改变了可嗅探的会话对拨号失败的表现 | SOCKS5 与 `CONNECT` 的客户端看不到失败的原因，只看到隧道随即关闭 | 只对可嗅探的会话这样做；引擎里的请求另有错误页；登记 |
| B | 嗅探的 15 秒对服务器先说话的协议 | 这些协议在 80 / 443 等端口上晚 15 秒才通 | 只在可嗅探的端口上；`always-raw-tcp-*`；日志里的计数看得到超时的数量 |
| C | hyper 的限制（头的顺序、拼写、1xx） | 个别依赖头顺序或拼写的服务、阶段 5 的 `full-header-mode` | 登记；编解码一层可以替换 |
| D | 证书兼容性：各平台、各浏览器对叶证书的要求在变（有效期一年比一年短） | 解密在某些客户端上失败 | 第 7.3 节取最严的一档；手工验收覆盖三个平台的浏览器；参数集中在一处 |
| E | 解密改变了站点看到的 TLS 与 HTTP/2 指纹 | 个别站点的风控（hudsucker #144 一类，原因没有证实） | 名单由使用者决定；登记，不尝试伪装 |
| F | 内存：正文缓冲（32 MB × 并发）、抓包、叶证书缓存 | 大量并发的大正文改写时内存冲高 | 只在有匹配的规则时缓冲；抓包有总量；M5 的基准量一次 |
| G | 用户流量里的秘密进了记录与抓包 | URL 的查询串、`Cookie` 等经 API 出去 | 日志不写查询串与请求头；抓包遮住四个头；归所有者定（Q1） |
| H | 新包的供应链 | 许可、维护、传递依赖 | 只为做不出来的东西引入；逐个写明；所有者可以撤回（Q4） |
| I | 范围大 | 迟迟没有可用的版本 | M1 交付时明文 HTTP 的规则与记录就有用；M2 交付时解密有用；重写、抓包逐个加 |
| J | jq 的求值不可中断、可能递归过深；表达式可以来自第三方的模块 | 一个坏的表达式拖住线程；栈溢出结束整个进程 | 第 3.4 节：jq 不在异步的工作线程上求值，M3 拿出隔离的办法之前不合并 jq |
| K | 正则的方言是推断（Surge 用 ICU 的语法没有证实），fancy-regex 与 ICU 有悄悄不同的写法 | 社区模块里的正则在 rurge 上匹配得不一样，且不报错 | 第 3.3 节：模板自己展开、悄悄不同的写法编译前改写、编译不过的告警；差异登记；手工验收拿几个常用的社区模块试 |
| L | 嗅探先于规则、引擎逐请求求值，改变了所有 80 / 443 会话的路径 | 阶段 1 ～ 3 的用例与行为大面积变化 | M1 拆两份计划；不可嗅探的会话一行不变；三个开关都关着时照样嗅探，但不解密、不重写，规则看到的只多了 SNI 与协议 |

## 17. 已决事项与开放问题

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P4-D1 | 里程碑与顺序 | 第 1.4 节：会话入口与明文 HTTP 引擎 → MITM → 重写与 Map Local → 抓包与记录的完整 → `auto-quic-block`、基准与遗留 → 收尾 |
| P4-D2 | 会话入口 | 可嗅探的会话先答后读，嗅探先于规则；不可嗅探的照旧（第 5.3 节） |
| P4-D3 | 嗅探的判定与等待 | 端口 80 / 443；`force-http-engine-hosts` 按连接的目标比（不带端口只对 80）、与开关无关；`[MITM]` 名单写明的端口（M2 起，`mitm` 开着且有 CA 时）；完整的 ClientHello（≤ 64 KiB）、方法记号加空格；等至多 15 秒；判不出、超时按原样转发（第 5.2、5.4 节） |
| P4-D4 | HTTP 的实现 | hyper 1.x，不用 `legacy::Client`；限制登记（第 3.1、6.9 节） |
| P4-D5 | 规则与目标 | 每个请求各自求规则、选策略；目标与拨号的地方是连接的目标，例外只有监听器的绝对形式请求与 URL Rewrite 的 `header`；嗅探到的连接上的代理式请求原样转给连接的目标；连接一层不另求；解密与否由名单定（第 6.3 节） |
| P4-D6 | 上游连接池 | 自己写；属于一条客户端连接、不跨客户端共享；键含出站链与配置代次；重载与出口表变化之后旧连接不再借出；选路与拨号两步，拨号时的记账照旧只在真拨号时做；`TCP_NODELAY`（第 6.4 节） |
| P4-D7 | MITM 的握手次序 | 先与客户端握手、再按请求连上游；不照抄上游证书与 ALPN（第 7.4 节） |
| P4-D8 | 叶证书 | 每张各自的 P-256 密钥、16 字节随机序列号、SAN 分 DNS 与 IP、回拨 2 天、总长 47 天、按名字缓存（第 7.3 节） |
| P4-D9 | CA | `ca-keystore-name` 优先；出问题告警且 MITM 不生效；`generate` 用 P-256、3DES / SHA1 的 p12，私钥只写进只属主可读的文件（第 7.2、7.6 节） |
| P4-D10 | 上游校验 | 系统的根 ∪ `webpki-roots`，只用于解密的上游；`skip-server-cert-verify` 只跳过链与名字（第 7.5 节） |
| P4-D11 | HTTP/2 | `h2 = true` 时两侧都给 `h2`、`http/1.1`，各自协商、由引擎转换；不宣告扩展 `CONNECT`；明文先验不进引擎（第 6.1、7.4 节） |
| P4-D12 | WebSocket | 101 之后透传字节，计入请求的记录（第 6.6 节） |
| P4-D13 | 正文 | 有匹配的规则才缓冲；32 MB 断开、10 MB 原样流过，解压后同样的上限；解压后不压回；`br` 用 `brotli-decompressor`、`zstd` 用 `ruzstd`，`deflate` 兼容裸流（第 3.5、8.4 节） |
| P4-D14 | 正则 | `fancy-regex`，以 ICU 的语法为准（推断）：替换模板照 `NSRegularExpression` 自己展开；悄悄不同的写法编译前改写；替换全部匹配；Body Rewrite 多行且按 CRLF 读行；回溯超限时重写跳过并留备注（第 3.3 节） |
| P4-D15 | jq | jaq；不在异步的工作线程上求值，隔离的办法是 M3 合并 jq 的前提（第 3.4 节） |
| P4-D16 | Map Local | 在 URL Rewrite 之后、Body Rewrite 之前；应答不过响应一侧的重写；文件流式读；路径先问 `Reach`；URL 是外部资源（第 8.5 节） |
| P4-D17 | 请求记录 | 一个请求一条，M1 起就带所用上游连接的 `remoteAddr`；承载的连接只在没有走完一个请求时有一条（第 9.1 节） |
| P4-D18 | 抓包 | 内存里、总量与单条有上限、命名会话、`/v1/rurge/capture/…` 与 HAR；在所有者回答 Q1 之前遮住四个凭据头（第 9.2 节） |
| P4-D19 | 功能开关 | 持久化；`state.json` 第 2 版；缺省 `mitm` 与 `rewrite` 开、`capture` 关（第 10.1 节） |
| P4-D20 | `http-api-tls` | 按访问的地址现签；没有 CA 时 API 不启动；命令行信配置里的 CA（第 7.8 节） |
| P4-D21 | `auto-quic-block` | rustls 的 `quic` 解 Initial、跨包重组、按第 7.1 节的条件阻断（第 11 节） |
| P4-D22 | crate | 新的 `rurge-http`，不依赖引擎、规则、策略；引擎实现它的 trait；依赖 `rurge-proto` 只为 `RejectKind`、TLS 的校验器与 p12 的解码（第 4 节） |
| P4-D23 | 配置 | 五个节类型化；一行的切法支持两种引号；`[Replica]` 认得并忽略（第 12 节，第 2.3 节 3.24、3.25） |
| P4-D24 | 依赖 | 新包只为做不出来的东西引入、许可在白名单里、逐个写明；`p12-keystore` 升到 0.4.x；rcgen 成为正式依赖（第 3.7 节） |
| P4-D25 | 日志与用户流量 | 每个请求的日志行级别同今天的会话（完成的 DEBUG、被拒与失败的 INFO）；路径只在 DEBUG；不写查询串、userinfo、请求头与正文（第 13 节） |
| P4-D26 | 测试 | 测试从不碰系统的信任库；手册示例的黄金用例；嗅探的时限可在测试里调短（第 14 节） |

开放问题（留给项目所有者；本阶段按上表做，所有者的答复改变的部分在当时的里程碑里改）：

| 编号 | 问题 |
| ---- | ---- |
| Q1 | 用户流量的内容——请求记录里的 URL（含查询串）、抓包里的请求头与正文——算不算 CLAUDE.md「凭据」一条管的"API 输出"？本设计的临时做法：日志不写查询串、userinfo、请求头与正文；记录给完整的 URL（去掉 userinfo）；抓包遮住 `Authorization`、`Proxy-Authorization`、`Cookie`、`Set-Cookie` 的值、正文照给。要不要改，改成什么？ |
| Q2 | 脱敏视图里重写节的头值、`[Map Local]` 的 `header=` / `data=`：照阶段 1 的裁定照写的显示，还是换成只显示已知无害内容的视图（M8c 计划延后第 43、51、72 行）？ |
| Q3 | `[MITM] # my certificate` 这样节头后面跟注释的行：读成节头并告警（M8c 时的建议），还是保持今天的读法（M8c 计划延后第 52 行）？阶段 4 让 `[MITM]` 生效之后，这一行读错的后果从"一串不相干的错误"变成"MITM 的键落进上一节" |
| Q4 | 本阶段新引入的包（`jaq-core`、`jaq-std`、`jaq-json`、`brotli-decompressor`、`ruzstd`，M3 定版本）与 `p12-keystore` 升到 0.4.x，同意吗？`webpki-roots`（CDLA-Permissive-2.0）不在 NFR-11 的白名单里，它早已在依赖里、本阶段多一处用法：放进白名单，还是换掉？解密的上游校验要不要改用调用系统校验接口的 `rustls-platform-verifier`（新包；Windows 与 macOS 上还有吊销检查），代替"系统的根 ∪ `webpki-roots`"？ |
| Q5 | 把 CA 自动装进系统（与 Firefox 的）信任库（清单 L585 的 P2）要不要做？这会改系统设置，是"不改系统设置"之外的又一个例外 |
| Q6 | 需求文档第 4 节的阶段栏：FR-HTTP-10 的"1 基础 / 4 完整"之外，FR-RULE-07 / 08 的拨号之前部分阶段栏写 1、登记在 4（M8c 计划延后第 105 行）——本阶段做完之后改阶段栏，还是保持现状由验收文档说明？ |

另有几件仍归所有者、与本阶段相交、本阶段不动的：NFR-01 的读数（阶段 3 / M5 计划延后第 4 行）——HTTP 引擎加进来之后同样量，结论照旧写成数字；gQUIC 与草案 28 以前的 QUIC（同一张表第 16 行）——`auto-quic-block` 同样不认；`CORE_VERSION` 的新编码（基线重对计划延后第 6 行）。

## 18. 需同步的文档

| 文档 | 改动 | 时机 |
| ---- | ---- | ---- |
| `docs/requirements.md` | 3.2 节 `rurge-http` 一行的依赖与职责；3.3 节第 8 步写成"每个请求先选策略、再 Header Rewrite → URL Rewrite → Map Local → Body Rewrite → 脚本"；FR-HTTP-03 的"SNI 命中忽略端口"加上"不带端口的项、只对嗅探到 SNI 的 TLS 与 QUIC"（第 2.2 节） | 随本文档 |
| `CLAUDE.md` | 「现在在哪里」记下阶段 4 开始、本文档与三份报告 | 随本文档 |
| `docs/README.md` | 本文档与两份调研报告各一项（手册重对报告同） | 随本文档 |
| `docs/surge-compatibility-matrix.md` | 各行的状态随里程碑更新；重对报告"手册多说了"的 14 行；hyper 的限制（第 6.9 节）；第 2.3 节里登记的读法；先答后读（第 5.3 节）；L265 与 L314 | 各里程碑收尾 |
| `README.md`（中英） | 状态、特性表与路线图 | 各里程碑收尾 |
| `docs/api/phase4.md` | 新建：开关、`/v1/mitm/ca`、记录的新字段、`/v1/rurge/capture/…`、日志行 | M1 起 |
| `docs/acceptance/phase4-manual.md` | 新建：三个平台的真实浏览器与站点上的解密、证书固定的提示、CA 的安装步骤、`http-api-tls`、重写在真实站点上 | M2 起逐里程碑加 |
| `docs/acceptance/phase4-acceptance.md` | 新建：第 15 节逐条的证据与需求逐句对照代码的结果 | M6 |
| `docs/testing.md` | 新的测试约定：测试不碰信任库、手册示例的黄金用例、嗅探时限的测试选项 | M1、M2 |
