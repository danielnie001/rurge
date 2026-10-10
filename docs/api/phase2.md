# rurge HTTP API（阶段 2）

阶段 2 / M1（装配与控制面，见 `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md` 第 6.6 节）新增四个策略 / 策略组端点；阶段 2 / M3b（测速与自动组）又新增三个测试端点，并让 `POST /v1/policy_groups/select` 对自动组设临时覆盖（见「连通性测试与自动组（M3b）」一节）；阶段 2 / M3c 让这些端点也接纳 `smart` 组，并给请求记录加了两列耗时（见「`smart` 组（M3c）」一节）；阶段 2 / M7c 起，`encrypted-dns-follow-outbound-mode` 下 DoQ / DoH3 上游的 QUIC 载体也作为内部会话进请求记录（见末节）。阶段 2 / M8c 没有新增端点，也没有改响应的形状；变的是脱敏（`policies/detail` 与 `lineHash`）、几种出站失败时 `error` 的文本、`smart` 组怎样计失败，以及内部 DNS 会话的 `error`，各在相应的小节里以"M8c"标出。鉴权（`X-Key` 头 / `?x-key=` 查询参数、常量时间比较、封禁）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录本页新增端点自己的形状。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/policies/detail?policy_name=<name>` | | `{"<name>": "<脱敏后的定义>"}`；未知策略 → 404 |
| GET | `/v1/policy_groups` | | `{"<组名>": [Member…], …}` |
| GET | `/v1/policy_groups/select?group_name=<name>` | | `{"policy": "<当前生效的成员>"}`；未知组 → 404 |
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效，或是 `subnet` 组 → 400；对自动组即临时覆盖（M3b；`smart` 组 M3c 起） |
| POST | `/v1/policies/test` | `{"policy_names":["<name>",…],"url":"<可省略>"}` | `{"<name>": Result…}`；未知策略、`url` 不是 URL → 400（M3b） |
| GET | `/v1/policy_groups/test_results` | | `{"<组名>": {"<成员>": Result 或 null}}`（M3b） |
| POST | `/v1/policy_groups/test` | `{"group_name":"<name>"}` | `{"available":["<成员>",…]}`；未知组 → 400（M3b） |

**"暂定"标注**：手册没有给出 `/v1/policies/detail`、`/v1/policy_groups`（M1 设计 O2）与 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`（M3 设计 6.6）的响应示例；下面的形状按社区已知的 Surge 响应实现，拿到真实 Surge 实例的样本后再对齐，届时可能是破坏性变更。

## `GET /v1/policies/detail`

参数 `policy_name`（必填，查询参数）。

响应是 `{"<policy_name>": "<value>"}`：`value` 是该策略或组**脱敏后的定义**（不含 `名字 =` 前缀）；内置策略（`DIRECT` `REJECT` 等）的值是它自己的名字。脱敏用的是 `GET /v1/profiles/current` 那套规则里管策略定义的那一半（`docs/api/phase1.md`「脱敏后的配置」一节；整行键与注释的几条在这里用不上），同样是尽力而为，不是"没有秘密会出现"的保证：

- **行内参数**：名单里的 24 个参数（`password` `psk` `base64` `headers` `ws-headers` `ws-path` `shadow-tls-password` `policy-path` `test-url` `args` 等，全表见 `docs/api/phase1.md`）的取值换成 `***`。参数照解析器的读法认（M8c）：整项写在引号里的 `"policy-path=…"` 显示成 `policy-path=***`；读不成 `name=value` 的项再按名字搜索一遍，名字后面跟的是冒号、或隔着一个收尾的引号也认。取值的结尾按解析器自己的规则找第一个顶层逗号——引号可以在值里任何位置打开、括号会分组，所以 `password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 都是整个值变成 `***`；`headers=` 与 `ws-headers=` 的值整体变成 `***`（连 header 名也不保留），`ws-path=` 同样。
- **位置凭据**：类型带服务器与端口的定义（`trojan` `vmess` `ss` `anytls` 等都在内），以及第三个字段是端口号的任何定义，第 4 个字段起凡不是 `name=value` 具名参数的字段换成 `***`；类型后面漏了逗号的定义，拆开之后第三个字段是端口号时同样处理（M8c）。
- **URL**：任何位置的 URL 里，用户名口令换成 `***`（`https://***@host/…`）；scheme 不是 `http` `https` `h3` `quic` `tls` `tcp` 之一的 URL，从 `://` 之后到下一个空白整段换成 `***`（M8c；scheme 指 `://` 前面紧挨着的那串字母、数字、`+`、`-`、`.` 里从第一个字母起的部分，前面没有这样一串的正则不算）。

