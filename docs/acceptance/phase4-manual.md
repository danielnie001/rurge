# 阶段 4 手工验收清单

需要真实的浏览器、真实的网站、真实的手机与真实的信任库的项目。自动化测试只用回环地址：源站是测试自己的 TLS 服务器，客户端是 rustls 的客户端，MITM 的 CA 在内存里或用例的临时目录里生成，**从不装进任何信任库**（总设计 P4-D26、M2-D24），`rurge mitm ca generate` 打印的装 CA 命令与 `curl --pinnedpubkey` 一类只有真客户端才有的行为都没有执行过。所以下面这些覆盖不了：真浏览器与真站点的握手与 ALPN、真源站的证书链（过期的、自签的）、浏览器与操作系统怎么看待 rurge 装的 CA、证书固定的真实表现、Android 与 iOS 上的信任设置、局域网里别的设备经 rurge 上网。由项目所有者在自己的机器上验收；每一项记下日期、系统版本与结果。**一项也没有做过。**

清单随里程碑逐节增加：下面是 M1（会话入口与明文 HTTP 引擎）与 M2（MITM：M2a 的 HTTP/1.1、M2b 的 HTTP/2 与 M2c 的 CA 工具与 `http-api-tls`，合成一节），M3 ～ M6 的项随各自的里程碑加。M1 的设计见 `docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md`，M2 的设计见 `docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`（先读它第 23 节，第 22 ～ 27 条是 M2c 对正文的订正）；API、记录字段与日志行见 `docs/api/phase4.md`，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 2.1、7.1、7.2、10.4 节。**M1 的项在 `rurge run` 上就能做（增强模式那两条除外）；M2 的项都要先装 CA 并让浏览器信任它，做完要把 CA 卸掉。**

## 怎么用

**先做哪一节。** 建议的次序是 M1 → M2：M1 不需要 CA，先把三种入口与明文 HTTP 的记录认下来；M2 先做「生成与安装 CA」那一条，其余都是拿着这份 CA 做。每一节里先做不改系统设置的项，再做装 CA、开增强模式的项；做完一项就恢复一次（卸 CA、关增强模式），别把几项叠在一起做。

**共用的前置条件。**
- `cargo build --release`，用得到的二进制（`target/release/rurge`，Windows 下是 `target\release\rurge.exe`）。下文按平台写作 `rurge.exe`（Windows）或 `./rurge`（macOS / Linux）。
- 一份测试用 Profile（下文的 `<conf>`），M1 用这一份：

  ```ini
  [General]
  http-listen = 127.0.0.1:6152
  socks5-listen = 127.0.0.1:6153
  http-api = testkey@127.0.0.1:6171
  loglevel = debug

  [Rule]
  FINAL,DIRECT
  ```

  M2 的 Profile 在这一份之后多一个 `[MITM]` 节，内容见 M2 一节的前置条件。每一项要加的规则写在 `[Rule]` 里 `FINAL,DIRECT` 的前面；改完重启 `rurge run`（`[General]` 里的 `http-api` 与 `http-api-tls` 只在重启时才生效，见 M2 一节的 W4）。
- 你自己的浏览器（M1 用惯用的那个即可；M2 每个平台用两个：Windows 的 Edge 与 Chrome、macOS 的 Safari 与 Chrome、Linux 的 Firefox 与 Chrome）与 `curl`（Windows 用 `curl.exe`）。
- 日志：`rurge run -c <conf> --log-level debug --log-file <日志路径>`（或看标准错误）。下文「日志」都指这里。
- 看记录：`curl -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000"`（缺省只给最近 100 条）。PowerShell 里可以 `(curl.exe -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000" | ConvertFrom-Json).requests | Format-Table id,listener,protocol,method,url,statusCode,status`；有 `jq` 时用 `jq '.requests[] | {id,listener,protocol,method,url,statusCode,status,error}'`。
- 看引擎的计数：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/rurge/http`。它在 `requests` 之外有三个对象：`sniffed`（`tls`、`http`、`other`、`timeout`、`closed` 五个嗅探结局）、`requests`（走到选路的请求数，被拒的也算）、`mitm`（`decrypted`、`untrusted`、`failed`、`noRequest`）。自进程启动起累计，重载不清零。
- 装 CA 与增强模式的项要改系统设置，见下一小节；增强模式的前置条件（要管理员 / root、一次性的机器）与阶段 3 的清单相同，见 `docs/acceptance/phase3-manual.md` 的「怎么用」。
- 给 M1 一节的 P3 要一台在非标准端口上有明文 HTTP 的服务器（自己的一台局域网服务器就行，下文写作 `<服务器>:8080`）；给 M2 一节的 P4 要一台在非标准端口上讲 TLS 的服务器（下文写作 `<服务器>:8443`，没有就跳过那一半）。
- 每一步执行后，在对应行填写「结果」（通过 / 不通过，附简要说明）、「日期」与「系统版本」三栏。

### 改系统设置的项与恢复

改系统设置的只有两种：装 CA（M2 一节的 W1、M1、L1 与 P5）与开增强模式（M1 一节的 W2、M2、L2 与 M2 一节的 W2、M2、L2）。都只在一次性的机器或能还原的虚拟机上做；Windows 要管理员，macOS 与 Linux 要 root。

**装 CA。** `rurge mitm ca generate --out <目录>` 打印的就是下面这几行（逐字照它的输出，在 `<目录>` 里执行；它自己不装，也不改任何设置）：

```
to trust it (rurge does not install it; run these where the files are):
  Windows, this user:  certutil -user -addstore Root rurge-ca.cer
  macOS, every user:   sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rurge-ca.pem
  Debian, Ubuntu:      sudo cp rurge-ca.pem /usr/local/share/ca-certificates/rurge-ca.crt && sudo update-ca-certificates
  Fedora, RHEL:        sudo cp rurge-ca.pem /etc/pki/ca-trust/source/anchors/rurge-ca.pem && sudo update-ca-trust
  Arch Linux:          sudo trust anchor --store rurge-ca.pem
  NSS (Chrome and others on Linux): certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n "rurge MITM CA" -i rurge-ca.pem
  Firefox: Settings > Privacy & Security > Certificates > View Certificates > Authorities > Import rurge-ca.pem, and trust it to identify websites
  Android: Settings > Security > Encryption & credentials > Install a certificate > CA certificate > rurge-ca.cer (apps trust it only if they opt in to user CAs)
  iOS, iPadOS: open rurge-ca.cer on the device, install the profile in Settings, then turn on full trust in Settings > General > About > Certificate Trust Settings
