# 阶段 4 手工验收清单

需要真实的浏览器、真实的网站、真实的手机与真实的信任库的项目。自动化测试只用回环地址：源站是测试自己的 TLS 服务器，客户端是 rustls 的客户端，MITM 的 CA 在内存里或用例的临时目录里生成，**从不装进任何信任库**（总设计 P4-D26、M2-D24），`rurge mitm ca generate` 打印的装 CA 命令与 `curl --pinnedpubkey` 一类只有真客户端才有的行为都没有执行过。所以下面这些覆盖不了：真浏览器与真站点的握手与 ALPN、真源站的证书链（过期的、自签的）、浏览器与操作系统怎么看待 rurge 装的 CA、证书固定的真实表现、Android 与 iOS 上的信任设置、局域网里别的设备经 rurge 上网。由项目所有者在自己的机器上验收；每一项记下日期、系统版本与结果。**一项也没有做过。**

清单随里程碑逐节增加：下面是 M1（会话入口与明文 HTTP 引擎）与 M2（MITM：M2a 的 HTTP/1.1、M2b 的 HTTP/2 与 M2c 的 CA 工具与 `http-api-tls`，合成一节），M3 ～ M6 的项随各自的里程碑加。M1 的设计见 `docs/superpowers/specs/2026-10-29-phase4-m1-session-entry-http-engine-design.md`，M2 的设计见 `docs/superpowers/specs/2026-10-30-phase4-m2-mitm-design.md`（先读它第 23 节，第 22 ～ 27 条是 M2c 对正文的订正）；API、记录字段与日志行见 `docs/api/phase4.md`，兼容性清单里的差异见 `docs/surge-compatibility-matrix.md` 的 2.1、7.1、7.2、10.4 节。**M1 的项在 `rurge run` 上就能做（增强模式那三条除外：W2、M2、L2）；M2 的项都要先装 CA 并让浏览器信任它，做完要把 CA 卸掉。**M3（重写与 Map Local）的项 2026-10-31 加在这一份里，四级表的编号接着 M2 的数（W6、M6、L6 与 P7 起）。

## 怎么用

**先做哪一节。** 建议的次序是 M1 → M2：M1 不需要 CA，先把三种入口与明文 HTTP 的记录认下来；M2 先做「生成与安装 CA」那一条，其余都是拿着这份 CA 做。每一节里先做不改系统设置的项，再做装 CA、开增强模式、改系统代理的项；做完一项就恢复一次（卸 CA、关增强模式、把系统代理改回去），别把几项叠在一起做。M3 一节接在 M2 之后：先做它明文的项（W6 与 W8，其余平台同号）与 P7 ～ P9，再做要 CA 的 W7、M7、L7 与 P10；CA 照样做完就卸。

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

  M2 的 Profile 在这一份之后多一个 `[MITM]` 节，内容见 M2 一节的前置条件。每一项要加的规则写在 `[Rule]` 里 `FINAL,DIRECT` 的前面；改完重启 `rurge run`（`[General]` 里的 `http-api` 与 `http-api-tls` 只在重启时才生效，见 M2 一节的 W4）。M3 的 Profile 在 M2 那一份之后再多四个节（`[URL Rewrite]`、`[Header Rewrite]`、`[Body Rewrite]` 与 `[Map Local]`），内容见 M3 一节的前置条件。
- 你自己的浏览器（M1 用惯用的那个即可；M2 每个平台用两个：Windows 的 Edge 与 Chrome、macOS 的 Safari 与 Chrome、Linux 的 Firefox 与 Chromium）与 `curl`（Windows 用 `curl.exe`）。
- 让浏览器走 rurge 的代理，按浏览器分三种写法（前两种不改系统设置）：
  - **Edge、Chrome 与 Chromium**：带参数另起一个实例，把 `--user-data-dir=<一个新的空目录>` 与 `--proxy-server="http://127.0.0.1:6152"`（HTTP 代理那一次）或 `--proxy-server="socks5://127.0.0.1:6153"`（SOCKS5 那一次）一起给；这样主机名由 rurge 解析。Chromium 带参数启动之前要退出正在运行的实例，新的数据目录就是另起一个实例；关掉窗口就没了。程序名：Windows 的 `msedge.exe` / `chrome.exe`，macOS 的 `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`（路径带空格，要引号），Linux 的 `chromium` / `google-chrome`。
  - **Firefox**：在它自己的设置里（「设置 → 常规 → 网络设置 → 设置… → 手动配置代理」）。HTTP 代理那一次填 `127.0.0.1:6152` 并勾「也将此代理用于 HTTPS」；SOCKS5 那一次填「SOCKS 主机」`127.0.0.1`、端口 `6153`，选「SOCKS v5」，勾「使用 SOCKS v5 时代理 DNS 查询」。只影响 Firefox，做完改回「使用系统代理设置」。
  - **Safari**：只能用系统代理（「系统设置 → 网络 → <服务> → 详细信息… → 代理」：「网页代理（HTTP）」与「安全网页代理（HTTPS）」填 `127.0.0.1:6152`，或「SOCKS 代理」填 `127.0.0.1:6153`）。这是改系统设置，见下一小节。
- 日志与诊断走哪一条流：`rurge run -c <conf> --log-level debug --log-file <日志路径>` 把**日志行**写在标准输出（`--log-file` 另写一份到那个文件），`api on …` 也在标准输出；`rurge run` 打的诊断（`warning[W…]: …`，不带位置）与 `error: …` 在标准错误。`rurge check` 的输出在**标准输出**：每条诊断一行（有位置的带 `<文件>:<行>`，有 `hint` 的下面另起一行 `  hint: …`），末尾一行 `<文件>: N error(s), N warning(s), N note(s)`。下文「日志」都指这里。
- 清单里的 `curl -o NUL` 在 macOS 与 Linux 上写 `-o /dev/null`（写 `NUL` 会在当前目录里多一个叫 `NUL` 的文件）；`openssl s_client … </dev/null` 在 Windows 的 cmd 里写 `<NUL`（PowerShell 没有 `<` 重定向）。
- 看记录：`curl -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000"`（缺省只给最近 100 条）。PowerShell 里可以 `(curl.exe -s -H "X-Key: testkey" "http://127.0.0.1:6171/v1/requests/recent?limit=1000" | ConvertFrom-Json).requests | Format-Table id,listener,protocol,method,url,statusCode,status`；有 `jq` 时用 `jq '.requests[] | {id,listener,protocol,method,url,statusCode,status,error}'`。
- 看引擎的计数：`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/rurge/http`。它在 `requests` 之外有三个对象：`sniffed`（`tls`、`http`、`other`、`timeout`、`closed` 五个嗅探结局）、`requests`（走到选路的请求数，被拒的也算）、`mitm`（`decrypted`、`untrusted`、`failed`、`noRequest`）。自进程启动起累计，重载不清零。
- 装 CA、开增强模式、改 macOS 的系统代理（只有 Safari 那几次）与 P5 的几件要改系统设置，见下一小节；增强模式的前置条件（要管理员 / root、一次性的机器）与阶段 3 的清单相同，见 `docs/acceptance/phase3-manual.md` 的「怎么用」。
- 给 M1 一节的 P3 要一台在非标准端口上有明文 HTTP 的服务器（自己的一台局域网服务器就行，下文写作 `<服务器>:8080`）；给 M2 一节的 P4 要一台在非标准端口上讲 TLS 的服务器（下文写作 `<服务器>:8443`，没有就跳过那一半）。
- 每一步执行后，在对应行填写「结果」（通过 / 不通过，附简要说明）、「日期」与「系统版本」三栏。

### 改系统设置的项与恢复