```json
{"HK": "http, proxy.example.com, 8080, ***, ***"}
```

```json
{"DIRECT": "DIRECT"}
```

失败：`policy_name` 不是任何已知策略、组或内置名 → 404

```json
{"error": "unknown policy `HK-typo`"}
```

## 会话日志里的出站错误文本

trojan、vmess（± WebSocket）、anytls 出站，任何带 Shadow TLS 的出站，以及（M8c 起列出的）`http` / `socks5` 的握手与另外几种出站失败时，会话记录的 `error` 是下面按协议分节列出的固定文本之一；其余失败仍按连接失败的通用形式出现（拨号超时 `connect timed out`、TLS 握手失败 `tls: <原因>`、或底层 I/O 错误的原文——涵盖 TCP 连接失败、WebSocket 握手期间的 I/O 错误，以及已建立的连接在转发期间的 I/O 错误：后者经转发循环的失败文本进入会话记录）。对端给的字节（HTTP 响应头、WebSocket 握手响应体、anytls 的错误帧等）永不原样出现在下列固定文本中。

### trojan

| 错误文本 | 何时出现 |
| --- | --- |
| `trojan: the host name cannot be sent to the server` | 目标主机名转不成可发送的 ASCII 形式（如含 `@` 这类可能被下游误读成 authority 分隔符的字符）；连接不会被拨出 |
| `trojan: the host name is longer than 255 bytes` | 目标主机名（转换后）超出 trojan 地址编码一字节长度所能表示的范围；连接不会被拨出 |

`trojan` 的密码错误没有对应的错误文本：协议没有应答，服务端把连接交给它的回落站点，连接建立成功、会话记录 `error` 为空，异常只在转发阶段表现为对端提前关闭或回一段与预期不符的数据（见 `docs/surge-compatibility-matrix.md` 4.2 `trojan` 行）。

### vmess

| 错误文本 | 何时出现 |
| --- | --- |
| `vmess: the server closed the connection without answering` | 对端在应答的长度头到达之前就关闭了连接：UUID 错，或本机时钟与服务端偏差超过协议容忍的约 120 秒；两者表现相同，分辨不出 |
| `vmess: the response cannot be authenticated` | 应答的长度头或头本体未能通过 AEAD 认证——对端返回的字节不是针对这次请求密封的合法 VMess 应答（例如它根本不是 VMess 服务端） |
| `vmess: the response head is longer than the protocol allows` | 应答头声明的长度超过协议上限（4 + 255 字节） |
| `vmess: the response does not answer this request` | 应答头认证通过，但其中的校验字节 `V` 与这次请求送出的不一致 |
| `vmess: the connection ended in the middle of a chunk` | 读应答头本体或某个分块时连接被对端关闭，且不是在两个分块之间的边界上 |
| `vmess: a chunk shorter than its tag` | 分块声明的长度小于 AEAD tag（16 字节） |
| `vmess: a chunk cannot be authenticated` | 分块未能通过 AEAD 认证 |
| `vmess: the host name cannot be sent to the server` | 目标主机名转不成可发送的形式；连接不会被拨出 |
| `vmess: the host name is longer than 255 bytes` | 目标主机名（转换后）超出 VMess 地址编码一字节长度所能表示的范围；连接不会被拨出 |
| `vmess: no randomness available` | 本机操作系统的随机数源不可用（极少出现）；连接不会被拨出 |
| `vmess: the server ended the UDP connection` | UDP（M8c）：一个数据报发现到它目标的那条 VMess 连接已经结束，再拨出来的一条也已结束——服务端把每条 UDP 连接一建成就关。只再拨这一次，这个数据报的发送以这条文本失败（此前会反复重拨） |

连接在两个分块之间（下一个分块的长度字段尚未开始读）被对端干净关闭是正常的流结束，不产生错误文本——服务端省去收尾的空分块时也是如此。

### anytls

| 错误文本 | 何时出现 |
| --- | --- |
| `anytls: the session is closed` | 会话所在的 TLS 连接已经失败（读或写出错），或会话所在的后台任务已经退出；之后任何对这条会话的读写（含开一个新流）都以这条文本失败。M8c 起还有一种：复用的会话上，新流 3 秒内没有等到它的 `cmdSYNACK`，整条会话被关闭（见表下的说明） |
| `anytls: the stream is closed` | 这个流已经结束——本地结束（如调用过 `shutdown`），或服务端发来不带文本的 `cmdFIN`——之后又尝试写入 |
| `anytls: <文本>` | 服务端用带错误文本的 `cmdSYNACK` 拒绝了这一个流（如目标连不上）；只有这一条流失败，会话本身仍可用于下一个流 |
| `anytls: the server sent an alert: <文本>` | 服务端发送 `cmdAlert`；整条会话（及其正在使用的流）随之结束，不会被放回连接池 |
| `anytls: the host name cannot be sent to the server` | 目标主机名转不成可发送的形式；连接不会被拨出 |
| `anytls: the host name is longer than 255 bytes` | 目标主机名（转换后）超出 AnyTLS 地址编码一字节长度所能表示的范围；连接不会被拨出 |

