# rurge HTTP API（阶段 1）

rurge 在 `[General] http-api = <key>@<ip>:<port>` 指定的地址上提供 Surge 兼容的 HTTP API。本页记录阶段 1（M4a）实现的端点与 rurge 暂定的 JSON 结构；手册未定义的结构在阶段 6 与真实 Surge 对齐时可能调整（见 `docs/surge-compatibility-matrix.md` 第 10.4 节）。

## 鉴权与错误

- 每个请求带 `X-Key: <key>` 头或 `?x-key=<key>` 查询参数；比较为常量时间。
- 失败：`401 {"error":"unauthorized"}`。同一来源 IP 10 分钟内 5 次失败 → 之后 10 分钟内一律 `403 {"error":"banned"}`；封禁表最多跟踪 1024 个来源。
- 所有错误都是 `{"error":"<message>"}`：400（参数、JSON 请求体或查询字符串错误）、401、403、404（未知端点 / 未知请求 id / 未知功能名）、409（不可终止的内部会话）、500（内部错误）、501（本阶段未实现）。
- 无内容的成功响应是 `{}`。字段名 camelCase；手册已定义的字段（如 `policy-groups`、`dnsCache`）照抄。
- `http-api` 绑定到非环回地址时启动时打 WARN；改动 `http-api` 需重启 rurge（重载只警告，继续用旧地址 / 密钥）。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/outbound` | | `{"mode":"direct"\|"proxy"\|"rule"}` |
| POST | `/v1/outbound` | `{"mode":…}` | `{}`；`proxy` 且未设全局策略 → 400 |
| GET | `/v1/outbound/global` | | `{"policy":"<name>"\|null}` |
| POST | `/v1/outbound/global` | `{"policy":"<name>"}`（空串清除） | `{}`；未知策略 → 400 |
| GET | `/v1/policies` | | `{"proxies":[…5 个内置策略 + 配置策略],"policy-groups":[…]}` |
| GET | `/v1/rules` | | `{"rules":[{"index":0,"rule":"DOMAIN,…","hits":3}]}` |
| GET | `/v1/requests/recent?limit=N` | 默认 100；上限是 `--request-log-size` 的环形缓冲容量 | `{"requests":[Request…]}`（最新在前） |
| GET | `/v1/requests/active` | | 同上 |
| POST | `/v1/requests/kill` | `{"id":N}` | `{}`；404 不在进行中；409 `not killable: internal session` |
| GET | `/v1/traffic` | | 见下「Traffic」 |
| GET | `/v1/dns` | | `{"dnsCache":[Cache…],"upstreams":[…],"bootstrap":[…]}` |
| POST | `/v1/dns/flush` | | `{}` |
| POST | `/v1/test/dns_delay` | `{"name":"<host>"}`，缺省为 `internet-test-url` 的主机 | `{"delays":[{"upstream":"udp://…","ms":12,"error":null}]}` |
| GET | `/v1/profiles/current?sensitive=0\|1` | | `text/plain`；默认脱敏：`password`/`psk`/`private-key`/`base64` 参数、`ca-passphrase`、`ca-p12`、`key@` 前缀、`wifi-access-http-auth` 口令、`http`/`https`/`socks5`/`socks5-tls` 策略行里位置传递的凭据 → `***` |
| POST | `/v1/profiles/reload` | | `{"ok":true,"errors":0,"warnings":1,"listenersRebound":false}`；解析 / 重建失败或重绑监听器失败时 `ok:false`（运行中的配置保持不变） |
| POST | `/v1/profiles/check` | | `{"ok":…,"errors":N,"warnings":N,"diagnostics":[Diagnostic…]}`（校验磁盘上的当前配置，不影响运行；`Diagnostic` 与 `rurge check --json` 相同） |
| POST | `/v1/log/level` | `{"level":"verbose"\|"debug"\|"info"\|"notify"\|"warning"\|"error"}` | `{}` |
| GET | `/v1/features/{system_proxy\|enhanced_mode\|mitm\|capture\|rewrite\|scripting}` | | `{"enabled":false}` |
| POST | `/v1/features/{name}` | `{"enabled":bool}` | 501（`system_proxy` 在 M4b 生效） |
| GET | `/v1/modules` | | `{"enabled":[],"available":[]}` |
| GET | `/v1/scripting` | | `{"scripts":[]}` |
| GET | `/v1/events` | | `{"events":[]}` |
| POST | `/v1/stop` | | `{}`，随后进程退出（退出码 0） |

### Request

```json
{"id":12,"listener":"http","src":"127.0.0.1:51234","dst":"example.com:443","rule":"DOMAIN-SUFFIX,example.com,Proxy","policy":["Proxy","HK"],"sni":"example.com","protocol":"https","up":1234,"down":56789,"startedMs":1757200000000,"elapsedMs":812,"status":"completed","rejectKind":null,"error":null}
```

`listener` ∈ `http` `socks5` `tun` `forward` `internal`；`status` ∈ `active` `completed` `rejected` `failed`；`rejectKind` 在 `rejected` 时是 `REJECT` / `REJECT-DROP` / `REJECT-NO-DROP` / `REJECT-TINYGIF`；`protocol` 是嗅探到的协议小写名或 `null`。

### Traffic

```json
{"startTime":1757200000.5,"total":{"in":123,"out":45,"inCurrentSpeed":0,"outCurrentSpeed":0},"connector":{"DIRECT":{"in":123,"out":45}},"listener":{"http":{"in":123,"out":45},"socks5":{"in":0,"out":0}}}
```

`in` 是下行（远端 → 客户端）字节，`out` 是上行；速度是最近一秒的字节数；`startTime` 是 API 启动时的 Unix 时间戳，**秒**（`f64`，可带小数）——设计文档 §5.2 / 开放问题 Q1 曾写作毫秒，以本页为准。

### Cache

```json
{"domain":"example.com","data":["93.184.216.34"],"expiresTime":1757200060.2,"server":"udp://1.1.1.1:53","stale":false,"negative":false}
```

`expiresTime` 与 `startTime` 同单位（Unix 秒，`f64`）；条目没有剩余 TTL 信息时为 `null`。

## CLI 客户端

`rurge reload | stop | status [-c <conf>] [--remote host:port] [--key <key>] [--platform <p>]`，`status` 另有 `--json`。地址 / 密钥：`--remote` + `--key`（或 `RURGE_API_KEY`）→ `-c` 配置的 `http-api`（只解析）→ 都未提供时退出 2，并打印 `http-api is not configured; add "http-api = <key>@127.0.0.1:6171" to [General] or pass --remote/--key (reload can also be triggered by SIGHUP or --watch)`。退出码：2 参数缺失或鉴权失败（401 / 403），1 连不上、非 2xx，或 `reload` 语义失败（HTTP 200 但 `ok:false`），0 成功。