改系统设置的有四件：装 CA（M2 一节的 W1、M1、L1 与 P5）、开增强模式（M1 一节的 W2、M2、L2 与 M2 一节的 W2、M2、L2）、改 macOS 的系统代理（只有 Safari 那几次：M1 一节的 M1 与 M2 一节的 M2）与 P5 的几件（手机 Wi-Fi 的代理、手机上装的 CA、rurge 在局域网上监听、防火墙的放行）。四件都只在一次性的机器或能还原的虚拟机上做。增强模式在 Windows 要管理员、macOS 与 Linux 要 root；装 CA 在 Windows 用平时登录的那个账户、**不以管理员身份运行**（`certutil -user` 装进的是当前用户的库，提权到别的账户时装进那个账户的库，浏览器读不到），macOS 与 Linux 装 CA 要 root。M3 一节里还有两样：M6 的 Safari 改 macOS 的系统代理（用 Safari 做的 M7、M8 也是），以及 W7、M7、L7 与 P10 照 M2 一节装 CA、做完就卸。

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
- Windows：`certutil -user -delstore Root "<CN>"`（同样用平时的账户、不以管理员身份运行）。
- macOS：`sudo security delete-certificate -c "<CN>" /Library/Keychains/System.keychain`。
- Debian、Ubuntu：删掉 `/usr/local/share/ca-certificates/rurge-ca.crt`，再 `sudo update-ca-certificates --fresh`。
- Fedora、RHEL：删掉 `/etc/pki/ca-trust/source/anchors/rurge-ca.pem`，再 `sudo update-ca-trust`。
- Arch Linux：`sudo trust anchor --remove rurge-ca.pem`。
- NSS（Linux 上的 Chrome 一类）：`certutil -d sql:$HOME/.pki/nssdb -D -n "rurge MITM CA"`。
- Firefox、Android、iOS：在设置里删（Firefox 是「证书 → 查看证书 → 证书颁发机构 → 删除」；Android 是「加密与凭据 → 用户凭据 → 删除」；iOS 是删掉那个描述文件）。

`<目录>` 里的 `rurge-ca.p12` 与 `rurge-mitm.conf` 带着私钥与口令（Unix 上是 `0600`，Windows 上 rurge 不另设权限），做完把整个目录删掉。

**macOS 的系统代理（只有 Safari 那几次：M1 一节的 M1 与 M2 一节的 M2）。** 改之前先用 `networksetup -getwebproxy "<服务>"`、`networksetup -getsecurewebproxy "<服务>"`、`networksetup -getsocksfirewallproxy "<服务>"` 三条只读命令把原值抄下来（`<服务>` 是 Wi-Fi 或有线的网络服务名）。写回的办法照 `docs/acceptance/phase3-manual.md` 的「改系统设置的项与恢复」里 macOS 系统代理那一句（`networksetup -setwebproxy` / `-setsecurewebproxy` / `-setsocksfirewallproxy` 与对应的 `-set…state on|off`），这里不重抄；或者直接在「系统设置」里把三栏改回去。M3 一节的 M6（以及用 Safari 做的 M7、M8）照这一段做，做完把三栏改回去。

**P5 的几件（Android 与 iOS 经局域网）。** 这一项动四样东西：手机 Wi-Fi 的代理、手机上装的 CA、rurge 在局域网上监听（`http-listen = 0.0.0.0:6152`）、防火墙的放行。Windows 与 macOS 第一次有程序在 `0.0.0.0` 上监听时会问要不要放行，允许了就多一条放行规则。做完把 `http-listen` 改回 `127.0.0.1:6152`，删掉那一条放行规则（Windows 是「Windows Defender 防火墙 → 允许应用通过防火墙」里 `rurge` 那一行；macOS 是「防火墙选项」里 `rurge` 那一行的允许），把手机 Wi-Fi 的代理改回原样，删掉手机上的 CA。在 `0.0.0.0` 上监听，局域网里谁都能用这个代理：只在自己信得过的网络里做，做完就改回。

**增强模式。** 恢复的做法与阶段 3 的清单相同：Ctrl-C 或 `rurge stop` 之后核对虚拟网卡、路由与系统 DNS 回到原值，强杀之后靠下一次启动恢复；命令与要看哪几样见 `docs/acceptance/phase3-manual.md` 的「改系统设置的项与恢复」，这里不重抄。

### 第 7 节的五条验收标准各由哪几项证明

| 第 7 节 | 要看到什么 | Windows | macOS | Linux | 与平台无关 |
| --- | --- | --- | --- | --- | --- |
| 1 | 手册中每个重写示例有对应测试 | — | — | — | 由用例证明、不是手工项：`crates/rurge-engine/tests/rewrite.rs` 里名字以 `the_manual_` 开头的 16 条逐条照四页手册（URL Rewrite、Header Rewrite、Body Rewrite 与 Map Local 各一页）的示例行，其中 M3a 的 7 条（`the_manual_url_rewrite_header_example`、`the_manual_url_rewrite_302_and_307_examples`、`the_manual_url_rewrite_reject_example`、`the_manual_matching_details_example`、`the_manual_header_rewrite_examples`、`the_manual_header_rewrite_regex_example`、`the_manual_header_rewrite_framing_limit`）与 M3b 的 9 条（`the_manual_body_rewrite_request_example`、`the_manual_body_rewrite_response_example`、`the_manual_body_rewrite_pairs_example`、`the_manual_body_rewrite_capture_example`、`the_manual_body_rewrite_anchors_example`、`the_manual_body_rewrite_empty_body_example`、`the_manual_utf8_requirement`、`the_manual_map_local_examples`、`the_manual_map_local_empty_result`）；测试源站的端口插在主机后面，带斜杠的式子写 `:\d+`（修正记录第 73 行）；手工项 W6、M6、L6 拿真实浏览器的流量与它们对照 |
| 2 | 主流站点 MITM 解密成功且证书固定站点给出提示 | W2、W3 | M2、M3 | L2、L3 | P1（证书固定的提示）、P2 ～ P5 |
| 3 | Body Rewrite 与 jq 用例通过 | — | — | — | Body Rewrite 的正则形式有了项（W7、M7、L7）；jq 等 M3c——这一构建里 jq 的行是 `W0042`、那一行跳过（P10 的一处） |
| 4 | 大小限制行为与手册一致 | — | — | — | P9（32 MiB 与 10 MiB 的边界）；回环上的用例是 `the_request_limit_is_thirty_two_mib` 与 `the_response_limit_is_ten_mib` |
| 5 | 抓包可通过 API 读取 | — | — | — | M4 之后才有项 |

---

## M1　会话入口与明文 HTTP 引擎

自动化里证明过的是：嗅探的五种结局、判为 HTTP 的连接进引擎、每个请求各自求规则与选策略、记录里的 `method`、`url`、`statusCode`，以及三种入口（`CONNECT`、SOCKS5、内存设备上的虚拟网卡）——全部用回环上的 socket 对与测试自己写的 HTTP 客户端，客户端是 `curl` 一类的代码，不是浏览器。下面是 CI 与回环验不了的：真浏览器真的经代理发出去的请求长什么样、浏览器与站点之间的 keep-alive 与重试、增强模式开着时整台机器的流量、真站点返回的状态码与重定向、真 DNS 与 fake-IP 的配合。**一项也没有做过。**

### 前置条件

