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

不要在验收时使用 `--system-proxy`，除非你确实想让本机的系统代理指向 rurge（退出时会恢复）。
