# 阶段 4 调研（一）：手册对兼容性清单的逐行重对

日期：2026-10-29。只读调研：取手册的文本，读，对着兼容性清单逐行比；没有改仓库里别的文件，没有观察过 Surge 的行为。本文是阶段 4（HTTP 引擎）设计的输入。

## 0. 来源、做法、数字与记号

**来源**

- 手册文本：2026-10-29 照基线报告（`docs/reports/2026-10-14-manual-baseline-recheck.md`）附录 B 的办法取的 87 页。它与 2026-10-28 复查（`docs/reports/2026-10-28-manual-6.11-recheck.md`）取的文本逐页逐字节相同：手册在这两次之间没有改，对应的仍是 Surge Mac 6.11.0 / iOS 5.24.0，每页的摘要就是基线报告附录 A 的值换上 6.11 复查第 2 节表里那四页的新值。
- 从头读到尾的页：`http/overview`、`http/mitm`、`http/url-rewrite`、`http/header-rewrite`、`http/body-rewrite`、`http/map-local`、`rules/http`、`profile/host-list`、`profile/keystore`、`tools/dashboard`。读了相关一节的页：`profile/general`（HTTP API、Traffic Processing、`show-error-page`）、`tools/http-api`（鉴权、HTTPS、功能开关、请求、`/v1/mitm/ca`）、`rules/overview`（规则参数表）、`rules/ruleset`（`extended-matching`）、`getting-started/how-surge-works`、`scripting/http-request`、`scripting/http-response`、`policies/reject`、`profile/module`、`tools/cli`。其余各页用 grep 扫过 WebSocket、keep-alive、pipelin、HTTP/2、h2、chunked、100-continue、journal、capture、reuse 这些词。
- 清单：`docs/surge-compatibility-matrix.md`（仓库 `f87012f`）。行号是文件的行号，写作 `L576`；清单一改，行号就会移动。
- 需求：`docs/requirements.md` 第 4.7 节的 FR-HTTP-01 ～ 14、FR-RULE-14 与第 7 节「阶段 4」。

**做法**

- 清单的表格用脚本按列解析（单元格里转义的竖线 `\|` 不切），取「阶段」一格含 4 的行，再逐行对着手册读。清单里除了开头的记号表与末尾的统计表，每张表都有「阶段」一栏。
- 引文一律是手册的英文原文，写作 “…”（页）；页名是手册里的路径。每条引文都用脚本核过：去掉反引号、折叠空白之后，它是那一页文本的子串（引文里的省略写作 “…”，省略号两边各自是子串）。
- 不是手册原话、而是从手册推出来的，标 **（推断）**。

**数字**

- 「阶段」一格含 4 的行是 **64 行**。按这一格的写法分：`4` 51 行，`1 / 4` 6 行，`4 / 5` 2 行，`1 / 3 / 4`、`1 / 4 / 5`、`2 / 4`、`4 / 6`、`4 / 7` 各 1 行。按节分：1.2 节 5、1.6 节 1、2.1 节 4、3.1 节 2、3.2 节 2、3.3 节 1、3.5 节 1、7.1 节 8、7.2 节 13、7.3 节 5、7.4 节 4、7.5 节 6、7.6 节 7、10.3 节 1、10.4 节 4。第一遍用直接按 `|` 切的脚本数出 61 行，漏了单元格里有转义竖线的三行（L605、L614、L630）。
- 这 64 行的 rurge 一栏：✅ 58、🟡 4（L314、L584、L585、L884）、🔁 1（L588）；L860 在 10.3 节 CLI 的表里，那张表没有这一栏。
- 判定的定义同阶段 3 的重对（`docs/reports/2026-10-16-phase3-manual-recheck.md` 第 0 节）：**一致**＝清单这一行说的手册都支持，手册没有再说设计要知道的东西；**手册多说了**＝清单说的对，但手册还有这一行没有的东西（缺省、取值、语义、版本、平台、限制），逐条列出；**不一致**＝清单这一行与手册相冲，或把清单自己的推断写成了手册原话；**手册没有写**＝这一行说的是 rurge 自己的东西，手册不涉及。
- 结果（第 1 节）：一致 49，手册多说了 14，不一致 0，手册没有写 1。**没有一行与手册相冲。**

**没有覆盖**

- Surge 的实际行为：一处也没有观察过。「一致」说的是清单与手册的文字一致。
- 发行说明：只看了知识库里 Surge Mac 6 的发行说明页（<https://kb.nssurge.com/surge-knowledge-base/release-notes/surge-mac-6-release-note>，2026-10-29 经网页摘要读到，不是逐字的原文；这一页只写到 6.0）。与本阶段有关的两条：6.0 起 HTTP 支持 zstd 压缩（Body Rewrite 解压正文时要认得它，推断）；脚本的 `full-header-mode`（阶段 5，清单 L655 已列；它要求引擎保留同名的多行头与它们的先后，推断）。6.0 之后各版的发行说明没有读。
- 取文本时丢掉的：图片、链接的目标、表格的行列关系（附录 B 的脚本把每个单元格写成一行）、粗体与斜体。
- 阶段 5 的脚本各页：只读了与 HTTP 引擎相交的部分（正文的上限、`requires-body`、`max-size`）。