- 同「怎么用」：`cargo build --release`；一份 `http-listen`、`socks5-listen`、`http-api`、`loglevel` 的 `<conf>`；日志；两个看记录的 `curl`。
- 增强模式的三条（W2、M2、L2）要管理员 / root 与一次性的机器，照阶段 3 清单的前置条件。
- 明文 HTTP 的站点（下文写作 `<明文站点>`）要选只讲明文 HTTP、没有 HTTPS 的那种（例如 `neverssl.com` 一类专门不用 TLS 的站点，或你自己那台服务器的 80 端口）：Chrome 从 115 起会先把 `http://` 的导航升级成 `https://`，站点支持 HTTPS 时浏览器走的就是 `https://`，记录里就没有 `protocol` 是 `http` 的那一条了。非标准端口的那一条用 `<服务器>:8080`。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W1 | **真浏览器经 HTTP 代理与 SOCKS5 打开明文 HTTP 的站点。** 先读一次 `GET /v1/rurge/http` 的 `requests`。照「共用的前置条件」里 Edge 与 Chrome 的写法，带 `--proxy-server="http://127.0.0.1:6152"` 另起一个实例（先做这一种），打开 `http://<明文站点>/` 并点开里面的一两个链接；再带 `--proxy-server="socks5://127.0.0.1:6153"` 另起一个实例，重新打开同一个站点。每次之后读记录与计数 | 每个请求一条记录：经 HTTP 代理的 `listener` 是 `http`，经 SOCKS5 的是 `socks5`；`transport` 是 `tcp`；`protocol` 是 `http`；`method` 是 `GET`（或站点实际用的方法）、`url` 是 `http://<明文站点>/…`（去掉 userinfo、留着查询串）、`statusCode` 是站点实际答的码（一般是 `200`）；`status` 是 `completed`；`requests` 增加的数与发出的请求数一样。（HTTP 监听器的绝对形式请求不经过嗅探，所以 `sniffed` 不必跟着涨；经 SOCKS5 的每条连接 `sniffed.http` 加一。）Chrome 从 115 起会先把 `http://` 的导航升级成 `https://`，不成再退回：记录里可能先有一条到 443 的记录（`protocol` 是 `https`，多半是 `failed`），那是浏览器的试探，不算不通过，`sniffed.tls` 会因此加一。`<明文站点>` 选了支持 HTTPS 的站点时，浏览器走的就是 `https://`，记录里 `protocol` 是 `https`、`url` 是 `null`——这一行就对不上，不是 rurge 的问题（见前置条件）。浏览器自带的 DoH（「安全 DNS」）不影响这一条（请求走的是代理），但它会让增强模式那一条的记录里 `dst` 是地址而不是名字，所以做 W2 之前关掉它 | `GET /v1/rurge/http` 前后各一次；`recent` 的 JSON；浏览器的代理参数 | | | |
| W2 | **增强模式：不经代理的浏览器与命令行工具。** 管理员运行 `rurge.exe run -c <conf> --enhanced-mode --log-level debug`；系统代理与浏览器的代理都关着，浏览器与系统的「安全 DNS」/ DoH 也关掉（否则记录里的 `dst` 是地址而不是名字），`curl.exe` 不带 `-x`。用浏览器打开 `http://<明文站点>/`，再 `curl.exe -s -o NUL -w "%{http_code}\n" http://<明文站点>/`；读记录与计数。做完关掉增强模式并核对恢复（见「改系统设置的项与恢复」） | 两次都有记录，`listener` 是 `tun`（不是 `http` / `socks5`），`method`、`url`、`statusCode` 与 W1 一样，`curl.exe` 打印 `200`；`requests` 随每个请求加一（`curl.exe` 那一条固定加一）；`dst` 是名字与 80 端口。记录里 `pid` 与 `processPath` 是浏览器与 `curl.exe` | `recent`；`curl.exe` 的输出；`GET /v1/rurge/http` | | | |

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | **真浏览器经 HTTP 代理与 SOCKS5。** 照 W1 做，浏览器用 Safari 与 Chrome：Chrome 照「共用的前置条件」带 `--proxy-server=…` 另起一个实例；Safari 只能用系统代理（「系统设置 → 网络 → <服务> → 详细信息… → 代理」，见「改系统设置的项与恢复」那一条），改完重新打开 Safari，做完把三栏改回去；`./rurge run -c <conf> --log-level debug` | 同 W1；`processPath` 是 Safari 的网络进程与 `/Applications/Google Chrome.app/…` 一类的真实路径 | 同 W1；Safari 的系统代理原值与恢复 | | | |
| M2 | **增强模式。** 照 W2 做：`sudo ./rurge run -c <conf> --enhanced-mode --log-level debug`，浏览器的系统代理与「安全 DNS」关着，`curl` 不带 `-x` | 同 W2；`processPath` 是 Safari、Chrome 与 `/usr/bin/curl` | 同 W2 | | | |

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L1 | **真浏览器经 HTTP 代理与 SOCKS5。** 照 W1 做，浏览器用 Firefox 与 Chromium：Firefox 在它自己的设置里（见「共用的前置条件」），Chromium 带 `--proxy-server=…` 另起一个实例；`./rurge run -c <conf> --log-level debug` | 同 W1；`processPath` 是 `firefox`、`chromium` 一类的真实路径 | 同 W1；Firefox 改过的代理选项要改回「使用系统代理设置」 | | | |
| L2 | **增强模式。** 照 W2 做：`sudo ./rurge run -c <conf> --enhanced-mode --log-level debug`，浏览器的代理与「安全 DNS」关着，`curl` 不带 `-x` | 同 W2；`processPath` 是 Firefox、Chromium 与 `/usr/bin/curl` | 同 W2 | | | |

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | **443 上的 TLS 按 SNI 求规则，不解密。** 先读一次 `requests` 与 `sniffed.tls`。`<conf>` 的 `[Rule]` 第一行写 `DOMAIN,<HTTPS 站点>,REJECT,extended-matching`；`curl --socks5 127.0.0.1:6153 -sS -o NUL -w "%{http_code}\n" https://<HTTPS 站点>/`（`--socks5` 让 curl 自己解析名字，连接的目标是地址，所以规则只能靠 SNI 命中），再用浏览器打开同一个站点。然后把那一行改成 `DOMAIN,<HTTPS 站点>,DIRECT`（去掉 `extended-matching`），重启或重载，再各开一次 | 第一次：`curl` 以错误结束（连接被关，把它的原话记下来），浏览器是错误页；记录里 `sni` 是 `<HTTPS 站点>`、`protocol` 是 `https`、`rule` 是 `DOMAIN,<HTTPS 站点>,REJECT,extended-matching` 的原文、`policy` 是 `["REJECT"]`、`status` 是 `rejected`、`url` 与 `method` 是 `null`；`sniffed.tls` 增加（每条可嗅探的 TLS 连接加一）；`requests` 不因这两个连接增加（没有走到选路的请求）。第二次：站点打得开，浏览器里看到的证书是站点自己的（签发者不是 rurge 的 CA）；`curl` 那一条记录里 `protocol` 是 `https`、`rule` 是 `FINAL,DIRECT`、`url` 与 `method` 仍是 `null`（没有解密，也没有进 HTTP 引擎） | 记录；`curl` 的输出与退出码；`sniffed.tls`；浏览器的证书面板 | | | |
| P2 | **同一条 keep-alive 连接上的两个请求各走各的策略。** `<conf>` 的 `[Rule]` 写两行：`URL-REGEX,^http://<明文站点>/a,DIRECT` 与 `URL-REGEX,^http://<明文站点>/b,REJECT`（换成站点上真有的两个路径，下面写作 `/a` 与 `/b`）；经 HTTP 代理一次给两个 URL：`curl -sS -o NUL -w "%{http_code}\n" -x http://127.0.0.1:6152 http://<明文站点>/a http://<明文站点>/b`；读记录（`curl` 先打印一个 `200`，第二个 URL 的 `-w` 打印 `000`——连接被关、没有应答，把它的错误原话也记下来） | 第一个请求打得开、记 `200`、`rule` 是第一条、`policy` 是 `["DIRECT"]`、`status` 是 `completed`。第二个被拒：`status` 是 `rejected`、`rejectKind` 是 `REJECT`、`statusCode` 是 `null`（`show-error-page-for-reject` 缺省关着，不答就关）、`rule` 是第二条，`curl` 的 `-w` 给它打印 `000`。两条记录是**一条客户端连接**上的两个请求：`src` 相同（同一条连接、同一个来源端口）、`listenPort` 都是 `6152`，`url` 各自是自己的那个；`requests` 加二 | `recent`；`curl` 的输出 | | | |
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
| W1 | **生成并安装 CA（本平台）。** 在 `<CA 目录>` 里 `rurge.exe mitm ca generate --out .`（目录要先建好），照「Windows, this user」那一行 `certutil -user -addstore Root rurge-ca.cer` 装进当前用户的「受信任的根证书颁发机构」（用平时登录的那个账户、不以管理员身份运行；`certutil` 会弹一个安全警告，点「是」）；用 `certutil -user -store Root` 确认里面列着 `<CN>`；把 `rurge-mitm.conf` 的两行抄进 `<conf>` 的 `[MITM]`，写好 `hostname`，`rurge.exe run -c <conf> --log-level debug`；`rurge.exe check -c <conf>` | `generate` 打印四个文件的路径、抄进配置的说明与各平台的装 CA 步骤，**没有**口令与私钥；`certutil -user -store Root` 里列着 `<CN>`；`check` 没有 `warning[W0039]` 与 `warning[W0040]`；`GET /v1/features/mitm` 是 `{"enabled":true}`；`GET /v1/rurge/http` 的 `mitm` 四个数都是 `0` | `generate` 的输出；`certutil` 的输出；`check` 的输出；两个 API | | | |
| W2 | **真浏览器经三种入口打开解密的真站点。** 浏览器（Edge 与 Chrome 各一次）照「共用的前置条件」带 `--proxy-server="http://127.0.0.1:6152"` 另起一个实例，打开 `https://<站点 1>/`；再带 `--proxy-server="socks5://127.0.0.1:6153"` 另起一个实例打开同一个站点；再不带 `--proxy-server` 另起一个实例（也就是关掉浏览器的代理），管理员运行 `rurge.exe run -c <conf> --enhanced-mode --log-level debug`，系统代理与浏览器和系统的「安全 DNS」/ DoH 都关着（照 M1 一节的 W2；不关的话记录里的 `dst` 是地址，不是 `<站点 1>`），不经代理打开同一个站点。每次之后读记录与计数，并点地址栏的锁看证书 | 三次都打得开。每次的每条请求一条记录，`listener` 依次是 `http`、`socks5`、`tun`；`protocol` 是 `https`；`url` 以 `https://<站点 1>` 开头（带查询串、不带 userinfo），`method` 与 `statusCode` 有值；`sni` 是 `<站点 1>`；`status` 是 `completed`；`dst` 是 `<站点 1>` 与 443。浏览器里的证书的签发者是 `<CN>`，不是站点自己的 CA。`mitm.decrypted` 每次增加（每条解密的连接加一，浏览器会开好几条） | 浏览器的证书面板；`recent`；`/v1/rurge/http` | | | |
| W3 | **HTTP/2：`h2 = true` 与缺省（false）。** `<conf>` 的 `[MITM]` 写 `h2 = true`，重启；浏览器打开 `https://<站点 1>/`，在开发者工具的 Network 里看那一条请求的协议一栏；再用 `curl.exe --http1.1 -s -o NUL -w "%{http_code}\n" -x http://127.0.0.1:6152 https://<站点 1>/` 与 `curl.exe --http2 -s -o NUL -w "%{http_code}\n" -x http://127.0.0.1:6152 https://<站点 1>/` 各一次（两个都加 `-v`，把 curl 报的协议版本记下来）。curl 要信 rurge 的 CA：Windows 的 `curl.exe` 读系统的证书库；macOS 与 Linux 的 curl 不一定读系统里装的那一份，不信时加 `--cacert <CA 目录>/rurge-ca.pem`。然后删掉 `h2`（缺省 false），重启，重复一遍 | `h2 = true`：浏览器里的协议是 `h2`；两条 `curl` 都打印 `200`（或站点实际答的码），`-v` 里报的版本是 HTTP/2 与 HTTP/1.1；两条记录都是 `protocol` `https`、`url` 以 `https://` 开头。缺省（false）：浏览器里的协议是 `http/1.1`；两条 `curl` 仍然都打印 `200`（或站点实际答的码；`--http2` 的 curl 同时给 `h2` 与 `http/1.1`，rurge 只取 `http/1.1`），两条记录也都解密。**只给 `h2` 的客户端在 `h2 = false` 时不解密**——浏览器里一般看不到这一支，除非你用的客户端是那样配的。OpenSSL / LibreSSL 版的 curl 不信 CA 时，rurge 给的是证书固定的提示（见 P1）：看到那一行 WARN 先查 curl 信不信这张 CA | 浏览器开发者工具；`curl -v` 的输出；记录 | | | |
| W4 | **从浏览器下载 CA；`http-api-tls` 下的 `rurge status`。** `http-api-tls` 先不写（缺省 false）：浏览器打开 `http://127.0.0.1:6171/v1/mitm/ca?x-key=testkey`，看下载到的文件名。然后 `[General]` 写 `http-api-tls = true`，重启 `rurge.exe run -c <conf> --log-level debug`；另开一个窗口 `rurge.exe status -c <conf>`；再 `rurge.exe status --remote 127.0.0.1:6171 --key testkey`。做完把 `http-api-tls` 删掉，重启 | 浏览器下载到的文件名是 `rurge-ca.cer`（应答头 `Content-Disposition: attachment; filename="rurge-ca.cer"`、`Content-Type: application/x-x509-ca-cert`），内容是 DER，与 `rurge-ca.pem` 是同一张证书。开了 `http-api-tls`：`rurge run` 打印 `api on https://127.0.0.1:6171`，INFO 一行 `http-api listening`，带 `tls` 字段（`true`）；`status` 的第一行是 `rurge at https://127.0.0.1:6171`，并且读得到内容（说明 API 的证书由 `[MITM]` 的 CA 按 IP 签、命令行只信这一张）；`--remote` 那一次以 `1` 退出，标准错误是 `error: cannot reach rurge at http://127.0.0.1:6171: <原因>`（它不读配置、不知道 CA，照旧讲明文，所以对开了 `http-api-tls` 的实例连不上）。把浏览器对 `https://127.0.0.1:6171/v1/mitm/ca?x-key=testkey` 的看法也记下来（装了 CA 之后它应当也信） | 浏览器的下载与证书面板；`rurge run` 的输出；`status` 的两行输出与退出码 | | | |
| W5 | **不信任的客户端：卸掉 CA 之后。** 照「Windows」那一行卸掉 CA：`certutil -user -delstore Root "<CN>"`，`certutil -user -store Root` 确认没有它了；把浏览器完全退出再打开（浏览器会留着已经验过的连接），打开 `https://<站点 1>/`；读日志、记录与 `GET /v1/rurge/http` 的 `mitm` 计数。（rurge 不用重启：信不信这张 CA 是客户端的事。）这一项用浏览器做，不用 curl（见 P1） | 站点打不开（浏览器报证书错误，因为拿到的是 rurge 签的、而它不再被信任）。日志一行 WARN `the client does not trust the certificate rurge issued for <站点 1>: trust rurge's MITM CA on the client, or exclude the host with -<站点 1> in [MITM] hostname`（同一个名字一小时内的第二次只有 DEBUG，句子相同）；经增强模式来的连接，那一行不带名字与端口，写成 `the client does not trust the certificate rurge issued for a host reached through the virtual interface (the name is in the DEBUG log and the request record): trust rurge's MITM CA on the client, or exclude the host in [MITM] hostname`，名字另起一行 DEBUG、也在记录里。`mitm.untrusted` 加一；承载连接一条失败记录，`error` 是 `the client does not trust the certificate rurge issued (alert <警报的名字>)`，`status` 是 `failed`；记录里没有请求（`url` 与 `method` 是 `null`）。做完照 W1 那一行把 CA 装回去 | 浏览器；日志；`recent`；`/v1/rurge/http` | | | |

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M1 | **生成并安装 CA。** 照 W1 做，装 CA 用 `sudo security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain rurge-ca.pem`；确认用 `security find-certificate -c "<CN>" -a /Library/Keychains/System.keychain` 或「钥匙串访问」看 | 同 W1；Safari 与 Chrome 都认这张 CA | `generate` 与 `security` 的输出；`check`；两个 API | | | |
| M2 | **真浏览器经三种入口。** 照 W2 做，浏览器用 Safari 与 Chrome（Chrome 照前置条件带 `--proxy-server=…`；Safari 只能用系统代理，见「改系统设置的项与恢复」，做完把三栏改回去；增强模式那一次用 `sudo ./rurge run -c <conf> --enhanced-mode`，并把「安全 DNS」关掉） | 同 W2 | 同 W2 | | | |
| M3 | **HTTP/2 与 curl。** 照 W3 做（`curl.exe` 换成 `curl`） | 同 W3 | 同 W3 | | | |
| M4 | **下载 CA 与 `http-api-tls`。** 照 W4 做（`rurge.exe` 换成 `./rurge`） | 同 W4；Safari 下载到的文件名也是 `rurge-ca.cer` | 同 W4 | | | |
| M5 | **不信任的客户端。** 照 W5 做，卸 CA 用 `sudo security delete-certificate -c "<CN>" /Library/Keychains/System.keychain` | 同 W5；Safari 的报错原话记下来 | 同 W5 | | | |

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L1 | **生成并安装 CA。** 照 W1 做，装 CA 用本发行版那一行（Debian / Ubuntu 是 `sudo cp rurge-ca.pem /usr/local/share/ca-certificates/rurge-ca.crt && sudo update-ca-certificates`）；Chrome 一类用 NSS 库的浏览器还要 `certutil -d sql:$HOME/.pki/nssdb -A -t "C,," -n "rurge MITM CA" -i rurge-ca.pem`；Firefox 用自己的库，照打印的说明在设置里导入 | 同 W1；系统库（`/etc/ssl/certs` 里能看到条目）与 Firefox 各自认这张 CA 时的表现分别记下来——Firefox 不认系统库，要单独导入 | `generate` 与 `update-ca-certificates` 的输出；`check`；两个 API；浏览器的证书面板 | | | |
| L2 | **真浏览器经三种入口。** 照 W2 做，浏览器用 Firefox 与 Chromium（Firefox 在它自己的设置里，Chromium 带 `--proxy-server=…`；增强模式那一次用 `sudo ./rurge run -c <conf> --enhanced-mode`，并把「安全 DNS」关掉） | 同 W2 | 同 W2 | | | |
| L3 | **HTTP/2 与 curl。** 照 W3 做 | 同 W3 | 同 W3 | | | |
| L4 | **下载 CA 与 `http-api-tls`。** 照 W4 做（`rurge.exe` 换成 `./rurge`） | 同 W4 | 同 W4 | | | |
| L5 | **不信任的客户端。** 照 W5 做，卸 CA 用本发行版那一行（Debian / Ubuntu 是删掉 `/usr/local/share/ca-certificates/rurge-ca.crt` 再 `sudo update-ca-certificates --fresh`），NSS 那一份用 `certutil -d sql:$HOME/.pki/nssdb -D -n "rurge MITM CA"` | 同 W5 | 同 W5 | | | |

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P1 | **证书固定：握完手、一个请求也不发就关的客户端。** 选一个证书固定的站点（下文 `<固定站点>`，`[MITM] hostname` 里要有它），用你手边的工具算出它**真实**证书的公钥的 pin（SHA-256，Base64；`openssl s_client` 一类都行，算的是源站的，不是 rurge 签的那张）。先读一次 `GET /v1/rurge/http` 的整个 `mitm` 对象。**用 curl：**`curl -sS -o NUL -w "%{http_code}\n" -x http://127.0.0.1:6152 --pinnedpubkey 'sha256//<pin>' https://<固定站点>/`（curl 要信 rurge 的 CA，见 W3；不信时它先报的是证书链不信，不是 pin 不符）。**用 `openssl s_client`：**`openssl s_client -proxy 127.0.0.1:6152 -connect <固定站点>:443 -servername <固定站点> </dev/null`（握手完成、一个请求也不发就关；Windows 的 cmd 里把 `</dev/null` 写成 `<NUL`；你的 `openssl` 没有 `-proxy` 选项时跳过这一半——curl 那一半触发的是同一条提示）。两次都读日志、记录与计数 | 两次的结果一样：curl 以 **`90`** 退出（`CURLE_SSL_PINNEDPUBKEYNOTMATCH`，pin 不符），`-w` 打印 `000`（没有应答），`-sS` 把错误说出来；`openssl s_client` 完成握手之后退出。日志一行 WARN `<固定站点> is likely protected by certificate pinning: the client finished the TLS handshake and closed without a request; exclude it with -<固定站点> in [MITM] hostname`（同一个名字一小时内的第二次只有 DEBUG，句子相同）；`mitm.noRequest` 加一，`mitm.untrusted` 与 `mitm.failed` **不**涨；承载连接一条失败记录，`status` 是 `failed`、`error` 是 `no request after the TLS handshake (likely certificate pinning)`，记录里没有请求。真浏览器的预连接也会触发这一条。把 `<固定站点>` 写成 `-<固定站点>` 放进 `hostname` 之后，同一个 `curl --pinnedpubkey` 就通了（那是照原样转发的连接，rurge 没参与 TLS）。**为什么两种客户端都落到这一条：**curl 核 pin 在 TLS 握手完成之后（libcurl 的 `CURLOPT_PINNEDPUBLICKEY` 说 pin 不符时 "aborts the connection before sending or receiving any data"）；OpenSSL / LibreSSL 版的 curl（`curl -V` 里看得到）连证书链也是握手完成之后才核（给 OpenSSL 设 `SSL_VERIFY_NONE`，握手成功之后才看链的结果、再核 pin）。所以一个**不信** rurge 的 CA 的这种 curl，在 rurge 看来也是「握完手、什么也没发就关了」，给的同样是证书固定的提示，不是「不信任」。**「不信任」那几项（W5、M5、L5）用浏览器做，不用 curl。** Windows 自带的 `curl.exe`（Schannel）不信 CA 时怎样，没有核过。这是证书固定提示的第二个误报来源（第一个是浏览器的预连接），归项目所有者（计划「延后事项」第 1 行） | `curl` 的输出与退出码；`openssl s_client` 的输出；日志；`recent`；`/v1/rurge/http` | | | |
| P2 | **源站自己的证书不受信：`expired.badssl.com` 与 `self-signed.badssl.com`。** `[MITM] hostname` 里加上 `expired.badssl.com, self-signed.badssl.com`，重载或重启；经 SOCKS5 各打开一次：`curl -s -o NUL -w "%{http_code}\n" --socks5-hostname 127.0.0.1:6153 https://expired.badssl.com/` 与同样的 `https://self-signed.badssl.com/`（curl 要信 rurge 的 CA，见 W3：macOS 与 Linux 不信系统里装的那一份时加 `--cacert <CA 目录>/rurge-ca.pem`）；读记录与浏览器看到的页面。然后在 `[MITM]` 写 `skip-server-cert-verify = true`，重载，再各打开一次 | 前两次 `curl` 打印 `502`；浏览器看到 rurge 的错误页，标题是 `Connection failed`（`show-error-page` 缺省开着；关着时不答就关）。记录里 `status` 是 `failed`、`statusCode` 是 `502`、`error` 里有上游证书的问题（过期 / 不受信，把原话抄下来），`url` 与 `method` 有值（请求走到了选路）。`skip-server-cert-verify = true` 之后两次都打得开、打印 `200`，记录 `status` 是 `completed`——rurge 只放过了对上游证书的链与名字的校验，没有放过别的。看到的是证书固定的提示而不是 `502` 时，先查 curl 信不信这张 CA（见 P1 的说明） | `curl` 的输出；浏览器的错误页；`recent` | | | |
| P3 | **名单与来源：`hostname-disabled`、排除项、`client-source-address`。** (a) `[MITM]` 里 `hostname = <站点 1>, <站点 2>`，另有 `hostname-disabled = <站点 2>`；经 SOCKS5 各打开一次。(b) 把 `hostname` 写成 `hostname = <站点 1>, -<站点 3>`，打开 `<站点 3>`。(c) 回到 `hostname = <站点 1>`，`[MITM]` 写 `client-source-address = 127.0.0.1` 打开 `<站点 1>`；改成 `client-source-address = ::1` 再打开；改成 `client-source-address = -127.0.0.1` 再打开。每次读记录与证书 | (a) `<站点 1>` 解密（证书是 `<CN>` 签的，记录 `url` 以 `https://` 开头），`<站点 2>` 不解密（浏览器看到站点自己的证书；记录 `protocol` 是 `https`、`url` 与 `method` 是 `null`）；`mitm.decrypted` 只涨一次。(b) `<站点 3>` 不解密。(c) `127.0.0.1` 时解密；`::1` 时不解密——本机的会话一律按 `127.0.0.1` 比，`::1` 的项不命中它（登记的行为）；`-127.0.0.1` 时不解密 | 浏览器的证书；`recent`；`mitm.decrypted` | | | |
| P4 | **端口：`:0` 与写明的端口。** `<站点 1>` 在 443 上。(a) `hostname = <站点 1>`（不带端口），打开 `https://<站点 1>/`。(b) 改成 `hostname = <站点 1>:8443`，再打开同一个站点的 443。(c) 改成 `hostname = <站点 1>:0`，再打开。有自己那台 `<服务器>:8443` 上的 TLS 服务器时，把它也写进 `hostname`（例如 `hostname = <站点 1>:0, <服务器>:0`）再各打开一次 | (a) 443 上解密。(b) 443 上**不**解密（写明的端口要与连接相同，8443 的项不命中 443）；写了 `<服务器>:8443` 时它解密。(c) 443 上解密；`<服务器>:8443` 也解密（`:0` 让它命中的目标的全部端口可嗅探）。另外：不带端口的项**不让别的端口可嗅探**——`hostname = <站点 1>` 时 `<服务器>:8443` 上的连接不被嗅探、不解密，要解密就得写明端口或写 `:0`（登记的行为） | 浏览器的证书；`recent`；`mitm.decrypted` | | | |
| P5 | **Android 与 iOS 经局域网里的 rurge 上网。（有了设备才做。）** `<conf>` 的 `http-listen` 改成 `0.0.0.0:6152`（否则只监听回环），`[MITM] hostname` 里有要打开的站点，重启。在设备上按 `generate` 打印的说明装 CA（Android 是「设置 → 安全 → 加密与凭据 → 安装证书 → CA 证书」，iOS 是装上描述文件再到「设置 → 通用 → 关于本机 → 证书信任设置」里打开完全信任）；把设备 Wi-Fi 的 HTTP 代理指向运行 rurge 的那台机器的局域网地址与 6152 端口；在设备上用浏览器打开 `https://<站点 1>/`；读记录 | 页面打得开。记录里 `listener` 是 `http`、`src` 是设备的局域网地址（不是 `127.0.0.1`）、`protocol` 是 `https`、`url` 以 `https://` 开头、`sni` 是 `<站点 1>`。`[MITM]` 写了 `client-source-address` 时，设备的地址要在其中（本机应用按它连进来的地址比）。iOS 上没有打开「完全信任」时打不开，日志里有「不信任」那一行 WARN、`mitm.untrusted` 加一。做完把设备上的证书、Wi-Fi 的代理与 `http-listen` 都改回去（见「改系统设置的项与恢复」里 P5 那一行） | 设备上的浏览器；`recent`；日志 | | | |
| P6 | **`rurge check` 的 `W0039` / `W0040`，与没有 CA 时的 API。** (a) `<conf>` 的 `[MITM]` 写了 `hostname`、没有 `ca-p12` 与 `ca-keystore-name`：`rurge check -c <conf>`；同时 `rurge run` 起来，`curl -s -H "X-Key: testkey" http://127.0.0.1:6171/v1/mitm/ca`。(b) `[MITM]` 写一个打不开的 `ca-p12`（例如 `ca-p12 = QUJDRA`）：再 `check`。(c) `[General]` 写 `http-api-tls = true` 与 `http-api = testkey@127.0.0.1:6171`，`[MITM]` 的 CA 仍然是坏的（(b) 那个）：再 `check`；`rurge run -c <conf>` 看标准输出与标准错误；`curl -x http://127.0.0.1:6152 http://<明文站点>/ -s -o NUL -w "%{http_code}\n"`；`rurge status -c <conf>` 看消息与退出码。(d) 删掉 `ca-p12` 与 `ca-passphrase`（`hostname` 与 `http-api-tls = true` 留着）：再 `check`，再 `rurge status -c <conf>` | `check` 的输出都在**标准输出**，每条诊断一行（有位置的带 `<文件>:<行>`），末尾一行 `<conf>: N error(s), N warning(s), N note(s)`。位置就是诊断报的那一行：`hostname` 那一行或 `ca-p12` 那一行。次序按位置排，没有位置的（`W0040`）排在前面。(a) 一条 ``warning[W0039] <conf>:<hostname 那一行>: MITM is off: `hostname` lists hosts to decrypt but `[MITM]` sets no CA (`ca-p12` or `ca-keystore-name`)``，`check` 以 0 退出（警告不是错误）；`GET /v1/mitm/ca` 是 `404`，正文 `{"error":"no usable MITM CA"}`；代理照常可用。(b) 一条 `warning[W0039]`，位置是 `ca-p12` 那一行，消息以 `MITM is off:` 开头，说的是哪一种问题（Base64 坏了、口令错了、不是 CA 之类）。(c) `check` 给两条，**`W0040` 在前**：`warning[W0040]: http-api-tls is on but there is no usable MITM CA: the API does not start with this profile` 与它下面一行 `  hint: configure a usable MITM CA, or turn http-api-tls off`；`W0039` 在后，带 `ca-p12` 那一行的位置。次序是这样：`check` 按位置排，没有位置的排在前面（`W0040` 没有位置）。`rurge run` 启动时标准错误上也是这个次序，两条都**不带位置**（`warning[W0040]: …`、`warning[W0039]: …`），`W0040` 下面同样有 `  hint: …`；标准输出上是 WARN `http-api-tls is on but there is no usable MITM CA: the API is not started (it never falls back to plain HTTP)`、**没有** `api on` 那一行，进程照常跑、`curl` 经 6152 打印 `200`（API 不会退回明文）。`rurge status -c <conf>` 以 `2` 退出，标准错误以 `error:` 开头，后面是 **`MITM is off: …`**（即 (b) 那一句）：CA 读不出来时，它给的是 CA 的那一句，不是 `W0040`。(d) `check` 也是 `W0040` 在前（带 `hint` 那一行），`W0039` 在后、带 `hostname` 那一行的位置，消息仍是 ``MITM is off: `hostname` lists hosts to decrypt but `[MITM]` sets no CA (`ca-p12` or `ca-keystore-name`)``；`rurge status -c <conf>` 以 `2` 退出，标准错误以 `error:` 开头，后面是 `W0040` 那一句 `http-api-tls is on but there is no usable MITM CA: the API does not start with this profile`——`[MITM]` 一个 CA 也没写时，`status` 给的是这一句。修好 CA 之后 `check` 不再有 `W0039` / `W0040`，`status` 也读得到 | 两个命令的输出流与退出码；`rurge run` 两个流上的输出；`curl` 的输出；记录 | | | |

