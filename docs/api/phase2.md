# rurge HTTP API（阶段 2）

阶段 2 / M1（装配与控制面，见 `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md` 第 6.6 节）新增四个策略 / 策略组端点；阶段 2 / M3b（测速与自动组）又新增三个测试端点，并让 `POST /v1/policy_groups/select` 对自动组设临时覆盖（见末节「连通性测试与自动组（M3b）」）。鉴权（`X-Key` 头 / `?x-key=` 查询参数、常量时间比较、封禁）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录本页新增端点自己的形状。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/policies/detail?policy_name=<name>` | | `{"<name>": "<脱敏后的定义>"}`；未知策略 → 404 |
| GET | `/v1/policy_groups` | | `{"<组名>": [Member…], …}` |
| GET | `/v1/policy_groups/select?group_name=<name>` | | `{"policy": "<当前生效的成员>"}`；未知组 → 404 |
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效，或是 `smart` / `subnet` 组 → 400；对自动组即临时覆盖（M3b） |
| POST | `/v1/policies/test` | `{"policy_names":["<name>",…],"url":"<可省略>"}` | `{"<name>": Result…}`；未知策略、`url` 不是 URL → 400（M3b） |
| GET | `/v1/policy_groups/test_results` | | `{"<组名>": {"<成员>": Result 或 null}}`（M3b） |
| POST | `/v1/policy_groups/test` | `{"group_name":"<name>"}` | `{"available":["<成员>",…]}`；未知组 → 400（M3b） |

**"暂定"标注**：手册没有给出 `/v1/policies/detail`、`/v1/policy_groups`（M1 设计 O2）与 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`（M3 设计 6.6）的响应示例；下面的形状按社区已知的 Surge 响应实现，拿到真实 Surge 实例的样本后再对齐，届时可能是破坏性变更。

## `GET /v1/policies/detail`

参数 `policy_name`（必填，查询参数）。