```

**卸掉 CA**（每一项做完就卸，别把一张长期有效的 CA 留在信任库里；`<CN>` 是 `generate` 第一行里那个 `rurge MITM CA <生成的日期>`）：
- Windows：`certutil -user -delstore Root "<CN>"`。
- macOS：`sudo security delete-certificate -c "<CN>" /Library/Keychains/System.keychain`。
- Debian、Ubuntu：删掉 `/usr/local/share/ca-certificates/rurge-ca.crt`，再 `sudo update-ca-certificates --fresh`。
- Fedora、RHEL：删掉 `/etc/pki/ca-trust/source/anchors/rurge-ca.pem`，再 `sudo update-ca-trust`。
- Arch Linux：`sudo trust anchor --remove rurge-ca.pem`。
- NSS（Linux 上的 Chrome 一类）：`certutil -d sql:$HOME/.pki/nssdb -D -n "rurge MITM CA"`。
- Firefox、Android、iOS：在设置里删（Firefox 是「证书 → 查看证书 → 证书颁发机构 → 删除」；Android 是「加密与凭据 → 用户凭据 → 删除」；iOS 是删掉那个描述文件）。

`<目录>` 里的 `rurge-ca.p12` 与 `rurge-mitm.conf` 带着私钥与口令（Unix 上是 `0600`，Windows 上 rurge 不另设权限），做完把整个目录删掉。

**增强模式。** 恢复的做法与阶段 3 的清单相同：Ctrl-C 或 `rurge stop` 之后核对虚拟网卡、路由与系统 DNS 回到原值，强杀之后靠下一次启动恢复；命令与要看哪几样见 `docs/acceptance/phase3-manual.md` 的「改系统设置的项与恢复」，这里不重抄。

### 第 7 节的五条验收标准各由哪几项证明

| 第 7 节 | 要看到什么 | Windows | macOS | Linux | 与平台无关 |
| --- | --- | --- | --- | --- | --- |
| 1 | 手册中每个重写示例有对应测试 | — | — | — | 重写（URL / Header / Body）与 Map Local 是 M3 的事，那时才有项；这些项的形态是用例，不是手工项 |
| 2 | 主流站点 MITM 解密成功且证书固定站点给出提示 | W2、W3 | M2、M3 | L2、L3 | P1（证书固定的提示）、P2 ～ P5 |
| 3 | Body Rewrite 与 jq 用例通过 | — | — | — | M3 之后才有项 |
| 4 | 大小限制行为与手册一致 | — | — | — | M3 之后才有项 |
| 5 | 抓包可通过 API 读取 | — | — | — | M4 之后才有项 |

---

## M1　会话入口与明文 HTTP 引擎

自动化里证明过的是：嗅探的五种结局、判为 HTTP 的连接进引擎、每个请求各自求规则与选策略、记录里的 `method`、`url`、`statusCode`，以及三种入口（`CONNECT`、SOCKS5、内存设备上的虚拟网卡）——全部用回环上的 socket 对与测试自己写的 HTTP 客户端，客户端是 `curl` 一类的代码，不是浏览器。下面是 CI 与回环验不了的：真浏览器真的经代理发出去的请求长什么样、浏览器与站点之间的 keep-alive 与重试、增强模式开着时整台机器的流量、真站点返回的状态码与重定向、真 DNS 与 fake-IP 的配合。**一项也没有做过。**

### 前置条件

- 同「怎么用」：`cargo build --release`；一份 `http-listen`、`socks5-listen`、`http-api`、`loglevel` 的 `<conf>`；日志；两个看记录的 `curl`。
- 增强模式的两条（W2、M2、L2）要管理员 / root 与一次性的机器，照阶段 3 清单的前置条件。
- 明文 HTTP 的站点用你自己常上的一个（下文写作 `<明文站点>`），非标准端口的那一条用 `<服务器>:8080`。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W1 | **真浏览器经 HTTP 代理与 SOCKS5 打开明文 HTTP 的站点。** 先读一次 `GET /v1/rurge/http` 的 `requests`。在 Edge（或 Chrome）里把代理设成 HTTP 代理 `127.0.0.1:6152`（先做这一种），打开 `http://<明文站点>/` 并点开里面的一两个链接；再把浏览器的代理换成 SOCKS5 `127.0.0.1:6153`（Edge 与 Chrome 走的是系统代理，用系统设置里的「使用代理服务器」，SOCKS5 那一栏），重新打开同一个站点。每次之后读记录与计数 | 每个请求一条记录：经 HTTP 代理的 `listener` 是 `http`，经 SOCKS5 的是 `socks5`；`transport` 是 `tcp`；`protocol` 是 `http`；`method` 是 `GET`（或站点实际用的方法）、`url` 是 `http://<明文站点>/…`（去掉 userinfo、留着查询串）、`statusCode` 是站点实际答的码（一般是 `200`）；`status` 是 `completed`；`requests` 增加的数与发出的请求数一样。（HTTP 监听器的绝对形式请求不经过嗅探，所以 `sniffed` 不必跟着涨；经 SOCKS5 的那几次 `sniffed.http` 加一。）浏览器自带的 DoH（「安全 DNS」）不影响这一条（请求纵的是代理），但它会让增强模式那一条的记录里 `dst` 是地址而不是名字，所以做 W2 之前关掉它 | `GET /v1/rurge/http` 前后各一次；`recent` 的 JSON；浏览器的代理设置 | | | |
| W2 | **增强模式：不经代理的浏览器与命令行工具。** 管理员运行 `rurge.exe run -c <conf> --enhanced-mode --log-level debug`；系统代理与浏览器的代理都关着，浏览器与系统的「安全 DNS」/ DoH 也关掉（否则记录里的 `dst` 是地址而不是名字），`curl.exe` 不带 `-x`。用浏览器打开 `http://<明文站点>/`，再 `curl.exe -s -o NUL -w "%{http_code}\n" http://<明文站点>/`；读记录与计数。做完关掉增强模式并核对恢复（见「改系统设置的项与恢复」） | 两次都有记录，`listener` 是 `tun`（不是 `http` / `socks5`），`method`、`url`、`statusCode` 与 W1 一样，`curl.exe` 打印 `200`；`requests` 随每个请求加一（`curl.exe` 那一条固定加一）；`dst` 是名字与 80 端口。记录里 `pid` 与 `processPath` 是浏览器与 `curl.exe` | `recent`；`curl.exe` 的输出；`GET /v1/rurge/http` | | | |

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | **真浏览器经 HTTP 代理与 SOCKS5。** 照 W1 做，浏览器用 Safari 与 Chrome（Safari 的代理在「系统设置 → 网络 → 代理」，改完要重新打开 Safari）；`./rurge run -c <conf> --log-level debug` | 同 W1；`processPath` 是 Safari 的网络进程与 `/Applications/Google Chrome.app/…` 一类的真实路径 | 同 W1 | | | |
| M2 | **增强模式。** 照 W2 做：`sudo ./rurge run -c <conf> --enhanced-mode --log-level debug`，浏览器的系统代理关着，`curl` 不带 `-x` | 同 W2；`processPath` 是 Safari、Chrome 与 `/usr/bin/curl` | 同 W2 | | | |

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L1 | **真浏览器经 HTTP 代理与 SOCKS5。** 照 W1 做，浏览器用 Firefox 与 Chromium（Firefox 的代理在「设置 → 网络设置 → 手动配置代理」，Chromium 用环境变量 `http_proxy` / `https_proxy` 或桌面环境的代理设置）；`./rurge run -c <conf> --log-level debug` | 同 W1；`processPath` 是 `firefox`、`chromium` 一类的真实路径 | 同 W1 | | | |
| L2 | **增强模式。** 照 W2 做：`sudo ./rurge run -c <conf> --enhanced-mode --log-level debug`，浏览器的代理关着，`curl` 不带 `-x` | 同 W2；`processPath` 是 Firefox、Chromium 与 `/usr/bin/curl` | 同 W2 | | | |

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | **443 上的 TLS 按 SNI 求规则，不解密。** 先读一次 `requests` 与 `sniffed.tls`。`<conf>` 的 `[Rule]` 第一行写 `DOMAIN,<HTTPS 站点>,REJECT,extended-matching`；`curl --socks5 127.0.0.1:6153 -s -o NUL -w "%{http_code}\n" https://<HTTPS 站点>/`（`--socks5` 让 curl 自己解析名字，连接的目标是地址，所以规则只能靠 SNI 命中），再用浏览器打开同一个站点。然后把那一行改成 `DOMAIN,<HTTPS 站点>,DIRECT`（去掉 `extended-matching`），重启或重载，再各开一次 | 第一次：`curl` 以错误结束（连接被关，把它的原话记下来），浏览器是错误页；记录里 `sni` 是 `<HTTPS 站点>`、`protocol` 是 `https`、`rule` 是 `DOMAIN,<HTTPS 站点>,REJECT,extended-matching` 的原文、`policy` 是 `["REJECT"]`、`status` 是 `rejected`、`url` 与 `method` 是 `null`；`sniffed.tls` 增加（每条可嗅探的 TLS 连接加一）；`requests` 不因这两个连接增加（没有走到选路的请求）。第二次：站点打得开，浏览器里看到的证书是站点自己的（签发者不是 rurge 的 CA）；`curl` 那一条记录里 `protocol` 是 `https`、`rule` 是 `FINAL,DIRECT`、`url` 与 `method` 仍是 `null`（没有解密，也没有进 HTTP 引擎） | 记录；`curl` 的输出与退出码；`sniffed.tls`；浏览器的证书面板 | | | |
| P2 | **同一条 keep-alive 连接上的两个请求各走各的策略。** `<conf>` 的 `[Rule]` 写两行：`URL-REGEX,^http://<明文站点>/a,DIRECT` 与 `URL-REGEX,^http://<明文站点>/b,REJECT`（换成站点上真有的两个路径，下面写作 `/a` 与 `/b`）；经 HTTP 代理一次给两个 URL：`curl -s -o NUL -w "%{http_code}\n" http://<明文站点>/a http://<明文站点>/b`；读记录（`curl` 先打印一个 `200`，第二个 URL 以错误结束——把它的原话也记下来） | 第一个请求打得开、记 `200`、`rule` 是第一条、`policy` 是 `["DIRECT"]`、`status` 是 `completed`。第二个被拒：`status` 是 `rejected`、`rejectKind` 是 `REJECT`、`statusCode` 是 `null`（`show-error-page-for-reject` 缺省关着，不答就关）、`rule` 是第二条。两条记录是**一条客户端连接**上的两个请求：`src` 相同（同一条连接、同一个来源端口）、`listenPort` 都是 `6152`，`url` 各自是自己的那个；`requests` 加二 | `recent`；`curl` 的输出 | | | |
| P3 | **`force-http-engine-hosts`：非标准端口上的明文 HTTP。** 先不写它：`curl -s -o NUL -w "%{http_code}\n" --socks5-hostname 127.0.0.1:6153 http://<服务器>:8080/`，读记录与 `requests`。再在 `[General]` 写 `force-http-engine-hosts = <服务器>:0`，重启，再发一次同一个请求 | 第一次：页面打得开、打印 `200`。但这条连接没被嗅探（8080 既不是 80 也不是 443），照原样转发：记录里 `protocol`、`method`、`url`、`statusCode` 都是 `null`，`requests` 不变。第二次：记录里 `protocol` 是 `http`、`method` 是 `GET`、`url` 是 `http://<服务器>:8080/`、`statusCode` 是 `200`；`requests` 加一，`sniffed.http` 加一。这就是「非标准端口的明文 HTTP 被嗅探到」：要它进引擎只有 `force-http-engine-hosts`（`host:0` 是全部端口，只写 `<服务器>` 是 80），`always-raw-tcp-hosts` 反过来让它一定不经嗅探 | `recent`；`/v1/rurge/http`；`curl` 的输出 | | | |

