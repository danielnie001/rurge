# rurge HTTP API（阶段 1）

rurge 在 `[General] http-api = <key>@<ip>:<port>` 指定的地址上提供 Surge 兼容的 HTTP API。本页记录阶段 1（M4a / M4b）实现的端点与 rurge 暂定的 JSON 结构；手册未定义的结构在阶段 6 与真实 Surge 对齐时可能调整（见 `docs/surge-compatibility-matrix.md` 第 10.4 节）。

## 鉴权与错误

- 每个请求带 `X-Key: <key>` 头或 `?x-key=<key>` 查询参数；比较为常量时间。查询参数取字面值，不做百分号解码，因此只适用于 ASCII 密钥；含 `%` 等特殊字符的密钥请用 `X-Key` 头。
- 失败：`401 {"error":"unauthorized"}`。同一来源 IP 10 分钟内 5 次失败 → 之后 10 分钟内一律 `403 {"error":"banned"}`；封禁表最多跟踪 1024 个来源。
- 所有错误都是 `{"error":"<message>"}`：400（参数、JSON 请求体或查询字符串错误）、401、403、404（未知端点 / 未知请求 id / 未知功能名）、405（`{"error":"method not allowed"}`，请求方法与已注册路径不匹配，如对 `/v1/outbound` 发 `DELETE`）、409（不可终止的内部会话）、500（内部错误）、501（本阶段未实现）。
- 无内容的成功响应是 `{}`。字段名 camelCase；手册已定义的字段（如 `policy-groups`、`dnsCache`）照抄。
- `http-api` 绑定到非环回地址时启动时打 WARN；改动 `http-api` 需重启 rurge（重载只警告，继续用旧地址 / 密钥）。