## M3　重写与 Map Local

M3a（`[URL Rewrite]` 与 `[Header Rewrite]`）与 M3b（`[Body Rewrite]` 的正则形式与 `[Map Local]`）的行为都已经有用例：`302` / `307` / `reject` 三种模式的答复、`header` 换掉拨号的目标、Header Rewrite 的四种动作与改过的 `Host` 写回、Body Rewrite 的式子与解压（`gzip`、`x-gzip`、`deflate`）与严格的 UTF-8、四页手册（URL Rewrite、Header Rewrite、Body Rewrite 与 Map Local 各一页）的示例行（测试源站的端口插在主机后面，带斜杠的式子写 `:\d+`，修正记录第 73 行）、Map Local 的四种数据与 502、两个上限、`rewrite` 开关与 `body` / `mapLocal` 两个数。那些用例的客户端是测试自己写的 HTTP 客户端与 rustls 客户端，源站是回环上的测试服务器。下面是它们验不了的：真浏览器真的发出去的 URL、头与正文长什么样，浏览器怎么处理 rurge 答的 `302` 与 `Location`，真站点对改写过的正文的反应（缓存、`Content-Encoding`、压缩前后的大小、`Vary`、`ETag`），真站点的应答是不是 `br` 或 `zstd` 压的（这一构建不改写，只留备注），CDN 与代理对 Map Local 的替换的看法，以及真站点上改写正文之后页面还对不对。**一项也没有做过。**

