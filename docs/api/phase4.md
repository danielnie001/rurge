# rurge HTTP API（阶段 4）

阶段 4 / M1a（嗅探与规则之前的 SNI，设计 `docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md` 的第 4 ～ 7、11、12 节，计划 `docs/superpowers/plans/2026-10-29-phase4-m1a-sniffing-plan.md`）**新增一个端点** `GET /v1/rurge/http`，改了请求记录里 `sni` 与 `protocol` 的取值，并且没有新增日志行（见「日志」）。鉴权（`X-Key`）、错误体（`{"error":"<message>"}`）与无内容的成功响应（`{}`）沿用阶段 1 的约定，见 `docs/api/phase1.md`；本页只记录变了的与新增的部分。M1b（明文 HTTP 引擎）在同一份文档里加引擎的端点、记录的变化与 `GET /v1/rurge/http` 的 `requests`。

**这一份写的是本地提交的行为：M1a 的提交都在本地，CI 没有见过。** 下面每一处行为都只在一台 Windows 11 x86-64 开发机上的一次运行里由用例断言过（一台机器上的一次运行，不是 CI），没有手工验收；阶段 4 的手工清单从 M2 起建，M1 的行为随它写进去。

## M1a　嗅探与规则之前的 SNI

可嗅探的会话（经 SOCKS5、HTTP 监听器的 `CONNECT` 与虚拟网卡来的 TCP 会话，端口 80、443 或命中 `force-http-engine-hosts`，其余条件见「可嗅探的条件与等多久」）在答复之后先读头一批字节，判出 TLS、HTTP 或别的，再带着 SNI 与协议求规则、拨号。判为 HTTP 的会话在 M1a 照原样转发、协议记 `http`，进 HTTP 引擎随 M1b；判为 TLS 的照原样转发，解密随 M2 的 MITM。

### `GET /v1/rurge/http`

`/v1/rurge/…` 是总设计给 rurge 专有接口定的前缀，Surge 没有这个端点（设计第 11 节，M1-D24）。它要钥匙，与别的端点在同一层：没有钥匙回 401 `{"error":"unauthorized"}`。

应答的形状（`crates/rurge-api/src/routes/rurge_http.rs`）：

```json
{"sniffed":{"tls":0,"http":0,"other":0,"timeout":0,"closed":0}}
```

键的次序就是 `tls`、`http`、`other`、`timeout`、`closed`；用例 `rurge_http_counts_start_at_zero`（`crates/rurge-api/tests/api.rs`）断言正文的字节恰好是这一串，另外断言不带钥匙时是 401。

每个数是一个嗅探结局的次数（设计第 7.5 节）：

- `tls`：读到完整的 ClientHello，判为 TLS；
- `http`：判为明文 HTTP；
- `other`：判为别的（读不懂的 ClientHello、不是方法记号的开头、`PRI` 一类）；
- `timeout`：等满时限（15 秒）还没有判出来；
- `closed`：等满时限之前客户端关闭了写端，或读出错（计划 P4：读出错也计在这里）。

嗅探被停止（虚拟网卡停、守护进程退出）打断的不计——它没有结局。自进程启动起累计，重载不清零。M1a 只有 `sniffed`；M1b 加 `requests`（走到选路的请求数，设计第 11 节）。

### 请求记录

可嗅探的会话的 `sni` 与 `protocol` 在求规则之前就定了（设计第 7.1、7.2 节）：判为 TLS 的，`sni` 是 ClientHello 里第一个 `host_name`（转成小写），`protocol` 是 `https`；判为 HTTP 的，`protocol` 是 `http`；判为别的、超时与关闭的不设协议。

记录里这两个键各取自哪里，照 `crates/rurge-engine/src/observe.rs` 的 `record_of` 的那两行：

- `sni` 是 `s.sni.clone().or_else(|| h.sni())`：先取会话信息里的（嗅探读出的完整 SNI），没有时取句柄侧槽里的（不可嗅探的会话由转发循环从首个客户端数据块至多 8 KiB 里认出 SNI 时写进去的）；
- `protocol` 是 `h.protocol().or(s.protocol)`：先取句柄侧槽里的（转发循环的首块嗅探写的），没有时取会话信息里的。

不可嗅探的会话照旧：SNI 与协议只在出站建立之后的首块嗅探里认出，决定策略时规则看不见它们。

可嗅探的会话在被嗅探完、拨号时才建句柄与记录：嗅探中途被停止的会话没有记录（被拒绝、拨号失败的会话有记录，同今天）。读出错的会话不拨号，也没有记录。

### 日志

嗅探不另写日志行：结局在计数里，协议与 SNI 在记录与会话日志里（设计第 12 节）。

- `CONNECT` 的会话在升级失败、还没有会话时写一行 DEBUG `CONNECT upgrade failed before the session began`，带 `listener = "http"` 与 `error`，不带地址（计划 P8）。
- SOCKS5 写成功应答失败时照今天由监听器已有的那一行 DEBUG（`socks5 session ended with an error`，带 `source`）记下，不另写（计划 P8）：可嗅探的会话这时还没有句柄。

虚拟网卡上的名字与地址照旧只在 DEBUG。

### 可嗅探的条件与等多久

可嗅探的条件是：TCP、经三种入口（`CONNECT`、SOCKS5、虚拟网卡）、引擎没有暂停、预匹配不拒（只对经代理端口来的会话问一次，且不计数）、不命中 `always-raw-tcp-hosts` 与 `always-raw-tcp-keywords`、端口是 80 或 443 或命中 `force-http-engine-hosts`（这个名单的缺省端口是 80）；经代理端口来、目标是 rurge 地址空间里的 fake 地址时，先换回它背后的名字再比。等多久：15 秒（`EngineShared.sniff_wait` 只供测试调短，不进命令行与环境变量）。全文与登记的差异见兼容性清单 7.1 节与设计第 4、6、13 节，这里不重抄。

### 没有覆盖的

- 没有手工验收；**M1a 的提交都在本地，CI 没有见过**，上面每一条只在一台机器上的一次运行里证明过。
- `timeout` 那一个数只在把 `EngineShared.sniff_wait` 调短的用例里出现过；没有用例等满缺省的 15 秒，用缺省的 15 秒、不调短的只有一条（`a_stop_during_the_wait_ends_a_tun_session_at_once`），它让停止落在等待之中，证明的是停止、不是时限。
- 只有 HTTP 监听器与 SOCKS5 的应答次序改了；`CONNECT` 的 502 错误页、SOCKS5 的失败应答码在可嗅探的会话上不再出现，那两种失败现在是一个随即关闭的隧道（设计第 13 节第 1 条，登记在兼容性清单 7.1 节），没有在真机上观察过。
