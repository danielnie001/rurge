# 阶段 2 手工验收清单

需要真实公网节点的项目，自动化测试（只用回环）覆盖不了，由项目所有者用自己的节点验收。每一项记下日期、平台与结果。

## M2a　Trojan

前置：一份只含自己节点的配置（`[Proxy]` 里一条 `trojan` 策略，`[Rule]` 里 `FINAL,<策略名>`），`rurge check -c <配置>` 零错误。

| # | 步骤 | 期望 |
| - | ---- | ---- |
| 1 | `rurge run -c <配置> --log-level info`，浏览器或 `curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` | 返回 200；`rurge status` 的请求记录里策略链是该 trojan 策略、`error` 为空 |
| 2 | 同上，经 SOCKS5：`curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` | 同上 |
| 3 | 明文 HTTP：`curl -x http://127.0.0.1:<端口> http://example.com/ -I` | 返回 200（经隧道，不是绝对 URI 转发） |
| 4 | 服务端先说话的协议：经代理 `ssh -o ProxyCommand='…' <任一公网 SSH 主机>` 或 `curl --socks5-hostname … telnet://<SMTP 主机>:25` | 能看到对端的欢迎行（客户端 100 ms 内不发数据时请求头单独发出） |
| 5 | 把配置里的 `password` 改错一位，重载 | 连接能建立但拿不到正常响应（协议性质：密码错误在连接期无法识别）；会话记录的 `error` 为空或为转发阶段的错误，**不含口令** |
| 6 | （节点带 WebSocket 时）`ws=true, ws-path=…, ws-headers=Host:…` | 同第 1 项 |
| 7 | `GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` | 输出里 `password`、`ws-path`、`ws-headers` 都是 `***` |
| 8 | 在 `select` 组里放两条 trojan 策略，经 `POST /v1/policy_groups/select` 切换 | 下一条连接走新选中的策略 |

## M2b　VMess

前置：一份只含自己节点的配置（`[Proxy]` 里一条 `vmess` 策略，带 `vmess-aead=true`，`[Rule]` 里 `FINAL,<策略名>`），`rurge check -c <配置>` 零错误（没有 `W0007`）。

| # | 步骤 | 期望 |
| - | ---- | ---- |
| 1 | `rurge run -c <配置> --log-level info`，浏览器或 `curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` | 返回 200；`rurge status` 的请求记录里策略链是该 vmess 策略、`error` 为空 |
| 2 | 同上，经 SOCKS5：`curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` | 同上 |
| 3 | 把配置里的 `username`（UUID）改错一位，重载后重复第 1 步 | 会话记录的 `error` 是 `vmess: the server closed the connection without answering`；**不含 UUID 原文** |
| 4 | 改回正确的 UUID，把本机时钟拨偏 3 分钟（超出协议约 120 秒的容忍窗口），重复第 1 步 | 表现与第 3 步相同，同一条错误文本，分辨不出是 UUID 错还是时钟偏差（见 `docs/surge-compatibility-matrix.md` 4.2 `vmess` 行）；改回时钟后恢复正常 |
| 5 | （节点带 WebSocket 时）`ws=true, ws-path=…, ws-headers=Host:…` | 同第 1 项 |
| 6 | `GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` | 输出里 `username` 是 `***` |

## M2b　AnyTLS

前置：一份只含自己节点的配置（`[Proxy]` 里一条 `anytls` 策略，`[Rule]` 里 `FINAL,<策略名>`），`rurge check -c <配置>` 零错误。

| # | 步骤 | 期望 |
| - | ---- | ---- |
| 1 | `rurge run -c <配置> --log-level info`，浏览器或 `curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` | 返回 200；`rurge status` 的请求记录里策略链是该 anytls 策略、`error` 为空 |
| 2 | 把配置里的 `password` 改错一位，重载后重复第 1 步 | 连接能建立但拿不到正常响应，或会话记录的 `error` 是 `anytls: the session is closed`（协议性质：服务端把认不出的连接当普通网站处理）；**不含口令原文** |
| 3 | 改回正确的口令，连续发两次独立的请求（如两次 `curl`） | 服务端自己的日志里只看到一条 TLS 连接（第二个请求的流复用了第一个的会话） |
| 4 | 改一条与这条 anytls 策略无关的策略后 `rurge reload`，再请求一次 | 服务端日志里仍是同一条 TLS 连接（未改动的 anytls 策略按指纹复用了上一代的出站与连接池） |
| 5 | 把这条 anytls 策略自己的参数（如 `password`）改掉后 `rurge reload`，再请求一次 | 服务端日志出现新的一条 TLS 连接（出站被换新，旧连接池随之释放） |
| 6 | `GET /v1/policies/detail?policy_name=<策略名>` | 输出里 `password` 是 `***` |

不要在验收时使用 `--system-proxy`，除非你确实想让本机的系统代理指向 rurge（退出时会恢复）。