## 端点

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/v1/outbound` | | `{"mode":"direct"\|"proxy"\|"rule"}` |
| POST | `/v1/outbound` | `{"mode":…}` | `{}`；`proxy` 且未设全局策略 → 400 |
| GET | `/v1/outbound/global` | | `{"policy":"<name>"\|null}` |
| POST | `/v1/outbound/global` | `{"policy":"<name>"}`（空串清除） | `{}`；未知策略 → 400；出站模式为 `proxy` 时用空串清空 → 400（先切到 direct / rule） |
| GET | `/v1/policies` | | `{"proxies":[…5 个内置策略 + 配置策略],"policy-groups":[…]}` |
| GET | `/v1/rules` | | `{"rules":[{"index":0,"rule":"DOMAIN,…","hits":3}]}` |
| GET | `/v1/requests/recent?limit=N` | 默认 100；上限是 `--request-log-size` 的环形缓冲容量；`limit=0` → 400 `limit must be at least 1` | `{"requests":[Request…]}`（最新在前） |
| GET | `/v1/requests/active` | | 同上 |
| POST | `/v1/requests/kill` | `{"id":N}` | `{}`；404 不在进行中；409 `not killable: internal session` |
| GET | `/v1/traffic` | | 见下「Traffic」 |
| GET | `/v1/dns` | | `{"dnsCache":[Cache…],"upstreams":[…],"bootstrap":[…]}` |
| POST | `/v1/dns/flush` | | `{}` |
| POST | `/v1/test/dns_delay` | `{"name":"<host>"}`，缺省为 `internet-test-url` 的主机 | `{"delays":[{"upstream":"udp://…","ms":12,"error":null}]}` |
| GET | `/v1/profiles/current?sensitive=0\|1` | | `text/plain`；默认（`sensitive=0`）把下列内容替换为 `***`，其余内容与行数、行尾 CRLF 原样保留：① 独立成行的密钥 `key = value`（键名大小写不敏感）`password`、`ca-passphrase`、`ca-p12`、`private-key`、`psk`、`pre-shared-key`、`token`；② 值里任意位置的内联参数 `name = value`（值到**第一个顶层逗号**为止，顶层按解析器自己的规则判定：`"` 或 `'` 在值里**任何位置**都会开启一段引号，`"` 内 `\` 转义下一个字符、`'` 内不转义，`(` / `)` 分组，引号内与括号内的逗号都属于值——`password="p,w"`、`password=ab"c,d"`、`password=a(b,c)d` 都是一整个值；引号或括号未闭合时抹到行尾。参数名前面必须是行首、逗号、空白或 `(`）`password`、`psk`、`private-key`、`pre-shared-key`、`base64`、`token`、`uuid`、`username`、`headers`、`ws-headers`、`ws-path`、`shadow-tls-password`、`policy-path`、`external-policy-modifier`、`test-url`（后三个是阶段 2 加的：订阅链接与订阅行设的测试 URL 常带 token，修饰列表能设任何参数；`username` 会连带脱敏无害的 SSH 用户名；`headers=` 与 `ws-headers=` 的值整体被抹掉，连 header 名也不保留，`ws-path=` 的值同样整体被抹掉——这是有意的过度脱敏，自定义 header 的值与 WebSocket 路径都属于凭据）；③ `http-api` / `external-controller-access` / `http-listen` / `socks5-listen` 的 `key@` 前缀与 `wifi-access-http-auth` 的口令；④ 所有写作 `type, server, port` 的代理类型（`http` `https` `h2-connect` `socks5` `socks5-tls` `ss` `snell` `vmess` `trojan` `tuic` `tuic-v5` `hysteria2` `masque` `anytls` `trust-tunnel` `ssh`）策略行第 4 个起的 token——**凡不是 `name=value` 具名参数的 token 一律抹掉**，最常见的就是根本不含 `=` 的裸 token（位置凭据）；含 `=` 时还要首个 `=` 之后非空且不全是 `=` 才算具名参数并保留（`tfo=true` 保留，`aHVudGVyMg==` 脱敏，`sni=` 属于可接受的过度脱敏；以引号开头的 token 整个算一个位置值，里面的 `=` 与逗号都不作数）。切分只在顶层逗号处进行（引号内与括号内的逗号不切，与 ② 同一套扫描）。前四种是 Surge 文档化的位置凭据写法；其余类型这个位置本就是多余参数（`W0001`），按偏安全一侧一并抹掉。列出的以外一律不脱敏 |
| POST | `/v1/profiles/reload` | | `{"ok":true,"errors":0,"warnings":1,"listenersRebound":false}`；解析失败或构建 `Runtime` 失败时 `ok:false`，运行中的配置不变；重绑监听器失败时也是 `ok:false`，但新配置这时已经生效，只是监听器归零，直到下一次重载成功为止（下一次重载会无条件重试绑定） |
| POST | `/v1/profiles/check` | | `{"ok":…,"errors":N,"warnings":N,"diagnostics":[Diagnostic…]}`（校验磁盘上的当前配置，不影响运行；`Diagnostic` 与 `rurge check --json` 相同） |
| POST | `/v1/log/level` | `{"level":"verbose"\|"debug"\|"info"\|"notify"\|"warning"\|"error"}` | `{}` |
| GET | `/v1/features/{system_proxy\|enhanced_mode\|mitm\|capture\|rewrite\|scripting}` | | `system_proxy` 返回真实状态 `{"enabled":bool}`；其余恒 `{"enabled":false}` |
| POST | `/v1/features/{name}` | `{"enabled":bool}` | `system_proxy`：成功 `{}`；失败 500 `{"error":"<原因>"}`（例如不支持的 Linux 桌面会带 `export http_proxy=…` 提示）；其余功能 501 |
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

`total` 统计所有会话，包括仍在进行中的会话与 rurge 自己的内部 DNS 拨号（与 `inCurrentSpeed` / `outCurrentSpeed` 同源，长传输期间不会出现「总量 0 但速度非 0」）；`connector` 与 `listener` 只统计已结束的会话，且 `listener` 在阶段 1 只有 `http` / `socks5` 两项。因此这三个层级不应相互对得上，不要拿它们互相校验。