**给设计的要点**（细节在第 1 ～ 3 节）

1. **先选策略，再处理 HTTP。**“After a policy is selected, HTTP requests and responses can be rewritten, mapped to local data, or processed by scripts.”（`getting-started/how-surge-works.html`）。HTTP 类规则看的是一个请求：“HTTP rules match properties of an HTTP request: the User-Agent header and the request URL.”（`rules/http.html`）。所以引擎里的每个请求各自匹配规则、各自得到策略，然后才改写（推断）；URL Rewrite 的 `header` 模式“redirects it to another host if necessary”（`http/url-rewrite.html`），改写之后的主机走的是改写之前选好的策略（推断）。
2. **嗅探只在 80 与 443 端口上。**“Surge will automatically sniff the protocol for TCP requests sent to ports 80 and 443”（`profile/general.html`）；“Cleartext HTTP on non-standard ports may not be detected by sniffing.”（`http/overview.html`）。别的端口上的明文 HTTP 要靠 `force-http-engine-hosts` 进引擎，别的端口上的 TLS 要靠 MITM 名单里写明的端口（推断）。`always-raw-tcp-hosts` 命中的连接“without sniffing or parsing”（`http/overview.html`）。
3. **MITM 名单是按顺序匹配的 Host List**。不带端口的项只对 443；对“TLS or QUIC connections identified by the sniffed SNI”，不带端口的项不看端口（`http/mitm.html`）；`hostname-disabled` 里的项从生效的名单里去掉；非法项只告警。
4. **证书固定的提示只在握手完成之后。**“If a client completes the TLS handshake but disconnects without sending a request, Surge logs a hint that the host is likely protected by certificate pinning.”（`http/mitm.html`）。握手本身失败（客户端不信任 CA）不是这一条说的情形（推断）。
5. **正文改写的上限与例外手册写全了**：请求体至多缓冲 32 MB，超出断开；响应体 macOS 上至多 10 MB，超出放弃修改、原样流过；`Transfer-Encoding: chunked` 或 `Expect: 100-continue` 的请求体不改写、告警、原样转发；改写之前按需解压，之后重算 `Content-Length`；不是 UTF-8（正则）或不是 JSON（jq）时跳过并在请求记录里留备注；jq 的输出为空时保留原文。
6. **Header Rewrite 不能改消息的框架**：改了 `Content-Length` 或 `Transfer-Encoding` 的改写被拒绝，“the request fails”（`http/header-rewrite.html`）。
7. **Map Local 要 Rewrite 开关开着**；`file` 的路径相对 Profile 的目录，也可以是 URL（下载并缓存）。在 rurge 里，把 Profile 文本变成本机路径的东西先问 `Reach`（CLAUDE.md「下载到的文本」），下载走资源管理器（推断）。
8. **手册的 HTTP API 没有读抓包内容的端点**。功能开关有 `capture`，规则参数有 `always-capture=<session-name>`，但没有一个端点给出抓到的请求头与正文。需求 FR-HTTP-11 要「通过 API 与 Dashboard 查看」，所以这一组端点是 rurge 专有的；抓包的容量是 rurge 专有的运行时选项，照 FR-CFG-17 只走命令行参数与环境变量（推断）。
9. **`http-api-tls` 用 MITM 的 CA 现签服务端证书**，“for the corresponding access address”（`tools/http-api.html`）。
10. 手册没有定义、设计只能自己定的，列在第 3 节（25 处）。

---

## 1. 逐行对照

### 1.1 第 1 节 配置文件格式（L50 ～ L54、L129）

- **L50 `[URL Rewrite]`、L51 `[Header Rewrite]`、L52 `[Body Rewrite]`、L53 `[Map Local]`、L54 `[MITM]`**（1.2；✅；4）——**一致**（五行）。五个节在手册里各有一页（`http/url-rewrite.html` 等），语义逐项见 L594 ～ L632 与 L576 ～ L588。
- **L129 Keystore 的引用点**（1.6；✅；2 / 4）——**一致**。
  - 手册：“A `p12` item can provide the CA certificate and key for HTTPS decryption, referenced with the `ca-keystore-name` parameter in the `[MITM]` section, as an alternative to the inline `ca-p12` parameter.”（`profile/keystore.html`）；“The value is the name of a Keystore item of p12 type. When set, it takes precedence over `ca-p12`.”（`http/mitm.html`）。
  - 清单的备注说条目内容变了的重载会重建引用它的策略；`ca-keystore-name` 引用的条目变了时 MITM 的 CA 跟着换，是设计要做的（推断）。