`anytls: <文本>` 与 `anytls: the server sent an alert: <文本>` 里的 `<文本>` 来自服务端，已去除控制字符且截到 256 个字符。`anytls` 的口令错误没有专门的错误文本：服务端把认证失败的连接当普通网站处理并直接关闭，这条连接因此按上表第一条 `anytls: the session is closed` 失败，与会话因其它原因整体关闭时表现相同（见 `docs/surge-compatibility-matrix.md` 4.2 `anytls` 行）。连接超时覆盖 TCP 连接、TLS 握手、鉴权写出与首个会话层包（`cmdSettings ‖ cmdSYN ‖ cmdPSH`）的**入队**；该包由会话自己的后台任务物理写出，拨号不等服务端的 `cmdSYNACK`；这之后的卡顿，在新会话的第一条流上由转发阶段的空闲超时兜底，在复用的会话上由 **SYNACK 看门狗**兜底（M8c；条件与时限同参考客户端 anytls-go）：服务端在它的设置里宣告的协议版本不低于 2 时，复用的会话上的新流（不是会话的第一条流）3 秒内没有等到属于它自己的 `cmdSYNACK`，整条会话就被关闭，这条流以 `anytls: the session is closed` 结束。请求不重试（它的首段数据可能已经到了服务端），池不再交出这条会话。经 HTTP 入站的 CONNECT 在复用的会话上仍立即回 200，所以这种失败出现在转发阶段——请求记录的 `error` 是这条文本，客户端看到的是隧道关闭——，不是 502；连接要 3 秒以上的目标在复用的会话上因此在第 3 秒失败，并赔上这条会话。看门狗没有对着会触发它的真实服务端验证过（回环假服务端应答的先后是照参考实现的源码写的）；后果与限制的全文见 `docs/surge-compatibility-matrix.md` 4.2 `anytls` 行，手工的两项在 `docs/acceptance/phase2-manual.md` 的 M8c 一节。

### WebSocket（trojan、vmess 共用）

| 错误文本 | 何时出现 |
| --- | --- |
| `ws: handshake failed: HTTP <code>` | 对端把 WebSocket 升级请求回了非 101 的 HTTP 响应；`<code>` 是状态码，不带原因短语或响应体 |
| `ws: handshake failed` | 握手失败，但不是上一条"回了非 101 响应"的情形（如响应格式不合法）；库自己的错误文本不会被引用 |
| `ws: protocol error` | 握手成功之后遇到的、上面几条都对不上的协议错误（如对端发了不合法的帧）；tungstenite 自己的错误文本可能引用请求头的值，因此一律映射成这条固定文本 |
| `ws: the server sent a text frame` | 对端发了一个文本帧；trojan / vmess over WebSocket 只承载二进制帧 |
| `ws: the server sent a frame larger than the limit` | 单帧或重组后的消息超过入站上限（1 MiB） |
| `ws: the connection is closed` | 写入或刷出发生在连接已经关闭之后（对端主动关闭，或本端 `poll_shutdown` 已经发出自己的 Close）；读到对端的关闭本身是普通 EOF，不会产生这条文本 |

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

### http / socks5 的握手（M8c）

| 错误文本 | 何时出现 |
| --- | --- |
| `http proxy closed the connection during CONNECT` | `http` / `https` 出站：代理在 CONNECT 的交换期间关闭或重置了连接——读到 EOF，或不带自己文字的重置、中止、断管，读的时候与写的时候都算。M8c 之前，被重置时出现的是操作系统自己的错误文本 |
| `socks5: the proxy closed the connection during the handshake` | `socks5` / `socks5-tls` 出站：同上，发生在 SOCKS5 握手的任何一步 |
| `http proxy sent a response header larger than 16384 bytes` | 代理对 CONNECT 的响应头真的超过 16384 字节。M8c 之前，读响应头时连接自己的错误也被说成这一句 |