### Cache

```json
{"domain":"example.com","data":["93.184.216.34"],"expiresTime":1757200060.2,"server":"udp://1.1.1.1:53","stale":false,"negative":false}
```

`expiresTime` 与 `startTime` 同单位（Unix 秒，`f64`）；条目没有剩余 TTL 信息时为 `null`。

## 系统代理

M4b 起 `system_proxy` 已生效（见上面「端点」表）；这里记录地址取值、`skip-proxy` 转换与生命周期。

- **地址**：http / https 取第一个 `http-listen`；`set-system-socks-proxy = true`（默认）时 socks 取第一个 `socks5-listen`，为 false 时不设置 socks。监听地址是通配地址时换成**同族**回环：`0.0.0.0` → `127.0.0.1`，`::` → `::1`（`[::]` 在 Windows 上默认只监听 v6，`127.0.0.1` 连不上）。一个 http/socks5 监听器都没有时视为「未启用」，`POST` 打开会失败。
- **`skip-proxy` → 系统绕过列表**：采用 macOS 语义。取反项（`-host`）与 `<…>` 特殊记号（`<ip-address>` 等）整条丢弃；端口一律丢弃（`host:port` 只留主机部分）；CIDR 网段保留 `/前缀`、主机位清零（如 `192.168.1.5/16` → `192.168.0.0/16`），单地址网段（`/32`、`/128`）写成裸地址。macOS 与 Linux（GNOME `ignore-hosts` / KDE 逗号拼接的 `NoProxyFor`）原样使用这份列表；Windows 的 `ProxyOverride` 没有 CIDR 语法，把 IPv4 网段展开成通配符模式（`10.0.0.0/8` → `10.*`；`172.16.0.0/12` 展开成 16 条 `172.16.*`…`172.31.*`；单条 `/1`、`/9`、`/17` 或 `/25` 网段最多展开到 128 条，没有上限），IPv6 字面量加中括号（`::1` → `[::1]`），IPv6 网段无法表示、丢弃。`exclude-simple-hostnames` 只在 Windows 生效（追加 `<local>`）；macOS 经 `networksetup` 无法设置，`true` 时 WARN 并忽略这一项，其余设置照常应用；Linux 无对应项。
- **`state.json` 备份与崩溃恢复**：开启前先 `snapshot()` 当前系统设置，连同 `features.system_proxy = true` 一起写入 `state.json`（先于 `apply`，这样崩溃后仍能找回原始设置；已经存在的备份不会被覆盖——它就是最初的原值）；`apply` 失败会尽力 `restore` 并清空这两个字段。关闭或优雅退出 → `restore` 并清空。启动时若 `system_proxy_backup` 非空（上一次异常退出留下的）→ 先 `restore` 并打一条 WARN，再按本次是否带 `--system-proxy` 决定要不要重新开启（详见下一条）；`state.json` 的写入是尽力而为（原子写但不 `fsync`），写失败只记日志，不影响调用方。
- **崩溃恢复不依赖本次能否启动**：任何一次 `rurge run` 都做这件事，**不需要**带 `--system-proxy`——只要 `state.json` 里有非空的 `system_proxy_backup` 就恢复。这一步紧跟打开 `state.json` 之后，**早于**监听器绑定与 `http-api` 绑定：上一次崩溃留下的端口往往正是本次绑不上的原因，所以即使本次以 `error: cannot bind listener`（或 `cannot bind http-api`）退出 1，崩溃前的系统设置也已经放回去了；否则机器会一直指着一个死掉的端口，而且每次重试都卡在同一处。排在恢复之前的只剩加载配置、打开日志文件、解析运行时参数和取数据目录的实例锁（见下一条）这几步：前三步失败时退出 2，rurge 还没有打开 `state.json`，恢复要等下一次能走到这一步的启动；实例锁被另一个还在运行的实例占着时退出 1——那份备份属于它，本来就不该由别的进程恢复。
- **带 `--system-proxy` 时一台机器只跑一个实例**：备份存在各自的 `--data-dir` 的 `state.json` 里。两个实例用**不同**的数据目录先后开启系统代理时，第二个实例会把第一个实例刚写进去的地址当成「原始设置」快照下来；两个都退出后，系统设置停在一个已经失效的地址上，而两份 `state.json` 里都没有真正的原值可以恢复。共用同一个 `--data-dir` 的情况由实例锁挡住：每次 `rurge run`（不限于带 `--system-proxy` 的）都会在打开 `state.json` **之前**对 `<data-dir>/rurge.lock` 取独占锁，同一数据目录上的第二个实例在碰任何东西之前就以 `error: another rurge instance is already running with the data directory <路径>` 退出 1——既不改系统设置，也不动 `state.json`。锁由操作系统在进程死亡时释放，所以崩溃之后的下一次启动照常拿到锁并恢复备份；反过来说，握着锁时在 `state.json` 里看到的备份必定属于一个已经死掉的运行，而不是一个还活着的实例。锁文件从不删除——删除会与另一个进程的打开竞争。文件系统不支持加锁（或锁文件打不开）时，rurge 记一条 WARN 后不带这项检查继续启动：不能因为文件系统的限制把守护进程整个丢掉。
- **运行期不巡检**：rurge 只在开启、重载跟随、关闭这三个时刻写系统设置。运行期间别的程序（或用户自己）改了系统代理，rurge 既不会察觉也不会重新写回去；退出时 rurge 把**开启那一刻**的快照原样写回，覆盖掉期间的任何改动。
- **`RURGE_SYSTEM_PROXY_BACKEND` 是测试钩子**：取值只能是 `file:<path>`，把「系统代理」换成对一个 JSON 文件的读写（端到端测试用，见设计文档 §15 的 P5）。其它任何取值都直接报错退出，不会静默退回真实后端——否则测试里的一个拼写错误就会改动开发机的真实设置。这个变量不是给用户用的。
- **重载跟随**：`POST /v1/profiles/reload`（或 SIGHUP / `--watch`）之后，如果系统代理开着，rurge 会用新配置重新计算地址并 `apply`（监听地址或 `skip-proxy` 变了就能看出来）。如果重载后没有任何可用的监听器，rurge **不会**顺带关闭系统代理——那等于替用户把流量静默改成直连——而是打一条 WARN，系统设置继续指向（已经失效的）旧地址，直到下一次重载成功或者进程退出。
- **启动输出顺序**：`listening on …`（每个监听器一行）→ `api on http://<addr>`（配置了 `http-api` 时）→ `system proxy enabled: <地址描述>`（带 `--system-proxy` 启动且成功开启时；地址描述形如 `http 127.0.0.1:6152` 或 `http 127.0.0.1:6152, socks 127.0.0.1:6153`）→ 汇总行 `rurge <version> running: …`。开启失败会改为 stderr 打印 `error: cannot enable the system proxy: <原因>` 并以退出码 1 结束，不打印汇总行。

## CLI 客户端

`rurge reload | stop | status [-c <conf>] [--remote host:port] [--key <key>] [--platform <p>]`，`status` 另有 `--json`。地址 / 密钥：`--remote` + `--key`（或 `RURGE_API_KEY`）→ `-c` 配置的 `http-api`（只解析）→ 都未提供时退出 2，并打印 `http-api is not configured; add "http-api = <key>@127.0.0.1:6171" to [General] or pass --remote/--key (reload can also be triggered by SIGHUP or --watch)`。退出码：2 参数缺失或鉴权失败（401 / 403），1 连不上、非 2xx，或 `reload` 语义失败（HTTP 200 但 `ok:false`），0 成功。