### 1.2 第 2 节 `[General]`（L166、L172 ～ L174）

- **L166 `http-api-tls`**（2.1；✅；4）——**手册多说了**。
  - 一致：“Use HTTPS protocol instead of HTTP for the HTTP API. The MITM CA certificate must be configured first.”（`profile/general.html`）。
  - 多说的：服务端证书是用 MITM 的 CA 现签的，签给访问它的那个地址：“Surge will use the CA certificate of MITM to generate the server certificate for the corresponding access address. You need to install the certificate on the client device manually.”（`tools/http-api.html`）。与 L904 同。
- **L172 `force-http-engine-hosts`**（2.1；✅；4）——**一致**。
  - 手册：“By default, only connections on port 80 are matched. Append `:port` to match another port, `host:0` to match all ports of a host, or `*:0` to match everything. Prefix an entry with `-` to exclude it.”（`profile/general.html`）；“It only affects cleartext traffic and never decrypts TLS.”（`http/overview.html`，清单 L564 已写）。
- **L173 `always-raw-tcp-hosts`**（2.1；✅；4）——**手册多说了**。
  - 一致：Host List；“Matching connections are forwarded as raw TCP streams without sniffing or parsing.”（`http/overview.html`）。
  - 多说的：版本标记 iOS 5.8.0+ / Mac 5.4.0+（`profile/general.html` 的标题行），清单的平台一格写“全部”。
  - 没有说的：不带端口的项对哪个端口生效。Host List 的规矩是“If a port number is not provided, Surge automatically appends the standard port number for that parameter.”（`profile/host-list.html`），但这个参数的标准端口手册没有写（第 3 节 3.10）。
- **L174 `always-raw-tcp-keywords`**（2.1；✅；4）——**一致**。
  - 手册：“Any hostname containing one of the keywords will skip protocol sniffing and stay in raw TCP mode, which helps when the exact hostname is not predictable.”（`profile/general.html`）；版本 Mac 5.5.0+，清单已写。大小写手册没有说（第 3 节 3.11）。

### 1.3 第 3 节 规则（L240、L241、L265、L271、L283、L314）

- **L240 `USER-AGENT`**（3.1；✅；1 / 4）——**一致**。
  - 手册：“Wildcard characters `*` and `?` are supported. Matching is case-sensitive.”；“`USER-AGENT` can only match if the client happens to send a User-Agent header in the proxy CONNECT request.”（`rules/http.html`）。
- **L241 `URL-REGEX`**（3.1；✅；1 / 4）——**手册多说了**。
  - 一致：“The rule matches when the pattern is found anywhere in the URL; anchor with `^` when you want a prefix match. The regular expression is case-sensitive.”（`rules/http.html`）。
  - 多说的：解密之后的 HTTPS 请求，被测的是解密后的 URL：“For HTTPS requests with MITM enabled, it is the decrypted URL, e.g. `https://example.com/path?query`.”（`rules/http.html`）。
- **L265 `extended-matching`**（3.2；✅；1 / 4）——**一致**。
  - 手册：“Also match the TLS SNI and the HTTP Host header (or `:authority`). On RULE-SET/DOMAIN-SET, applies to every entry.”（`rules/overview.html`）；对 `URL-REGEX`：“also test URL variants in which the host part is replaced by the TLS SNI and by the HTTP Host header (or `:authority`)”（`rules/http.html`）。
  - 清单的备注写的是 rurge 的现状：决定策略时会话还没有 SNI，所以对真实会话不起作用。要它起作用，嗅探要在规则匹配之前（推断；见要点 2）。
- **L271 `always-capture=<session>`**（3.2；✅；4）——**一致**。
  - 手册：适用于“Any rule, including FINAL”；“Force-enable HTTP capture for connections matched by the rule, recorded under the named capture session. Intended for debugging.”（`rules/overview.html`）。
- **L283 HTTP 类规则的匹配语义**（3.3；✅；1 / 4）——**一致**。
  - 手册：“They only take effect for requests processed by Surge's HTTP engine; plain TCP or UDP connections are never matched by these rules.”；“This includes requests sent to Surge's HTTP proxy port and TCP connections forced into HTTP processing with `force-http-engine-hosts`.”；“For HTTPS requests that are not decrypted, no URL is available, so `URL-REGEX` cannot match.”（`rules/http.html`）。
- **L314 规则集文件里一行上的 `extended-matching`**（3.5；🟡；1 / 4）——**一致**。
  - 手册：“Per-line options such as `no-resolve` and `extended-matching` are allowed.”；“Note that if any domain rule inside the set file already carries the flag, it is applied to all domain rules of that set.”（`rules/ruleset.html`）。
  - 清单写明 rurge 没有做完（进了域名索引的 `DOMAIN` 与 `DOMAIN-SUFFIX` 行丢了标志，“一条带、整集都带”也没有做），到阶段 4 才露出来。阶段 4 让 SNI 进入规则匹配之后，这一行要一起补上（推断）。

