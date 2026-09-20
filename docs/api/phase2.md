# rurge HTTP API（阶段 2 / M1）

阶段 2 / M1（装配与控制面，见 `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md` 第 6.6 节）新增四个策略 / 策略组端点。鉴权（`X-Key` 头 / `?x-key=` 查询参数、常量时间比较、封禁）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录本页新增端点自己的形状。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/policies/detail?policy_name=<name>` | | `{"<name>": "<脱敏后的定义>"}`；未知策略 → 404 |
| GET | `/v1/policy_groups` | | `{"<组名>": [Member…], …}` |
| GET | `/v1/policy_groups/select?group_name=<name>` | | `{"policy": "<当前生效的成员>"}`；未知组 → 404 |
| POST | `/v1/policy_groups/select` | `{"group_name":"<name>","policy":"<member>"}` | `{}`；组或成员无效、或不是 `select` 组 → 400 |

**"暂定"标注**：手册没有给出 `/v1/policies/detail` 与 `/v1/policy_groups` 的响应示例（M1 设计 O2）；下面的形状按社区已知的 Surge 响应实现，拿到真实 Surge 实例的样本后再对齐，届时可能是破坏性变更。

## `GET /v1/policies/detail`

参数 `policy_name`（必填，查询参数）。

响应是 `{"<policy_name>": "<value>"}`：`value` 是该策略或组**脱敏后的定义**（不含 `名字 =` 前缀），脱敏规则与 `GET /v1/profiles/current` 相同（`docs/api/phase1.md`：独立密钥行、内联的 `password` / `psk` / `base64` / `headers` / `ws-headers` / `ws-path` 等参数、`http` / `https` / `socks5` / `socks5-tls` 行第 4 个起的位置凭据）；`headers=` 与 `ws-headers=` 的值整体变成 `***`（连 header 名也不保留），`ws-path=` 同样整体变成 `***`；内置策略（`DIRECT` `REJECT` 等）的值是它自己的名字。

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

trojan（± WebSocket）出站失败时，会话记录的 `error` 是下列固定文本之一；其余失败仍按连接失败的通用形式出现（拨号超时 `connect timed out`、TLS 握手失败 `tls: <原因>`、或底层 I/O 错误的原文——涵盖 TCP 连接失败与 WebSocket 握手期间的 I/O 错误）。对端给的字节（HTTP 响应头、WebSocket 握手响应体等）永不原样出现在下列固定文本中。

| 错误文本 | 何时出现 |
| --- | --- |
| `trojan: the host name cannot be sent to the server` | 目标主机名转不成可发送的 ASCII 形式（如含 `@` 这类可能被下游误读成 authority 分隔符的字符）；连接不会被拨出 |
| `trojan: the host name is longer than 255 bytes` | 目标主机名（转换后）超出 trojan 地址编码一字节长度所能表示的范围；连接不会被拨出 |
| `ws: handshake failed: HTTP <code>` | 对端把 WebSocket 升级请求回了非 101 的 HTTP 响应；`<code>` 是状态码，不带原因短语或响应体 |
| `ws: handshake failed` | 握手失败，但不是上一条"回了非 101 响应"的情形（如响应格式不合法）；库自己的错误文本不会被引用 |
| `ws: protocol error` | 握手成功之后遇到的、上面几条都对不上的协议错误（如对端发了不合法的帧）；tungstenite 自己的错误文本可能引用请求头的值，因此一律映射成这条固定文本 |
| `ws: the server sent a text frame` | 对端发了一个文本帧；trojan over WebSocket 只承载二进制帧 |
| `ws: the server sent a frame larger than the limit` | 单帧或重组后的消息超过入站上限（1 MiB） |
| `ws: the connection is closed` | 写入或刷出发生在连接已经关闭之后（对端主动关闭，或本端 `poll_shutdown` 已经发出自己的 Close）；读到对端的关闭本身是普通 EOF，不会产生这条文本 |

`trojan` 的密码错误没有对应的错误文本：协议没有应答，服务端把连接交给它的回落站点，连接建立成功、会话记录 `error` 为空，异常只在转发阶段表现为对端提前关闭或回一段与预期不符的数据（见 `docs/surge-compatibility-matrix.md` 4.2 `trojan` 行）。

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

- 哈希对象是脱敏之后的文本，不是配置文件里的原始行。**只改动凭据（密码、`base64`、`psk`、`headers=`、`ws-headers=`、`ws-path=` 等被脱敏的字段）不会改变 `lineHash`**——因为脱敏后两行文本相同——这是有意的：`lineHash` 经这个公开的、无需鉴权之外任何权限的端点暴露，如果它是对原始定义取哈希，持有 API key 的人就能对着猜测的凭据反复计算哈希、离线核对是否猜中，等于把凭据的验证能力带出了进程。任何由凭据派生的东西都不允许离开 rurge 进程（`global-constraints.md`），`lineHash` 因此必须建立在脱敏后的文本上。
- 名字参与哈希且在一份配置里唯一，所以两个不同成员不会撞哈希；端口、服务器地址、TLS 参数等任何非凭据字段的改动都会改变 `lineHash`。

## `GET /v1/policy_groups/select`

参数 `group_name`（必填，查询参数）。响应 `{"policy": "<当前生效的成员>"}`：`select` 组是它当前的选择（没有选择时是第一个成员）；非 `select` 组（`url-test` / `fallback` / `subnet` 等）是它当前解析到的成员，同样按"没有选择用第一个成员"的规则；**一个配置合法但没有成员的组**（例如 `subnet` 组——它的目标写在 `conditions` / `default` 里，不写在成员列表）返回 `{"policy": ""}`，这不是错误。

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
| `group_name` 存在但不是 `select` 组 | `` `Auto` is not a select group `` |
| `policy` 不是该组的成员 | `` `Somewhere` is not a member of `Proxy` `` |

选择立即生效：**下一条**使用该组（或途经该组的链）的连接就会解析到新成员，正在进行中的连接不受影响。选择按 Profile 持久化到 `state.json` 的 `group_selections[<Profile 文件名>][<组名>]`（文件名，不含目录，如 `surge.conf`）；rurge 重启后从 `state.json` 恢复，已消失的旧选择（成员被从配置里删除）按"没有选择"处理，回落到第一个成员。

## `POST /v1/profiles/check`

自阶段 2 / M1 起，`POST /v1/profiles/check`（阶段 1 端点，见 `docs/api/phase1.md`）除原有的加载诊断外，还包含干构建：磁盘上的配置能解析、但其中某个策略构建不出来（例如 `client-cert` 指向的 p12 解不开、密码错误）时，`ok` 为 `false`，`errors` 计入这条 `E0022`，`diagnostics` 里带上该策略定义所在的行号。`rurge check`、`run`、`reload` 的行为相同（见 `docs/surge-compatibility-matrix.md` 10.3 / 10.4 节）。