---

## M2　MITM

M2a（HTTP/1.1）、M2b（HTTP/2）与 M2c（`rurge mitm ca generate` / `export`、`http-api-tls`）的行为都已经有用例：解密的连接上每个请求一条记录、`protocol` 是 `https`、`url` 以 `https://` 开头、`h2` 开关两侧的四种组合、`:authority` 进规则、拒绝与终止只重置一个流、握手的三种失败与证书固定的提示、CA 的生成与读回、API 的 TLS 与命令行的 HTTPS。那些用例的客户端都是 rustls 的客户端，根只有测试自己的 CA，源站是回环上的测试服务器。下面是它们验不了的：真浏览器与真站点的握手、浏览器怎么认 rurge 装的 CA、真源站的证书链、证书固定的真实表现（浏览器的预连接与 App 的固定）、真 `curl` 的 TLS 栈与 ALPN、Android 与 iOS 上的信任设置、局域网设备经 rurge 上网。**一项也没有做过。**

### 前置条件

- M1 的前置条件，外加：
- 一个只有你能读的目录（下文 `<CA 目录>`），在里面跑 `rurge mitm ca generate --out <CA 目录>`。它写出 `rurge-ca.pem`、`rurge-ca.cer`、`rurge-ca.p12`、`rurge-mitm.conf`，第一行是 `wrote a new MITM CA, "rurge MITM CA <日期>", valid until <日期>:`。下文把那个 CN（`rurge MITM CA <日期>`）写作 `<CN>`。
- 照打印的说明装 CA（本平台那一行，逐字，见「改系统设置的项与恢复」）。每一项做完卸掉。
- `<conf>` 的 `[MITM]` 节：把 `rurge-mitm.conf` 的两行抄进来，再写要解密的站点：

  ```ini
  [MITM]
  ca-p12 = <rurge-mitm.conf 里 ca-p12 那一行的值>
  ca-passphrase = <rurge-mitm.conf 里 ca-passphrase 那一行的值>
  hostname = <站点 1>, <站点 2>
  ```

  `hostname` 里写你真会打开的几个站点（下文写作 `<站点 1>`、`<站点 2>`）。`mitm` 开关缺省是开的：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/features/mitm` 是 `{"enabled":true}`；关掉了就 `curl -s -X POST -H "X-Key: testkey" -d '{"enabled":true}' http://127.0.0.1:6171/v1/features/mitm`。