### 1.4 第 7 节 HTTP 处理

#### 7.1 HTTP 引擎（L562 ～ L569）

- **L562 新 TCP 连接嗅探首字节**（✅；4）——**一致**。
  - 手册：“When a new TCP connection arrives, Surge sniffs the first bytes; if they look like an HTTP request, the connection is passed to the HTTP engine.”（`http/overview.html`）。
- **L563 80 / 443 自动嗅探，`always-raw-tcp-*` 关闭嗅探**（✅；4）——**一致**。清单的备注已经把两页的两句话并列，要设计都对上；本文的读法见要点 2。
- **L564 `force-http-engine-hosts` 不解密 TLS**（✅；4）——**一致**（引文见 L172）。
- **L565 HTTPS 缺省不透明**（✅；4）——**一致**。
  - 手册：“HTTPS traffic is an opaque TLS tunnel by default. Surge can only process it after decrypting it with MITM for that hostname.”（`http/overview.html`）。
- **L566 流水线顺序**（✅；4 / 5）——**手册多说了**。
  - 一致：Header Rewrite、URL Rewrite、Body Rewrite、Script processing 依次（`http/overview.html`）。
  - 多说的：这些处理在选定策略之后：“After a policy is selected, HTTP requests and responses can be rewritten, mapped to local data, or processed by scripts.”（`getting-started/how-surge-works.html`）。见要点 1。
- **L567 每个请求 / 响应至多一个脚本，重写可以串联**（✅；4 / 5）——**一致**。
  - 手册：“A request or response may be modified by only one script. The rewrite features have no such limit: if multiple rewrite rules match, they take effect in sequence.”（`http/overview.html`）。例外是 URL Rewrite 的 `header` 模式只用第一条（L595）。
- **L568 Map Local 短路**（✅；4）——**一致**。
  - 手册：“Map Local is also evaluated in the request path: when a Map Local rule matches, Surge returns the local response directly and skips the upstream request.”（`http/overview.html`）。它排在流水线的哪一步，手册没有说（第 3 节 3.1）。
- **L569 抓包视图逐条展示，含备注**（✅；4 / 6）——**手册多说了**。
  - 一致：“Requests handled by the HTTP engine appear as individual HTTP requests in the Dashboard's capture viewer, with full headers and bodies available.”；“the request detail view shows the notes for applied rewrites and scripts.”（`http/overview.html`）。
  - 多说的：请求头与正文是全的（“with full headers and bodies available”），清单这一行没有写正文。正文是不是只在抓包开着时才保存，手册没有说（第 3 节 3.18）。

#### 7.2 `[MITM]`（L576 ～ L588）

- **L576 `hostname`**（✅；4）——**手册多说了**。
  - 一致：Host List，缺省端口 443，`:0` 全端口，`-` 排除，非法项只告警（“An invalid entry in the list produces a profile warning instead of a parse error”，`http/mitm.html`）。
  - 多说的：（a）按顺序匹配，排除项要写在前面：“Items are matched in order, so put exclusions first”（`http/mitm.html`）；“Once a match succeeds, matching stops.”（`profile/host-list.html`）。（b）不看端口的只是**不带端口的项**，并且只对靠嗅探到的 SNI 认出来的 TLS 与 QUIC 连接：“For TLS or QUIC connections identified by the sniffed SNI, a bare entry matches regardless of the connection's actual port.”（`http/mitm.html`）；清单写的“SNI 命中时忽略端口”没有这两个限定。（c）特殊的项 `<ip-address>`、`<ipv4-address>`、`<ipv6-address>`、`<simple-hostname>`（`profile/host-list.html`）。（d）“Surge only decrypts traffic to hosts declared in `hostname`.”（`http/mitm.html`）。
- **L577 `hostname-disabled`**（✅；4）——**一致**。“Entries in this list are removed from the effective `hostname` list.”（`http/mitm.html`）。
- **L578 `ca-p12`**（✅；4）——**一致**。“The CA certificate and private key in PKCS#12 format, base64-encoded.”（`http/mitm.html`）。
- **L579 `ca-passphrase`**（✅；4）——**一致**。“The passphrase of the PKCS#12 data. It cannot be empty due to system limitations.”（`http/mitm.html`）。这是 Apple 平台的限制；rurge 的平台上空口令要不要接受，见第 3 节 3.23。
- **L580 `ca-keystore-name`**（✅；4）——**一致**（引文见 L129）。
- **L581 `skip-server-cert-verify`**（✅；4）——**一致**。“This relaxes verification of the real server only; the client side still receives the Surge-generated certificate.”（`http/mitm.html`）。
- **L582 `h2`**（✅；4）——**一致**。“MITM over HTTP/2: decrypt HTTPS traffic with MITM over the HTTP/2 protocol, which can improve the performance of concurrent requests.”（`http/mitm.html`）。对客户端与对上游各说什么协议，手册没有说（第 3 节 3.6）。
- **L583 `client-source-address`**（✅；4 / 7）——**手册多说了**。
  - 一致：IP 或 CIDR，`-` 排除，不设即全部（“This is equivalent to `client-source-address = 0.0.0.0/0, ::/0`.”），Mac 6.1.0 起可以写 MAC 地址（`http/mitm.html`）。
  - 多说的：本机的流量以 `127.0.0.1` 计：“`127.0.0.1` should be included if you want to enable MITM for the current device.”（`http/mitm.html`）。排除写在前面的例子 `client-source-address = -192.168.1.2, 0.0.0.0/0` 说明它也是按顺序匹配（推断）。
