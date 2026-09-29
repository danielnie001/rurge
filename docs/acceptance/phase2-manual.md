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

## M3c　smart

需要至少三个真实节点，其中一个可以随时停掉（或把它的端口写错后 `rurge reload`），自动化测试（只用回环）覆盖不了。

- [ ] `Smart = smart, <节点 A>, <节点 B>, <节点 C>`：经 `Smart` 正常浏览十几分钟，`GET /v1/requests/recent` 里经 `Smart` 的会话都有 `connectMs` 与 `firstByteMs`（规则为 `policy test` 的测试会话两者为 `null`），数值与节点的实际延迟量级相当；`GET /v1/policy_groups/select?group_name=Smart` 是这段时间用得最多的节点。
- [ ] 停掉节点 A：之后经 `Smart` 的请求仍然正常，个别会话的 `error` 是 ``smart group `Smart`: `A` failed to connect, used `B` ``；A 第一次失败之后，新会话基本不再选 A。恢复 A 之后，它要等失败罚分（每次 800 ms，5 分钟减半）衰减到分数回到最好节点的 1.2 倍以内才会重新被选到，通常要十几到二十几分钟。
- [ ] 一个能连上、但出口访问不了某个 HTTPS 网站的节点：访问那个网站几次之后，经 `Smart` 访问它改走别的节点，访问其它网站仍可能用这个节点（站点记忆）。明文 `http://` 网站经 HTTP 代理节点时不适用：代理回的错误页也算收到了回应。
- [ ] `policy-priority="<节点 C>:0.5"`：C 明显更常被选中；改为 `"<节点 C>:3"` 后 C 很少被选中。
- [ ] `POST /v1/policy_groups/select {"group_name":"Smart","policy":"<节点 B>"}`：之后固定走 B，停掉 B 时请求失败、不换成员；`{"policy":""}` 清除后恢复。
- [ ] 日志（含 `--log-level verbose`）里 `smart` 相关的行只有节点名，没有 URL 与凭据；经 `Smart` 用过的节点停掉后，连续三次失败（会话或测速）时有一条 `smart: the policy counts as failed`，恢复后第一次成功时有一条 `smart: the policy works again`；从没经 `smart` 组用过的策略不记这两行。
- [ ] 用一份混有尚未实现协议的节点（如 `ss`）、总数超过 12 个的订阅建 `smart` 组（`policy-path=<订阅>`）：请求照常，会话的 `policy` 链里从不出现 `ss` 节点；全部能测的节点都有了结论之后（连不上的节点要连续三次测试失败才算），`GET /v1/requests/recent` 里规则为 `policy test` 的会话大约每 5 分钟一批，不会接连不断。

## M4a　SSH

需要一台自己的 SSH 服务器（OpenSSH `sshd`，允许 TCP 转发），自动化测试（只用回环与假服务端）覆盖不了。