响应是 `{"<policy_name>": "<value>"}`：`value` 是该策略或组**脱敏后的定义**（不含 `名字 =` 前缀），脱敏规则与 `GET /v1/profiles/current` 相同（`docs/api/phase1.md`：独立密钥行、内联的 `password` / `psk` / `base64` / `headers` / `ws-headers` / `ws-path` / `shadow-tls-password` 等参数、所有写作 `type, server, port` 的代理类型第 4 个起的位置值——`trojan` `vmess` `ss` `anytls` 等都在内）；取值的结尾按解析器自己的规则找第一个顶层逗号——引号可以在值里任何位置打开、括号会分组，所以 `password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 都是整个值变成 `***`；`headers=` 与 `ws-headers=` 的值整体变成 `***`（连 header 名也不保留），`ws-path=` 同样整体变成 `***`；内置策略（`DIRECT` `REJECT` 等）的值是它自己的名字。

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

trojan、vmess（± WebSocket）、anytls 出站，以及任何带 Shadow TLS 的出站失败时，会话记录的 `error` 是下面按协议分节列出的固定文本之一；其余失败仍按连接失败的通用形式出现（拨号超时 `connect timed out`、TLS 握手失败 `tls: <原因>`、或底层 I/O 错误的原文——涵盖 TCP 连接失败、WebSocket 握手期间的 I/O 错误，以及已建立的连接在转发期间的 I/O 错误：后者经转发循环的失败文本进入会话记录）。对端给的字节（HTTP 响应头、WebSocket 握手响应体、anytls 的错误帧等）永不原样出现在下列固定文本中。

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

连接在两个分块之间（下一个分块的长度字段尚未开始读）被对端干净关闭是正常的流结束，不产生错误文本——服务端省去收尾的空分块时也是如此。

### anytls

| 错误文本 | 何时出现 |
| --- | --- |
| `anytls: the session is closed` | 会话所在的 TLS 连接已经失败（读或写出错），或会话所在的后台任务已经退出；之后任何对这条会话的读写（含开一个新流）都以这条文本失败 |
| `anytls: the stream is closed` | 这个流已经结束——本地结束（如调用过 `shutdown`），或服务端发来不带文本的 `cmdFIN`——之后又尝试写入 |
| `anytls: <文本>` | 服务端用带错误文本的 `cmdSYNACK` 拒绝了这一个流（如目标连不上）；只有这一条流失败，会话本身仍可用于下一个流 |
| `anytls: the server sent an alert: <文本>` | 服务端发送 `cmdAlert`；整条会话（及其正在使用的流）随之结束，不会被放回连接池 |
| `anytls: the host name cannot be sent to the server` | 目标主机名转不成可发送的形式；连接不会被拨出 |
| `anytls: the host name is longer than 255 bytes` | 目标主机名（转换后）超出 AnyTLS 地址编码一字节长度所能表示的范围；连接不会被拨出 |

`anytls: <文本>` 与 `anytls: the server sent an alert: <文本>` 里的 `<文本>` 来自服务端，已去除控制字符且截到 256 个字符。`anytls` 的口令错误没有专门的错误文本：服务端把认证失败的连接当普通网站处理并直接关闭，这条连接因此按上表第一条 `anytls: the session is closed` 失败，与会话因其它原因整体关闭时表现相同（见 `docs/surge-compatibility-matrix.md` 4.2 `anytls` 行）。连接超时覆盖 TCP 连接、TLS 握手、鉴权写出与首个会话层包（`cmdSettings ‖ cmdSYN ‖ cmdPSH`）的**入队**；该包由会话自己的后台任务物理写出，服务端的 `cmdSYNACK` 不被等待，这之后的卡顿由转发阶段的空闲超时兜底。

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

## `GET /v1/policy_groups`

无参数。响应是 `{"<组名>": [Member…], …}`，键是每个策略组的名字（配置顺序），值是该组成员的列表（配置顺序）。永不失败（没有策略组时返回 `{}`）。

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

- 哈希对象是脱敏之后的文本，不是配置文件里的原始行。**只改动凭据（密码、`base64`、`psk`、`headers=`、`ws-headers=`、`ws-path=`、`shadow-tls-password=`、`test-url=` 等被脱敏的字段）不会改变 `lineHash`**（含 `password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 这些带引号或带括号的值：取值按解析器的顶层逗号规则整体被抹掉，不会有尾巴漏进哈希）——因为脱敏后两行文本相同——这是有意的：`lineHash` 经这个公开的、无需鉴权之外任何权限的端点暴露，如果它是对原始定义取哈希，持有 API key 的人就能对着猜测的凭据反复计算哈希、离线核对是否猜中，等于把凭据的验证能力带出了进程。任何由凭据派生的东西都不允许离开 rurge 进程（`global-constraints.md`），`lineHash` 因此必须建立在脱敏后的文本上。
- 名字参与哈希且在一份配置里唯一，所以两个不同成员不会撞哈希；端口、服务器地址、TLS 参数等任何非凭据字段的改动都会改变 `lineHash`。

## `GET /v1/policy_groups/select`

参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；自动组（`url-test` / `fallback` / `load-balance`）是当前生效的成员：临时覆盖优先，其次是按测试结果选出的（没有结果时是第一个成员；`load-balance` 显示第一个通过测试的成员，M3b）；`smart`（M3c 之前）是第一个成员；**一个没有成员的组**（订阅还没下载到，或过滤把成员滤光了）返回 `{"policy": ""}`，这不是错误；`subnet` 组在阶段 3 之前代表它的 `default`（M3a）。

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
| `group_name` 是 `smart`（M3c 之前）或 `subnet` 组 | `` `Smart` does not take a selection `` |
| `policy` 不是该组的成员 | `` `Somewhere` is not a member of `Proxy` `` |

选择立即生效：**下一条**使用该组（或途经该组的链）的连接就会解析到新成员，正在进行中的连接不受影响。选择按 Profile 持久化到 `state.json` 的 `group_selections[<Profile 文件名>][<组名>]`（文件名，不含目录，如 `surge.conf`）；rurge 重启后从 `state.json` 恢复，已消失的旧选择（成员被从配置里删除）按"没有选择"处理，回落到第一个成员。对自动组的同一个请求是临时覆盖，见末节。