- `[MITM]` 的 `h2` 缺省是 false（缺省只讲 HTTP/1.1），W3 要把它打开再关掉。
- 每一项执行后填「结果」「日期」「系统版本」三栏。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W1 | **生成并安装 CA（本平台）。** 在 `<CA 目录>` 里 `rurge.exe mitm ca generate --out .`（目录要先建好），照「Windows, this user」那一行 `certutil -user -addstore Root rurge-ca.cer` 装进当前用户的「受信任的根证书颁发机构」；用 `certutil -user -store Root` 确认里面列着 `<CN>`；把 `rurge-mitm.conf` 的两行抄进 `<conf>` 的 `[MITM]`，写好 `hostname`，`rurge.exe run -c <conf> --log-level debug`；`rurge.exe check -c <conf>` | `generate` 打印四个文件的路径、抄进配置的说明与各平台的装 CA 步骤，**没有**口令与私钥；`certutil -user -store Root` 里列着 `<CN>`；`check` 没有 `warning[W0039]` 与 `warning[W0040]`；`GET /v1/features/mitm` 是 `{"enabled":true}`；`GET /v1/rurge/http` 的 `mitm` 四个数都是 `0` | `generate` 的输出；`certutil` 的输出；`check` 的输出；两个 API | | | |
| W2 | **真浏览器经三种入口打开解密的真站点。** 浏览器（Edge 与 Chrome 各一次）把 HTTP 代理设为 `127.0.0.1:6152`，打开 `https://<站点 1>/`；再换成 SOCKS5 `127.0.0.1:6153` 打开同一个站点；再关掉浏览器的代理，管理员运行 `rurge.exe run -c <conf> --enhanced-mode --log-level debug`，系统代理也关着，不经代理打开同一个站点。每次之后读记录与计数，并点地址栏的锁看证书 | 三次都打得开。每次的每条请求一条记录，`listener` 依次是 `http`、`socks5`、`tun`；`protocol` 是 `https`；`url` 以 `https://<站点 1>` 开头（带查询串、不带 userinfo），`method` 与 `statusCode` 有值；`sni` 是 `<站点 1>`；`status` 是 `completed`；`dst` 是 `<站点 1>` 与 443。浏览器里的证书的签发者是 `<CN>`，不是站点自己的 CA。`mitm.decrypted` 每次增加（每条解密的连接加一，浏览器会开好几条） | 浏览器的证书面板；`recent`；`/v1/rurge/http` | | | |
| W3 | **HTTP/2：`h2 = true` 与缺省（false）。** `<conf>` 的 `[MITM]` 写 `h2 = true`，重启；浏览器打开 `https://<站点 1>/`，在开发者工具的 Network 里看那一条请求的协议一栏；再用 `curl.exe --http1.1 -s -o NUL -w "%{http_code}\n" https://<站点 1>/` 与 `curl.exe --http2 -s -o NUL -w "%{http_code}\n" https://<站点 1>/` 各一次（两个都加 `-v`，把 curl 报的协议版本记下来）。然后删掉 `h2`（缺省 false），重启，重复一遍 | `h2 = true`：浏览器里的协议是 `h2`；两条 `curl` 都打印 `200`，`-v` 里报的版本是 HTTP/2 与 HTTP/1.1；两条记录都是 `protocol` `https`、`url` 以 `https://` 开头。缺省（false）：浏览器里的协议是 `http/1.1`；两条 `curl` 仍然都打印 `200`（`--http2` 的 curl 同时给 `h2` 与 `http/1.1`，rurge 只取 `http/1.1`），两条记录也都解密。**只给 `h2` 的客户端在 `h2 = false` 时不解密**——浏览器里一般看不到这一支，除非你用的客户端是那样配的 | 浏览器开发者工具；`curl -v` 的输出；记录 | | | |
| W4 | **从浏览器下载 CA；`http-api-tls` 下的 `rurge status`。** `http-api-tls` 先不写（缺省 false）：浏览器打开 `http://127.0.0.1:6171/v1/mitm/ca?x-key=testkey`，看下载到的文件名。然后 `[General]` 写 `http-api-tls = true`，重启 `rurge.exe run -c <conf> --log-level debug`；另开一个窗口 `rurge.exe status -c <conf>`；再 `rurge.exe status --remote 127.0.0.1:6171 --key testkey`。做完把 `http-api-tls` 删掉，重启 | 浏览器下载到的文件名是 `rurge-ca.cer`（应答头 `Content-Disposition: attachment; filename="rurge-ca.cer"`、`Content-Type: application/x-x509-ca-cert`），内容是 DER，与 `rurge-ca.pem` 是同一张证书。开了 `http-api-tls`：`rurge run` 打印 `api on https://127.0.0.1:6171`，INFO 一行 `http-api listening`，带 `tls` 字段（`true`）；`status` 的第一行是 `rurge at https://127.0.0.1:6171`，并且读得到内容（说明 API 的证书由 `[MITM]` 的 CA 按 IP 签、命令行只信这一张）；`--remote` 那一次以 `1` 退出，标准错误是 `error: cannot reach rurge at http://127.0.0.1:6171: <原因>`（它不读配置、不知道 CA，照旧讲明文，所以对开了 `http-api-tls` 的实例连不上）。把浏览器对 `https://127.0.0.1:6171/v1/mitm/ca?x-key=testkey` 的看法也记下来（装了 CA 之后它应当也信） | 浏览器的下载与证书面板；`rurge run` 的输出；`status` 的两行输出与退出码 | | | |
| W5 | **不信任的客户端：卸掉 CA 之后。** 照「Windows」那一行卸掉 CA：`certutil -user -delstore Root "<CN>"`，`certutil -user -store Root` 确认没有它了。浏览器打开 `https://<站点 1>/`；读日志、记录与 `GET /v1/rurge/http` 的 `mitm` 计数。（rurge 不用重启：每一条新连接都重新拿 CA 签，客户端的信任是它自己的事） | 站点打不开（浏览器报证书错误，因为拿到的是 rurge 签的、而它不再被信任）。日志一行 WARN `the client does not trust the certificate rurge issued for <站点 1>: trust rurge's MITM CA on the client, or exclude the host with -<站点 1> in [MITM] hostname`（同一个名字一小时内的第二次只有 DEBUG，句子相同）；`mitm.untrusted` 加一；承载连接一条失败记录，`error` 是 `the client does not trust the certificate rurge issued (alert <警报的名字>)`，`status` 是 `failed`；记录里没有请求（`url` 与 `method` 是 `null`）。做完照 W1 那一行把 CA 装回去 | 浏览器；日志；`recent`；`/v1/rurge/http` | | | |

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | **生成并安装 CA。** 照 W1 做，装 CA 用 `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rurge-ca.pem`；确认用 `security find-certificate -c "<CN>" -a /Library/Keychains/System.keychain` 或「钥匙串访问」看 | 同 W1；Safari 与 Chrome 都认这张 CA | `generate` 与 `security` 的输出；`check`；两个 API | | | |
| M2 | **真浏览器经三种入口。** 照 W2 做，浏览器用 Safari 与 Chrome（Safari 的代理在「系统设置 → 网络 → 代理」；增强模式那一次用 `sudo ./rurge run -c <conf> --enhanced-mode`） | 同 W2 | 同 W2 | | | |
| M3 | **HTTP/2 与 curl。** 照 W3 做（`curl.exe` 换成 `curl`） | 同 W3 | 同 W3 | | | |
| M4 | **下载 CA 与 `http-api-tls`。** 照 W4 做（`rurge.exe` 换成 `./rurge`） | 同 W4；Safari 下载到的文件名也是 `rurge-ca.cer` | 同 W4 | | | |
| M5 | **不信任的客户端。** 照 W5 做，卸 CA 用 `sudo security delete-certificate -c "<CN>" /Library/Keychains/System.keychain` | 同 W5；Safari 的报错原话记下来 | 同 W5 | | | |

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L1 | **生成并安装 CA。** 照 W1 做，装 CA 用本发行版那一行（Debian / Ubuntu 是 `sudo cp rurge-ca.pem /usr/local/share/ca-certificates/rurge-ca.crt && sudo update-ca-certificates`）；Chrome 一类用 NSS 库的浏览器还要 `certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n "rurge MITM CA" -i rurge-ca.pem`；Firefox 用自己的库，照打印的说明在设置里导入 | 同 W1；系统库（`/etc/ssl/certs` 里能看到条目）与 Firefox 各自认这张 CA 时的表现分别记下来——Firefox 不认系统库，要单独导入 | `generate` 与 `update-ca-certificates` 的输出；`check`；两个 API；浏览器的证书面板 | | | |
| L2 | **真浏览器经三种入口。** 照 W2 做，浏览器用 Firefox 与 Chromium（Chromium 用桌面环境的代理设置或环境变量；增强模式那一次用 `sudo ./rurge run -c <conf> --enhanced-mode`） | 同 W2 | 同 W2 | | | |
| L3 | **HTTP/2 与 curl。** 照 W3 做 | 同 W3 | 同 W3 | | | |
| L4 | **下载 CA 与 `http-api-tls`。** 照 W4 做（`rurge.exe` 换成 `./rurge`） | 同 W4 | 同 W4 | | | |
| L5 | **不信任的客户端。** 照 W5 做，卸 CA 用本发行版那一行（Debian / Ubuntu 是删掉 `/usr/local/share/ca-certificates/rurge-ca.crt` 再 `sudo update-ca-certificates --fresh`），NSS 那一份用 `certutil -d sql:$HOME/.pki/nssdb -D -n "rurge MITM CA"` | 同 W5 | 同 W5 | | | |

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | **证书固定。** 选一个证书固定的站点（下文 `<固定站点>`，`[MITM] hostname` 里要有它）。**先看 `curl` 的那一种：**用你手边的工具算出它**真实**证书的公钥的 pin（SHA-256，Base64；`openssl s_client` 一类都行，算的是源站的，不是 rurge 签的那张）；先读一次 `mitm.untrusted`；`curl -s -o NUL -w "%{http_code}\n" --socks5-hostname 127.0.0.1:6153 --pinnedpubkey 'sha256//<pin>' https://<固定站点>/`；再读日志、记录与计数。**再看握手之后才拒绝的那一种（rurge 的证书固定提示）：**`openssl s_client -proxy 127.0.0.1:6152 -connect <固定站点>:443 -servername <固定站点> </dev/null`（握手完成、一个请求也不发就关）；先读一次 `mitm.noRequest`，做完再读日志、记录与计数 | `curl` 拒绝这个连接（报的是证书固定的错误），**不**打印 `200`；日志里是「不信任」那一行 WARN `the client does not trust the certificate rurge issued for <固定站点>: trust rurge's MITM CA on the client, or exclude the host with -<固定站点> in [MITM] hostname`，`mitm.untrusted` 加一，承载连接的 `error` 是 `the client does not trust the certificate rurge issued (alert <警报的名字>)`，`mitm.failed` 不涨。**这不是缺陷，要照实记下来**：curl 在证书校验那一层就拒绝了，发的是 `unknown_ca` / `bad_certificate` 一类的警报，rurge 分不清「没装 CA」与「证书固定」（设计第 8.2 节），所以它说的是「不信任」。`openssl s_client` 那一次：日志一行 WARN `<固定站点> is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with -<固定站点> in [MITM] hostname`（同一个名字一小时内的第二次只有 DEBUG，句子相同）；`mitm.noRequest` 加一；`mitm.untrusted` 与 `mitm.failed` **不**涨；承载连接一条失败记录，`error` 是 `no request after the TLS handshake (likely certificate pinning)`，`status` 是 `failed`，记录里没有请求。真浏览器的预连接也会触发这一条。把 `<固定站点>` 写成 `-<固定站点>` 放进 `hostname` 之后，同一个 `curl --pinnedpubkey` 就通了（那是照原样转发的连接，rurge 没参与 TLS） | `curl` 的输出与退出码；`openssl s_client` 的输出；日志；`recent`；`/v1/rurge/http` | | | |
| P2 | **源站自己的证书不受信：`expired.badssl.com` 与 `self-signed.badssl.com`。** `[MITM] hostname` 里加上 `expired.badssl.com, self-signed.badssl.com`，重载或重启；经 SOCKS5 各打开一次：`curl -s -o NUL -w "%{http_code}\n" --socks5-hostname 127.0.0.1:6153 https://expired.badssl.com/` 与同样的 `https://self-signed.badssl.com/`；读记录与浏览器看到的页面。然后在 `[MITM]` 写 `skip-server-cert-verify = true`，重载，再各打开一次 | 前两次 `curl` 打印 `502`；浏览器看到 rurge 的错误页，标题是 `Connection failed`（`show-error-page` 缺省开着；关着时不答就关）。记录里 `status` 是 `failed`、`statusCode` 是 `502`、`error` 里有上游证书的问题（过期 / 不受信，把原话抄下来），`url` 与 `method` 有值（请求走到了选路）。`skip-server-cert-verify = true` 之后两次都打得开、打印 `200`，记录 `status` 是 `completed`——rurge 只放过了对上游证书的链与名字的校验，没有放过别的 | `curl` 的输出；浏览器的错误页；`recent` | | | |
| P3 | **名单与来源：`hostname-disabled`、排除项、`client-source-address`。** (a) `[MITM]` 里 `hostname = <站点 1>, <站点 2>`，另有 `hostname-disabled = <站点 2>`；经 SOCKS5 各打开一次。(b) 把 `hostname` 写成 `hostname = <站点 1>, -<站点 3>`，打开 `<站点 3>`。(c) 回到 `hostname = <站点 1>`，`[MITM]` 写 `client-source-address = 127.0.0.1` 打开 `<站点 1>`；改成 `client-source-address = ::1` 再打开；改成 `client-source-address = -127.0.0.1` 再打开。每次读记录与证书 | (a) `<站点 1>` 解密（证书是 `<CN>` 签的，记录 `url` 以 `https://` 开头），`<站点 2>` 不解密（浏览器看到站点自己的证书；记录 `protocol` 是 `https`、`url` 与 `method` 是 `null`）；`mitm.decrypted` 只涨一次。(b) `<站点 3>` 不解密。(c) `127.0.0.1` 时解密；`::1` 时不解密——本机的会话一律按 `127.0.0.1` 比，`::1` 的项不命中它（登记的行为）；`-127.0.0.1` 时不解密 | 浏览器的证书；`recent`；`mitm.decrypted` | | | |
| P4 | **端口：`:0` 与写明的端口。** `<站点 1>` 在 443 上。(a) `hostname = <站点 1>`（不带端口），打开 `https://<站点 1>/`。(b) 改成 `hostname = <站点 1>:8443`，再打开同一个站点的 443。(c) 改成 `hostname = <站点 1>:0`，再打开。有自己那台 `<服务器>:8443` 上的 TLS 服务器时，把它也写进 `hostname`（例如 `hostname = <站点 1>:0, <服务器>:0`）再各打开一次 | (a) 443 上解密。(b) 443 上**不**解密（写明的端口要与连接相同，8443 的项不命中 443）；写了 `<服务器>:8443` 时它解密。(c) 443 上解密；`<服务器>:8443` 也解密（`:0` 让它命中的目标的全部端口可嗅探）。另外：不带端口的项**不让别的端口可嗅探**——`hostname = <站点 1>` 时 `<服务器>:8443` 上的连接不被嗅探、不解密，要解密就得写明端口或写 `:0`（登记的行为） | 浏览器的证书；`recent`；`mitm.decrypted` | | | |
| P5 | **Android 与 iOS 经局域网里的 rurge 上网。（有了设备才做。）** `<conf>` 的 `http-listen` 改成 `0.0.0.0:6152`（否则只监听回环），`[MITM] hostname` 里有要打开的站点，重启。在设备上按 `generate` 打印的说明装 CA（Android 是「设置 → 安全 → 加密与凭据 → 安装证书 → CA 证书」，iOS 是装上描述文件再到「设置 → 通用 → 关于本机 → 证书信任设置」里打开完全信任）；把设备 Wi-Fi 的 HTTP 代理指向运行 rurge 的那台机器的局域网地址与 6152 端口；在设备上用浏览器打开 `https://<站点 1>/`；读记录 | 页面打得开。记录里 `listener` 是 `http`、`src` 是设备的局域网地址（不是 `127.0.0.1`）、`protocol` 是 `https`、`url` 以 `https://` 开头、`sni` 是 `<站点 1>`。`[MITM]` 写了 `client-source-address` 时，设备的地址要在其中（本机应用按它连进来的地址比）。iOS 上没有打开「完全信任」时打不开，日志里有「不信任」那一行 WARN、`mitm.untrusted` 加一。做完把设备上的证书删掉（「改系统设置的项与恢复」里那两行） | 设备上的浏览器；`recent`；日志 | | | |
| P6 | **`rurge check` 对 CA 的两种告警，与没有 CA 时的 API。** (a) `<conf>` 的 `[MITM]` 写了 `hostname`、没有 `ca-p12` 与 `ca-keystore-name`：`rurge check -c <conf>`；同时 `rurge run` 起来，`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/mitm/ca`。(b) `[MITM]` 写一个打不开的 `ca-p12`（例如 `ca-p12 = QUJDRA`）：再 `check`。(c) `[General]` 写 `http-api-tls = true` 与 `http-api = testkey@127.0.0.1:6171`，`[MITM]` 的 CA 仍然是坏的：再 `check`；`rurge run -c <conf>` 看输出；`curl -x http://127.0.0.1:6152 http://<明文站点>/ -s -o NUL -w "%{http_code}\n"`；`rurge status -c <conf>` 看消息与退出码 | (a) 一条 ``warning[W0039]: MITM is off: `hostname` lists hosts to decrypt but `[MITM]` sets no CA (`ca-p12` or `ca-keystore-name`)``；`GET /v1/mitm/ca` 是 `404`，正文 `{"error":"no usable MITM CA"}`；代理照常可用。(b) 一条 `warning[W0039]`，消息以 `MITM is off:` 开头，说的是哪一种问题（Base64 坏了、口令错了、不是 CA 之类）。(c) 一条 `warning[W0040]: http-api-tls is on but there is no usable MITM CA: the API does not start with this profile`，下面一行 `  hint: configure a usable MITM CA, or turn http-api-tls off`；`rurge run` 打一行 WARN `http-api-tls is on but there is no usable MITM CA: the API is not started (it never falls back to plain HTTP)`，**没有** `api on` 那一行，进程照常跑、`curl` 经 6152 打印 `200`（API 不会退回明文）；`rurge status -c <conf>` 以 `2` 退出，标准错误以 `error:` 开头，后面是 `W0040` 的那一句（CA 读不出来时是 `MITM is off: …` 那一句，同样以 `2` 退出）。修好 CA 之后再 `check` 不再有 `W0039` / `W0040`，`status` 也读得到 | 两个命令的输出与退出码；`rurge run` 的输出；`curl` 的输出；记录 | | | |