- [ ] 口令登录：`S = ssh, <服务器>, 22, username=<用户>, password=<口令>`，经 `S` 浏览几个网站正常；日志里有一条 `ssh: no server-fingerprint; the server's host key is not verified`（只带 `policy=S`），之后再用多少次都不再出现。
- [ ] 密钥登录：`ssh-keygen -t ed25519 -N "" -f key` 生成一把不带口令的密钥，`key.pub` 加进服务器的 `authorized_keys`，把 `key` 文件整个 Base64 编码后写进 `[Keystore]`（`key1 = type=openssh-private-key, base64=<…>`），`S = ssh, <服务器>, 22, username=<用户>, private-key=key1`：经 `S` 正常。换成 RSA 密钥（`ssh-keygen -t rsa -b 3072 -N ""`）同样正常，服务器日志（`LogLevel VERBOSE`；`journalctl -u ssh` 或 `/var/log/auth.log`）里记下的签名算法是 `rsa-sha2-512` 或 `rsa-sha2-256`。
- [ ] 带口令的私钥（`ssh-keygen -t ed25519 -N pw`）：`rurge check` 报 `E0022`，文本是 ``keystore item `key1` is protected by a passphrase, which rurge cannot use; remove the passphrase``，输出里没有私钥内容。
- [ ] 主机密钥校验：`ssh-keyscan -t ed25519 <服务器>` 的输出去掉开头的主机名，写成 `server-fingerprint="ssh-ed25519 AAAA…"`：连接正常，也没有上面那条告警；换成另一台机器的公钥后 `rurge reload`，经 `S` 的会话失败，请求记录的错误是 `ssh: the server's host key is not one of server-fingerprint`。
- [ ] 只钉 ECDSA（或 RSA）主机密钥：`ssh-keyscan -t ecdsa <服务器>`（或 `-t rsa`）的输出去掉开头的主机名，写成 `server-fingerprint="ecdsa-sha2-nistp256 AAAA…"`（或 `"ssh-rsa AAAA…"`）：连接正常——服务器同时有 Ed25519 主机密钥时，rurge 协商的是被钉住的那把的算法。
- [ ] 会话复用：同时开几个经 `S` 的下载，服务器上（`ss -tnp | grep sshd` 或 `last`）只看到 rurge 的一次登录、一条连接。
- [ ] 空闲断开：`idle-timeout=30`，最后一个经 `S` 的连接关掉 30 秒后，服务器上那条连接消失，再访问时重新登录；一个开着但没有流量的连接（如网页上的 WebSocket）不会因为 `idle-timeout` 被断开。
- [ ] 断线重建：让服务器断开 rurge 的会话（在服务器上结束那次登录对应的 `sshd` 进程，或重启服务器）之后，下一个经 `S` 的连接正常（重新登录）；断网两分钟再恢复之后同样正常（旧会话在 3 次保活无回应后判定已断）。
- [ ] 口令写错：会话失败，错误是 `ssh: authentication failed`；日志与 `GET /v1/requests/recent` 里没有用户名与口令；第一次失败之后约一分钟内，新连接不连服务器、立即得到同一个错误；一直有请求（开着网页、rurge 做系统代理）时，服务器日志（`journalctl -u ssh` 或 `/var/log/auth.log`）里失败的登录前后相隔约 1、2、4、8 分钟，之后每 10 分钟一次，同时打开很多网页也不会多出来；把口令改对后 `rurge reload`，下一个连接立即登录成功（改了行的策略重建出站，退避从头开始）。
- [ ] 日志（含 `--log-level verbose`）里搜不到口令与私钥内容。

## M4b　WireGuard

需要一个自己的 WireGuard 服务端（`wg-quick`、路由器或云主机均可），WARP 一项另需一份 Cloudflare WARP 的配置（带 `client-id`）。自动化测试只用回环与假对端，对 sing-box 的互操作只在 CI 上跑，覆盖不了真实网络。

