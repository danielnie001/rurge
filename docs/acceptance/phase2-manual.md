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

## M2c　Shadow TLS

需要一台真实的 Shadow TLS 服务端（官方 `shadow-tls` 或 sing-box 的 `shadowtls` 入站）与一个真实的伪装站点，自动化测试（只用回环）覆盖不了。

- [ ] v3：`<协议>, <服务器>, <端口>, …, shadow-tls-password=…, shadow-tls-version=3, shadow-tls-sni=<伪装站点>`，经它打开几个 HTTPS 网站、下载一个 100 MB 以上的文件、保持一条长连接 10 分钟以上，均正常。
- [ ] v3：服务端分别是官方 `shadow-tls`（`--v3 --strict`）与 sing-box（`strict_mode: true`）时都成立。
- [ ] v3：口令写错——会话记录的错误是 `shadow-tls: the server did not authenticate itself`；在服务端一侧抓包可见客户端与伪装站点完成了握手并发了一个 HTTP 请求。
- [ ] v3：`shadow-tls-sni` 指向一个只支持 TLS 1.2 的站点——错误是 `shadow-tls: the handshake server does not support TLS 1.3`。
- [ ] v2：同样的三项（浏览、大文件、长连接）；服务器以 IP 配置而不写 `shadow-tls-sni` 时，错误文本明确指向证书与名字对不上。
- [ ] v2：伪装站点是自己的域名（服务端把握手转给同一个域名的真实站点）且不写 `shadow-tls-sni`：连接成功，服务端一侧抓包可见 ClientHello 里没有 SNI。
- [ ] 半关闭：`curl --http1.0` 之类"发完请求就关闭写方向"的客户端经 sing-box 服务端下载一个大文件，内容完整（对应清单 4.4 里"alert 记录跳过"那一条）。
- [ ] `underlying-proxy`：带 Shadow TLS 的策略作为链的出口。
- [ ] `GET /v1/policies/detail` 与 `GET /v1/profiles/current` 里 `shadow-tls-password` 的值是 `***`。

## M3a　成员装配与订阅

需要一个真实的机场订阅链接（Surge 格式）与其中至少两个可用节点，自动化测试（只用回环）覆盖不了。

- [ ] `G = select, policy-path=<订阅 URL>, update-interval=3600`：数据目录里没有缓存时首次启动，`GET /v1/policy_groups` 里 `G` 先是空的，几秒内出现订阅里的节点；经 `POST /v1/policy_groups/select` 选一个节点后浏览正常。
- [ ] 重启 rurge：`G` 的成员一启动就在（从缓存载入），没有空组阶段；`rurge reload` 同样。
- [ ] 标准输出、`--log-file` 的日志（含 `--log-level verbose`）里搜不到订阅链接里的 token；`GET /v1/profiles/current` 与 `GET /v1/policies/detail?policy_name=G` 里 `policy-path` 的值是 `***`。
- [ ] `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier="tfo=true"`：成员名与 `policies/detail` 里的定义符合预期。
- [ ] 组级 `underlying-proxy=<中继>`：成员名显示为 `<节点> (via <中继>)`；经它浏览时，中继服务器一侧能看到到节点服务器的连接。
- [ ] 把订阅换成一个 Clash YAML 链接：启动输出里有 `W0023`（内容可能不是 Surge 格式），经该组的请求直连（空组兜底）；加 `--empty-group-reject` 后同样的请求被拒绝。
- [ ] 机场更新了订阅（或手动改一个本地订阅文件）：不重启、不重载，组成员随之变化，日志里有一条 `policy group members updated`（只有组名与增减数量）；一个正在进行的大文件下载不中断。
- [ ] 用一个不带格式参数的通用订阅链接（机场面板可能按 `User-Agent` 选格式，rurge 发的是 `rurge/<版本> (Surge-compatible)`）：组是否照常填充；填不出时记下面板名称与实际返回的内容（`docs/surge-compatibility-matrix.md` 的 `policy-path` 行）。

## M3b　测速与自动组

需要至少两个延迟明显不同的真实节点（可以来自 M3a 的订阅），自动化测试（只用回环）覆盖不了。

- [ ] `Auto = url-test, <节点 A>, <节点 B>`（不写 `test-url`，即默认的 `http://bing.com/`）：经 `Auto` 浏览几次后，`GET /v1/policy_groups/test_results` 里两个节点都有 `delay`，数值与 Surge（或节点面板）的延迟量级相当；`GET /v1/policy_groups/select?group_name=Auto` 是较快的那个。
- [ ] `GET /v1/requests/recent` 里能看到测试会话（`rule` 为 `policy test`），它们经各自的节点出去（`policy` 是节点名），目标只有 `bing.com:80`，没有路径。
- [ ] 给一个节点写 `test-url=https://www.gstatic.com/generate_204`：测试通过，`delay` 与 HTTP 测试 URL 的同量级（第二次 `HEAD` 复用已建立的 TLS 连接，不含握手）。
- [ ] `Fallback = fallback, <节点 A>, <节点 B>, interval=60`：停掉节点 A（或把它的端口写错后 `rurge reload`），一分钟内经 `Fallback` 的请求改走节点 B；`POST /v1/policy_groups/test {"group_name":"Fallback"}` 立即返回只含 B 的 `available`。
- [ ] `Balance = load-balance, <节点 A>, <节点 B>, persistent=true`：同一个网站的多次请求在请求记录里都经同一个节点，不同网站分散到两个节点。
- [ ] `evaluate-before-use=true`：刚启动后第一次经该组的请求多等一会（测完一轮）再经可用的节点出去；两个节点都不可用时这次请求失败，请求记录的错误是 `policy group evaluation failed`。
- [ ] `POST /v1/policy_groups/select {"group_name":"Auto","policy":"<较慢的节点>"}`：之后的请求经较慢的节点，且不再产生 `Auto` 的测试会话；`{"policy":""}` 清除后恢复自动选择；重启 rurge 后覆盖不在了。
- [ ] 日志（含 `--log-level verbose`）里搜不到任何策略的 `test-url` 的路径与参数（订阅行可能把 token 放在测试 URL 里）。