经 `underlying-proxy` 时，这一跳的握手跑在下面一跳给的连接上：下面一跳带着自己文字的重置、中止、断管原样报出（如 `ss: the server reset the connection without answering (wrong password or method?)`），不被上面的两句盖掉。**已知限制**：下面一跳的拒绝表现为 EOF 时，仍被说成这一跳的代理关了连接。连接自己的 TLS 错误照 TLS 库的原话报出：`https` 代理要客户端证书而策略没有 `client-cert` 时是 `received fatal alert: CertificateRequired`（TLS 1.2 上是别的 alert 的文本）。

### ssh 与 wireguard（M8c 新增或改了含义的几条）

| 错误文本 | 何时出现 |
| --- | --- |
| `ssh: the host name cannot be sent to the server` | 目标主机名转不成可发送的 ASCII 形式（M8c 起名字先转成 A-label，同其它出站）；不建会话、不登录 |
| `dns: wireguard: the host name cannot be looked up` | 目标是域名、节里配了隧道内的 `dns-server`：名字转不成 A-label，没有哪个 DNS 问题带得了它，列表里又没有能回答它的 `system` |
| `wireguard: the tunnel has closed` | 正等着隧道的操作（拨号、隧道内的 DNS 查询、握手测速）遇上隧道结束——它的设备任务因任何原因结束，或协议栈的锁被发现中毒；隧道上的每个 UDP 载体也以这条文本结束。经这个策略的下一个连接启动一条新隧道 |
| `wireguard: a newer configuration of this tunnel is in use` | 同一私钥、有共同 peer 的更晚的配置取代了这条隧道（M4b 起就有）；M8c 起，被取代时正等着这条隧道的操作也立刻以它结束 |

已结束的隧道上的 TCP 连接读到的是连接被重置、写到的是管道已断（操作系统式的文本），不是上面两条——只有协议栈的锁中毒时，已建立的连接才报 `wireguard: the tunnel has closed`。细节见 `docs/surge-compatibility-matrix.md` 4.2 `wireguard` 行。

## `GET /v1/policy_groups`

无参数。响应是 `{"<组名>": [Member…], …}`，键是每个策略组的名字（配置顺序），值是该组成员的列表（配置顺序）。永不失败（没有策略组时返回 `{}`）。组参数 `hidden` 不在响应里：写了 `hidden=true` 的组同别的组一样列出（它在 Surge 里把组从界面上隐藏，rurge 没有界面；要不要由 API 携带它，随阶段 6 的 Dashboard 定）。

`Member` 的字段：

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `name` | string | 成员名字 |
| `typeDescription` | string | 策略 = 类型关键字（如 `http`）；组 = 组类型关键字（如 `select`）；内置（`DIRECT` 等）= 它自己的名字 |
| `isGroup` | bool | 该成员是否是另一个策略组（嵌套组） |
| `enabled` | bool | 恒为 `true`：rurge 没有单独禁用某个成员的机制 |
| `lineHash` | string | 见下面「`lineHash`」 |

```json
{
  "Proxy": [
    {"name": "HK", "typeDescription": "http", "isGroup": false, "enabled": true, "lineHash": "3f2a9c1d4b5e6a7f"},
    {"name": "DIRECT", "typeDescription": "DIRECT", "isGroup": false, "enabled": true, "lineHash": "9b7e1c4a2f6d8035"}
  ]
}
```

### `lineHash`

`lineHash` 是 `SHA-256("<名字> = <脱敏后的定义>")` 的前 16 个十六进制字符；内置策略（没有自己的定义行）对**它自己的名字**取哈希。它只用来**识别**一条定义（同一份配置里两次请求看到同样的哈希，就是同一条定义），不是这条定义原文的指纹：