- [ ] 基本连通：照服务端写 `[WireGuard home]`（`private-key`、`self-ip`、`peer = (public-key = …, allowed-ips = 0.0.0.0/0, endpoint = <服务器>:<端口>)`）与 `WG = wireguard, section-name=home`，经 `WG` 浏览几个网站正常；日志里有一条 `wireguard: handshake completed`（只带 `policy=WG peer=1`）；服务端 `wg show` 里 rurge 这个 peer 有 latest handshake 与收发字节。
- [ ] 目标域名：不写 `dns-server` 时目标域名在本机解析（`[Host]` 里写的映射生效）；写上 `dns-server = <隧道那头的 DNS>` 后经隧道查询（服务端抓包或 DNS 日志能看到查询），查不到的名字请求失败，错误是 `dns: wireguard: dns lookup of <名字> failed`。
- [ ] 路由：`allowed-ips` 只写服务端内网网段（如 `10.8.0.0/24`），规则把一个公网域名指到 `WG`：请求立即失败，错误是 `wireguard: no peer's allowed-ips covers <地址>`，没有改走直连。
- [ ] 吞吐：经 `WG` 下载、上传一个几百 MiB 的文件，速度与官方客户端在同一量级、全程不卡住；记下实测数字（自动化基准只测回环，两端都是 smoltcp）。
- [ ] WARP：用 WARP 的配置（`client-id = <三个数字>`，endpoint `engage.cloudflareclient.com:2408`），经 `WG` 访问 `https://www.cloudflare.com/cdn-cgi/trace`，输出里有 `warp=on`。
- [ ] WARP 启动后的第一次查询：WARP 的节写上 `dns-server = 1.1.1.1`，每次刚启动 rurge（或刚重载改了这个节）之后，第一个经 `WG` 的域名请求就成功（查询等隧道第一次握手完成后才发，没有回应时在 2 秒内重发）；重复几次（每次重启 rurge）都如此。
- [ ] 测速：把 `WG` 放进 `url-test` 组。不写 `dns-server` 与 `test-url` 时测速结果是握手往返时间（`GET /v1/policy_groups/test_results`），请求记录里测试会话的目标是服务端的 endpoint；写上 `test-url=http://…` 后改为经隧道的 URL 测试。
- [ ] 重载：只改无关的内容后 `rurge reload`，经 `WG` 的下载不中断、服务端没有新的握手；改了 `[WireGuard home]`（如 `mtu`）后 `rurge reload`，旧连接断开，新连接正常，服务端 `wg show` 里这个 peer 的 endpoint 稳定在一个地址上。
- [ ] endpoint 写成域名：让它的解析结果换成同一服务端的另一个地址（或另一台同配置的服务端），5 分钟内日志出现 `wireguard: the peer's endpoint moved`，之后的新连接走新地址。
- [ ] 服务端停掉：经 `WG` 的请求在拨号时限处失败；约 90 秒后日志有一条 `wireguard: the peer did not answer the handshake`，服务端一直停着时此后约每 90 秒再有一条（每次换一条新载体重试）；服务端恢复后下一个请求正常，日志再有一条 `wireguard: handshake completed`。
- [ ] 网络变化：经 `WG` 的连接在用时，把本机换到另一个网络（如从 Wi-Fi 换到手机热点，或拔掉网线改用 Wi-Fi），不重启 rurge：约两分钟内经 `WG` 的新连接恢复正常（若日志先记了一条 `wireguard: the peer did not answer the handshake`，恢复时会再记一条 `wireguard: handshake completed`）；服务端 `wg show` 里这个 peer 的 endpoint 变成新网络的出口地址。
- [ ] 日志（含 `--log-level verbose`）里搜不到私钥与 `preshared-key` 的内容；`GET /v1/profiles/current?sensitive=0` 里二者都是 `***`。

## M4c　external

前置：本机能用 `ssh` 登录一台自己的服务器（密钥登录，不需要输入口令），服务器的主机密钥已在 `known_hosts` 里。外部程序不能交互式提问（Unix 上读终端会被挂起，rurge 仍当它在运行），所以 `ssh` 一律带 `-o BatchMode=yes -o ExitOnForwardFailure=yes`（后者让本机端口绑定失败时 `ssh` 直接退出）。配置里写 `[Proxy]` 一条 `Ext = external, exec = "<ssh 的完整路径>", args = "-N", args = "-o", args = "BatchMode=yes", args = "-o", args = "ExitOnForwardFailure=yes", args = "-D", args = "127.0.0.1:1080", args = "<用户>@<服务器>", local-port = 1080`，`[Rule]` 里 `FINAL,Ext`；`rurge check -c <配置>` 零错误（没有 `W0007`）。三个平台各验一遍。