- **L584 `auto-quic-block`**（🟡；4）——**手册多说了**。
  - 一致：布尔，缺省 true；“When a QUIC connection (i.e. HTTP/3) hits the MITM hostname list, Surge automatically blocks that QUIC connection, causing the client to fall back to HTTP/2 or HTTP/1.1 so the traffic can be intercepted by MITM.”（`http/mitm.html`）。
  - 多说的：版本 iOS 5.8.0+ / Mac 5.4.0+，清单没有写。QUIC 连接怎样“hits the MITM hostname list”见 L576（b）：靠嗅探到的 SNI，不带端口的项不看端口。
- **L585 CA 的生成**（🟡；4）——**手册多说了**。
  - 一致：生成器在 Dashboard（Mac）与 iOS 的配置编辑器里，生成之后让系统信任（`http/mitm.html`）。
  - 多说的：“The certificate is generated locally and saved only in your profile and the system Keychain.”（`http/mitm.html`）。rurge 不改写用户的配置文件（CLAUDE.md「兼容性原则」），所以 `rurge mitm ca generate` 只能给出要写进 `[MITM]` 的几行，由用户自己写进去（推断）。
- **L586 使用现有的 CA**（✅；4）——**一致**。“Export the certificate to PKCS#12 format (.p12) with a passphrase. The passphrase cannot be empty due to system limitations. Use the `base64` command to encode the certificate, then set `ca-p12` and `ca-passphrase` in the profile.”（`http/mitm.html`）。
- **L587 证书固定的提示**（✅；4）——**一致**（引文见要点 4）。
- **L588 手册没有定义的键**（🔁；4）——**一致**。手册的参数一节恰好九个键：`hostname`、`hostname-disabled`、`ca-p12`、`ca-passphrase`、`ca-keystore-name`、`skip-server-cert-verify`、`h2`、`client-source-address`、`auto-quic-block`（`http/mitm.html`）。

#### 7.3 `[URL Rewrite]`（L594 ～ L598）

- **L594 语法**（✅；4）——**一致**。“Each rule consists of three parts: a regular expression, a replacement, and a type. If the type is omitted, `header` is used.”；“The regular expression is matched against the full request URL. The replacement supports capture group references such as `$1`.”（`http/url-rewrite.html`）。
- **L595 `header` 模式**（✅；4）——**手册多说了**。
  - 一致：“If multiple header-mode rules match a request, only the first one is applied. The rewritten URL must be a valid `http`, `https`, or `ws` URL, otherwise the rewrite is discarded.”；“The `Host` field in the request header is modified to match the new URL.”（`http/url-rewrite.html`）。
  - 多说的：请求改道到别的主机，客户端看不出：“Surge modifies the request in place and redirects it to another host if necessary. The client does not notice the rewrite.”（`http/url-rewrite.html`）。改了协议（`http` 改成 `https`）时上游怎样连，手册没有说（第 3 节 3.13）。
- **L596 `302` / `307`**（✅；4）——**手册多说了**。
  - 一致：“Surge returns a 302 redirect response to the client, with the replacement as the `Location`.”（`http/url-rewrite.html`）。
  - 多说的：占位符换成什么：“the replacement may contain the placeholder `{{{GATEWAY_ADDRESS}}}`, which is substituted with the default router address of the current outgoing network interface.”；307 的区别：“Unlike 302, a 307 redirect requires the client to keep the original request method and body when following the redirect.”（`http/url-rewrite.html`）。
- **L597 `reject`**（✅；4）——**一致**。“Reject the request if the pattern is matched. The replacement parameter is ignored; use `_` as a placeholder.”（`http/url-rewrite.html`）。给客户端什么应答，手册没有说（第 3 节 3.13）。
- **L598 用 Host 头与连接的主机名重建的 URL 也匹配**（✅；4）——**一致**。“Besides the request-line URL, Surge also tries matching with the URL rebuilt from the `Host` header value and from the underlying connection's hostname.”（`http/url-rewrite.html`）。