M3 的明文项在 `rurge run` 上就能做；解密的项（W7、M7、L7 与 P10）要先装 M2 那份 CA（做完照 M2 一节卸掉）。

### 前置条件

- M1 的 `[General]`、`[Rule]` 与日志、记录的命令；M2 的 `[MITM]`、`<CA 目录>` 与装 CA 的三条（只有解密的项要它）。
- `<conf>` 在 M2 那一份之后多四个节，下面是这一节要用的样子（`<明文站点>`、`<回显站点>`、`<站点 1>`、`<服务器>:8080` 与 `<那个词>` 见下）：

  ```ini
  [URL Rewrite]
  ^http://<明文站点>/m3-redirect http://<明文站点>/new 302
  ^https://<站点 1>/m3-redirect https://<站点 1>/new 302

  [Header Rewrite]
  http-request ^http://<服务器>:8080/echo header-add X-M3 yes
  http-request ^https?://<回显站点>/ header-add X-M3 yes

  [Body Rewrite]
  http-request ^https?://<回显站点>/ value rurge
  http-request ^http://<明文站点>/m3-upload value rurge
  http-response ^https://<站点 1>/ <那个词> rurge
  http-response ^http://<服务器>:8080/m3-big <那个词> rurge

  [Map Local]
  ^http://<明文站点>/m3-file data-type=file data="data/map-local.json"
  ^http://<明文站点>/m3-url data-type=file data="http://<服务器>:8080/data.json"
  ```

  改完重启 `rurge run`。要关掉整条重写：`curl -s -X POST -H "X-Key: testkey" -d '{"enabled":false}' http://127.0.0.1:6171/v1/features/rewrite`；Windows 的 `cmd` 不认单引号，那里写成 `curl.exe -s -X POST -H "X-Key: testkey" -d "{\"enabled\":false}" http://127.0.0.1:6171/v1/features/rewrite`（照 `docs/acceptance/phase3-manual.md` 第 95 行的写法，P7 不再重抄）。`[URL Rewrite]` 的两行里 `Location` 的路径换成你选中的那个站点上真有的路径；第二行要 `<站点 1>` 在 `[MITM]` 的 `hostname` 里。
