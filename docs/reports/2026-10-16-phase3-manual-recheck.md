# 阶段 3 调研（一）：手册对兼容性清单的逐行重对

日期：2026-10-16。只读调研：没有改仓库里别的任何文件，没有运行任何东西，没有联网。本文是阶段 3（增强模式 / TUN）设计的输入。

## 0. 来源、做法、数字与记号

**来源**

- 手册文本：手册基线重对时取下的手册文本（2026-10-14 取，Surge Mac 6.10.0 / iOS 5.23.0；取法见 `docs/reports/2026-10-14-manual-baseline-recheck.md` 附录 B）。本文读了或扫过其中的 `features/enhanced-mode`、`features/ip-rewrite`、`features/subnet-settings`、`features/gateway`、`features/dhcp`、`dns/overview`、`dns/advanced`、`dns/dns-server`、`dns/encrypted-dns`、`dns/local-dns-mapping`、`profile/general`、`profile/host-list`、`profile/format`、`profile/module`、`rules/process`、`rules/protocol-and-network`、`rules/source-and-port`、`rules/ip`、`rules/overview`、`rules/final`、`rules/domain`、`rules/ruleset`、`policy-groups/subnet`、`policy-groups/overview`、`policy-groups/parameters`、`policies/built-in`、`policies/udp`、`policies/parameters`、`policies/reject`、`policies/external`、`policies/wireguard`、`policies/tailscale`、`getting-started/how-surge-works`、`getting-started/platform-differences`、`scripting/event`、`scripting/api`、`scripting/rule`、`tools/http-api`、`tools/cli`、`tools/logbook`、`http/url-rewrite`、`http/mitm`、`features/port-forwarding`、`features/ponte`、`features/mtproto` 共 40 多页；其余页用 grep 扫过 VIF、fake、198.18、ICMP、gateway、DHCP、hijack、process、ssid、cellular、network change、mtu、privilege 等词。
- 清单：`docs/surge-compatibility-matrix.md`（936 行，仓库 HEAD `4f42735` 的工作区；表格行号即文件行号，下文写作 `L149`，清单一改行号就会移动）。
- 需求：`docs/requirements.md` 第 7 节阶段 3、FR-IN-06～10、FR-OUT-11、FR-GRP-02、FR-RULE-01 / 08、FR-DNS-05 / 08 / 09 与平台矩阵里写阶段 3 的行。
- 上一轮重对：`docs/reports/2026-10-14-manual-baseline-recheck.md`（它自己写明：手册比阶段 3 及以后的行多说的缺省、取值、版本、平台没有收集，每个阶段的设计开始时要把那几节对着清单重读，第 6 节末段）。本文就是这一遍。

**做法**

- 每页从头读到尾。拿每个 `.text` 对着它的 `.html` 逐句比过（去掉行内代码的反引号与空白之后），所有被引用的页都没有被截断，差异只是行内代码周围的空白。`.text` 丢掉了超链接的目标（如 `features/subnet-settings.html` 里的 “see Subnet Expressions” 没有地址）和图片：`getting-started/how-surge-works.html` 有一张架构图（`Surge-Architecture.png`），图里画了什么这里读不到。
- 清单的表格用脚本按列解析（阶段列含 3 的行），再人工逐行对照手册。
- 引文一律是手册英文原文，写作 “…”（页），页名就是手册里的路径，如（`dns/advanced.html`）。每一条引文都用脚本核对过：去掉反引号、折叠空白后必须是该页 `.text` 的子串。引文里的省略用 “…”。
- 凡不是手册原话、而是我从手册推出来的，一律标 **（推断）**。

**数字**

- 清单里阶段列含 3 的行实为 **65 行**，不是 69。按阶段格的写法分：`3` 56 行，`1 / 3` 5 行，`2 / 3`、`3 / 4`、`3 / 6`、`1 / 3 / 4` 各 1 行。按清单的节分：1.2 节 2、2.1 节 8、2.3 节 1、2.4 节 2、3.1 节 3、3.2 节 1、3.4 节 6、4.1 节 3、5.1 节 1、5.2 节 2、6.1 节 2、6.2 节 1、6.3 节 2、6.4 节 9、7.2 节 1、9.1 节 8、9.4 节 5、9.10 节 6、10.3 节 1、10.4 节 1。用别的口径（grep 含 `| 3 |` 的行等）数出过 63、70、71、72、73、75，没有一种是 69；多出来的几行都是附录统计表或阶段格不含 3 的行。为不漏，**附录 A 另列了阶段格不含 3、但行里明说依赖阶段 3 或 VIF 的 67 行**，口径不同的读者可以在那里找到他要的那几行。
- 判定的定义：**一致**＝清单这一行说的手册都支持，手册没有再说设计要知道的东西；**手册多说了**＝清单说的对，但手册还有清单这一行没有的东西（缺省、取值、语义、版本、平台、限制），逐条列出；**不一致**＝清单这一行与手册相冲，或把清单自己的推断写成了手册原话；**手册没有写**＝这一行说的是 rurge 自己的东西，手册不涉及。
- 结果（第 1 节）：一致 39，手册多说了 25，不一致 0，手册没有写 1。**没有一行与手册相冲。**有四处清单把自己的读法写得比手册的话更肯定，已在各行里标出（L266、L334、L712、L741），都不改变判定。

**没有覆盖**

- Surge 的实际行为：一处也没有观察过，「一致」说的是清单与手册的文字一致。
- 手册之外的发行说明：上一轮报告第 3.3 节记了三件手册还没有写到、而落在阶段 3 的 QUIC 识别上的事——`block-quic=adaptive`（`block-quic=auto` 对 DIRECT 之外的策略改用它）、QUIC 主机名识别在 ClientHello 跨几个包时也行、另加 QUIC v2 与 draft-29。这三件不是手册的话，本文没有核对，设计 FR-OUT-11 / FR-RULE-08 时要看。
- 阶段 7 的网关、DHCP、端口转发、Ponte、MTProto、Snell 服务器各页：只读了与阶段 3 相交的部分。
- 图片与链接目标（见上）。

**给设计的要点**（详见各节；T- 是第 2 节的条目，Q- 是第 5 节的疑问）

1. **ICMP 是转发，不是本机应答。**“Surge forwards ICMP packets directly and the VIF returns responses itself”（`features/enhanced-mode.html`）；`icmp-forwarding` 缺省 true，关掉之后 ping 不通，理由是转发会泄漏使用者的 IP。需求 FR-IN-06 写的「ICMP 本地应答」与手册不符；发往 fake 地址的 ping 怎么转发，手册没写（L708、L159、Q-01、附录 B）。
2. **fake-IP 应答器的规格手册基本写全了，清单基本抄全了。**清单漏的是：iOS 的 TTL 是 5 秒；地址族不一致时给空应答；真实解析被推迟、规则只看到原域名；`hijack-dns` 只收 IPv4；没被 `always-real-ip` 豁免的域名其 `[Host]` 映射到真正建连时才生效；SVCB 缺省回 not-implemented 的理由（L543、L544、L151、L152、L149）。
3. **路由模型。**缺省只有默认路由，网卡自己更具体的路由会把一部分流量留在网卡上；`tun-included-routes` 压过 `tun-excluded-routes` 与本地网络的路由；TCP / UDP / ICMP 以外的协议「过不去」；不要把整段私有网段列进 `tun-included-routes`；外部程序的服务器地址从 VIF 路由里排除、关闭增强模式时清理路由表项；L3 策略把 socket 绑在物理网卡上防环（T-01 ～ T-09、Q-02 ～ Q-04、Q-12）。
4. **`[IP Rewrite]` 早于 TCP / UDP 协议栈、规则和策略。**命中的包不成为请求；只管进了 VIF 的流量，所以包要先被路由进 VIF；`reject` 对 TCP 回 RST、对其它包回 ICMP administratively prohibited（L796 ～ L801、Q-26）。
5. **`[SSID Setting]` 与网络变化。**`suspend` 手册只讲了 iOS（隧道不断，Surge 只是不再处理流量），清单和需求里「释放系统代理 / TUN、保留 API」是推断；DNS 覆盖是替换，全局配了加密 DNS 时必须显式写 `encrypted-dns-server=off`；`SUBNET` 只看「出站网卡」的网络；网络变化的八个下游分散在手册各处（L741、L505、L251、第 4.12 节）。
6. 另：`PROCESS-NAME` 的文件名模式与全路径模式支持 `*` 和 `?`，区分大小写，只有 Mac，不支持 `pre-matching`（L242）。

---

## 1. 逐行对照

### 1.1 第 1 节 配置文件格式（L56、L61）

- **L56 `[SSID Setting]`**（1.2 节；✅；阶段 3）——**一致**。
  - 手册：“For compatibility reasons, the subnet settings section is named `[SSID Setting]` in the profile.”（`features/subnet-settings.html`）。节清单里也是 “Subnet settings”（`profile/format.html`）。

- **L61 `[IP Rewrite]`**（1.2 节；✅；阶段 3）——**一致**（语义逐项见 L796～L801）。
  - 手册：“The `[IP Rewrite]` section decides what happens to a packet entering Surge VIF based on its destination address.”（`features/ip-rewrite.html`）。页标题带 “iOS 5.23.0+ Mac 6.10.0+”，清单已写。

### 1.2 第 2 节 `[General]`（L149～L220）

- **L149 `allow-dns-svcb`**（2.1；✅；阶段 3）——**手册多说了**。清单：布尔，默认 false，全部平台，fake-IP 应答器拒绝 type 65 查询。
  - 一致：“Optional, Boolean, default: false”（`dns/advanced.html`）。
  - 多说的：（a）理由：“Such answers can carry IP hints, which would bypass the fake-IP mechanism.”（`dns/advanced.html`）；（b）拒绝的方式与打开之后的意思只有这一句：“By default, Surge rejects these queries as not-implemented, forcing the client to fall back to a standard A record lookup. Enable this option only if you need SVCB/HTTPS records to pass through.”（`dns/advanced.html`）；（c）两页的措辞不一样：`profile/general.html` 写 “The iOS/macOS system might perform an SVCB record DNS lookup instead of a standard A record lookup.”，`dns/advanced.html` 写 “SVCB/HTTPS (type 65)”。类型 64（SVCB）是否同样拒绝，手册没有说（见第 5 节 Q-09）。

- **L151 `hijack-dns`**（2.1；✅；阶段 3）——**手册多说了**。清单：`ip[:port]` 或 `*[:port]` 列表，默认端口 53。
  - 多说的：（a）**只收 IPv4 地址**：“Each entry is an IPv4 address or `*`, with an optional port (default 53).”（`dns/advanced.html`）；`profile/general.html` 的取值栏写 `ip[:port]`，正文同样说 “Each entry is either an IPv4 address (optionally with a port) or `*`; use `hijack-dns = *:53` to hijack all DNS queries.”（`profile/general.html`）；（b）不写这个键时的行为：“By default, only queries sent to the designated Surge DNS addresses are answered with fake IPs; queries sent to a standard DNS server pass through normally.”（`dns/advanced.html`）；（c）只动真正的 DNS 查询：“Only packets that parse as valid DNS queries are hijacked; other traffic to the listed destinations passes through unaffected.”（`dns/advanced.html`；L546 已写「合法 DNS 查询」）；（d）动机：“Some devices or software always use a hardcoded DNS server (for example, Google speakers always use 8.8.8.8).”（`dns/advanced.html`）；（e）“The fake DNS responder listens on `198.18.0.2` (IPv4) and `fd00:6152::2` (IPv6), so even pure IPv6 networks can point their clients to Surge.”（`profile/general.html`）。

- **L152 `always-real-ip`**（2.1；✅；阶段 3）——**手册多说了**。清单：Host List。
  - 多说的：（a）语义：“This option asks Surge to return a real IP address instead of a fake IP address when Surge VIF handles a DNS question. The DNS packet is forwarded to upstream DNS servers.”（`profile/general.html`）；（b）**没被豁免的域名**：“For all other domains, the client still receives a fake IP, and any `[Host]` mapping takes effect later when Surge establishes the real connection.”（`dns/advanced.html`；L547 只写了豁免域名的一半）；（c）用途：“Some scenarios require the client to receive a real, routable IP address instead of a fake one — for example, NAT-type detection for game consoles, or hostnames used by VPN clients.”（`dns/advanced.html`）；（d）模块里用 `%APPEND%` 追加的写法（`profile/module.html` 的 Game Console SNAT 例子：`always-real-ip = %APPEND% *.srv.nintendo.net, …`）。

- **L156 `ipv6-vif`**（2.1；✅；阶段 3）——**手册多说了**。清单：`disabled` `auto` `always`，默认 `disabled`，旧值 `off`（L217）。
  - 多说的：（a）用途：“Allow IPv6 through Surge VIF. Useful when you want Surge to handle raw TCP connections connecting to IPv6 addresses.”（`profile/general.html`）；（b）三个值的意思：“`disabled`: Never set up the Surge VIF with IPv6.”；“`auto`: Only set up the Surge VIF with IPv6 if the local network has a valid IPv6 network.”；“`always`: Always set up the Surge VIF with IPv6. This may break apps on networks without IPv6 connectivity, and Surge warns about it during profile verification.”（均 `profile/general.html`）；（c）`always` 在校验 Profile 时要告警；（d）一个具体理由：“SideStore ignores VPN interfaces with an IPv6 address when looking for this loopback, hence `ipv6-vif = disabled`.”（`features/ip-rewrite.html`）。

- **L157 `tun-excluded-routes`**（2.1；✅；阶段 3）——**手册多说了**。清单：只写「CIDR 列表」。
  - 多说的：（a）取值：“Optional, comma-separated IPv4/IPv6 CIDRs”；（b）只管 VIF：“Notice: This option only works for Surge VIF. Requests handled by Surge Proxy Server aren't affected. Combine `skip-proxy` and `tun-excluded-routes` to make sure that specific traffic bypasses Surge.”；（c）IPv6 的条件：“IPv6 CIDR blocks are supported when IPv6 VIF is enabled.”；（d）为什么要有它：“Surge VIF can only process TCP, UDP, and ICMP protocols. Use this option to bypass specific IP ranges to allow all traffic to pass through.”（以上 `profile/general.html`）；（e）“bypass specific IP ranges from the VIF, letting all traffic in those ranges pass through untouched.”（`features/enhanced-mode.html`）。

- **L158 `tun-included-routes`**（2.1；✅；阶段 3）——**手册多说了**。清单：只写「CIDR 列表」。
  - 多说的：（a）机制：“By default, the Surge VIF interface declares itself as the default route. However, since the Wi-Fi interface has a smaller route, some traffic may not go through the Surge VIF interface. Use this option to add a smaller route.”（`profile/general.html`）；（b）警告：“Avoid listing private ranges (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16) here; it is normally unnecessary and can cause system issues.”（`profile/general.html`）；（c）IPv6：“IPv6 CIDR blocks are supported when IPv6 VIF is enabled.”；（d）“publish additional smaller routes on the VIF so they take priority over interface-local routes.”（`features/enhanced-mode.html`）；（e）用法例：`tun-included-routes = %INSERT% 10.7.0.1/32`（`features/ip-rewrite.html`）。

- **L159 `icmp-forwarding`**（2.1；✅；阶段 3）——**手册多说了**。清单：布尔，默认 true。
  - 多说的：（a）**版本**：页上标 “iOS 5.14.3+ Mac 5.10.0+”（`profile/general.html`），清单这一行没有；（b）理由与代价：“When the enhanced mode is enabled, to reduce interference with users, Surge will directly forward all ICMP packets so as not to affect the use of tools like ping.”；“However, this may lead to IP leakage for some users who are extremely privacy-conscious. Disable this option to stop ICMP forwarding; ping will no longer work through Surge.”（`profile/general.html`）。