#### 7.4 `[Header Rewrite]`（L605 ～ L608）

- **L605 语法**（✅；4）——**一致**。方向可以省略：“Old versions only supported modifying the request, so this part can be omitted, which means `http-request`”（`http/header-rewrite.html`）。
- **L606 四种动作**（✅；4）——**一致**。“Append a new header line to the header, even if the header field already exists.”；“Delete a header line from the header.”；“Replace a header value in the header. If the header field doesn't exist, nothing happens.”；“Replace a header value with a regular expression and a template. If the header field doesn't exist, nothing happens.”（`http/header-rewrite.html`）。同名多行、大小写、替换几处，见第 3 节 3.15。
- **L607 多条按顺序生效**（✅；4）——**一致**（`http/overview.html` 的引文见 L567）。
- **L608 改消息框架的改写被拒绝**（✅；4）——**一致**。“Header Rewrite cannot change the message framing. If a rewrite changes the `Content-Length` or `Transfer-Encoding` headers, the modification is rejected and the request fails.”（`http/header-rewrite.html`）。

#### 7.5 `[Body Rewrite]`（L614 ～ L620）

- **L614 正则的形式**（✅；4）——**一致**。方向不能省：“Each line contains a rewrite rule, with parameters separated by spaces, starting with `http-request` or `http-response`.”；“The `^` and `$` anchors match at the beginning and end of each line of the body.”；“If a request hits multiple body rewrite rules, they are executed in sequence.”；“Even if the original request does not contain a body, new content may still be generated through body rewrite, such as using the `^$` expression.”（`http/body-rewrite.html`）。
- **L615 正文须是 UTF-8**（✅；4）——**一致**。“The body must be valid UTF-8 text for regex-based rewriting. If it is not, the rule is skipped and a note is added to the request journal.”（`http/body-rewrite.html`）。
- **L616 jq 的形式**（✅；4）——**一致**。“The body must be valid JSON; otherwise, the rule is skipped and a note is added to the request journal.”；“If the jq expression produces empty output, the original body is kept.”；“An invalid jq expression produces a warning and the rule is skipped.”（`http/body-rewrite.html`）。手册的例子把含空格的表达式放在单引号里：“http-response-jq ^http://httpbingo.org/anything '.headers |= with_entries(select(.key | test("^X-") | not))'”（`http/body-rewrite.html`）——这一节的一行按空白切，引号里的空白不切（推断）。
- **L618 大小上限**（✅；4）——**一致**。“A request body may be buffered up to 32 MB; if it exceeds the limit, the connection is dropped.”；“A response body may be modified only up to 1 MB on iOS and 10 MB on macOS. If the response exceeds the limit, Surge gives up the modification and streams the response through unmodified.”（`http/body-rewrite.html`）。
- **L619 chunked 与 `Expect` 的请求体**（✅；4）——**一致**。“Request body rewrite is not supported when the request uses `Transfer-Encoding: chunked` or `Expect: 100-continue`. Surge logs a warning and forwards the request unmodified.”（`http/body-rewrite.html`）。
- **L620 解压与 `Content-Length`**（✅；4）——**一致**。“When the body is modified, Surge normalizes the related headers automatically: the body is decompressed first if necessary, and `Content-Length` is recalculated.”（`http/body-rewrite.html`）。改写之后压不压回去，手册没有说（第 3 节 3.16）。

#### 7.6 `[Map Local]`（L626 ～ L632）

- **L626 语法与前提**（✅；4）——**一致**。“Map Local only applies to requests handled by the HTTP engine, and only when the Rewrite feature is enabled.”（`http/map-local.html`）。
- **L627 `data-type`**（✅；4）——**手册多说了**。四种与缺省 `file` 一致（“If `data-type` is omitted, `file` is used.”，`http/map-local.html`）；多说的是 `text`、`tiny-gif`、`base64` 三种的版本标记 iOS 5.9.1+ / Mac 5.5.1+，清单没有写。
- **L628 `data`**（✅；4）——**手册多说了**。
  - 一致：“For `file` type, this field should be the path to the data file, with relative paths being relative to the profile's directory. On macOS, absolute paths can also be used. A URL may also be used; the resource is downloaded and cached.”（`http/map-local.html`）。
  - 多说的：“On iOS and tvOS, the file size is limited to 15 MB.”（`http/map-local.html`）。macOS 上有没有上限，手册没有说（第 3 节 3.17）。
- **L629 空响应**（✅；4）——**一致**。“You can use `data-type=text data=""` to return an empty result.”（`http/map-local.html`）。
- **L630 `header`**（✅；4）——**一致**。“The value may also be a base64-encoded string of newline-separated header lines. If the value contains no `:`, Surge treats it as base64.”（`http/map-local.html`）。
- **L631 `status-code`**（✅；4）——**一致**。“Values outside 200–999 are invalid. The upper bound of 999 allows mocking non-standard status codes.”（`http/map-local.html`）。
- **L632 `Content-Type` 的补全**（✅；4）——**一致**。“For `file` type, Surge tries to convert the file extension to a MIME type. If it fails, `application/octet-stream` is used.”（`http/map-local.html`），其余三种的缺省同清单。