- [ ] 第一次用到时拉起：`rurge run -c <配置> --log-level info` 启动后没有 `ssh` 进程；`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 返回 200，此时有了 `ssh` 进程，日志里有 `external: the program started`（策略名与 pid，没有参数）。
- [ ] 日志文件：`<数据目录>/external/Ext.log` 里有一行 `--- rurge: starting the program …`，`ssh` 自己的输出（如有）在它后面。
- [ ] 再拉起：手动结束这个 `ssh` 进程，日志里有 `external: the program exited`；再发一个请求，2 秒左右后返回 200，出现了新的 `ssh` 进程。
- [ ] 停止：`rurge stop`（或 Ctrl-C）之后没有 `ssh` 进程残留（Windows：任务管理器；Unix：`ps`）；日志里有 `external: the program stopped`。
- [ ] Windows 上结束 rurge 进程（任务管理器里"结束任务"）：`ssh` 进程随之消失。
- [ ] Windows 上在运行 rurge 的控制台里按一次 Ctrl-C：日志里是 `external: the program stopped`（不是 `exited`），之后没有 `ssh` 进程残留。
- [ ] 重载时改了这一行（如多加一个 `args = "-v"`）而 `local-port` 不变：重载后第一个请求返回 200，只剩一个 `ssh` 进程（新的那个），日志里旧程序有 `external: the program stopped`。
- [ ] 未知主机：把服务器换成一台主机密钥不在 `known_hosts` 里的（或临时改名 `known_hosts`），带着 `BatchMode=yes`：请求失败（不挂住），`<数据目录>/external/Ext.log` 里有 `ssh` 给出的原因（如 `Host key verification failed.`）。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=Ext` 与 `GET /v1/profiles/current` 里每个 `args` 都是 `***`；日志（含 `--log-level verbose`）里搜不到服务器地址与用户名。
- [ ] UDP（M5a）：配置里写 `udp-relay=true`，把 `ssh -D` 换成一个支持 SOCKS5 UDP 的外部程序（`ssh -D` 不支持 UDP），经 rurge 的 SOCKS5 UDP 往返一次（见下面 M5a 一节的客户端）。
- [ ] 订阅：把一行 `external` 放进自己的订阅文件，重载后该行被跳过，`rurge check` 报 `` `external` policies are not imported from subscriptions ``。

## M5a　UDP 地基

前置：一个支持 SOCKS5 UDP 的客户端（如 Proxifier、SocksCap64，或设置了 SOCKS5 代理的游戏 / 语音软件、Telegram 桌面版的语音通话），指向 rurge 的 `socks5-listen`；一个开了 UDP 的上游 SOCKS5 节点（`udp-relay=true`）。

- [ ] DNS：客户端经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`），得到回答；`GET /v1/requests/recent` 里有一条 `transport` 为 `udp`、`dst` 为 `8.8.8.8:53` 的记录，回答后约 10 秒结束。
- [ ] 游戏或语音：经 rurge 的 SOCKS5 进行一次语音通话或联机游戏，DIRECT 与经上游 SOCKS5 节点各一次，都能通话 / 联机；通话结束后约 60 秒，对应的记录都结束。
- [ ] 全锥：用 NAT 类型检测工具（STUN）经 rurge 的 SOCKS5 检测，结果是 Full Cone（经 DIRECT 与经 SOCKS5 节点；节点本身须是全锥）。
- [ ] `block-quic`：配置 `block-quic = all`，客户端发往 UDP 443 的 QUIC 被丢弃，请求记录为 REJECT 与 `QUIC blocked`，应用回落到 TCP 后照常可用；改为 `always-allow` 后 QUIC 照常经过。
- [ ] 不支持 UDP 的策略：规则把 UDP 分到一条 `http` 策略，默认 REJECT（记录写 `policy does not support UDP`）；配置 `udp-policy-not-supported-behaviour = DIRECT` 后改经 DIRECT。
- [ ] 关联结束：客户端断开（关闭软件），它的全部 UDP 记录随即结束。
- [ ] NAT 后面的中继：上游 SOCKS5 节点放在带 NAT 的云主机上（本机私网地址、另有公网地址），看它对 UDP ASSOCIATE 回的中继地址：回未指定地址（`0.0.0.0`）或公网地址时 UDP 能往返；回私网地址时 UDP 不通（已知限制，见兼容性清单 `socks5` 一行），记下节点软件与它的设置。