- 哈希对象是脱敏之后的文本，不是配置文件里的原始行。**只改动凭据（密码、`base64`、`psk`、`headers=`、`ws-headers=`、`ws-path=`、`shadow-tls-password=`、`test-url=`、`args=`（`external`，M4c 起）等被脱敏的字段）不会改变 `lineHash`**（含 `password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 这些带引号或带括号的值：取值按解析器的顶层逗号规则整体被抹掉，不会有尾巴漏进哈希）——因为脱敏后两行文本相同——这是有意的：`lineHash` 经这个公开的、无需鉴权之外任何权限的端点暴露，如果它是对原始定义取哈希，持有 API key 的人就能对着猜测的凭据反复计算哈希、离线核对是否猜中，等于把凭据的验证能力带出了进程。任何由凭据派生的东西都不允许离开 rurge 进程（项目的硬性约束：凭据不进日志、错误、API 输出与 `Debug`），`lineHash` 因此必须建立在脱敏后的文本上。
- 名字参与哈希且在一份配置里唯一，所以两个不同成员不会撞哈希；端口、服务器地址、TLS 参数等任何非凭据字段的改动都会改变 `lineHash`。
- **M8c 让它变过一次**：哈希建立在脱敏后的文本上，而 M8c 改了脱敏（见上面 `GET /v1/policies/detail` 一节）。脱敏后的定义因此显示得不一样的那些策略与组——例如组行上带旧参数 `url=` 的、定义里有带用户名口令的 URL 的、有 scheme 不是那六种的 URL 的——`lineHash` 跟着变了一次，其余的没有变。存着旧值的客户端会看到它变这一次，此后照旧稳定。

## `GET /v1/policy_groups/select`

参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；自动组（`url-test` / `fallback` / `load-balance`）是当前生效的成员：临时覆盖优先，其次是按测试结果选出的（没有结果时是第一个成员；`load-balance` 显示第一个通过测试的成员，M3b）；`smart`（M3c 起）是最近 10 分钟用得最多的成员，最近没用过时是排在第一的成员；**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。

```json
{"policy": "HK"}
```

失败：`group_name` 不是任何已知策略组 → 404

```json
{"error": "unknown policy group `Proxy-typo`"}
```

## `POST /v1/policy_groups/select`

请求体 `{"group_name": "<name>", "policy": "<member>"}`。成功 `{}`。

```json
{"group_name": "Proxy", "policy": "HK"}
```

失败（均为 400）：

| 情形 | `error` |
| --- | --- |
| `group_name` 不是任何已知策略组 | `` unknown policy group `Proxy-typo` `` |
| `group_name` 是 `subnet` 组 | `` `Office` does not take a selection `` |
| `policy` 不是该组的成员 | `` `Somewhere` is not a member of `Proxy` `` |

选择立即生效：**下一条**使用该组（或途经该组的链）的连接就会解析到新成员，正在进行中的连接不受影响。选择按 Profile 持久化到 `state.json` 的 `group_selections[<Profile 文件名>][<组名>]`（文件名，不含目录，如 `surge.conf`）；rurge 重启后从 `state.json` 恢复，已消失的旧选择（成员被从配置里删除）按"没有选择"处理，回落到第一个成员。对自动组的同一个请求是临时覆盖，见「连通性测试与自动组（M3b）」一节。

## `POST /v1/profiles/check`

自阶段 2 / M1 起，`POST /v1/profiles/check`（阶段 1 端点，见 `docs/api/phase1.md`）除原有的加载诊断外，还包含干构建（自 M3a 起还有订阅装配，见下一节）：磁盘上的配置能解析、但其中某个策略构建不出来（例如 `client-cert` 指向的 p12 解不开、密码错误）时，`ok` 为 `false`，`errors` 计入这条 `E0022`，`diagnostics` 里带上该策略定义所在的行号。`rurge check`、`run`、`reload` 的行为相同（见 `docs/surge-compatibility-matrix.md` 10.3 / 10.4 节）。

## 订阅导入与派生的策略（M3a）

自阶段 2 / M3a 起（`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md` 第 5、8 节），上面几个端点读的都是运行中的策略表，而策略表会随订阅更新重建：

- `GET /v1/policies` 的 `proxies` 依次是 5 个内置策略、配置里的策略、订阅导入的策略、组级 `underlying-proxy` 派生的 `Name (via Relay)`；`policy-groups` 不变。
- `GET /v1/policies/detail` 能查导入与派生的策略：导入的是订阅里那一行（前缀与 `external-policy-modifier` 已应用），派生的是其来源策略的定义加上 `underlying-proxy=<中继>`；脱敏规则同上，`policy-path`、`external-policy-modifier` 与 `test-url`（M3b 起）也在脱敏名单里；`external` 策略的每个 `args` 整体变成 `***`（M4c 起）；组行上的旧参数 `url=` 与 `test-url` 同样处理（M8c 起）。
- `GET /v1/policy_groups` 的成员表是装配后的：写在组行上的、`include-other-group` 与 `include-all-proxies` 取来的、订阅导入的，经过滤与去重，有中继的组里代理成员换成派生名。`lineHash` 对导入与派生的成员同样建立在脱敏后的文本上。
- `GET` / `POST /v1/policy_groups/select` 按装配后的成员表读与校验；选择按名字保存，订阅更新后名字不在了就回落到第一个成员。
- `POST /v1/profiles/check`：配置本身无错时，连同守护进程数据目录里已缓存的订阅一起装配检查，不联网；没有内容的订阅报 `W0022`，订阅里跳过的行报 `W0023`，订阅的 URL 不出现在输出里。

## 连通性测试与自动组（M3b）

自阶段 2 / M3b 起（`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md` 第 6 节），`url-test` / `fallback` / `load-balance` 组按连通性测试的结果选成员。下面三个端点里，手册只给了 `POST /v1/policy_groups/test` 的响应示例，另外两个的形状是"暂定"。

### 测试结果（Result）

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| `delay` | 整数 | 通过时：毫秒。同一条连接上第二次 `HEAD` 从发出到收到响应头的时间；服务端不保持连接时是第一次 `HEAD` 从开始拨号起的完整时间 |
| `error` | string | 失败时：原因，如 `connect: <出站的错误>`、`tls: <原因>`、`http: <原因>`、`timed out`、`the test URL does not fit in a request line`（测试 URL 长到放不进请求行，不发起连接）；不含测试 URL |
| `time` | 数字 | 这次测试结束的时间，Unix 秒（带小数） |

`delay` 与 `error` 二者有一。

```json
{"delay": 128, "time": 1758790000.25}
```

```json
{"error": "timed out", "time": 1758790005.02}
```

### `POST /v1/policies/test`

请求体 `{"policy_names": ["<名字>", …], "url": "<URL>"}`，`url` 可省略（或为空字符串）。

- 省略 `url`：每个策略按它自己的测试 URL 与超时测（策略的 `test-url` / `test-timeout`，否则 `[General]` 的 `proxy-test-url`——直连类用 `internet-test-url`——与 `test-timeout`）；`wireguard` 策略没有 `dns-server` 也没写 `test-url` 时改为握手测速（见下面「`wireguard` 的测速」）；结果保存，自动组随即按它选择。
- 给了 `url`：全部策略在这个 URL 上各测一次，超时仍用各自的；结果**不保存**，不影响任何组。
- 各策略同时测（全进程最多 8 个测试同时进行，其余排队）；全部测完才返回。

响应 `{"<名字>": Result 或 {"error": "not testable"}}`；不能测的是策略组、`REJECT` 族、rurge 不实现的协议或变体（加载时报 `W0007` 的策略：`tailscale`、`snell` 的 `version=6`、手册不再列出的 `ss` 方法名），以及自己的测试 URL 解析不了的策略（即使请求里给了 `url`）。

UDP 测试（M5c）：策略有 UDP 测试时——策略的 `test-udp`，否则 `[General]` 的 `proxy-test-udp`——它的 Result 多一个 `udp` 键，与 URL 测试同时进行：经策略的 UDP 向 `hostname@ipv4` 的 53 端口问一次 A 记录，`{"delay": <毫秒>}` 或 `{"error": "<原因>"}`（如 `udp test timed out`），时限同该策略的测试超时；没有 UDP 测试、策略不载 UDP、不能测时没有这个键。UDP 测试的结果不保存、不影响任何组，也不进请求记录；新增的键不改变已有的形状。

```json
{"HK": {"delay": 128, "time": 1758790000.25, "udp": {"delay": 41}}}
```

```json
{"HK": {"delay": 128, "time": 1758790000.25}, "Pick": {"error": "not testable"}}
```

失败（均为 400）：某个名字不是任何已知策略、组或内置名 → `` unknown policy `HK-typo` ``；`url` 不是 URL → `` `url` is not a URL ``。

### `GET /v1/policy_groups/test_results`

无参数。响应 `{"<组名>": {"<成员>": Result 或 null}}`，只列 `url-test` / `fallback` / `load-balance` / `smart` 组（`smart` 自 M3c 起），成员是装配后的成员表；`null` 表示没有结果：还没测过，或结果对应的定义、测试 URL、超时已经变了；嵌套组、`REJECT` 族、rurge 不实现的协议（`W0007`）与测试 URL 解析不了的策略恒为 `null`（`url-test` / `fallback` / `load-balance` 在选择时把后三种当作失败；`smart` 组不选 rurge 不实现的协议的成员，测试 URL 解析不了的成员只按真实会话的表现排序）。

```json
{"Auto": {"HK": {"delay": 128, "time": 1758790000.25}, "JP": {"error": "timed out", "time": 1758790005.02}}}
```

### `POST /v1/policy_groups/test`

请求体 `{"group_name": "<name>"}`。立即测该组全部成员（嵌套组的成员一并测），不看 `interval`；任何类型的组都能测。响应是本轮通过的成员（写了 `timeout` 时分数须低于它），按成员顺序：

```json
{"available": ["HK", "JP"]}
```

失败：`group_name` 不是任何已知策略组 → 400 `` unknown policy group `Proxy-typo` ``。

### `POST /v1/policy_groups/select` 对自动组

对 `url-test` / `fallback` / `load-balance` / `smart` 组，同一个请求体设置**临时覆盖**：该组从下一条连接起固定用这个成员，期间不因使用而测试；`"policy": ""` 清除覆盖，恢复按测试结果选择。覆盖不写 `state.json`，进程重启后不在；组定义（不计行位置）不变的重载保留它，组消失或定义变了就清除；覆盖的成员从成员表里消失（订阅更新）时覆盖失效。

### `wireguard` 的测速（M4b）

- 节里没有 `dns-server`、策略也没写 `test-url` 时，测试是一次握手：向每个 peer 强制握手，`delay` 是从强制发起到任一 peer 第一个握手完成的毫秒数——通常是一个往返，之前已有一次发起在途时（隧道刚启动、换密钥、另一条策略的测试）可能更短；它只证明 peer 可达，不证明路由与出口（照手册）。peer 不回应时 `error` 是 `timed out`；隧道起不来时是出站的错误（如 `via Up: the underlying policy cannot carry UDP`）。
- 否则经隧道两次 `HEAD`，与其它策略相同。
- 两种都在超时之外另加 10 秒（第一次测试可能要先启动隧道）；`POST /v1/policies/test` 给了 `url` 时一律按那个 URL 测。

### 测试会话

每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数；`wireguard` 的握手测速是第一个 peer 的 endpoint）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。

## `smart` 组（M3c）

- **选择**：按成员的分数——真实会话的首字节耗时与测速结果的时间加权平均，加上失败罚分，乘以 `policy-priority` 因子——排序；最优者 1.2 倍以内的成员里随机选一个；最近在同一目标主机上成功过、分数不超过最优者 2 倍的成员优先，最近在那里失败过的排到最后。细节见 `docs/superpowers/specs/2026-09-26-phase2-m3c-smart-design.md`。
- **成员表**：`smart` 组只取代理策略；写在里面的嵌套组、内置策略与 `direct` / `reject` 别名被忽略（加载时记一行 INFO），`GET /v1/policy_groups`、`select` 与测试端点里的成员表也不含它们——对这样一个名字，`POST /v1/policy_groups/select` 回 400 `` `Sel` is not a member of `S` ``。rurge 不实现其协议的成员（`W0007`）留在成员表里，但不会被选中，也不进换成员的名单；组里一个能用的成员都没有时才用第一个成员（得到带说明的 REJECT）。
- **换成员**：选中的成员连不上时，这次会话依次再试排在后面的两个成员，每次最多用"剩余时间 ÷ 剩余尝试次数"，总计 10 秒。请求记录的 `policy` 是实际用上的那条链；换过成员时 `error` 是 ``smart group `S`: `A` failed to connect, used `B` ``（会话本身成功）；都连不上时是 ``smart group `S`: tried `A`, `B`, `C`; <最后一次的错误>``。DNS 会话与链的中间跳（`underlying-proxy`）不换成员。
- **失败怎样计入成员（M8c）**：经 `smart` 组的会话失败——拨不通、出站就绪后 3 秒没有回应、上游在回话之前断开——一律记进该目标主机的站点记忆（约一小时，从那里最后一次失败算起）；只有记下的那一刻站点**没有任何未过期的失败记录**——别的成员的，或这个成员自己上一次在那里的结果——时，才另算成员自己的一次失败（罚 800 ms，连续失败加一；连续三次判为失败）。所以一个失效的地址不论被访问多少次、同时来多少条会话，合计只让一个成员挨一次罚；N 个不同的失效地址仍罚 N 次。被豁免的失败不碰成员的记录，测速的失败总是计入。端点的形状都没有变：能观察到的是会话经哪个成员出去（请求记录的 `policy`），以及 `smart: the policy counts as failed` / `smart: the policy works again` 这两行 INFO 日志比以前少。后果与限制的全文见 `docs/surge-compatibility-matrix.md` 5.1 `smart` 行。
- **发不出去的目标（M8c）**：目标是没有 ASCII 形式的域名，或其 ASCII 形式（结尾的点也算）超过 255 字节时，哪个成员都发不了它：会话只经组选中的成员拨一次，不换成员，也不向组回报任何东西。请求记录的 `policy` 仍是 `[组, 成员]`，`error` 是那个成员自己的话（如 `the target host name is not valid for an HTTP proxy request`），没有上面 `smart group … tried …` 的说明；UDP 流相同。
- **请求记录**：`connectMs` / `firstByteMs` 两个字段见 `docs/api/phase1.md`（所有会话都有）。
- **`GET /v1/policy_groups/select`**：最近 10 分钟用得最多的成员，最近没用过时是排在第一的成员；读取不触发测速。
- **`POST /v1/policy_groups/select`**：临时覆盖，同其它自动组；覆盖期间不换成员重试。
- **`GET /v1/policy_groups/test_results`**：列出 `smart` 组，内容与其它自动组相同（成员 → 最近测速结果）；分数不经 API 给出。
- **`POST /v1/policy_groups/test`**：测全部成员（拨号触发的常规轮次在测得了的成员超过 12 个时只测其中 12 个），返回测完后健康的成员。

## 跟随出站模式下 DNS 的内部会话（M7c）

`[General] encrypted-dns-follow-outbound-mode = true` 时，解析器到加密 DNS 上游的连接由规则选策略，每条连接是请求记录（`GET /v1/requests/recent`、`GET /v1/requests/active`）里的一条内部会话：`listener` 为 `internal`，`dst` 是上游服务器的地址与端口，`rule` 与 `policy` 是命中的规则与策略链。TCP / DoT / DoH 上游的连接自阶段 1 起如此（`transport` 为 `tcp`，`protocol` 按端口标为 `dot`（853）、`doh`（443）或 `dns`）；自阶段 2 / M7c 起，DoQ / DoH3 上游的 QUIC 载体也是：

- **形状**：`transport` 为 `udp`，`protocol` 按端口标为 `doh3`（443）或 `doq`（其余端口——不在 443 的 DoH3 服务器因此标成 `doq`）；`up` / `down` 是载体发出与收到的数据报字节数，`connectMs` 到载体打开为止，`firstByteMs` 到服务器的第一个回包为止。
- **存续**：每个载体——上游的一条 QUIC 连接，或到它某个地址的一次连接尝试——一条会话，随载体存续：在用时 `status` 是 `active`；连接关闭（闲置满两端空闲超时中较短的那个——rurge 一侧 30 秒——、服务端关闭、上游重建）时载体随之放手，会话结束，`status` 为 `completed`。
- **失败**：`status` 为 `failed` 时 `error` 是——载体打不开时代理自己的错误（如 `socks5: authentication failed`）；载体打开之后发不出去或自己断了时载体的原话（如 `masque: the server does not support CONNECT-UDP`，这种服务端到发第一个包才知道）；规则选中不载 UDP 的代理、`udp-policy-not-supported-behaviour` 为 `REJECT`（缺省）时 `policy does not support UDP`。上游这样立刻失败时，同一次查找里每秒再问它一次，所以它是唯一能问的上游时，一次查找留下至多 5 条失败的记录。M8c 起：载体自己结束时（代理关了 UDP 关联、服务端关了承载 UDP 的那条连接）`error` 是它的原话，如 `socks5: the proxy closed the UDP association`、`trojan: the server closed the UDP connection`——此前是固定的一句 `the carrier through the chain has closed`，那一句现在只在原话已经报过一次之后、或载体以只有种类没有文本的错误结束时出现；载体对单个数据报报出的"连接被重置"不是失败：会话照常，不记 `error`，也不算 `smart` 成员的一次失败——QUIC 本来就跳过这种错误、接着用这条载体，此前这样的会话被记成 `failed`，而查询其实照常得到应答；载体因被重置而**结束**的（跑在一条流上的 UDP 载体，如 `trojan`、`anytls`，服务端重置了那条连接）仍是 `failed`，`error` 是它的原话。
- **说明**：会话本身成功而 `error` 不为空的三种——`policy does not support UDP; sent through DIRECT`（上一条的设置为 `DIRECT`：载体改经 DIRECT 的 UDP 发出，`policy` 仍是规则选中的那条链）、`dns-follow: reject bypassed to keep DNS working`（规则选中 REJECT，或选中的代理链底是 REJECT，改走直连）、`dns-follow: proxy configured by host name bypassed to avoid a resolution loop`（代理以域名配置，改走直连）。后两种改走直连的载体之后报错时会话为 `failed`，`error` 是这句说明后接 `; ` 与操作系统自己的错误文本（随系统的语言）。M8c 起"连接被重置"不算这样的报错——Windows 上已连接的 UDP socket 收到 ICMP 端口不可达时，下一次接收报的就是它（os error 10054）——：会话照常，`error` 仍只是那句说明。别的系统对同一件事的报法不同（Linux 交给下一次发送，macOS 的接收拿到"连接被拒绝"、那条载体如实以失败结束）；这两种按读源码得出，只有 Windows 上的端到端用例跑过。
- 与其它内部会话一样，不能经 `POST /v1/requests/kill` 终止（409）。没开这个选项时 QUIC 载体直连，不产生会话。