- **L176 `udp-priority`**（2.1；🟡；阶段 3）——**一致**。
  - 手册：“When enabled, Surge prioritizes UDP packets when the system load is very high and packet processing is delayed. Also known as game mode.”（`profile/general.html`），“Optional, Boolean, default: true”。清单「高负载下优先处理 UDP，尽力而为」与之相符。

- **L207 `subnet-exp-wifi-always-match`**（2.3；✅；阶段 3）——**手册多说了**。清单：布尔，默认 true（在 macOS 专属表里）。
  - 多说的：（a）语义：“When enabled, SSID/BSSID subnet expression patterns still match even when Wi-Fi is not the primary network interface. Disable it to make these patterns match only when Wi-Fi is the primary interface.”（`profile/general.html`）；（b）仅 Mac：`getting-started/platform-differences.html` 的 Mac 专属键清单里有它（清单已把它放在 2.3 节）。

- **L217 `ipv6-vif = off`**（2.4；✅；阶段 3）——**一致**。手册：“(The legacy value `off` is also accepted.)”（`profile/general.html`）。

- **L220 `use-default-policy-if-wifi-not-primary`**（2.4；✅；阶段 3）——**一致**。手册：“This key replaces the legacy `use-default-policy-if-wifi-not-primary` key, which is accepted and migrated with its meaning inverted.”（`profile/general.html`）。

### 1.3 第 3 节 规则（L242～L298）

- **L242 `PROCESS-NAME`**（3.1；🟡；阶段 3）——**手册多说了**。清单：文件名 / 以 `/` 开头的全路径 / 以 `/` 结尾的前缀三种模式，Mac 6.0+，区分大小写，Mac only；rurge 三平台都做。
  - 一致：“Process rules are available on Surge Mac only; Surge iOS ignores these rules.”；“Matching is case-sensitive in all modes.”；App Bundle 模式的 “Mac 6.0.0+”（均 `rules/process.html`）。
  - 多说的：（a）**通配符**：文件名模式 “Wildcards `*` and `?` are supported. For example: `PROCESS-NAME,Google*`”，全路径模式 “Wildcards `*` and `?` are supported in this mode as well.”，例 `PROCESS-NAME,/Applications/*.app/Contents/MacOS/*`（`rules/process.html`）。清单这一行完全没提通配符；（b）三种模式的分界写得更严：全路径模式是 “If the expression starts with `/` (and does not end with `/`), it matches the executable's full absolute path.”，前缀模式是 “If the expression starts with `/` and also ends with `/`, the executable path is matched by prefix.”；（c）文件名模式 “it matches only the executable file name, regardless of its path.”；（d）不支持预匹配及原因：“Process rules do not support the `pre-matching` parameter, since the originating process is not known at the DNS-query stage.”（L266 的适用表里确实没有它，原因没写）；（e）“Matches the executable of the process that originated the request.”，即匹配的是可执行文件，不是进程显示名（推断：Windows / Linux 上「可执行文件」如何落到这三种模式是 rurge 要定的，手册只讲 Mac）。

- **L249 `PROTOCOL`**（3.1；🟡；阶段 1 / 3）——**手册多说了**（小）。清单的内容（取值表、`TCP` / `UDP` 的涵盖、区分大小写、`DOH*` 等只匹配 Surge 自己的 DNS 连接且需 `encrypted-dns-follow-outbound-mode`）与手册相符（`rules/protocol-and-network.html`）。
  - 多说的：（a）“Due to the existence of multiple draft versions of QUIC, not all QUIC traffic can be recognized by Surge.”；（b）STUN 的用途：“STUN detection is available for filtering P2P traffic; `PROTOCOL,STUN` matches STUN packets specifically so they can be blocked or forwarded.”；（c）“The PROTOCOL rule does not support the `pre-matching` flag.”（均 `rules/protocol-and-network.html`）。

- **L251 `SUBNET`**（3.1；🟡；阶段 3）——**手册多说了**。清单：子网表达式；`TYPE:CELLULAR` / `MCCMNC:` 在桌面永不匹配。
  - 多说的：“Only the network of the outgoing interface is considered: the rule describes the network Surge is currently on, not the destination of the request.”（`rules/protocol-and-network.html`）。“outgoing interface” 指哪一块网卡，手册没有说（第 5 节 Q-14）。`SUBNET` 可以带 `pre-matching`（`rules/overview.html` 的适用表与 `policies/reject.html` 的清单都列了它，L266 已写）。

- **L266 `pre-matching`**（3.2；✅；阶段 1 / 3）——**手册多说了**；另有一处归属问题。清单的适用规则表与手册 `rules/overview.html` 的表一致：“Domain types, IP types, SRC-IP, DEST-PORT, SRC-PORT, SUBNET, CELLULAR-CARRIER, CELLULAR-RADIO, logical rules, RULE-SET, DOMAIN-SET”。
  - 多说的：（a）**版本**：“iOS 5.14.0+ Mac 5.9.0+”（`policies/reject.html`、`rules/domain.html`、`rules/ip.html` 的标记），清单这一行没有；（b）不支持预匹配的类型除 `PROCESS-NAME`、`PROTOCOL` 外还有 `SCRIPT`：“The SCRIPT rule does not support the `pre-matching` flag.”（`rules/script.html`），规则集文件里也不许写：“`FINAL` and the `pre-matching` flag are not allowed inside a rule set file.”（`rules/ruleset.html`）；`RULE-SET` 行本身可以写：“the whole set then participates in the pre-matching phase.”（`rules/ruleset.html`）；（c）手册自己前后不一：“In case of numerous requests, Surge escalates to packet loss. UDP is handled similarly.”（`policies/reject.html`，预匹配一节）与同页 “Since UDP packets have no handshake overhead, there is no pre-matching phase; they are directly matched using the main rule set:”（见第 5 节 Q-08）。
  - **归属问题**：清单写 「出站模式是直连或全局时不评估规则，预匹配随之不生效（手册原话）」。手册原话是 “When the outbound mode is set to Direct or Global instead of Rule-Based, the rule list is not consulted at all.”（`rules/overview.html`）和 “In Direct or Global Proxy outbound mode, the rule system — including FINAL — is bypassed entirely: all traffic goes direct or to the selected global policy.”（`rules/final.html`）。这两句没有提 `pre-matching`；「预匹配随之不生效」是从「规则系统整个被绕过」推出的（推断）。DNS 应答器那一半（`dns/advanced.html`：“enforced by the DNS responder itself”）在直连 / 全局模式下还执不执行，手册没有说。

- **L293 `SSID:<value>`**（3.4；✅；阶段 3）——**一致**。“Matches the Wi-Fi network name (SSID). Wildcard characters `*` and `?` are allowed. The comparison is case-sensitive.”（`rules/protocol-and-network.html`）。

- **L294 `BSSID:<value>`**（3.4；✅；阶段 3）——**一致**。“Matches the Wi-Fi access point MAC address (BSSID). Wildcard characters are allowed. The comparison is case-insensitive.”（`rules/protocol-and-network.html`）。

- **L295 `ROUTER:<ip>`**（3.4；✅；阶段 3）——**一致**。“Matches the default gateway IP address of the current network exactly.”（`rules/protocol-and-network.html`）。

- **L296 `TYPE:WIFI` / `TYPE:WIRED` / `TYPE:CELLULAR`**（3.4；🟡；阶段 3）——**一致**。“Matches all Wi-Fi networks, all wired networks, or all cellular networks respectively. The type keyword is case-insensitive.”（`rules/protocol-and-network.html`）。“wired” 和 “cellular” 在桌面上各指什么，手册不涉及（第 5 节 Q-15）。

- **L297 `MCCMNC:<digits>`**（3.4；🔁；阶段 3）——**手册多说了**（小）。清单：运营商，永不匹配。
  - 多说的：“Matches the mobile carrier of the current cellular network by MCC+MNC. The expression only matches when the device is not on Wi-Fi.”（`rules/protocol-and-network.html`）；另见 `[SSID Setting]` 一页只列了四种形式 “The subnet expression may use the `SSID:`, `BSSID:`, `ROUTER:`, and `TYPE:` forms”（`features/subnet-settings.html`），而 `rules/protocol-and-network.html` 说 “To match the same condition inside a subnet expression (for example in a Subnet Policy Group or an SSID Setting), use the `MCCMNC:` prefix described below.”——两页对 `[SSID Setting]` 认不认 `MCCMNC:` 说法不一（第 5 节 Q-16）。

- **L298 裸值（legacy）**（3.4；✅；阶段 3）——**一致**。“If no prefix is given, the value is compared for compatibility with old profiles: a value containing `*` or `?` is wildcard-matched against the SSID; otherwise it matches the SSID exactly, the BSSID case-insensitively, or the router IP exactly.”（`rules/protocol-and-network.html`）。

### 1.4 第 4 节 出站策略（L332～L334）

- **L332 pre-matching 下的 REJECT 行为（DNS 阶段）**（4.1；✅；阶段 3）——**一致**。
  - 手册（`policies/reject.html`）：“If a REJECT policy is matched, Surge returns a No Record DNS response. If the frequency limit built into the REJECT policy is triggered, the DNS query is discarded without a response.”；“If a REJECT-DROP policy is matched, the DNS query is discarded without a response.”；“If a REJECT-NO-DROP policy is matched, it returns a special IP address 198.18.0.244; Surge generates TCP RST responses for all TCP connections accessing this address.”
  - 同一件事在 DNS 一侧的写法：“Domain-based rules flagged `pre-matching` with a REJECT-family policy are enforced by the DNS responder itself”（`dns/advanced.html`）。`198.18.0.244` 在 `198.18.0.0/15` 里、在 fake 地址池 `198.18.1.1`–`198.19.255.254` 之外（推断：池与这个特殊地址不会冲突）。`REJECT-TINYGIF` 在这个阶段怎么办，手册没有写（第 5 节 Q-10）。

- **L333 pre-matching 下的 TCP 行为**（4.1；✅；阶段 3）——**一致**。
  - 手册（`policies/reject.html`）：“If a REJECT policy is matched, Surge directly generates TCP RST responses; if the frequency limit built into the REJECT policy is triggered, it discards the corresponding TCP SYN handshake packet.”；“If a REJECT-DROP policy is matched, Surge directly discards the TCP SYN handshake packet.”；“If a REJECT-NO-DROP policy is matched, Surge directly generates TCP RST responses.”；“Even for the REJECT-NO-DROP policy, when Surge generates a large number of TCP RST packets in a short period of time (the threshold is 100 times within 3 seconds in the current version), a protection mechanism pauses returning TCP RST and drops packets instead.”

- **L334 UDP 行为**（4.1；🟡；阶段 2 / 3）——**一致**（含一处清单的读法，标推断）。
  - 手册（`policies/reject.html`）：“Since UDP packets have no handshake overhead, there is no pre-matching phase; they are directly matched using the main rule set:”；“If a REJECT policy is matched, an ICMP Administratively Prohibited response is generated. If the frequency limit built into the REJECT policy is triggered, packets are discarded.”；“If a REJECT-DROP policy is matched, packets are directly discarded.”；“If a REJECT-NO-DROP policy is matched, an ICMP Administratively Prohibited response is generated.”
  - 清单括注 「REJECT-NO-DROP ……（没有限频）」：手册没有说 UDP 上的 `REJECT-NO-DROP` 有没有限频；「3 秒 100 次」的保护手册只写在 TCP RST 上（上一行引文）。「没有限频」是清单的读法（推断），不是手册的话。
  - 另见 `[IP Rewrite]` 的 `reject` 对非 TCP 包也回 ICMP administratively prohibited（L798），和这里是同一类包。

### 1.5 第 5 节 策略组（L453、L467、L468）

- **L453 `subnet`（旧名 `ssid`）**（5.1；🟡；阶段 3）——**一致**（小处：版本标记）。
  - 手册（`policy-groups/subnet.html`）：“A `subnet` group automatically selects a policy based on the current network environment, such as the connected Wi-Fi network or the network type.”；“Conditions are evaluated in the declared order and the first match wins; if no condition matches, the `default` policy is used. The result is re-evaluated whenever the network changes.”；“Subnet groups do not support the other common group parameters (`policy-path`, `include-*`, `policy-regex-filter`, `no-alert`, ...); only `hidden`, `icon-url` and `category` are available besides the entries below.”
  - 清单这一行的长备注里已有这三句的读法与 `W0028` 的处理。清单没有的只有版本标记：“Starting from Surge iOS 4.12.0 and Surge Mac 4.5.0, the SSID group was renamed to Subnet Group.”（`policy-groups/subnet.html`）。清单对 `no-alert` 「不告警地收下」，手册把它列在 subnet 组不支持的参数里（清单自己的决定，不是冲突）。

- **L467 `default`（subnet 组）**（5.2；✅；阶段 3）——**一致**。手册：“Required” 与 “The policy used when no subnet expression matches.”（`policy-groups/subnet.html`）。

- **L468 `cellular`（subnet 组）**（5.2；🔁；阶段 3）——**手册多说了**。清单：策略名，已弃用（用 `TYPE:CELLULAR`）。
  - 多说的：“The policy for cellular networks. If set, it takes precedence over the condition entries when the device is on a cellular network. If it is not provided, the condition list and then the default policy apply.”（`policy-groups/subnet.html`）。即设了它，蜂窝网络上先于条件表生效。桌面上「永不处于蜂窝网络」的前提见第 5 节 Q-15。

### 1.6 第 6 节 DNS 与 `[Host]`（L499～L550）

- **L499 缓存**（6.1；✅；阶段 1 / 3）——**一致**。手册：“The cache is flushed automatically on network changes, and can be flushed manually from the UI, the CLI, or the HTTP API.”（`dns/overview.html`）。清单已写「网络切换时清空」并标明没人调用，要等阶段 3 的网络变化检测（FR-IN-09）。

- **L505 `[SSID Setting]` 里的 `dns-server` / `encrypted-dns-server`**（6.1；✅；阶段 3）——**手册多说了**。清单：按网络覆盖。
  - 多说的：（a）覆盖是替换，不是合并：“These replace the global values while connected to that network.”（`dns/dns-server.html`）；（b）全局配了加密 DNS 时，想在这个网络上用传统 DNS **必须显式写 `off`**：“If encrypted DNS is configured in the global DNS settings, you must explicitly set `encrypted-dns-server=off` to use traditional DNS on the matching network.”（`features/subnet-settings.html`）。L517 与 L745 没有写这一条。

- **L517 特殊值 `off`**（6.2；✅；阶段 1 / 3）——**一致**。手册：“The special value `off` disables encrypted DNS. It is mainly useful in per-network overrides: the `[SSID Setting]` section accepts an `encrypted-dns-server` parameter to replace or disable the global value on a specific network.”（`dns/encrypted-dns.html`）。

- **L531 `server:system` / `server:syslib`**（6.3；✅；阶段 1 / 3）——**一致**。手册（`dns/local-dns-mapping.html`）：“In normal (proxy) mode, the lookup is performed by the system resolver library.”；“In enhanced mode, the query stays inside Surge but is forwarded to the DNS servers currently configured in the operating system, since the traditional system resolver might be bypassed.”；“`server:syslib` is an alias of `server:system`; the two values behave identically.”

- **L532 `server:force-syslib`**（6.3；🟡；阶段 3）——**手册多说了**。清单：始终用系统解析库（mDNS 等特殊域名），Mac 6.4.3+。
  - 多说的：警告：“`server:force-syslib` always uses the system resolver library, even in enhanced mode. This is intended for special domains such as mDNS names. Do not use it for general domains, because it may cause recursive requests.”（`dns/local-dns-mapping.html`）。“recursive requests” 的来由手册没写；增强模式下系统解析库的 DNS 服务器就是 Surge 自己的应答器，这是它会递归的原因（推断）。设计要避免系统解析库的查询被应答器再接回自己。