### 1.5 第 10 节 工具与可观测性（L860、L871、L884、L901、L904）

- **L860 `rurge mitm ca generate/export`**（10.3；1 / 3 / 4）——**手册没有写**。这是 rurge 专有的子命令。手册的 CLI 页只有一族开关命令：“Inspect or control MitM, Rewrite, Scripting, HTTP Capture, Packet Capture, and Cellular Mode.”（`tools/cli.html`），属阶段 6。
- **L871 `/v1/features/mitm`、`capture`、`rewrite`、`scripting`**（10.4；✅；1 / 4 / 5）——**一致**。“Use the GET method to obtain the state of a capability.”，应答与请求都是 `{"enabled":true}` 这样的形状（`tools/http-api.html`）。开关是否跨重启保留，手册没有说；需求 FR-HTTP-13 要持久化，是 rurge 自己的要求（第 3 节 3.19）。
- **L884 `/v1/requests/recent`、`active`、`kill`**（10.4；🟡；1 / 4）——**一致**。手册只列了路径与 `kill` 的请求体 `{"id": 100}`（`tools/http-api.html`），没有给记录的形状。
- **L901 `GET /v1/mitm/ca`**（10.4；✅；4）——**一致**。“Obtain the CA certificate for MITM, in DER binary format. (Certificate only, no private key included)”（`tools/http-api.html`）。手册另说密钥可以放在查询参数里，“For example, directly downloading the CA certificate through a browser.”（`tools/http-api.html`）；rurge 阶段 1 起就认 `?x-key=`（清单 L165）。
- **L904 `http-api-tls`**（10.4；✅；4）——**手册多说了**（同 L166）。

### 1.6 汇总

| 判定 | 行数 | 行 |
| ---- | ---- | -- |
| 一致 | 49 | 其余各行 |
| 手册多说了 | 14 | L166、L173、L241、L566、L569、L576、L583、L584、L585、L595、L596、L627、L628、L904 |
| 不一致 | 0 | — |
| 手册没有写 | 1 | L860 |

---

## 2. 手册里有、清单里没有的

逐行对照之外，读到的、清单里没有一行写着的：

1. **策略在 HTTP 处理之前选定**（要点 1）。清单 7.1 节的流水线一行只写了四种处理的先后。
2. **Body Rewrite 的方向不能省**，Header Rewrite 的可以省（L605、L614 的引文）。清单两行的写法（一个带方括号、一个不带）与此一致，但没有明说。
3. **单引号**：jq 的例子用单引号包住含空格的表达式（L616）。四个重写节的一行按空白切，引号怎样护住空白，手册没有写（第 3 节 3.25）。
4. **`[Replica]`**：“The legacy `[Replica]` section used by the HTTP capture feature was removed in Surge Mac 5.4.0, so modules no longer need to patch it.”（`profile/module.html`）。清单 1.2 节的 24 个节里没有它；旧的配置里写着它时 rurge 怎样，见第 3 节 3.24。
5. **脚本的正文上限与 Body Rewrite 共用**：请求脚本的 `max-size` 写 `-1` 时“a hard cap of 32 MB for request bodies still applies”（`scripting/http-request.html`）；响应脚本拿到的是解压之后的正文（“Buffers the entire response body (decompressed) and passes it to the script”），超出 `max-size` 时“Surge falls back to passthrough mode: the script is skipped for this request and a note is attached to the request record.”（`scripting/http-response.html`）。脚本属阶段 5，但正文怎样缓冲、超出上限怎样回落，阶段 4 的引擎要给它留好同一套做法（推断）。
6. **`$surge.setHTTPCaptureEnabled`** 与 `setRewriteEnabled`：脚本也能切抓包与重写的开关（`scripting/api.html`；清单 8 节 L681 已列，阶段 5 / 6）。阶段 4 的开关要能被 API 以外的地方切（推断）。
7. **`show-error-page-for-reject`** 只对明文 HTTP 请求：“Show an error webpage for the REJECT policy if the request is a plain HTTP request.”（`profile/general.html`）。解密之后的 HTTPS 请求算不算“plain HTTP request”，手册没有说（第 3 节 3.2）。

## 3. 手册没有写、设计只能自己定的

下面每一处都是“在读过的页里没有找到”（推断）；设计各自定一个做法并写明理由。