- 一个回显请求头与请求正文的真实站点（下文写作 `<回显站点>`，例如 `httpbingo.org` 的 `/anything`）：请求正文的那一半靠它看出效果，`[Header Rewrite]` 的第二行也配上它。它在 `[MITM]` 的 `hostname` 里时，解密的项看得到；`[Header Rewrite]` 的第一行配的是自己那台 `<服务器>:8080` 上的 `echo`（回显请求头的小脚本），明文的 Header 那一半用它。
- 自己那台 `<服务器>:8080` 上的三个文件：`echo`（回显请求头）、`data.json`（内容随意，例如 `{"from":"url"}`）、`m3-big`（20 MiB 的 ASCII，里面放一个 `<那个词>`）。`<conf>` 同目录下建一个 `data/`，在那里写一个 `map-local.json`（内容随意，例如 `{"mapped":true}`）——Map Local 的相对路径接在 Profile 所在的目录后面。
- `<那个词>` 是 `<站点 1>` 某个页面里真实出现的一个词（挑一个去掉也不坏布局的，例如页面标题里的一个词）；改写之后它会变成 `rurge`。
- `<明文站点>` 同 M1 的前置条件（只讲明文 HTTP 的站点）；`<站点 1>` 同 M2（你真会打开的一个 HTTPS 站点）。
- 每一项执行后填「结果」「日期」「系统版本」三栏；做完把 `[MITM]`、四个节、Map Local 的文件与 `data/` 恢复。