- **L542 应答器监听地址**（6.4；✅；阶段 3）——**一致**。手册（`dns/advanced.html`）：“macOS: `198.18.0.2` (also advertised via DHCP in gateway mode)”；“iOS: `198.18.0.4`”；“IPv6: `fd00:6152::2`”；“Any query sent to an address in the range `198.18.0.2`–`198.18.0.9` is treated as addressed to Surge's DNS responder.”

- **L543 A / AAAA 立即返回 fake IP**（6.4；✅；阶段 3）——**手册多说了**。清单：A / AAAA 立即返回 `198.18.0.0/15` 内的 fake IP，池 `198.18.1.1`–`198.19.255.254`，v6 用 `fd00:6152::` 下的专用前缀，映射持久保存，连接到达时反查域名。以上与手册一致：“Surge remembers the fake IP ⇄ domain mapping persistently.”（`dns/advanced.html`）。
  - 多说的：（a）**真实解析被推迟**：“No DNS latency is added before the connection starts; the real lookup happens only if the matched policy actually needs an IP address.”；（b）规则看到的永远是原域名：“Rules always see the original domain, even for clients that resolve addresses themselves.”；（c）应答器对 “ordinary A and AAAA questions” 不做真实查询：“For ordinary A and AAAA questions, the responder does not perform a real lookup.”（均 `dns/advanced.html`）。（a）的另一半在规则一侧：“If the request targets a domain, evaluation pauses at the first IP-based rule, Surge performs the DNS lookup, and evaluation resumes at the same rule.”（`rules/overview.html`）。哪些策略“actually needs an IP address”手册没有列；推断：DIRECT 要，用域名拨号的代理不要（第 5 节 Q-06）。

- **L544 fake 应答的 TTL 与地址族**（6.4；✅；阶段 3）——**手册多说了**。清单：macOS 30 秒，只返回与查询所用地址族一致的 fake 地址。
  - 多说的：（a）**iOS 是 5 秒**：“Fake answers use a short TTL (5 seconds on iOS, 30 seconds on macOS) so that clients re-query frequently and stale mappings disappear quickly.”；（b）地址族不一致时的应答：“A query only receives fake addresses for the address family it arrived over: an AAAA question arriving over IPv4 gets an empty answer, and vice versa, so clients only get fake IPs they can actually route to Surge.”（均 `dns/advanced.html`）。清单只写了「一致」，没写不一致时给**空应答**（“vice versa”＝A 问题经 IPv6 来也给空应答）。

- **L545 非 A / AAAA 查询转发上游**（6.4；✅；阶段 3）——**一致**。手册：“Queries that are not simple A/AAAA questions (such as TXT or MX) are forwarded to the upstream DNS servers, using the same server configuration as the internal DNS client. Local DNS Mapping `server:` items are honored for forwarded queries as well.”（`dns/advanced.html`）。

- **L546 `hijack-dns`**（6.4；✅；阶段 3）——**手册多说了**，同 L151（只收 IPv4、缺省行为、只劫持合法 DNS 包）。

- **L547 `always-real-ip`**（6.4；✅；阶段 3）——**手册多说了**，同 L152，尤其是没被豁免的域名仍给 fake IP、`[Host]` 映射到真正建连时才生效。

- **L548 `allow-dns-svcb`**（6.4；✅；阶段 3）——**一致**（多出的理由见 L149）。

- **L549 pre-matching REJECT 在应答器层执行**（6.4；✅；阶段 3）——**一致**。手册：“Domain-based rules flagged `pre-matching` with a REJECT-family policy are enforced by the DNS responder itself: a matching query is answered with no record (REJECT), silently dropped (REJECT-DROP), or answered with the special sink address `198.18.0.244` (REJECT-NO-DROP), so the request is blocked before any connection is attempted.”（`dns/advanced.html`）。

- **L550 Firefox canary 域**（6.4；✅；阶段 3）——**一致**。手册：“The responder answers the canary domain `use-application-dns.net` with NXDOMAIN, which tells Firefox to disable its built-in DNS over HTTPS and keep using the system resolver, so that Surge can continue to see the domains being requested.”（`dns/advanced.html`）。

### 1.7 第 7 节（L582）

- **L582 `[MITM]` 的 `auto-quic-block`**（7.2；🟡；阶段 3 / 4）——**手册多说了**。清单：布尔，默认 true，命中 MITM 列表的 QUIC 连接自动阻断以回落 H2 / H1。
  - 多说的：（a）**版本**：页上标 “iOS 5.8.0+ Mac 5.4.0+”（`http/mitm.html`），清单这一行没有；（b）QUIC 连接怎样「命中」主机名列表：“For TLS or QUIC connections identified by the sniffed SNI, a bare entry matches regardless of the connection's actual port.”（`http/mitm.html`）。清单备注 「需解析 QUIC Initial 中的 SNI；仅在 VIF 或 UDP 可见时生效」 是清单的读法（推断），手册没有说「仅在 VIF 或 UDP 可见时生效」。

### 1.8 第 9 节 高级网络功能（L706～L713、L740～L745、L796～L801）

- **L706 创建虚拟网卡并注册为默认路由**（9.1；✅；阶段 3）——**一致**。手册：“Some applications do not follow the system proxy settings. Enhanced Mode ensures Surge handles the traffic of all applications: Surge creates a virtual network interface (VIF) and registers it as the default route, capturing raw traffic regardless of proxy settings.”；“On Surge iOS, the VIF is part of the VPN-based takeover and is enabled by default. On Surge Mac, Enhanced Mode must be started manually.”（均 `features/enhanced-mode.html`）。

- **L707 只处理 TCP / UDP / ICMP**（9.1；✅；阶段 3）——**手册多说了**。
  - 多说的：“The Surge VIF can only process TCP, UDP, and ICMP traffic. Other protocols cannot pass through the VIF, so only enable this feature when necessary.”（`features/enhanced-mode.html`）。清单只写了前半。后半的后果（别的协议「过不去」）和出路（用 `tun-excluded-routes` 把目的地址段放行，见 L157）清单没有写；「过不去」是丢弃还是绕过，手册没说（第 5 节 Q-02）。

- **L708 ICMP**（9.1；✅；阶段 3）——**一致**。清单照手册两句写、并注明「转发出去、再把应答交回，不是在本机伪造应答」。手册：“ICMP traffic cannot be proxied. Surge forwards ICMP packets directly and the VIF returns responses itself, so tools like ping keep working. Privacy-conscious users can disable this behavior with the `icmp-forwarding` option.”（`features/enhanced-mode.html`）。
  - 手册自己有一处措辞偏离：“It handles only TCP, UDP, and ICMP; ICMP cannot be proxied, so the VIF answers it directly.”（`getting-started/how-surge-works.html`），“answers it directly” 单看可读成本地应答。`icmp-forwarding` 一节（`profile/general.html`）的 “this may lead to IP leakage” 只有在包真的发出本机时才说得通，所以实际是转发（推断；见第 4.4 节、第 5 节 Q-01）。需求文档 FR-IN-06 仍写 「ICMP 本地应答」，与手册不符（附录 B）。

- **L709 fake IP `198.18.0.0/15`**（9.1；✅；阶段 3）——**一致**。手册：“While the VIF is active, Surge answers all DNS queries with a virtual (fake) IP address in the `198.18.0.0/15` block. When a connection to a fake IP arrives on the VIF, Surge maps it back to the original domain name for rule matching and establishes the real connection itself.”（`features/enhanced-mode.html`）。“answers all DNS queries” 与 `hijack-dns` 一节的缺省行为（只答发往 Surge 的 DNS 地址的查询）看上去有出入，见第 5 节 Q-05。

- **L710 `tun-excluded-routes` / `tun-included-routes` / `ipv6-vif`**（9.1；✅；阶段 3）——**一致**（逐键的细节见 L156～L158）。手册 “Related [General] Options” 一节（`features/enhanced-mode.html`）列了四个键，第四个 `icmp-forwarding` 清单在 L708 / L159 里。

- **L711 `vif-mode`**（9.1；🔁；阶段 3）——**一致**。手册：“Existing configuration parameters such as `vif-mode` stay in the profile for backward compatibility but no longer affect the runtime behavior.”（`features/enhanced-mode.html`）。该键与 L221 重复（那里阶段 1、同样 🔁）。

- **L712 开关不写在 Profile 中**（9.1；✅；阶段 3 / 6）——**一致**（含一处推断）。手册：“On Surge Mac, Enhanced Mode must be started manually.”（`features/enhanced-mode.html`）；`GET /v1/features/enhanced_mode (Surge Mac Only)`（`tools/http-api.html`）；“Toggles Enhanced Mode.”（`scripting/api.html`，`$surge.setEnhancedModeEnabled(enabled<Boolean>)` Mac Only）；“System Proxy and Enhanced Mode are also available on macOS.”（`tools/cli.html`，`feature` 命令）。
  - 「开关不写在 Profile 中」 手册只对网关模式明说了：“Gateway Mode is enabled from the Surge Mac app UI; the profile does not carry a switch for it.”（`features/gateway.html`）。对增强模式是从 `[General]` 页没有对应的键推出来的（推断）。清单这一行列的入口是 UI / API / CLI，少了脚本 API（L680，阶段 5）。

- **L713 实现基础：Network Extension**（9.1；🟡；阶段 3）——**一致**。手册：“Starting from Surge Mac 5.8.0, Enhanced Mode is powered by Apple's Network Extension framework instead of the legacy utun driver.”（`features/enhanced-mode.html`）。

- **L740 `[SSID Setting]` 的行格式**（9.4；✅；阶段 3）——**一致**。手册：“Subnet settings apply specific parameters only while the device is on a matching network. Each line consists of a subnet expression followed by comma-separated `key=value` parameters.”（`features/subnet-settings.html`）。例子里表达式与参数之间是空格，参数之间是逗号。手册没有说表达式里有空格怎么办（第 5 节 Q-17）。

- **L741 `suspend`**（9.4；✅；阶段 3）——**手册多说了**。清单：匹配网络下暂停；「暂停 = 停止接管并释放系统代理 / TUN，保留 API」。
  - 手册只有一句定义：“Suspend Surge temporarily under the matching networks.”（`features/subnet-settings.html`）。多说的是 iOS 的情形：“On Surge iOS, suspending does not disconnect the VPN tunnel; Surge simply stops handling traffic. If `include-all-networks` is enabled, iOS still forces all traffic into the tunnel, so the device loses network connectivity entirely on the matching networks. Do not combine `suspend=true` with `include-all-networks=true` (and options that depend on it, such as `include-apns`).”（同页）。`profile/general.html` 的 `include-all-networks` 一节同样写 “This option is not compatible with `suspend=true` in Subnet Settings.”
  - **清单的读法**：「停止接管并释放系统代理 / TUN，保留 API」（需求 FR-IN-10 同）手册没有这样说。手册对 Mac 一个字没写；iOS 上的说法是「隧道不断、Surge 只是不再处理流量」（推断：这更像「放行」而不是「释放」）。「保留 API」手册没提。见第 5 节 Q-18。

- **L742 `cellular-fallback`**（9.4；🔁；阶段 3）——**一致**（iOS 专属，🔁 合适）。手册（`features/subnet-settings.html`）：“Control the Wi-Fi Assist and Hybrid Network behavior for the matching networks.”；四个值 `default` / `off` / `wifi-assist` / `hybrid`，`default` 是 “Use the global Wi-Fi Assist and Hybrid Network settings.”

- **L744 `tfo-behaviour`**（9.4；🟡；阶段 3）——**手册多说了**。清单：三个值，「依平台 TFO 支持情况」。
  - 多说的：三个值的意思：“Use the default TFO behavior.”（`auto`）；“Disable TFO for the network completely.”（`force-disabled`）；“Forcibly enable TFO for the network. This option makes Surge ignore the system TFO blackhole detection mechanism.”（`force-enabled`，均 `features/subnet-settings.html`）。「系统的 TFO 黑洞检测」是 `force-enabled` 要绕过的东西。

- **L745 `[SSID Setting]` 的 `dns-server` / `encrypted-dns-server`**（9.4；✅；阶段 3）——**手册多说了**。清单已把逗号歧义写得很清楚（取值是列表，参数之间也是逗号）。
  - 多说的：同 L505（“replace” 与必须显式写 `off`）；取值栏：`dns-server` 是 “Optional, comma-separated IP addresses or `system`”，`encrypted-dns-server` 是 “Optional, comma-separated URLs or `off`”（均 `features/subnet-settings.html`）；例子 `SSID:MyHome dns-server=8.8.8.8,encrypted-dns-server=https://1.1.1.1/`。

- **L796 行的写法与求值**（9.10；✅；阶段 3）——**一致**。“The lines are evaluated from top to bottom, and the first matching line wins. IPv4 and IPv6 addresses are both supported.”（`features/ip-rewrite.html`）；行的形状 `<IP address or CIDR> = <action>` 也在页上。

- **L797 `reflect`**（9.10；✅；阶段 3）——**一致**。“The packet is sent straight back to its sender with the source and destination addresses swapped. To the sender, the address behaves like a peer that loops all its traffic back to the sender itself. In Gateway Mode, a packet from a LAN device is reflected back to that device.”（`features/ip-rewrite.html`）。

- **L798 `reject`**（9.10；✅；阶段 3）——**一致**。“A TCP connection attempt is answered with a TCP RST, and other packets with an ICMP "administratively prohibited" message, so the sender fails immediately instead of waiting for a timeout.”（`features/ip-rewrite.html`）。

- **L799 `drop`**（9.10；✅；阶段 3）——**一致**。“The packet is silently discarded.”（`features/ip-rewrite.html`）。

- **L800 作用的范围与时机**（9.10；✅；阶段 3）——**一致**。“It works at the IP layer: a matching packet is handled as soon as it arrives, before it reaches Surge's TCP/UDP stacks, rules or policies, and it never shows up as a request.”；“This section only applies to traffic handled by Surge VIF, i.e. Enhanced Mode and Gateway Mode. Requests sent to Surge's proxy server are not affected.”（均 `features/ip-rewrite.html`）。

- **L801 路由**（9.10；✅；阶段 3）——**一致**。“The packets must be routed to Surge VIF in the first place. If an address is covered by `tun-excluded-routes` or a local network route, add it to `tun-included-routes`.”（`features/ip-rewrite.html`）。页上的例子（SideStore，`tun-included-routes = %INSERT% 10.7.0.1/32` 加 `10.7.0.1 = reflect`）写明：“`tun-included-routes` makes sure `10.7.0.1` reaches Surge VIF even when it belongs to an excluded or local network range.”（同页）即 `tun-included-routes` 里的项压过 `tun-excluded-routes` 与本地网络的路由；凭什么压过（推断：前缀更具体，例子里是 /32）、两边写了同一个前缀时谁赢，手册没说（第 5 节 Q-03）。

### 1.9 第 10 节 工具与可观测性（L849、L862）

- **L849 rurge 专有：`rurge run -c <path> [--tun] [--system-proxy]` 等**（10.3；阶段 1 / 3 / 4）——**手册没有写**。`--tun` 是 rurge 自己的入口，手册只有 Mac 的 UI、`feature` 命令、HTTP API 与脚本 API 四个开关入口（见 L712）。

- **L862 `GET/POST /v1/features/enhanced_mode`**（10.4；✅；阶段 3）——**一致**。手册：“GET /v1/features/enhanced_mode (Surge Mac Only)”、“POST /v1/features/enhanced_mode (Surge Mac Only)”，GET 应答例 `{"enabled":true}`，POST 请求例 `{"enabled":true}`（`tools/http-api.html`）。

### 1.10 汇总

按本文的定义，65 行的判定如下。