| # | 问题 | 相关的行 |
| - | ---- | -------- |
| 3.1 | Map Local 排在流水线的哪一步：在 Header Rewrite 与 URL Rewrite 之前还是之后，相对请求脚本在哪里 | L568 |
| 3.2 | 一条进了引擎的连接里，规则怎样逐个请求求值；连接这一层（CONNECT 的目标、SNI）的规则结果与请求这一层的是什么关系；解密的连接在第一个请求之前遇到 `REJECT` 时怎样；解密之后的请求被 `REJECT` 时给不给错误页 | L283、L566 |
| 3.3 | 嗅探等多久、看多少字节；客户端不先说话的协议（服务器先说话）怎样不被卡住 | L562、L563 |
| 3.4 | SOCKS5 与 HTTP `CONNECT` 什么时候答“成功”：要先嗅探到客户端的第一批字节，就要在拨号之前答（推断），拨号失败只能关连接 | L562 |
| 3.5 | 叶子证书：密钥类型、有效期、SAN 与扩展、缓存多大；`rurge mitm ca generate` 生成的 CA 的参数；p12 用什么加密算法写、能读哪些算法 | L576、L585、L586 |
| 3.6 | `h2 = true` 时对客户端在 ALPN 里给什么、对上游用什么；上游不支持 HTTP/2 时 | L582 |
| 3.7 | 不跳过校验时上游的证书不合法：给客户端什么 | L581 |
| 3.8 | 没有 SNI 的 ClientHello（按 IP 访问）时，MITM 名单拿什么去比（`<ip-address>` 一类的项说明 IP 也当主机名比，推断） | L576 |
| 3.9 | `client-source-address` 对经虚拟网卡来的本机流量取哪个地址（手册只说本机以 `127.0.0.1` 计） | L583 |
| 3.10 | `always-raw-tcp-hosts` 不带端口的项对哪个端口生效（嗅探只在 80 与 443，推断两个都算） | L173 |
| 3.11 | `always-raw-tcp-keywords` 分不分大小写 | L174 |
| 3.12 | 同一个主机既在 `always-raw-tcp-*` 里又在 MITM 名单里时谁优先（“without sniffing or parsing” 说明不解密，推断） | L173、L576 |
| 3.13 | URL Rewrite：`reject` 给什么应答；`302` / `307` 的应答带不带正文；没有网关时 `{{{GATEWAY_ADDRESS}}}` 换成什么；`header` 模式把 `http` 改成 `https` 时上游走 TLS；`wss` 算不算合法（手册只写了 `ws`） | L595 ～ L597 |
| 3.14 | 用 Host 头与连接的主机名重建的 URL，是否也用于 Header Rewrite、Body Rewrite 与 Map Local 的匹配（手册只在 URL Rewrite 一页写了） | L598 |
| 3.15 | Header Rewrite：同名的多行，`header-del` / `header-replace` 对全部还是第一行；字段名分不分大小写；`header-replace-regex` 替换全部匹配还是第一处；响应方向的“the request fails”给客户端什么 | L606、L608 |
| 3.16 | Body Rewrite：每个正则替换全部匹配还是第一处；`^$` 在非空正文上（多行模式下它匹配每个空行）；解压之后压不压回去、`Content-Encoding` 怎样；jq 产出多个值时；jq 的方言（jq 1.6 还是 1.7）；正则的方言（Surge 跑在 Apple 平台上，推断用的是 `NSRegularExpression` 即 ICU 的语法，替换模板里的 `$1` 也是它的写法） | L614 ～ L620 |
| 3.17 | Map Local：`file` 的大小上限（手册只写了 iOS / tvOS 的 15 MB）；URL 下载的缓存多久更新；`header` 里写了 `Content-Type` 或 `Content-Length` 时 | L628、L630 |
| 3.18 | 抓包：保存什么、总量与单条的上限、命名会话怎样查看与清除、开关关掉之后已经存的怎样；没开抓包时请求记录里有没有请求头 | L569、L271 |
| 3.19 | 功能开关是否跨重启保留 | L871 |
| 3.20 | 请求记录：一个 HTTP 请求一条（“appear as individual HTTP requests”），承载它们的连接是否另有一条 | L569、L884 |
| 3.21 | HTTP/1.1 的细节：keep-alive、管线化、不改写正文时的 `Expect: 100-continue`、WebSocket（手册只在 URL Rewrite 的 `ws` 里提到） | L562 |
| 3.22 | `http-api-tls = true` 而没有配置 CA 时 | L166 |
| 3.23 | `ca-passphrase` 为空（手册说因为系统的限制不能为空；rurge 的平台没有这个限制） | L579 |
| 3.24 | 旧配置里的 `[Replica]` 节 | 第 2 节第 4 条 |
| 3.25 | 四个重写节与 `[Map Local]` 的一行怎样切成各项：按空白切；`"` 与 `'` 怎样护住空白与引号本身（手册的例子里有 `'…'` 包住的 jq 表达式与 `data="…"`、`header="a:b\|foo:bar"`） | L594、L605、L614、L616、L626 |