---

### Windows

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| W6 | **URL Rewrite 与 Header Rewrite（明文的站点）。** 先读一次 `GET /v1/rurge/http` 的 `rewrite` 与 `requests`。浏览器照「共用的前置条件」带 `--proxy-server="http://127.0.0.1:6152"` 另起一个实例，打开 `http://<明文站点>/m3-redirect`，看地址栏最后停在哪里；再用同一实例打开 `http://<服务器>:8080/echo`，看页面里有没有 `X-M3`；读记录与计数 | `/m3-redirect` 由 rurge 当场答复：浏览器落到 `/new` 并把它打开；记录里那一条 `statusCode` 是 `302`、`remoteAddr` 是 `null`（一个字节也没有拨），`url` 加一，备注是 `URL Rewrite: 302 by ^http://<明文站点>/m3-redirect`。`/new` 那一条照常到源站、`status` 是 `completed`。`echo` 页面上 `X-M3` 是 `yes`，备注是 `Header Rewrite: request header-add X-M3 by ^http://<服务器>:8080/echo`（`by` 后面是那一行的 URL 式子，不是整行），`header` 加一——只有配上式子的请求才数，`/m3-redirect` 与 `/new` 不配上 `[Header Rewrite]` 的任何一行，头没改、`header` 也不动（设计第 6、7 节；用例 `the_manual_url_rewrite_302_and_307_examples`、`the_manual_header_rewrite_examples`、`header_mode_sends_the_request_to_the_new_target`） | 浏览器的地址栏；`echo` 页面；`recent` 的 `notes`、`statusCode`、`remoteAddr`；`rewrite` 的 `url` 与 `header` | | | |
| W7 | **解密站点上的 Body Rewrite：应答与请求。** 照 M2 一节装好 CA，`[MITM]` 写 `hostname = <站点 1>, <回显站点>`；浏览器打开 `https://<站点 1>/`，看 `<那个词>` 原先在的地方，并在开发者工具的 Network 里看那一条应答的 `Content-Encoding` 与 `Content-Length`；再用 `curl.exe -s -x http://127.0.0.1:6152 --data "value abc" https://<回显站点>/anything`（curl 要信 rurge 的 CA，见 M2 一节的 W3；不信时加 `--cacert <CA 目录>/rurge-ca.pem`），看应答里回显的正文与头；再用 `curl.exe -s -x http://127.0.0.1:6152 https://<站点 1>/`（**不带** `--compressed`：curl 就不发 `Accept-Encoding`，源站答没压过的正文；信 CA 的写法同上），在输出里找 `<那个词>`；读记录、计数与日志 | 浏览器那一次：那一条应答的 `Content-Encoding` 是 `gzip`、`x-gzip`、`deflate` 或没有时，`<那个词>` 变成 `rurge`、`Content-Length` 是新长度、应答没有 `Content-Encoding`（设计第 9.6 节）；是 `br` 或 `zstd` 时页面不变，备注是 `Body Rewrite skipped: ^https://<站点 1>/: content-encoding br is not supported`（或 `zstd`）、`skipped` 加一——这是登记的不同（这一构建不改写 `br` 与 `zstd`，M3c），不是这一项不通过。不带 `--compressed` 的 `curl` 那一次：源站答的是没压过的正文，输出里 `<那个词>` 变成了 `rurge`。`--data` 那一条：回显的正文是 `rurge abc`（`value` 换成了 `rurge`），应答里有 `X-M3: yes`（`[Header Rewrite]` 的第二行配上了 `<回显站点>`）。证书是 `<CN>` 签的（连接解密了）；记录都是 `protocol` `https`、`status` `completed`，备注是 `Body Rewrite: response by ^https://<站点 1>/`、`Body Rewrite: request by ^https?://<回显站点>/` 与 `Header Rewrite: request header-add X-M3 by ^https?://<回显站点>/`；`body` 每个请求只加一（同一个请求两个方向都改也只一）。日志里没有正文（`value abc` 一个字也搜不到） | 浏览器的页面、证书面板与开发者工具；`curl` 的应答；`recent` 的 `notes`；`rewrite.body`；`rurge run` 的日志 | | | |
| W8 | **Map Local 的文件与 URL。** 浏览器经同一个实例各打开一次 `http://<明文站点>/m3-file` 与 `http://<明文站点>/m3-url`；再看 `GET /v1/external_resources`；把 `<conf>` 同目录的 `data/map-local.json` 改一个值，页面刷新一次 | 两个地址显示的都是 rurge 答的数据（第一个是 `{"mapped":true}`，第二个是服务器上 `data.json` 的内容），`Content-Type` 是 `application/json`（按扩展名补的，没有 `charset`）；记录里两条的 `statusCode` 都是 `200`、`remoteAddr` 是 `null`、备注分别是 `Map Local: file by ^http://<明文站点>/m3-file` 与 `… /m3-url`，`mapLocal` 加二。`/v1/external_resources` 里多一项：`type` 是 `data`、`path` 是 `map local data <键的前 8 位> (http://<服务器>:8080/…)`、`ready` 是 `true`。改了文件之后下一次请求显示的是新值（每个请求都重读文件；用例 `the_file_is_read_on_every_request`） | 浏览器的页面与开发者工具；`recent`；`GET /v1/external_resources`；`rewrite.mapLocal` | | | |