## `POST /v1/profiles/check`

自阶段 2 / M1 起，`POST /v1/profiles/check`（阶段 1 端点，见 `docs/api/phase1.md`）除原有的加载诊断外，还包含干构建（自 M3a 起还有订阅装配，见下一节）：磁盘上的配置能解析、但其中某个策略构建不出来（例如 `client-cert` 指向的 p12 解不开、密码错误）时，`ok` 为 `false`，`errors` 计入这条 `E0022`，`diagnostics` 里带上该策略定义所在的行号。`rurge check`、`run`、`reload` 的行为相同（见 `docs/surge-compatibility-matrix.md` 10.3 / 10.4 节）。

## 订阅导入与派生的策略（M3a）

自阶段 2 / M3a 起（`docs/superpowers/specs/2026-09-23-phase2-m3-groups-subscriptions-design.md` 第 5、8 节），上面几个端点读的都是运行中的策略表，而策略表会随订阅更新重建：

- `GET /v1/policies` 的 `proxies` 依次是 5 个内置策略、配置里的策略、订阅导入的策略、组级 `underlying-proxy` 派生的 `Name (via Relay)`；`policy-groups` 不变。
- `GET /v1/policies/detail` 能查导入与派生的策略：导入的是订阅里那一行（前缀与 `external-policy-modifier` 已应用），派生的是其来源策略的定义加上 `underlying-proxy=<中继>`；脱敏规则同上，`policy-path`、`external-policy-modifier` 与 `test-url`（M3b 起）也在脱敏名单里。
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

- 省略 `url`：每个策略按它自己的测试 URL 与超时测（策略的 `test-url` / `test-timeout`，否则 `[General]` 的 `proxy-test-url`——直连类用 `internet-test-url`——与 `test-timeout`）；结果保存，自动组随即按它选择。
- 给了 `url`：全部策略在这个 URL 上各测一次，超时仍用各自的；结果**不保存**，不影响任何组。
- 各策略同时测（全进程最多 8 个测试同时进行，其余排队）；全部测完才返回。

响应 `{"<名字>": Result 或 {"error": "not testable"}}`；不能测的是策略组、`REJECT` 族、尚未实现的协议，以及自己的测试 URL 解析不了的策略（即使请求里给了 `url`）。

```json
{"HK": {"delay": 128, "time": 1758790000.25}, "Pick": {"error": "not testable"}}
```

失败（均为 400）：某个名字不是任何已知策略、组或内置名 → `` unknown policy `HK-typo` ``；`url` 不是 URL → `` `url` is not a URL ``。

### `GET /v1/policy_groups/test_results`

无参数。响应 `{"<组名>": {"<成员>": Result 或 null}}`，只列 `url-test` / `fallback` / `load-balance` 组（`smart` 随 M3c），成员是装配后的成员表；`null` 表示没有结果：还没测过，或结果对应的定义、测试 URL、超时已经变了；嵌套组、`REJECT` 族、尚未实现的协议与测试 URL 解析不了的策略恒为 `null`（组在选择时把后三种当作失败）。

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

对 `url-test` / `fallback` / `load-balance` 组，同一个请求体设置**临时覆盖**：该组从下一条连接起固定用这个成员，期间不因使用而测试；`"policy": ""` 清除覆盖，恢复按测试结果选择。覆盖不写 `state.json`，进程重启后不在；组定义（不计行位置）不变的重载保留它，组消失或定义变了就清除；覆盖的成员从成员表里消失（订阅更新）时覆盖失效。

### 测试会话

每次测试是请求记录（`GET /v1/requests/recent`）里的一条内部会话：`listener` 为 `internal`、`rule` 为 `policy test`、`policy` 是被测的策略、目标是测试 URL 的主机与端口（不含路径与参数）；失败时 `error` 是上面 Result 里的原因。它们与 DNS 会话一样不能经 `POST /v1/requests/kill` 终止（409）。