- **一致（39）**：L56、L61、L176、L217、L220、L293、L294、L295、L296、L298、L332、L333、L334、L453、L467、L499、L517、L531、L542、L545、L548、L549、L550、L706、L708、L709、L710、L711、L712、L713、L740、L742、L796、L797、L798、L799、L800、L801、L862。
- **手册多说了（25）**：L149、L151、L152、L156、L157、L158、L159、L207、L242、L249、L251、L266、L297、L468、L505、L532、L543、L544、L546、L547、L582、L707、L741、L744、L745。其中 L266 另有一处归属问题（清单把自己的推断写成了「手册原话」），L546 / L547 与 L151 / L152 是同一件事的两处。
- **不一致（0）**：没有一行的内容与手册相冲。清单把自己的读法写得比手册的话更肯定的四处：L266（「手册原话」）、L334（「没有限频」）、L712（「开关不写在 Profile 中」）、L741（「释放系统代理 / TUN，保留 API」）。
- **手册没有写（1）**：L849。

合计 39 + 25 + 0 + 1 = 65。

---

## 2. 手册里有、清单里没有的

下面每条是与阶段 3 有关的手册陈述，清单里没有对应的行，或最近的行没有写它。「最近的行」给出清单行号。第 1 节里已经按行列过的「手册多说了」不在这里重复展开，只在需要时指回去。

### 2.1 VIF 与路由

- **T-01 别的协议过不去**。“The Surge VIF can only process TCP, UDP, and ICMP traffic. Other protocols cannot pass through the VIF, so only enable this feature when necessary.”（`features/enhanced-mode.html`）；“Surge VIF can only process TCP, UDP, and ICMP protocols. Use this option to bypass specific IP ranges to allow all traffic to pass through.”（`profile/general.html`，`tun-excluded-routes`）。最近的行 L707 只写了 「只处理 TCP / UDP / ICMP」。缺的是后果（“cannot pass through”）与出路（按目的地址段放行）。

- **T-02 路由机制：默认路由 + 更具体的路由**。“By default, the Surge VIF interface declares itself as the default route. However, since the Wi-Fi interface has a smaller route, some traffic may not go through the Surge VIF interface. Use this option to add a smaller route.”（`profile/general.html`，`tun-included-routes`）；“publish additional smaller routes on the VIF so they take priority over interface-local routes.”（`features/enhanced-mode.html`）。最近的行 L158 只写 「CIDR 列表」。要点：缺省只有默认路由，本机网卡自己的更具体的路由会把一部分流量留在网卡上（推断：就是连接的子网那一条）；要进 VIF 就得发布比它更具体的路由。

- **T-03 私有网段警告**。“Avoid listing private ranges (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16) here; it is normally unnecessary and can cause system issues.”（`profile/general.html`，`tun-included-routes`）。清单没有这一条。

- **T-04 `tun-*` 只管 VIF，要和 `skip-proxy` 配合**。“Notice: This option only works for Surge VIF. Requests handled by Surge Proxy Server aren't affected. Combine `skip-proxy` and `tun-excluded-routes` to make sure that specific traffic bypasses Surge.”（`profile/general.html`，`tun-excluded-routes`）。清单没有这一条（L157、L160 都没写）。

- **T-05 IPv6 路由与 `ipv6-vif` 的三个值**。“IPv6 CIDR blocks are supported when IPv6 VIF is enabled.”（`profile/general.html`，两个路由键都有这句）；`ipv6-vif` 的 `auto` / `always` 的意思见 L156。最近的行 L156 只写了取值与默认值。

- **T-06 VIF 默认不接 LAN、不接绕到物理网卡上的应用**（iOS 专属键里透出来的缺省）。“Enable this option to make Surge VIF handle requests sent to LAN. (Requires iOS 14.2 or above.)”（`profile/general.html`，`include-local-networks`，且 “Must be used in conjunction with `include-all-networks=true`.”）；“By default, some requests might not be taken over by Surge. For example, apps can bind to the physical network interface to bypass Surge VIF.”（`profile/general.html`，`include-all-networks`）。最近的行 L196 只列了键名。要点：Surge 自己写明，默认就有流量进不了 VIF——发往 LAN 的，和显式绑定物理网卡的应用的。桌面的 `tun-included-routes` 是否承担 `include-local-networks` 的角色，手册没说（推断：是）。

- **T-07 `compatibility-mode`（iOS）各值的接管方式**。“1: Proxy Takeover + VIF. In this mode, proxy takeover has higher priority than VIF takeover, offering the best performance, but some apps may check for proxy settings and refuse to work.”；“4: Proxy Takeover + VIF, but the proxy uses the VIF address instead of the loopback address.”；“5: VIF Takeover Only, but the VIF routing uses multiple smaller routes for takeover and does not configure a default route. Can be used to bypass some special issues (e.g., HomeKit Secure Camera).”（`profile/general.html`；注意 0 的意思：“0: Auto. In versions of Surge iOS prior to 5.8.0 this is equivalent to 1; from 5.8.0 it is equivalent to 3.”）。最近的行 L188 只写 “0–5”，🔁。模式 5 是「不设默认路由、用多条更小的路由接管」，是个现成的备选路由方案（推断：桌面上的 `tun-included-routes` 自动展开成几条前缀，也是这个意思）。

- **T-08 外部程序 `addresses`、路由清理与进程树**。“A proxy server IP address to be excluded from the VIF routes when Enhanced Mode is on. The parameter can be used repeatedly to append multiple addresses. Hostnames and domains are not supported.”；“Surge always uses the `DIRECT` policy for requests from external processes. Children of external processes are handled the same way for plugin programs such as `obfs-local`.”；“Surge automatically shuts down all external processes when it exits, and automatically cleans up route table items when Enhanced Mode shuts down.”；“The behavior of items 3 and 4 overlaps. Prefer the `addresses` declaration to exclude VIF processing, which reduces processing overhead. Item 4 is an additional protection.”（均 `policies/external.html`）。最近的行 L438 已写 `addresses`（阶段 3）与「外部进程的流量走 DIRECT 到阶段 3」，没有写：关闭增强模式时清理路由表项；「外部进程的子进程同样走 DIRECT」要求按进程树识别；两条保护（排除地址、按进程走 DIRECT）有重叠，前者开销小。