### macOS

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| M6 | **URL Rewrite 与 Header Rewrite（明文的站点）。** 照 W6 做：Chrome 照前置条件带 `--proxy-server=…`，Safari 只能用系统代理（见「改系统设置的项与恢复」，做完把三栏改回去）；`./rurge run -c <conf> --log-level debug` | 同 W6 | 同 W6；Safari 的系统代理原值与恢复 | | | |
| M7 | **解密站点上的 Body Rewrite：应答与请求。** 照 W7 做（装 CA 照 M2 一节的 M1；`curl.exe` 换成 `curl`） | 同 W7 | 同 W7 | | | |
| M8 | **Map Local 的文件与 URL。** 照 W8 做 | 同 W8 | 同 W8 | | | |

### Linux

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| L6 | **URL Rewrite 与 Header Rewrite（明文的站点）。** 照 W6 做，浏览器用 Firefox 与 Chromium（Firefox 在它自己的设置里，Chromium 带 `--proxy-server=…`）；`./rurge run -c <conf> --log-level debug` | 同 W6 | 同 W6；Firefox 改过的代理选项要改回「使用系统代理设置」 | | | |
| L7 | **解密站点上的 Body Rewrite：应答与请求。** 照 W7 做（装 CA 照 M2 一节的 L1；`curl.exe` 换成 `curl`） | 同 W7 | 同 W7 | | | |
| L8 | **Map Local 的文件与 URL。** 照 W8 做 | 同 W8 | 同 W8 | | | |

### 与平台无关

| # | 操作 | 期望 | 怎么看 | 结果 | 日期 | 系统版本 |
| --- | --- | --- | --- | --- | --- | --- |
| P7 | **`rewrite` 开关关着时四个节一概不生效。** 先按 W6 与 W8 各做一次、记下 `rewrite` 的六个数；照前置条件里那句关掉整条重写（`{"enabled":false}`；Windows 用那里给的 `curl.exe` 写法），再打开 `/m3-redirect`、`/m3-file` 与 `<服务器>:8080/echo`；再 `POST` 回 `{"enabled":true}`，重复一次 | 关着时：`/m3-redirect` 显示站点自己的内容（没有 302、浏览器不跳），`/m3-file` 显示站点的 404 或那一页自己的内容（不是 Map Local 的数据），`echo` 页面上没有 `X-M3`；`rewrite` 的六个数一个也不动，`notes` 一条也没有。开着时回到 W6 与 W8 的样子。切回去对下一个请求立刻生效（设计第 11 节；用例 `the_switch_turns_body_rewrite_and_map_local_off`、`the_rewrite_switch_turns_every_rewrite_off`） | 浏览器；`recent` 的 `notes`；`GET /v1/rurge/http` 的前后两次 | | | |
| P8 | **Map Local 的两种失败。** (a) 把 `<conf>` 同目录的 `data/map-local.json` 改名，浏览器打开 `/m3-file`。(b) 把 URL 的那一条改成 `data="http://<服务器>:8080/missing.json"`（那个文件不存在），重载或重启，打开 `/m3-url`；再读 `GET /v1/external_resources` 里那一项的 `error`。做完改回去 | 两个都是 502 的错误页（`show-error-page` 缺省开着；关着时不答就关连接）：(a) 的标题是 `Connection failed`，页里写着 `The file a Map Local rule names cannot be read.`，记录里 `status` 是 `failed`、`error` 是 `Map Local: cannot read <路径>`，备注是 `Map Local failed: ^http://<明文站点>/m3-file: the file cannot be read`，`failed` 加一、`mapLocal` 不动；(b) 的页里写着 `The data a Map Local rule names has not been downloaded yet.`，记录是 `Map Local: the data from http://<服务器>:8080/… is not available yet`，备注的原因是 `the data is not available yet`，那一项在外部资源的列表里有 `error`（设计第 10.3 节；用例 `a_missing_file_answers_502`、`a_missing_map_local_file_answers_502`、`url_data_that_is_not_there_answers_502`、`map_local_url_data_that_never_came_answers_502`） | 浏览器的错误页；`recent` 的 `status`、`error`、`notes`；`rewrite.failed`；`/v1/external_resources` | | | |
| P9 | **32 MiB 与 10 MiB 的边界。** 请求那一半：`curl -x http://127.0.0.1:6152 -H "Expect:" --data-binary @<40 MiB 的文件> http://<明文站点>/m3-upload`（curl 对大于 1 MB 的 POST 缺省带 `Expect: 100-continue`，而 rurge 对带它的请求不缓冲——一律一条 `Body Rewrite skipped: {式子}: the request expects 100-continue`、正文照原样转发，走不到 32 MiB 那一支，所以 `-H "Expect:"` 让它不发；不加时看到的正是那一条 `Skipped`、正文原样到源站、`failed` 不动，这一半也记下来）。应答那一半：`curl -x http://127.0.0.1:6152 -o <落盘的文件> http://<服务器>:8080/m3-big`。各读一次记录与计数 | 请求那一条：连接被关掉（`curl` 以错误结束、没有应答），记录里 `status` 是 `failed`、`error` 是 `Body Rewrite: the request body is over 32 MiB`，`failed` 加一。应答那一条：20 MiB 一个字节不少地到了客户端（落盘的字节数与内容一致），`statusCode` 是 `200`、`status` 是 `completed`，备注是 `Body Rewrite skipped: ^http://<服务器>:8080/m3-big: the body is over 10 MiB`，`failed` 不动（读到的先发、其余接着流过；设计第 9.3 节；用例 `the_request_limit_is_thirty_two_mib`、`the_response_limit_is_ten_mib`、`a_response_over_the_limit_streams_through`） | `curl` 的输出与退出码；落盘文件的大小；`recent`；`rewrite.failed` 与 `body` | | | |
| P10 | **正文、数据与头的值不进备注与日志；jq 的行给 `W0042`。** 规则不动，照 W7 的 `--data` 那条 `curl` 发一个请求，正文里放规则不碰的一段：`curl.exe -s -x http://127.0.0.1:6152 --data "value s3cr3t-m3" -H "X-M3-Secret: h3ad3r-m3" "https://<回显站点>/anything?q=qu3ry-m3"`（信 CA 的写法照 W7），回显里是 `rurge s3cr3t-m3`。跑完在 `rurge run` 的日志里搜 `s3cr3t-m3`、`h3ad3r-m3` 与 `qu3ry-m3`（标准输出、标准错误与 `--log-file` 那一份都搜）；再读那一条记录的 `notes`；做过 W8 的话再在日志里搜一次 `mapped`。最后在 `[Body Rewrite]` 加一行 `http-response-jq ^https://<站点 1>/ .`，`rurge check -c <conf>` | 日志里一个也搜不到 `s3cr3t-m3`、`h3ad3r-m3` 与 `qu3ry-m3`，`notes` 里只有规则的式子与原因，没有正文、头的值、Map Local 的数据与 URL 的查询串；`mapped` 也搜不到。`check` 给一条 ``warning[W0042] <conf>:<那一行>: [Body Rewrite]: jq body rewrite is not available in this build of rurge; the line is ignored``（警告不是错误，`check` 以 0 退出，那一行跳过；设计第 13 节、第 8.5 与 14 节；用例 `no_body_or_data_reaches_the_notes_or_the_log`、`notes_never_carry_bodies`、`jq_lines_are_ignored_with_w0042`） | `rurge run` 的日志（两个流与 `--log-file`）；`recent` 的 `notes`；`rurge check` 的输出与退出码 | | | |