- **T-09 L3 策略的防环办法**。“Direct UDP paths. When the effective underlying policy is DIRECT and `derp-only` is disabled, Surge opens physical UDP sockets bound to the primary physical interface (so Tailscale's own transport packets do not loop back into the Surge virtual interface).”（`policies/tailscale.html`）；“Without it, Surge resolves peer endpoint hostnames with the normal Surge DNS resolver and sends datagrams directly from the selected physical interface.”（`policies/wireguard.html`，`underlying-proxy`）。清单 L355 只说 「不建虚拟网卡、不改路由」，L356 是远期。要点：手册里只有这两处明说「出站 socket 绑物理网卡以免回到 VIF」；对其它协议（TCP 代理、DoH 等）手册没有写 rurge 自己的出站连接怎样不回到 TUN（第 5 节 Q-12）。

- **T-10 三种接管方式与引擎的位置**。“Surge VIF: a virtual network interface that captures raw TCP, UDP, and ICMP traffic from apps that do not honor proxy settings.”；“All captured traffic ultimately flows through it for rule matching and policy execution.”（`getting-started/how-surge-works.html`，后一句说的是 Surge Proxy Server）；同页：“Surge Mac can additionally act as a layer-3 gateway for other devices; see Gateway Mode.”。清单没有这一类「架构」行；L706 只写了 VIF。要点：VIF 收进来的流量最终和代理端口收进来的走同一个引擎。

- **T-11 `skip-proxy` 与 `exclude-simple-hostnames` 在 iOS 上的语义**。“In the iOS version, this option forces connections to these domain/IP ranges to be handled by Surge VIF instead of Surge proxy. In the macOS version, these settings are applied to the system when "Set as System Proxy" is enabled. This option is used to fix compatibility problems with some apps; it is not a way to keep requests off a proxy policy.”（`profile/general.html`，`skip-proxy`）；“Just like the `skip-proxy` parameter. This option lets requests using simple hostnames (without a dot) be handled by Surge VIF instead of Surge proxy.”（`exclude-simple-hostnames`）。L160 / L161 只写了系统代理一侧的语义。

### 2.2 DNS 应答器

- **T-12 fake 应答的 iOS TTL 与不一致地址族的空应答**——见 L544。
- **T-13 真实解析推迟、规则永远看到原域名**——见 L543。
- **T-14 `hijack-dns` 只收 IPv4、缺省行为、只劫持合法 DNS 包**——见 L151。
- **T-15 `always-real-ip` 对其余域名的语义**——见 L152。
- **T-16 `allow-dns-svcb` 的理由与措辞**——见 L149。
- **T-17 应答器「通常」给 fake 地址**：“A DNS responder for the virtual network interface: when Enhanced Mode or the iOS VPN tunnel is active, DNS queries from the system and other devices are answered by Surge directly, normally with fake IP addresses.”（`dns/overview.html`）。 “normally” 说明不是所有查询都给 fake 地址（`always-real-ip`、非 A / AAAA、被 REJECT 的）。清单没有这一句，L542 之后各行把例外分别写了。
- **T-18 `server:force-syslib` 的递归警告**——见 L532。
- **T-19 `[SSID Setting]` 的 DNS 覆盖是替换，全局加密 DNS 要显式 `off`**——见 L505。
- **T-20 CLI 里的 fake-IP 表**。“Query the virtual IP database for a specific IP address or domain.”（`tools/cli.html`，`dump virtual-ip`）；“Show engine memory usage, uptime, active requests, and the sizes of the DNS cache, virtual IP, and temporary rule tables.”（同页，`dump performance`）。清单 L841（阶段 6）列了这两个子命令的名字，没写语义；要点：fake-IP 表可按地址或按域名查，大小是可观测指标。

### 2.3 规则与匹配

- **T-21 `PROCESS-NAME` 的通配符、边界与不支持预匹配的原因**——见 L242。
- **T-22 预匹配的版本与不支持预匹配的类型**——见 L266。
- **T-23 `SUBNET` 只看「出站网卡」的网络**——见 L251。
- **T-24 来自本机的 VIF 流量没有「来源设备」**。“Requests originating from apps on the local device all share the same local source, so `SRC-IP`, `DEVICE-NAME`, and `MAC-ADDRESS` are only meaningful when Surge handles traffic from other devices:”（`rules/source-and-port.html`）。清单 L246～L248 写了 `DEVICE-NAME` / `MAC-ADDRESS` 依赖网关模式，没有写「本机应用共用同一个本地来源」。`IN-PORT` 的定义是 “Matches if the Surge listen port that accepted the request matches.”（同页），VIF 进来的流量没有监听端口，手册没说它们的 `IN-PORT` 是什么（第 5 节 Q-23）。
- **T-25 `extended-matching` 为什么对 VIF 流量有用**。“This helps when a client connects to an IP address directly, so the requested hostname alone would not reveal the destination domain.”（`rules/domain.html`）。L265 说基于 SNI 的匹配随阶段 4；这一句说明增强模式里客户端直连 IP 的流量（没经过 fake-IP 的）要靠它认域名。
- **T-26 `PROTOCOL` 的 QUIC 识别是尽力的、STUN 的用途**——见 L249。
- **T-27 `LAN` 规则集会触发 DNS 解析**。“Matches private and special-purpose IP ranges and the `.local` suffix. Note that this rule set triggers a DNS lookup for domain hostnames (unless `no-resolve` is added to the RULE-SET line).”（`rules/ruleset.html`）。这一条清单已经有：L305 写了 `LAN` 对域名目标先解析、写了 `no-resolve` 就不解析（同手册）。列在这里只为它在增强模式里的后果：一条没带 `no-resolve` 的 `RULE-SET,LAN,DIRECT` 会让每个 fake-IP 域名在评估到它时触发一次真实解析（推断）。

### 2.4 子网与网络环境

- **T-28 `subnet-exp-wifi-always-match` 的语义**——见 L207。
- **T-29 `suspend` 在 iOS 上的行为与 `include-all-networks` 冲突**——见 L741。
- **T-30 subnet 组的 `cellular` 参数先于条件表**——见 L468。
- **T-31 `tfo-behaviour` 三个值的意思与「黑洞检测」**——见 L744。
- **T-32 网络变化的全部下游**：手册在八个地方写了网络变化的后果，清单散在各处，没有一行把它们收在一起，Tailscale 的那一条没有任何行。逐条引文与清单行号见第 4.12 节。
- **T-33 `auto-suspend`（iOS 5.11.0+）**。“Automatically suspend Surge iOS when a network taken over by a Surge Mac gateway is detected.”（`profile/general.html`）。L189 只写了键名与 🔁。它说明「检测到网络已被网关接管」是一种网络状态，iOS 专属，桌面不需要。

### 2.5 清单没有写的版本标记（阶段 3 相关键）

手册页上有、清单对应行没有写版本的（2.1、3.1 等表有「Surge 平台」一栏，别的行在那里写过版本，如 L244 的「iOS 5.8.4 / Mac 5.4.4+」；下列各行写的是「全部」，3.2 与 4.3 的表没有这一栏）：

- `icmp-forwarding`：“iOS 5.14.3+ Mac 5.10.0+”（`profile/general.html`，L159）。
- `proxy-restricted-to-lan`、`gateway-restricted-to-lan`：“iOS 5.13.1+ Mac 5.8.1+”（同页，L162、L163）。
- `block-quic`（全局）：“iOS 5.14.6+ Mac 5.10.3+”（同页，L177）；策略参数 `block-quic`：“iOS 5.8.0+ Mac 5.4.0+”，且 “If this parameter is omitted, proxy policies block QUIC by default, while DIRECT and other built-in policies do not.”（`policies/parameters.html`）后面标 “Mac 6.4.3+”（L374）。
- `auto-quic-block`：“iOS 5.8.0+ Mac 5.4.0+”（`http/mitm.html`，L582）。
- `pre-matching`：“iOS 5.14.0+ Mac 5.9.0+”（`policies/reject.html`、`rules/domain.html`、`rules/ip.html`，L266）。
- `auto-suspend`：“iOS 5.11.0+”（`profile/general.html`，L189）。
- subnet 组的更名：“Starting from Surge iOS 4.12.0 and Surge Mac 4.5.0, the SSID group was renamed to Subnet Group.”（`policy-groups/subnet.html`，L453）。
- 事件脚本 `engine-started`、`profile-reloaded`：“iOS 5.22.0+ Mac 6.9.0+”（`scripting/event.html`，L655 只列了事件名）。

清单已经写对的版本：`[IP Rewrite]` iOS 5.23.0+ / Mac 6.10.0+（L61、9.10 节标题）、`PROCESS-NAME` 前缀模式 Mac 6.0+（L242）、`server:force-syslib` Mac 6.4.3+（L532）、DHCP Mac 6.5+、UDP Fast Path Mac 6.4+、静态地址排除 Mac 6.8.0+（`features/dhcp.html` 标 “Mac 6.8.0+”，9.3 节表没有备注列，未写）。

### 2.6 手册里没有定义、设计只能自己定的（推断：只是我在读过的页里没有找到）

这些在读过的页里一处都没有，grep 过 `mtu`、`utun`、`privilege`、`root`、`administrator`、`idle`、`timeout` 等词：

- VIF 的接口名、MTU、自己的地址（只有 `compatibility-mode=4` 里提到 “the VIF address”，`profile/general.html`）。
- fake-IP 池的大小限制、耗尽后怎么回收、持久化映射的存放与清理（只有 “Surge remembers the fake IP ⇄ domain mapping persistently.”，`dns/advanced.html`，与 CLI 的 `dump virtual-ip`）。
- 回环 / 链路本地 / 组播 / 广播的流量归谁（第 5 节 Q-04）。
- UDP 流的 NAT 映射与空闲超时（FR-IN-08 要的）。手册只有网关 UDP Fast Path 的阈值，和各代理协议自己的空闲时间。
- 启动增强模式需要什么权限、权限不足时怎么提示、异常退出后路由怎么恢复（只有外部程序一页的 “automatically cleans up route table items when Enhanced Mode shuts down”，`policies/external.html`）。
- 把系统 DNS 指向应答器的办法（只有 “the system (or the client devices in gateway mode) is configured to use it”，`dns/advanced.html`）。
- 不让某个应用 / 进程进 VIF 的办法：没有按进程排除的键。能排除的只有目的地址段（`tun-excluded-routes`）、`skip-proxy` 的主机、外部程序的 `addresses`。
- `ipv6-vif = auto` 里 “a valid IPv6 network”（`profile/general.html`）的判据。
- IPv6 fake 地址前缀的长度（只说 “a dedicated prefix under `fd00:6152::`”，`dns/advanced.html`）。
- 数据包捕获：CLI 的 `feature` 命令列了 “Packet Capture”（`tools/cli.html`），没有任何页描述它。

---

## 3. 配置键一览

阶段 3 碰到的每一个配置键、参数、规则写法与常量。「缺省」一栏写手册明说的；手册没有写的写「手册未写」，括号里的「空」是推断。「清单行」是清单里覆盖它的行（括号里是那一行的阶段）；「无」表示清单没有对应的行。引文取最能说明作用的一句，更多的话见第 1、2、4 节。

### 3.1 `[General]` 里的键

| 键 / 写法 | 取值形式 | 缺省 | 手册说它做什么 | 平台 | 清单行 |
| --- | --- | --- | --- | --- | --- |
| `tun-excluded-routes` | 逗号分隔的 IPv4 / IPv6 CIDR | 手册未写（空） | “bypass specific IP ranges from the VIF, letting all traffic in those ranges pass through untouched.”（`features/enhanced-mode.html`） | Mac + iOS | L157、L710（3） |
| `tun-included-routes` | 同上；模块里可用 `%INSERT%` / `%APPEND%` | 手册未写（空） | “publish additional smaller routes on the VIF so they take priority over interface-local routes.”（`features/enhanced-mode.html`） | Mac + iOS | L158、L710、L801（3） |
| `ipv6-vif` | `disabled` / `auto` / `always` | `disabled`；旧值 `off` 也收 | “control whether the VIF is set up with IPv6.”（`features/enhanced-mode.html`） | Mac + iOS | L156、L217、L710（3） |
| `icmp-forwarding` | 布尔 | `true` | “Disable this option to stop ICMP forwarding; ping will no longer work through Surge.”（`profile/general.html`） | iOS 5.14.3+、Mac 5.10.0+ | L159、L708（3）；版本无 |
| `hijack-dns` | 逗号分隔的 IPv4 地址或 `*`，各带可选端口 | 手册未写（空）；端口缺省 53 | “You may use this option to hijack the query to get a fake address.”（`profile/general.html`） | Mac + iOS | L151、L546（3） |
| `always-real-ip` | Host List | 手册未写（空） | “This option asks Surge to return a real IP address instead of a fake IP address when Surge VIF handles a DNS question.”（`profile/general.html`） | Mac + iOS | L152、L547（3） |
| `allow-dns-svcb` | 布尔 | `false` | “So by default, the SVCB record lookup is forbidden to force the system to perform an A record lookup.”（`profile/general.html`） | Mac + iOS | L149、L548（3） |
| `ipv6` | 布尔 | `false` | “Enable full IPv6 support. After enabling this option, Surge queries AAAA records when accessing domain names.”（`profile/general.html`） | Mac + iOS | L155（1） |
| `dns-server` | 逗号分隔的 IP[:port] 或 `system` | 不写则 “Surge uses the system DNS servers.”（`dns/dns-server.html`） | 上游 DNS；`[SSID Setting]` 里同名键整个替换 | Mac + iOS | L145（1）、L505、L745（3） |
| `encrypted-dns-server` | 逗号分隔的 URL，或 `off` | 手册未写（空） | “If encrypted DNS is configured, the traditional DNS will only be used to test the connectivity and resolve the domain in the encrypted DNS URL.”（`profile/general.html`） | Mac + iOS | L146（1 / 2）、L517、L745（3） |
| `skip-proxy` | Host List | 手册未写（空） | iOS：“forces connections to these domain/IP ranges to be handled by Surge VIF instead of Surge proxy.”；Mac：“these settings are applied to the system when "Set as System Proxy" is enabled.”（`profile/general.html`） | Mac + iOS，语义不同 | L160（1）；只写了 Mac 一侧 |
| `exclude-simple-hostnames` | 布尔 | `false` | “This option lets requests using simple hostnames (without a dot) be handled by Surge VIF instead of Surge proxy.”（`profile/general.html`） | Mac + iOS | L161（1）；只写了系统代理一侧 |
| `udp-priority` | 布尔 | `true` | “Surge prioritizes UDP packets when the system load is very high and packet processing is delayed.”（`profile/general.html`） | Mac + iOS | L176（3） |
| `block-quic`（全局） | `per-policy` / `all-proxy` / `all` / `always-allow` | `per-policy` | “This parameter is used to globally override the behavior of whether to block QUIC traffic:”（`profile/general.html`） | iOS 5.14.6+、Mac 5.10.3+ | L177（2）；版本无 |
| `udp-policy-not-supported-behaviour` | `REJECT` / `DIRECT` | `REJECT`（Mac 6.0.0 起） | “The fallback behavior when UDP traffic matches a policy that doesn't support UDP relay.”（`profile/general.html`） | Mac + iOS | L175（2） |
| `proxy-restricted-to-lan`、`gateway-restricted-to-lan` | 布尔 | `true` | “restrict proxy and gateway services to only accept devices from the current subnet.”（`profile/general.html`） | iOS 5.13.1+、Mac 5.8.1+ | L162（1）、L163（7）；版本无 |
| `subnet-exp-wifi-always-match` | 布尔 | `true` | “SSID/BSSID subnet expression patterns still match even when Wi-Fi is not the primary network interface.”（`profile/general.html`） | Mac only | L207、L220（3） |
| `vif-mode`（旧键） | — | — | “stay in the profile for backward compatibility but no longer affect the runtime behavior.”（`features/enhanced-mode.html`） | Mac，5.8.0 起无效 | L711（3）、L221（1） |
| `compatibility-mode` | 整数 0 到 5 | `0` | “This option is used to control the working mode of Surge iOS.”（`profile/general.html`） | iOS only | L188（1，🔁） |
| `auto-suspend` | 布尔 | `true` | “Automatically suspend Surge iOS when a network taken over by a Surge Mac gateway is detected.”（`profile/general.html`） | iOS 5.11.0+ | L189（1，🔁） |
| `include-all-networks` | 布尔 | `false` | “Enable the Include All Networks option to make sure all requests are handled by Surge without leaking.”（`profile/general.html`） | iOS only | L196（1，🔁） |
| `include-local-networks` | 布尔；必须与 `include-all-networks=true` 同用 | `false` | “Enable this option to make Surge VIF handle requests sent to LAN. (Requires iOS 14.2 or above.)”（`profile/general.html`） | iOS only | L196（1，🔁） |
| `include-apns` | 布尔；同上 | `false` | “Enable this option to make Surge VIF handle network traffic for the Apple Push Notification service (APNs).”（同页） | iOS only | L196（1，🔁） |
| `include-cellular-services` | 布尔；同上 | `false` | “Enable this option to make Surge VIF handle internet-routable network traffic for cellular services (VoLTE, Wi-Fi Calling, IMS, MMS, Visual Voicemail, etc.).”（同页） | iOS only | L196（1，🔁） |
| `include-device-communication` | 布尔；同上 | `false` | “Enable this option to make Surge VIF handle network traffic used for communicating with devices connected via USB or Wi-Fi, such as Xcode debugging and Surge Dashboard via USB.”（同页；需要 iOS 17.4） | iOS 5.23.0+ | L197（1，🔁） |

### 3.2 `[SSID Setting]`

写法：每行 “a subnet expression followed by comma-separated `key=value` parameters”（`features/subnet-settings.html`）。表达式的形式见 3.4 节（`SSID:`、`BSSID:`、`ROUTER:`、`TYPE:`、`MCCMNC:`、裸值），清单 L293～L298。

| 键 | 取值形式 | 缺省 | 手册说它做什么 | 平台 | 清单行 |
| --- | --- | --- | --- | --- | --- |
| `suspend` | 布尔 | 手册未写（不暂停） | “Suspend Surge temporarily under the matching networks.”（`features/subnet-settings.html`） | Mac + iOS（页上的补充只讲 iOS） | L741（3） |
| `cellular-fallback` | `default` / `off` / `wifi-assist` / `hybrid` | 手册未写；`default` 的意思是 “Use the global Wi-Fi Assist and Hybrid Network settings.” | “Control the Wi-Fi Assist and Hybrid Network behavior for the matching networks.”（同页） | iOS only | L742（3，🔁） |
| `cellular-mode` | 布尔 | 手册未写 | “Treat the matching networks as metered networks.”（同页） | Mac only | L743（远期，❓） |
| `tfo-behaviour` | `auto` / `force-enabled` / `force-disabled` | 手册未写；`auto` 的意思是 “Use the default TFO behavior.” | “Override the TCP Fast Open behavior for the matching networks.”（同页） | Mac + iOS | L744（3） |
| `dns-server` | 逗号分隔的 IP 或 `system` | 手册未写 | “Override the DNS settings for the matching networks.”（同页） | Mac + iOS | L745、L505（3） |
| `encrypted-dns-server` | 逗号分隔的 URL，或 `off` | 手册未写 | 同上；“you must explicitly set `encrypted-dns-server=off` to use traditional DNS on the matching network.”（同页） | Mac + iOS | L745、L517（3） |

### 3.3 策略组 `subnet`

| 写法 | 取值形式 | 缺省 | 手册说它做什么 | 平台 | 清单行 |
| --- | --- | --- | --- | --- | --- |
| 组类型 `subnet`（旧写法 `ssid`） | `Name = subnet, default = P, <子网表达式> = P, …` | — | “A `subnet` group automatically selects a policy based on the current network environment, such as the connected Wi-Fi network or the network type.”（`policy-groups/subnet.html`） | Mac + iOS | L453（3） |
| `default` | 策略名；必填 | — | “The policy used when no subnet expression matches.”（同页） | 同上 | L467（3） |
| `cellular` | 策略名；已弃用 | 无 | “If set, it takes precedence over the condition entries when the device is on a cellular network.”（同页） | 同上 | L468（3，🔁） |
| `hidden`、`icon-url`、`category` | 见组参数页 | — | 其余通用组参数都不支持 | 同上 | L470、L471、L472（2 / 6） |

### 3.4 规则

| 写法 | 取值形式 | 手册说它做什么 | 平台 | 清单行 |
| --- | --- | --- | --- | --- |
| `PROCESS-NAME,<表达式>,<策略>` | 文件名模式 / 以 `/` 开头且不以 `/` 结尾的全路径模式 / 以 `/` 开头并以 `/` 结尾的前缀模式；前两种支持 `*` `?`；区分大小写 | “Matches the executable of the process that originated the request.”（`rules/process.html`）；不支持 `pre-matching` | Mac only（iOS 忽略这条规则）；前缀模式 Mac 6.0.0+ | L242（3） |
| `SUBNET,<子网表达式>,<策略>` | 同上 | “Rule matches if the current network matches the subnet expression.”（`rules/protocol-and-network.html`） | Mac + iOS | L251（3） |
| `PROTOCOL,<关键字>,<策略>` | `HTTP` `HTTPS` `TCP` `UDP` `QUIC` `STUN` `MTProto` `DOH` `DOH3` `DOQ` `DOT` `DNS`；区分大小写 | “`PROTOCOL,UDP` also matches QUIC and STUN connections.”（`rules/protocol-and-network.html`）；不支持 `pre-matching` | Mac + iOS；`MTProto` iOS 5.21.0+、Mac 6.8.0+ | L249（1 / 3） |
| `IN-PORT` / `SRC-PORT` / `SRC-IP` / `DEVICE-NAME` / `MAC-ADDRESS` | 端口表达式或地址、通配名、MAC | 见 `rules/source-and-port.html`；本机应用共用同一个本地来源 | `SRC-PORT` iOS 5.8.4+、Mac 5.4.4+；`MAC-ADDRESS` Mac 6.1.0+ | L244～L248（1 / 7） |
| 规则参数 `pre-matching` | 标志；顶层规则；策略须为 REJECT 系 | “Rules tagged with `pre-matching` are extracted and checked before everything else”（`rules/overview.html`） | iOS 5.14.0+、Mac 5.9.0+ | L266（1 / 3）；版本无 |
| 规则参数 `extended-matching` | 标志 | “the rule also tries to match the TLS SNI and the HTTP Host header (or `:authority`)”（`rules/domain.html`） | iOS 5.8.0+、Mac 5.4.0+ | L265（1 / 4） |
| 规则参数 `no-resolve`、FINAL 的 `dns-failed` | 标志 | `dns/advanced.html` 的 “Related Options” 一节把它们列为与应答器相关的选项 | Mac + iOS | L263、L264（1） |

### 3.5 `[IP Rewrite]`、`[Host]`、策略行

| 写法 | 取值形式 | 手册说它做什么 | 平台 | 清单行 |
| --- | --- | --- | --- | --- |
| `[IP Rewrite]`：`<IP 地址或 CIDR> = <动作>` | 动作 `reflect` / `reject` / `drop`；自上而下首条命中；IPv4 与 IPv6 | “decides what happens to a packet entering Surge VIF based on its destination address.”（`features/ip-rewrite.html`） | iOS 5.23.0+、Mac 6.10.0+ | L796～L801（3） |
| `[Host]`：`server:system` / `server:syslib` | 键值右侧 | “In enhanced mode, the query stays inside Surge but is forwarded to the DNS servers currently configured in the operating system”（`dns/local-dns-mapping.html`） | Mac + iOS | L531（1 / 3） |
| `[Host]`：`server:force-syslib` | 同上 | “always uses the system resolver library, even in enhanced mode.”（同页） | Mac 6.4.3+ | L532（3） |
| 策略参数 `block-quic` | `auto` / `on` / `off` | “Forwarding QUIC traffic through a proxy may cause performance issues.”（`policies/parameters.html`） | iOS 5.8.0+、Mac 5.4.0+ | L374（2）；版本无 |
| 策略参数 `interface`、`allow-other-interface`、`dns-follow-interface` | 网卡名、布尔、布尔 | “Force the policy to use a specified outgoing network interface”（`policies/parameters.html`） | `dns-follow-interface` iOS 5.15.2+、Mac 5.2.0+；WireGuard 与 Tailscale 不支持 `interface` | L365～L367（2） |
| `external` 策略的 `addresses` | IP 地址；可重复；不收主机名 | “A proxy server IP address to be excluded from the VIF routes when Enhanced Mode is on.”（`policies/external.html`） | Mac only（iOS 把 `external` 当 REJECT） | L438（2；`addresses` 标阶段 3） |
| `[MITM]` 的 `auto-quic-block` | 布尔 | “Surge automatically blocks that QUIC connection, causing the client to fall back to HTTP/2 or HTTP/1.1”（`http/mitm.html`） | iOS 5.8.0+、Mac 5.4.0+ | L582（3 / 4）；版本无 |

### 3.6 开关与可编程入口（不是 Profile 键）

| 入口 | 手册 | 平台 | 清单行 |
| --- | --- | --- | --- |
| UI | “On Surge Mac, Enhanced Mode must be started manually.”（`features/enhanced-mode.html`） | Mac | L712 |
| HTTP API | “GET /v1/features/enhanced_mode (Surge Mac Only)”、“POST /v1/features/enhanced_mode (Surge Mac Only)”（`tools/http-api.html`） | Mac | L862（3） |
| CLI | “System Proxy and Enhanced Mode are also available on macOS.”（`tools/cli.html`，`feature`） | Mac | L839（6） |
| 脚本 | “Toggles Enhanced Mode.”（`scripting/api.html`，`$surge.setEnhancedModeEnabled(enabled<Boolean>)`） | Mac | L680（5） |
| 事件脚本 | “`network-changed`: Triggered when the system network changes. No event data.”（`scripting/event.html`） | Mac + iOS | L655（5） |

### 3.7 常量

| 项 | 值 | 手册 | 清单行 |
| --- | --- | --- | --- |
| fake IPv4 块 | `198.18.0.0/15` | “the reserved block `198.18.0.0/15`”（`dns/advanced.html`） | L543、L709 |
| fake IPv4 池 | `198.18.1.1`–`198.19.255.254` | “IPv4 pool `198.18.1.1`–`198.19.255.254`”（同页） | L543 |
| 应答器地址 | macOS `198.18.0.2`；iOS `198.18.0.4`；IPv6 `fd00:6152::2` | “macOS: `198.18.0.2` (also advertised via DHCP in gateway mode)”（同页） | L542 |
| 视为发往应答器的范围 | `198.18.0.2`–`198.18.0.9` | “Any query sent to an address in the range `198.18.0.2`–`198.18.0.9` is treated as addressed to Surge's DNS responder.”（同页） | L542 |
| IPv6 fake 前缀 | `fd00:6152::` 之下的专用前缀，长度手册没写 | “IPv6 fake addresses use a dedicated prefix under `fd00:6152::`”（同页） | L543 |
| fake 应答 TTL | macOS 30 秒；iOS 5 秒 | “5 seconds on iOS, 30 seconds on macOS”（同页） | L544（只有 macOS） |
| `REJECT-NO-DROP` 的特殊地址 | `198.18.0.244` | “the special sink address `198.18.0.244`”（同页） | L332 |
| canary 域 | `use-application-dns.net` → NXDOMAIN | “answers the canary domain `use-application-dns.net` with NXDOMAIN”（同页） | L550 |
| DNS 缓存上限 | iOS 200 条；macOS 2000 条 | “The cache holds up to 200 entries on iOS and 2000 entries on macOS”（`dns/overview.html`） | L499 |
| REJECT 自动升级阈值 | 30 秒内 50 次（升级为 REJECT-DROP） | “the threshold is 50 times within 30 seconds in the current version”（`policies/reject.html`） | L324 |
| TCP RST 保护阈值 | 3 秒内 100 次 | “the threshold is 100 times within 3 seconds in the current version”（同页） | L333 |
| 网关 UDP Fast Path 阈值 | 1 秒 10 条连接或 10 秒 30 条连接；目的端口小于 1024 不降级 | “10 connections within 1 second or 30 within 10 seconds”（`features/gateway.html`） | L724（7） |

---

## 4. 行为细节

设计必须照办或必须自己定的手册陈述，按主题分。每条给手册原话与页；我的读法一律标（推断）。

### 4.1 VIF 抓什么、不抓什么

- 抓的是不遵守代理设置的应用的原始流量，在 IP 层接进来，交给同一个引擎。“Some apps do not obey system proxy settings (such as Mail.app) because they use raw TCP sockets. The VIF captures this traffic at the IP layer and feeds it into the proxy engine.”（`getting-started/how-surge-works.html`）。
- 只处理 TCP、UDP、ICMP，别的协议「过不去」：见 T-01 的引文。过不去是丢弃还是绕过，手册没说（Q-02）。
- 注册为默认路由，但网卡自己更具体的路由会把一部分流量留在网卡上：见 T-02 的引文。
- 不是所有流量都进 VIF：“By default, some requests might not be taken over by Surge. For example, apps can bind to the physical network interface to bypass Surge VIF.”（`profile/general.html`，`include-all-networks`，iOS）。同一页 `include-local-networks`：“Enable this option to make Surge VIF handle requests sent to LAN.”——所以发往 LAN 的流量缺省不进 VIF（推断：要有这个选项才进得去）；桌面上同样的事靠 `tun-included-routes`（也是推断）。
- 开关：Mac 手动开，iOS 随 VPN 缺省开：“On Surge iOS, the VIF is part of the VPN-based takeover and is enabled by default. On Surge Mac, Enhanced Mode must be started manually.”（`features/enhanced-mode.html`）。
- 实现：Mac 5.8.0 起是 Network Extension，此前是 utun：见 L713 的引文。

### 4.2 路由

- `tun-excluded-routes`：放行的目的地址段，“letting all traffic in those ranges pass through untouched.”（`features/enhanced-mode.html`）。
- `tun-included-routes`：发布更具体的路由，使其“take priority over interface-local routes.”（`features/enhanced-mode.html`）；不要列私有网段：“it is normally unnecessary and can cause system issues.”（`profile/general.html`）。
- 两者重叠时 `tun-included-routes` 赢：“`tun-included-routes` makes sure `10.7.0.1` reaches Surge VIF even when it belongs to an excluded or local network range.”（`features/ip-rewrite.html`）。同一前缀两边都写时怎样，手册没说（Q-03）。
- IPv6 路由只在 IPv6 VIF 开着时有效：“IPv6 CIDR blocks are supported when IPv6 VIF is enabled.”（`profile/general.html`）。
- 外部程序的 `addresses` 从 VIF 路由里排除；关闭增强模式时清理路由表项：见 T-08。
- iOS 的 `compatibility-mode=5` 是不设默认路由、用多条更小的路由接管：见 T-07。
- 对照：WireGuard 策略“does not install a system-wide WireGuard VPN … The device's global route table is not changed.”（`policies/wireguard.html`）。
- 策略 `interface` 参数：“Make sure the interface has a valid route table for the destination address.”（`policies/parameters.html`）；直连别名可以绑到另一个 VPN 的接口，例 `Corp-VPN = direct, interface = utun0`（`policies/built-in.html`）。

### 4.3 fake-IP 与 DNS 应答器

- **何时有应答器**：“When the VIF is active, Surge runs its own DNS responder, and the system (or the client devices in gateway mode) is configured to use it.”（`dns/advanced.html`）。页的范围：“in Enhanced Mode, the iOS VPN tunnel, or gateway mode”。
- **地址**：macOS `198.18.0.2`、iOS `198.18.0.4`、IPv6 `fd00:6152::2`，`198.18.0.2`–`198.18.0.9` 都算发给应答器的；见 3.7 节。
- **分配与映射**：A / AAAA 立即给 `198.18.0.0/15` 里的 fake 地址，不做真实查询；“Surge remembers the fake IP ⇄ domain mapping persistently. When a connection to a fake IP arrives through the VIF, Surge converts it back to the original domain for rule matching and outbound handling.”（`dns/advanced.html`）。
- **真实解析推迟**：见 L543 的引文。规则里遇到 IP 类规则时才解析：“If the request targets a domain, evaluation pauses at the first IP-based rule, Surge performs the DNS lookup, and evaluation resumes at the same rule.”（`rules/overview.html`），“The result is cached, so at most one lookup is performed per request.”（同页）。
- **TTL**：iOS 5 秒、macOS 30 秒，理由 “so that clients re-query frequently and stale mappings disappear quickly.”（`dns/advanced.html`）。
- **地址族**：见 L544 的引文——不一致的族给空应答。
- **转发**：非 A / AAAA 查询转发上游，用与内部 DNS 客户端相同的服务器配置，`[Host]` 的 `server:` 同样生效（L545 的引文）。`[SSID Setting]` 的覆盖替换全局的服务器（L505），所以对转发的查询同样生效（推断）。
- **SVCB / HTTPS**：缺省拒绝（not-implemented）；打开才让它过去（L149）。
- **canary**：`use-application-dns.net` 答 NXDOMAIN（L550）。
- **`hijack-dns`**：让发往写明的 IPv4 地址（可带端口）或全部（`*`）的合法 DNS 查询也得到 fake 地址（L151）。
- **`always-real-ip`**：匹配的查询转发上游并把真实答案交回；`[Host]` 的 IP 映射对它们由应答器权威应答；其余域名仍给 fake 地址，`[Host]` 映射到真正建连时才生效（L152）。
- **`[Host]`**：“Local DNS mapping applies to Surge's internal DNS client and to the fake-IP DNS responder in Enhanced Mode. The hostnames of your proxy servers are never matched against `[Host]` items, to avoid resolution loops.”（`dns/local-dns-mapping.html`）。`server:system` / `syslib` 在增强模式下留在 Surge 里、转给操作系统当前配置的 DNS 服务器；`server:force-syslib` 始终走系统解析库（L531、L532）。
- **预匹配 REJECT 在应答器层**：三种应答（无记录 / 丢弃 / `198.18.0.244`），见 L332、L549。
- **加密 DNS 与应答器的关系**：Surge 自己发出的加密 DNS 连接是另一回事，“By default, encrypted DNS connections always use the DIRECT policy and bypass the rule system.”（`dns/encrypted-dns.html`）；打开 `encrypted-dns-follow-outbound-mode` 才走规则。它们是 Surge 自己建的连接，不经 VIF 抓取这一步（推断；手册没有明说它们怎样避开 VIF，Q-12）。
- **特殊主机名**：简单主机名交系统 DNS 并追加搜索域，`.local` 交系统解析库（`dns/overview.html` 的 Special Hostnames）。应答器遇到这样的名字是给 fake 地址还是转发，手册没有说（Q-11）。
- **可观测**：CLI 的 `dump virtual-ip` 按地址或域名查 fake-IP 表，`dump performance` 给出它的大小（T-20）。

### 4.4 ICMP

- 转发而不是本机伪造：“Surge forwards ICMP packets directly and the VIF returns responses itself, so tools like ping keep working.”（`features/enhanced-mode.html`）。`icmp-forwarding` 一节：“However, this may lead to IP leakage for some users who are extremely privacy-conscious. Disable this option to stop ICMP forwarding; ping will no longer work through Surge.”（`profile/general.html`）。关掉之后 ping 不通：不是「本机应答」，因为本机应答不会因为关掉转发而不通（推断）。
- 另一页措辞偏离：“ICMP cannot be proxied, so the VIF answers it directly.”（`getting-started/how-surge-works.html`）。
- ICMP 不能走代理：“ICMP traffic cannot be proxied.”（`features/enhanced-mode.html`）。所以 ICMP 不经规则与策略（推断）。
- 本机要合成的 ICMP：`reject` 动作与 UDP 的 REJECT 都要回 ICMP “administratively prohibited”（`features/ip-rewrite.html`、`policies/reject.html`）。
- 对照：WireGuard 策略“Surge only answers ICMP/ICMPv6 echo requests sent to the configured local tunnel address.”（`policies/wireguard.html`），那是隧道内部的协议栈，不是 VIF。
- 手册没有写：发往 fake 地址的 ping 怎么办（Q-01）、ICMP 差错（超时、不可达）、traceroute。

### 4.5 UDP 与 QUIC

- UDP 与 TCP 走同一套规则：“Surge processes UDP traffic with the same rule system as TCP. When UDP traffic matches a rule that resolves to a proxy policy, the policy must support UDP relay to forward it.”（`policies/udp.html`）；不支持 UDP 的策略的缺省处理是 REJECT（`profile/general.html` 的 `udp-policy-not-supported-behaviour`，Mac 6.0.0 起）；“The built-in DIRECT and REJECT policy families always handle UDP traffic.”（`policies/udp.html`）。
- UDP 没有预匹配阶段：“Since UDP packets have no handshake overhead, there is no pre-matching phase; they are directly matched using the main rule set:”（`policies/reject.html`；同页另一句相反，Q-08）。UDP 的 REJECT 回 ICMP administratively prohibited，限频后丢包。
- 发往不在 `hijack-dns` 里的 DNS 服务器的查询“pass through normally”（`dns/advanced.html`），即作为普通 UDP 流走规则（推断）。
- QUIC：全局 `block-quic` 四个取值；策略参数 `block-quic` 省略时代理策略阻断、DIRECT 与其它内置策略不阻断（`policies/parameters.html`，“Mac 6.4.3+”）；`[MITM]` 命中主机名的 QUIC 自动阻断（`auto-quic-block`）。识别是尽力的（L249 的引文）。手册没有把 QUIC 的识别限在 UDP 443。
- STUN 的识别是为了过滤 P2P：L249 的引文。
- `udp-priority`：高负载时优先 UDP（L176）。
- 网关模式的 UDP Fast Path 绕过规则与 MITM：“Packets forwarded via the fast path bypass the proxy engine entirely and therefore cannot be matched by rules or MITM.”（`features/gateway.html`，Mac 6.4.0+，阶段 7）。
- 脚本 `$request.protocol` 的取值：“Protocol of the request: `HTTP`, `HTTPS`, `TCP`, `UDP`, `QUIC`, or `STUN`.”（`scripting/rule.html`）。

### 4.6 私有网络与内网

- 缺省不进 VIF 的 LAN：见 4.1。
- `tun-included-routes` 不要列整段私有网段，要把某个私有网段里的单个地址送进 VIF 才写（`features/ip-rewrite.html` 的 `10.7.0.1/32`）。
- 内部规则集 `LAN`：“Matches private and special-purpose IP ranges and the `.local` suffix.”（`rules/ruleset.html`），内容含 `10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`169.254.0.0/16`、`224.0.0.0/4`、`fc00::/7`、`fe80::/10`、回环；“this rule set triggers a DNS lookup for domain hostnames (unless `no-resolve` is added to the RULE-SET line).”
- `skip-proxy` 的样例值包含私有网段与 `localhost`、`*.local`：`skip-proxy = 192.168.0.0/16, 10.0.0.0/8, localhost, *.local`（`profile/general.html` 开头的示例）。
- 简单主机名：“`SIMPLE`: hostname without a dot, such as `localhost`.”（`rules/protocol-and-network.html`）；`exclude-simple-hostnames` 让它们归 VIF 而不是代理。
- `.local` 缺省交系统解析库：L531 / `dns/overview.html`。

### 4.7 排除与例外（汇总）

能把流量留在 Surge 之外的，我在手册里只找到这些（推断：没有别的）：

- 目的地址段：`tun-excluded-routes`。
- 主机：`skip-proxy`、`exclude-simple-hostnames`（要与上一条配合，见 T-04）。
- 外部程序的服务器地址：`addresses`（T-08）。
- 外部程序的流量：一律 DIRECT。
- `[IP Rewrite]` 命中的包：根本不成为请求（4.10）。
- 发给 Surge 代理端口的请求不受 `tun-*` 与 `[IP Rewrite]` 影响：“Requests sent to Surge's proxy server are not affected.”（`features/ip-rewrite.html`）。
- 手册没有按进程 / 按应用排除的键（2.6 节）。

### 4.8 与系统代理的关系

- 三种接管并存：“System proxy settings: apps that honor the system HTTP/SOCKS proxy configuration send requests to Surge's proxy ports.”；“Local proxy ports: any app or device can be pointed at Surge's HTTP or SOCKS5 listener directly, including other devices on the LAN.”；“Surge VIF: a virtual network interface that captures raw TCP, UDP, and ICMP traffic from apps that do not honor proxy settings.”（`getting-started/how-surge-works.html`）。
- `skip-proxy`：iOS 上归 VIF，Mac 上写进系统代理的绕过列表；它“is not a way to keep requests off a proxy policy”（T-11，`profile/general.html`）。要真正绕开 Surge，“Combine `skip-proxy` and `tun-excluded-routes`”（T-04，`profile/general.html`）。
- 系统代理与增强模式的开关互相独立：HTTP API 里是两个端点 `system_proxy` 与 `enhanced_mode`（`tools/http-api.html`）；CLI 的 `feature` 命令两个都管（`tools/cli.html`）。
- 同时开时谁先：只有 iOS 的 `compatibility-mode` 谈到：“Proxy Takeover + VIF. In this mode, proxy takeover has higher priority than VIF takeover, offering the best performance, but some apps may check for proxy settings and refuse to work.”（`profile/general.html`，模式 1；T-07）。Mac 上的缺省怎样，手册没说（推断：设了系统代理的应用走代理端口，其余走 VIF）。
- 端口转发不依赖它们：“This feature works independently, even when Surge request handling (system proxy or enhanced mode) is not enabled.”（`features/port-forwarding.html`，阶段 7）。
- `set-system-socks-proxy`：“When "Set as System Proxy" is enabled, also configure the system SOCKS proxy setting, in addition to the system HTTP/HTTPS proxy settings.”（`profile/general.html`，Mac only）。

### 4.9 与网关模式的关系（阶段 7，但阶段 3 的设计要给它留口）

- 同一个应答器服务网关模式：L542 的 “also advertised via DHCP in gateway mode”（`dns/advanced.html`）；同一个 VIF：`[IP Rewrite]` 适用于 “Enhanced Mode and Gateway Mode”（`features/ip-rewrite.html`）。
- 网关模式没有 Profile 开关：L712 的引文。只接受当前子网的设备：“By default, the gateway service only accepts devices from the current subnet, so a misconfiguration (such as a DMZ setup) does not expose it to the Internet.”（`features/gateway.html`）。
- `reflect` 在网关模式下把局域网设备发来的包送回那台设备（L797）。
- `SRC-IP`、`DEVICE-NAME`、`MAC-ADDRESS` 在本机应用上没有意义（T-24）。
- 网关模式里 IPv6：CLI 的 `vmnet` 报告 “the IPv6 RA takeover state”（`tools/cli.html`）。

### 4.10 与 `[IP Rewrite]` 的关系

- 时机：包一到就处理，“before it reaches Surge's TCP/UDP stacks, rules or policies, and it never shows up as a request.”（`features/ip-rewrite.html`，L800）。所以不进请求记录、不经规则、不经应答器（推断：目的地址是 fake 地址的包也一样，规则里写的是 IP 地址而不是域名）。
- 范围：只管进了 VIF 的流量；包要先被路由进 VIF（L801）。
- 求值：自上而下首条命中，IPv4 与 IPv6 都行；动作三种（L796～L799）。
- 模块可以带这个节：“Rule, Script, URL Rewrite, Header Rewrite, Host, IP Rewrite”，“New lines are inserted at the top of the original content.”（`profile/module.html`）。
- 与 `ipv6-vif`：例子里 IPv6 VIF 关着也能工作，因为地址是 IPv4（L156 的 SideStore 句）。

### 4.11 与 `[SSID Setting]` 的 `suspend` 的关系

- 定义只有一句；iOS 上“suspending does not disconnect the VPN tunnel; Surge simply stops handling traffic”（`features/subnet-settings.html`，L741）。
- 桌面上暂停时 TUN 是撤掉路由还是放行流量、系统代理是否撤销、API 与监听器是否保留：手册都没说（Q-18）。
- 暂停与网络变化联动：`[SSID Setting]` 只在 “the device is on a matching network” 时生效（`features/subnet-settings.html`），所以进出匹配网络就是暂停 / 恢复（推断）。

### 4.12 网络变化时（手册写了的全部下游）

| 下游 | 手册 | 清单行 |
| --- | --- | --- |
| DNS 缓存 | “The cache is flushed automatically on network changes”（`dns/overview.html`） | L499 |
| AAAA 抑制 | “AAAA querying resumes after a network change or a DNS cache flush.”（`dns/overview.html`） | L497 |
| `url-test` / `fallback` 的测试结果 | “A network change also invalidates the result.”（`policy-groups/url-test.html`、`policy-groups/fallback.html`）；“Testing is lazy — Surge retests when a group is used and the previous result has expired or the network has changed.”（`policy-groups/overview.html`） | L449（「没有网络变化触发的重测（阶段 3）」） |
| `subnet` 组 | “The result is re-evaluated whenever the network changes.”（`policy-groups/subnet.html`） | L453 |
| WireGuard | “A network change or an effective underlying-policy change invalidates the sockets and triggers reconstruction.”（`policies/wireguard.html`） | L435（入口已有，探测器在阶段 3） |
| Tailscale | “A network change invalidates old sockets; control and transport state are rebuilt, peer data is refreshed from the new netmap, and always-on sessions warm up again without waiting for traffic.”（`policies/tailscale.html`） | 无（L356 是远期） |
| 事件脚本 | “`network-changed`: Triggered when the system network changes. No event data.”（`scripting/event.html`） | L655、L694（阶段 5） |
| Logbook | “Surge records Logbook entries for events such as profile reloads, network switching, crash recovery, updates, and DHCP-related changes.”（`tools/logbook.html`） | L820（阶段 6） |

另有两处读「当前网络」的地方：URL Rewrite 的占位符 “substituted with the default router address of the current outgoing network interface”（`http/url-rewrite.html`，L594，阶段 4）；脚本的 `$network`，字段为 `wifi`（`ssid`、`bssid`）、`v4`（`primaryAddress`、`primaryInterface`、`primaryRouter`）、`v6`（`primaryAddress`、`primaryInterface`）、`dns`（当前 DNS 服务器数组）（`scripting/api.html`，L667，阶段 5）。

手册没有写：网络变化多久之内恢复（需求文档阶段 3 的验收写的是 30 秒，那是需求自己的数）、怎样判定「网络变了」（接口集合、地址、网关、SSID 哪些算）。

### 4.13 `PROCESS-NAME` 与进程识别

- 三种模式、通配符、区分大小写、只有 Mac：L242。
- 不支持预匹配：“Process rules do not support the `pre-matching` parameter, since the originating process is not known at the DNS-query stage.”（`rules/process.html`）。即在 DNS 应答阶段拿不到进程（推断：增强模式里 DNS 查询来自哪个进程，手册认为那时不知道）。
- 脚本与 CLI 能拿到进程路径：“Path of the process that initiated the request (Surge Mac).”（`scripting/rule.html`，`$request.processPath`）；“All matching attributes — process path, User-Agent, source address, client device, protocol, and more — can be supplied as `key=value` options.”（`tools/cli.html`，`rule match`）。
- 远端设备没有进程信息：“A local Telegram process may provide process metadata. A client connecting from another device cannot provide the remote process identity to Surge.”（`features/mtproto.html`）。
- 外部程序的请求一律 DIRECT，子进程同样：T-08。
- 内部规则集 `SYSTEM` 含 `PROCESS-NAME,trustd`、`PROCESS-NAME,netbiosd`（`rules/ruleset.html`）。
- 计费网络模式按进程放行（Mac，远期）：“only the applications on the allowed list may access the Internet, and connections from other processes are rejected.”（`features/subnet-settings.html`，`cellular-mode`）。

### 4.14 规则匹配在 VIF 流量上

- 连到 fake 地址的连接按原域名匹配；域名规则从不触发 DNS（`rules/domain.html`：“never trigger a DNS lookup”）。
- 连到真实 IP 的连接（没有经过 fake-IP 的）按 IP 字面量匹配：“If the request targets an IP literal, the rule matches it directly; `GEOIP` and `IP-ASN` perform their database lookup without a DNS query.”（`rules/ip.html`）。要认出域名只能靠 `extended-matching`（T-25）。
- 第一个包之前不能决定：“Normally a rule decision happens only after Surge receives the first packet of a connection.”（`rules/overview.html`）。预匹配把 REJECT 类规则提前到 DNS 查询与 TCP 握手阶段。
- `HOSTNAME-TYPE` 把目标分成 `IPv4`、`IPv6`、`DOMAIN`、`SIMPLE`（`rules/protocol-and-network.html`）。fake 地址对应的名字按名字分类（推断）。
- 来源属性（`SRC-IP`、`SRC-PORT`、`IN-PORT`、`DEVICE-NAME`、`MAC-ADDRESS`）对本机 VIF 流量的取值：T-24。
- `PROTOCOL` 对 VIF 流量：`HTTP` / `HTTPS` / `TCP` / `UDP` / `QUIC` / `STUN`，嗅探见 L249 与阶段 4。

### 4.15 Mac 与 iOS 之间手册写明的差异（跨平台实现要选）

| 项 | Mac | iOS | 手册 |
| --- | --- | --- | --- |
| 增强模式开关 | 手动，UI / API / CLI / 脚本 | 随 VPN 缺省开 | `features/enhanced-mode.html`、`getting-started/platform-differences.html` |
| 实现 | 5.8.0 起 Network Extension，此前 utun | “Surge iOS runs as a Network Extension VPN”（`getting-started/platform-differences.html`） | `features/enhanced-mode.html` |
| 应答器地址 | `198.18.0.2` | `198.18.0.4` | `dns/advanced.html` |
| fake 应答 TTL | 30 秒 | 5 秒 | `dns/advanced.html` |
| DNS 缓存条数 | 2000 | 200 | `dns/overview.html` |
| `skip-proxy` / `exclude-simple-hostnames` | 写进系统代理的绕过列表 | 强制归 VIF | `profile/general.html` |
| `suspend` | 手册没写 | 隧道不断，Surge 只是不处理 | `features/subnet-settings.html` |
| `PROCESS-NAME` | 有 | 忽略这条规则 | `rules/process.html` |
| `cellular-mode` | 有（计费网络模式） | 无 | `features/subnet-settings.html` |
| `cellular-fallback`、`compatibility-mode`、`include-*`、`auto-suspend` | 无 | 有 | `profile/general.html`、`features/subnet-settings.html` |
| `subnet-exp-wifi-always-match` | 有 | 无 | `profile/general.html` |
| `external` 策略 | 有 | 当作 REJECT | `policies/external.html` |
| 网关模式 / DHCP / `MAC-ADDRESS` | 有 | 无 | `features/gateway.html`、`features/dhcp.html` |
| HTTP API `enhanced_mode`、脚本 `setEnhancedModeEnabled` | 有 | 无 | `tools/http-api.html`、`scripting/api.html` |

“Platform-specific keys are simply ignored on the other platform, so a single profile can be shared between Surge Mac and Surge iOS.”（`getting-started/platform-differences.html`）。

---

## 5. 疑问

手册含糊、前后不一、或 Mac 与 iOS 写法不同而跨平台实现必须选一边的地方。每条先写手册说了什么，再写含糊在哪，最后写设计要选的（候选一律标推断）。

**Q-01 ICMP：转发还是本机应答，发往 fake 地址的 ping 怎么办。**
- 手册三处措辞：“Surge forwards ICMP packets directly and the VIF returns responses itself, so tools like ping keep working.”（`features/enhanced-mode.html`）；“ICMP cannot be proxied, so the VIF answers it directly.”（`getting-started/how-surge-works.html`）；“However, this may lead to IP leakage for some users who are extremely privacy-conscious.”（`profile/general.html`，`icmp-forwarding`）。需求文档 FR-IN-06 写 「ICMP 本地应答」。
- 含糊：（a）第二句单看是本机应答，与第一句和 IP 泄漏的说法对不上；清单 L708 已选「转发」（推断：这是对的）；（b）`ping example.com` 在增强模式下先拿到 fake 地址，回显请求发到 fake 地址；转发要先把 fake 地址反查成域名、再解析成真实地址，手册没有写；（c）ICMP 差错（超时、不可达，traceroute）手册没有写。

**Q-02 TCP / UDP / ICMP 以外的协议：丢弃还是绕过。**
- 手册：T-01 的两句 “cannot pass through the VIF”（`features/enhanced-mode.html`）与 “to allow all traffic to pass through”（`profile/general.html`）。没有说进了 VIF 而不能处理的包的下场。候选（推断）：丢弃；只有目的地址落在 `tun-excluded-routes` 里才不进 VIF。

**Q-03 `tun-included-routes` 与 `tun-excluded-routes` 谁压谁。**
- 手册只有例子（L801 的引文）：更具体的 `10.7.0.1/32` 压过被排除的 / 本地网络的范围。两边写了同一前缀、或被排除的比被包含的更具体时怎样，没有写（推断：按路由的最长前缀）。

**Q-04 缺省捕获范围：LAN、回环、链路本地、组播、广播。**
- 手册没有直接写。线索：iOS 有 `include-local-networks` 才能让 “requests sent to LAN” 进 VIF；`tun-included-routes` 说 “since the Wi-Fi interface has a smaller route, some traffic may not go through the Surge VIF interface”（`profile/general.html`）。Mac 上的缺省怎样（推断：同 iOS，连接的子网不进）、回环与组播归谁，手册没说。

**Q-05 增强模式页的 “answers all DNS queries” 与 `hijack-dns` 一节的缺省。**
- “While the VIF is active, Surge answers all DNS queries with a virtual (fake) IP address”（`features/enhanced-mode.html`）；“By default, only queries sent to the designated Surge DNS addresses are answered with fake IPs; queries sent to a standard DNS server pass through normally.”（`dns/advanced.html`）。两句靠 「系统 DNS 被指到应答器」 才能都成立（推断）；如果不指，增强模式下系统的 DNS 查询不会得到 fake 地址。

**Q-06 哪些策略 “actually needs an IP address”。**
- “the real lookup happens only if the matched policy actually needs an IP address.”（`dns/advanced.html`）。手册没有列。推断：DIRECT、以 IP 为目标的出站（如 WireGuard 隧道内的路由）要；以域名拨号的代理（`http`、`socks5`、`trojan` 等把域名交给服务器解析的）不要；`use-local-host-item-for-proxy` 打开时按 `[Host]` 的地址拨。

**Q-07 fake-IP 表的寿命、容量与未知地址。**
- 手册只有 “Surge remembers the fake IP ⇄ domain mapping persistently.”（`dns/advanced.html`）和 CLI 的 `dump virtual-ip` / `dump performance`（T-20）。没有写：池耗尽后回收谁、是否跨重启保留（“persistently” 读作跨重启，推断）、连到一个表里没有的 fake 地址时怎么办（推断：连接失败或按 IP 字面量处理）。

**Q-08 预匹配对 UDP：同页两句相反。**
- “In case of numerous requests, Surge escalates to packet loss. UDP is handled similarly.”（`policies/reject.html`，Pre-matching Reject 一节）与 “Since UDP packets have no handshake overhead, there is no pre-matching phase; they are directly matched using the main rule set:”（同页，Technical Details 一节）。清单 L334 取后一句。

**Q-09 SVCB 与 HTTPS 记录。**
- `profile/general.html` 说 “SVCB record”，`dns/advanced.html` 说 “SVCB/HTTPS (type 65)”。类型 64（SVCB）与 65（HTTPS）是否都按 `allow-dns-svcb` 拒绝，手册没有分开讲（推断：都拒绝）。

**Q-10 `REJECT-TINYGIF` 在 DNS / SYN 阶段。**
- 手册 “Pre-matching Technical Details” 只分了 REJECT、REJECT-DROP、REJECT-NO-DROP 三种（`policies/reject.html`）。`REJECT-TINYGIF` 作为 REJECT 系的一员可以带 `pre-matching`，在没有 HTTP 请求的阶段怎么办，没有写（推断：同 REJECT）。

**Q-11 系统 DNS 怎样指到应答器；应答器遇到简单主机名与 `.local`；硬编码 DNS 与 DoH。**
- 手册只写 “the system … is configured to use it”（`dns/advanced.html`）。各平台怎么改（Windows 的接口 DNS 与跃点、Linux 的 resolved / resolv.conf、macOS 的 scutil）没有写。同时手册说 “Surge uses the DNS server addresses from the operating system by default.”（`dns/dns-server.html`）；系统的 DNS 被改成应答器之后，“the operating system” 的服务器指的是改之前的那些（推断），脚本的 `$network.dns` “an array of the current DNS servers”（`scripting/api.html`）在增强模式下是原来的服务器还是应答器地址也没有写。`server:system` 在增强模式下要转给 “the DNS servers currently configured in the operating system”（`dns/local-dns-mapping.html`）——系统 DNS 已经被改成应答器之后，原来的是哪些，要在改之前记下（推断）。简单主机名与 `.local` 在 `dns/overview.html` 里有各自的处理（追加搜索域交系统 DNS；交系统解析库），到了应答器给不给 fake 地址没有写。应用自己的 DoH 不是普通 DNS，`hijack-dns` 劫不到（推断），能看到的只有连到真实 IP 的连接（T-25）。

**Q-12 rurge 自己的出站连接怎样不回到 TUN。**
- 手册只有 L3 策略的两处（T-09），外部程序的两道保护（T-08），与 iOS 上 “apps can bind to the physical network interface to bypass Surge VIF”（`profile/general.html`）。其余出站（代理、DoH、订阅下载、GeoIP 下载、测速）手册没有写。需要设计：绑物理接口、或把服务器地址加进排除路由。清单 L438 / L355 / L356 也没有。

**Q-13 `PROCESS-NAME` 的细节。**
- `*` 是否跨过 `/`：域名通配符手册写了 “It also crosses dots, so `*.example.com` matches `a.b.example.com`.”（`rules/domain.html`），路径通配符没写。
- Windows 上可执行文件的路径与大小写：“Matching is case-sensitive in all modes.”（`rules/process.html`），Windows 的文件系统不分大小写。
- 子进程：只有外部程序一页提到 “Children of external processes are handled the same way for plugin programs such as `obfs-local`.”（`policies/external.html`）。
- 经网关来的流量没有进程：“A client connecting from another device cannot provide the remote process identity to Surge.”（`features/mtproto.html`，同类情形）。
- 清单 L242 已决定 Windows 路径用 `\`、前缀模式按路径归一化。

**Q-14 “outgoing interface”、“primary network interface” 指哪一块。**
- “Only the network of the outgoing interface is considered”（`rules/protocol-and-network.html`）；“even when Wi-Fi is not the primary network interface”（`profile/general.html`）；脚本的 `v4.primaryInterface`、`primaryRouter`（`scripting/api.html`）。多网卡（有线 + 无线 + VPN）时 “the network” 取哪一块、策略写了 `interface` 时 `SUBNET` 看哪一块，手册没有说。

**Q-15 `TYPE:WIRED` / `TYPE:CELLULAR`、subnet 组的 `cellular` 在桌面上。**
- “Matches all Wi-Fi networks, all wired networks, or all cellular networks respectively.”（`rules/protocol-and-network.html`）。Windows / Linux 上手机热点的 USB 共享、蓝牙网络、移动宽带、VPN 接口算哪一类，手册没有写。清单现在把 `TYPE:CELLULAR` 与 `MCCMNC:` 定为永不匹配（L296、L297）。

**Q-16 `MCCMNC:` 能不能写在 `[SSID Setting]` 里。**
- `features/subnet-settings.html` 只列四种形式（`SSID:`、`BSSID:`、`ROUTER:`、`TYPE:`）；`rules/protocol-and-network.html` 说在 `[SSID Setting]` 里也用 `MCCMNC:`。桌面上永不匹配，只影响解析是否报错。

**Q-17 `[SSID Setting]` 的行怎样拆；多行同时命中怎样。**
- 表达式与参数以空格分开，参数以逗号分开，列表值也以逗号分开（L745 已记）。还有：SSID 里有空格、逗号、等号怎么写（`rules/overview.html` 只对规则的值说了加引号；`[SSID Setting]` 与 subnet 组的条目 `SSID:My Home = P` 没说）；BSSID 里有冒号；多行同时匹配当前网络（如 `TYPE:WIFI` 一行加 `SSID:MyHome` 一行）时，是首行命中、逐键合并还是都生效，手册只对 subnet 组说了 “the first match wins”（`policy-groups/subnet.html`），对 `[SSID Setting]` 没说。

**Q-18 `suspend` 在桌面上的含义。**
- 见 L741。候选（推断）：撤掉 TUN 路由与系统代理设置、保留 API 与监听器（需求 FR-IN-10 的读法）；或像 iOS 那样路由留着、引擎直通。手册只给了 iOS 的描述。

**Q-19 Mac 与 iOS 的缺省值差异：选哪一边。**
- fake 应答 TTL（30 / 5 秒）、应答器地址（`198.18.0.2` / `198.18.0.4`）、DNS 缓存条数（2000 / 200）：见 4.15 节。清单 L542、L544、L499 已取桌面（Mac）的值。

**Q-20 `skip-proxy` / `exclude-simple-hostnames` 在 rurge 里属于哪一边的语义。**
- iOS：归 VIF；Mac：写进系统代理的绕过列表（T-11）。rurge 的桌面上两者可以同时有：绕过系统代理的主机，其流量落到默认路由，仍进 TUN（推断），所以两种语义在「增强模式开着」时的效果相同；增强模式关着时只有 Mac 的语义。

**Q-21 `ipv6-vif = auto`、与 `ipv6`、与 AAAA fake 地址。**
- `auto` 的判据 “if the local network has a valid IPv6 network”（`profile/general.html`）；网络变化时是否重新判断，手册没写。`ipv6 = false` 而 VIF 有 IPv6 时 AAAA 查询怎么答，手册没写；`dns/advanced.html` 只说地址族要和查询所用的族一致。

**Q-22 UDP：会话寿命、`udp-priority` 的机制、QUIC 识别范围。**
- 手册没有写 UDP 流的空闲超时（FR-IN-08 要的）；`udp-priority` 只有一句 “prioritizes UDP packets when the system load is very high”（`profile/general.html`，机制没写）；QUIC 识别是 “not all QUIC traffic can be recognized”（`rules/protocol-and-network.html`），没有限定端口（清单与需求限在 UDP 443，是 rurge 的决定）。

**Q-23 来自 VIF 的流量，`IN-PORT`、`SRC-PORT`、`SRC-IP` 取什么。**
- “Matches if the Surge listen port that accepted the request matches.”（`rules/source-and-port.html`，`IN-PORT`）；VIF 流量没有监听端口。`SRC-PORT` 是客户端源端口，对本机应用就是它的本地端口；`SRC-IP` 是本机地址（T-24）。手册没说 `IN-PORT` 对 VIF 流量是什么（推断：不匹配）。

**Q-24 `[SSID Setting]` 的 DNS 覆盖管到哪里。**
- “These replace the global values while connected to that network.”（`dns/dns-server.html`）。它是否也管应答器转发出去的查询、`[Host]` 里 `server:` 指定的上游、以及加密 DNS 的引导解析：手册只说替换全局值。

**Q-25 启动、退出、崩溃时的路由与 DNS 清理。**
- 手册只有一句 “automatically cleans up route table items when Enhanced Mode shuts down”（`policies/external.html`，讲外部程序）。崩溃后残留的路由与系统 DNS 设置怎么恢复、权限不足怎么提示（FR-IN-06）都没有写。

**Q-26 `[IP Rewrite]` 与 fake 地址。**
- 规则写的是 IP 地址。目的地址是 fake 地址的包会不会被覆盖 `198.18.0.0/15` 的行命中，手册没有写（推断：会，因为 `[IP Rewrite]` 在 “before it reaches Surge's TCP/UDP stacks, rules or policies”（`features/ip-rewrite.html`），早于 fake 地址反查）。
- 页上 “IPv4 and IPv6 addresses are both supported”（`features/ip-rewrite.html`）与 `ipv6-vif = disabled` 的关系：IPv6 行在 IPv6 VIF 关着时不会被命中（推断）。

**Q-27 出站模式为直连 / 全局时，应答器与预匹配照不照常。**
- 手册：“In Direct or Global Proxy outbound mode, the rule system — including FINAL — is bypassed entirely: all traffic goes direct or to the selected global policy.”（`rules/final.html`）。没有说这时应答器还给不给 fake 地址、DNS 一侧的预匹配 REJECT 还执不执行（L266）。fake 地址的映射本身与规则无关，所以直连 / 全局模式下仍给 fake 地址、连接到来时反查域名再直连或走全局策略是最少改动的读法（推断）。

**Q-28 增强模式运行中被打开 / 关闭时，已有的连接与 fake 地址怎么办。**
- 手册只有 HTTP API、CLI、脚本里的开关（L712、L862），没有写切换的过程：关闭后客户端手里还缓存着的 fake 地址、已建立的连接、系统 DNS 的恢复（Q-11、Q-25）。

---

## 附录 A. 阶段格不含 3、但行里明说依赖阶段 3 或 VIF 的清单行

按清单的节排。「手册」一栏写手册是否在这一行之外还有东西（指回上文）。

| 行 | 首格 | 阶段 | 与阶段 3 的关系 | 手册 |
| --- | --- | --- | --- | --- |
| L34 | 有序节 | 1 | `[IP Rewrite]`、`[SSID Setting]` 被列为有序节 | 一致：`profile/format.html` 的 line-based 清单里有这两个 |
| L113 | 模块可覆盖的节 | 5 | 模块可以给 `[IP Rewrite]` 加行（插入顶部） | 一致（`profile/module.html`） |
| L160、L161 | `skip-proxy`、`exclude-simple-hostnames` | 1 | 写了系统代理一侧的语义 | 多了 iOS 的 VIF 语义（T-11） |
| L162、L163 | `proxy-restricted-to-lan`、`gateway-restricted-to-lan` | 1、7 | 网关与监听的来源限制 | 多了版本 “iOS 5.13.1+ Mac 5.8.1+”（2.5 节） |
| L175 | `udp-policy-not-supported-behaviour` | 2 | TUN 上的 UDP 流同样适用 | 一致 |
| L177 | `block-quic`（全局） | 2 | 备注 「TUN 在阶段 3」 | 多了版本（2.5 节） |
| L188、L189 | `compatibility-mode`、`auto-suspend` | 1 | iOS 的接管方式 | 多了各值的语义（T-07、T-33） |
| L196、L197 | `include-*` | 1 | iOS 的 VPN 范围 | 多了语义；透露缺省 LAN 不进 VIF（T-06） |
| L204 | `socks5-listen` | 1 | 备注：SOCKS5 回不了 ICMP；TUN 上才能回 | 一致 |
| L221 | 旧键 `vif-mode` 等 | 1 | 与 L711 重复 | 一致 |
| L245、L246、L247、L248 | `IN-PORT`、`SRC-IP`、`DEVICE-NAME`、`MAC-ADDRESS` | 1、7 | VIF 流量的来源属性 | 多了 T-24；Q-23 |
| L252、L253 | `CELLULAR-RADIO`、`CELLULAR-CARRIER` | 1 | iOS 专属，永不匹配；可带 `pre-matching` | 一致 |
| L254 | `AND` / `OR` / `NOT` | 1 | 子规则里的 `PROCESS-NAME`、`SUBNET` 在阶段 3 前永不匹配 | 一致 |
| L263、L264 | `no-resolve`、`dns-failed` | 1 | `dns/advanced.html` 的 “Related Options” 列了它们 | 一致 |
| L265 | `extended-matching` | 1、4 | 备注：Host 头与目标不同的流量要等阶段 3 | 多了 T-25 |
| L305 | 内部集 `SYSTEM`、`LAN` | 1 | `PROCESS-NAME` 行要阶段 3 才匹配 | 一致（`LAN` 先解析、`no-resolve` 就不解析，L305 已写；增强模式里的后果见 T-27） |
| L335 | 被预匹配拒绝的规则 5 分钟内只在最近请求列表出现一次 | 4 | 这是预匹配（阶段 3）的观测面；阶段栏疑似错放（清单内部的问题，手册 “each rule only appears once in the recent request list every 5 minutes”，`policies/reject.html`） | 一致 |
| L355、L356 | `wireguard`、`tailscale` | 2、远期 | 不碰路由；网络变化重建；防环 | 多了 T-09 与 4.12 节 |
| L365、L367 | `interface`、`dns-follow-interface` | 2 | 绑定物理网卡 | 一致；4.2 节 |
| L374 | `block-quic`（策略） | 2 | TUN 上的 QUIC | 多了版本（2.5 节） |
| L401、L402、L403、L404 | UDP 中继四行 | 2 | UDP 流水线 | 一致 |
| L435 | WireGuard 生命周期 | 2 | 「网络已变化」的探测器在阶段 3 | 一致；4.12 节 |
| L438 | `external` | 2 | `addresses`、外部进程走 DIRECT 要阶段 3 | 多了路由清理与重叠保护（T-08） |
| L449 | `url-test` | 2 | 「没有网络变化触发的重测（阶段 3）」 | 一致；4.12 节 |
| L455 | 临时覆盖 | 2 | `subnet` 组的覆盖随阶段 3 | 一致 |
| L497 | AAAA 抑制 | 1 | 网络变化时解除 | 一致 |
| L501 | `.local` | 1 | 带作用域号的 IPv6 地址要连接器支持（阶段 3） | 一致 |
| L519 | `encrypted-dns-follow-outbound-mode` | 1 | 会话带主机名随阶段 3 的会话模型 | 一致 |
| L526 | `[Host]` | 1 | 作用于 fake-IP 应答器 | 一致 |
| L594 | URL Rewrite 的 `{{{GATEWAY_ADDRESS}}}` | 4 | 要读当前出站网卡的默认路由器 | 一致 |
| L655、L667 | `event-name`、`$network` | 5 | 网络变化事件与网络环境字段 | 多了版本（2.5 节） |
| L680 | `$surge.setEnhancedModeEnabled` | 5 | 增强模式的脚本开关 | 一致 |
| L692 | `rule` 脚本类型 | 5 | `processPath` 要阶段 3 的进程识别 | 一致 |
| L719～L725 | 网关模式七行 | 7 | 网关模式与 VIF 共用 | 一致 |
| L731～L734 | `[DHCP]` | 7 | 通告应答器地址 | 一致 |
| L751 | 端口转发 | 7 | 不依赖增强模式 | 一致 |
| L820 | Logbook | 6 | 网络切换事件 | 一致 |
| L839、L841、L843 | CLI 的 `feature`、`dump virtual-ip` 等、`vmnet` | 6、7 | 增强模式开关、fake-IP 表、网关接口 | 多了语义（T-20） |
| L870 | `GET/POST /v1/policy_groups/select` | 2 | `subnet` 组仍 400，随阶段 3 | 一致 |
| L879 | DNS 端点 | 1 | 手动清缓存的入口之一；网络变化时自动清空见 L499 | 一致（`dns/overview.html`：可从界面、CLI、HTTP API 清空） |
| L883 | 设备管理端点 | 7 | 网关模式 | 一致 |

---

## 附录 B. 需求文档里与手册对不上或手册没有依据的句子

只列阶段 3 相关的。需求文档没有改动，这里只记差异。

- **FR-IN-06**：「ICMP 本地应答与 `icmp-forwarding`」。手册是转发出去、再把应答交回（Q-01）。
- **FR-IN-10**：「`suspend` 时停止接管并释放系统代理与 TUN，API 保持可用」。手册对桌面没有写；iOS 是隧道不断（L741、Q-18）。
- **FR-RULE-01**：「`PROCESS-NAME` 三平台可用（文件名 / 全路径 / 前缀三种模式，Windows 路径归一化）」。手册多了通配符（L242）。
- **FR-OUT-11**：「增强模式下对 UDP 443 生效」。手册没有把 QUIC 限在 443（Q-22）。
- **FR-DNS-08**：「TTL 30 秒」。这是 macOS 的值，iOS 是 5 秒（L544）。
- **FR-DNS-05**：`force-syslib`。手册多了递归警告（L532）。
- **FR-DNS-09**：`[SSID Setting]` 的 DNS 覆盖。手册多了 “replace” 与必须显式写 `off`（L505）。
- **FR-IN-08**：UDP 会话表的超时回收。手册没有数（Q-22）。
- **FR-IN-09**：网络变化的下游（缓存清理、重测、`network-changed` 脚本、WireGuard 重建）与手册一致；手册另有 Tailscale 重建、`subnet` 组重算、AAAA 解除、Logbook（4.12 节）。阶段 3 验收的 「网络切换后 30 秒内恢复」 是需求文档自己的数。
- **平台矩阵**：「Wi-Fi 信息（SSID / BSSID）」、「网络变化事件」、「虚拟网卡」、「进程识别」 各行的实现方式手册都不涉及（Mac 的实现是 Network Extension，rurge 另行选择）。
