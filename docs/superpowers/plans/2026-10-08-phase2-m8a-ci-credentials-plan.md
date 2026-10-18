# 阶段 2 / M8a（CI 转绿与凭据不外泄）实施计划

> **执行方式**：按 M8 细化设计 M8-D2——任务写到"改哪里、要哪些用例、判据是什么"为止，每个任务先写失败的用例、再实现。任务 1 ～ 5 在这份计划写下时已经做完（它们是前面各份计划「延后事项」里早已定了去向的小修正，逐条先看到红再改绿），这里按做完的样子记下；任务 6 要等带着任务 1 的提交推送之后、读到 CI 的失败清单才能展开，届时在本文追加。

**目标**：让 CI 的失败不登录也读得到，并把配置层与 DNS 上游里还会带出凭据的地方收掉。

**依据**：M8 细化设计（`docs/superpowers/specs/2026-10-08-phase2-m8-wrapup-design.md`）第 3 节。

**门禁**（每个任务结束、合并之前）：

```bash
RUSTFMT="…/stable-x86_64-pc-windows-gnu/bin/rustfmt.exe" cargo fmt --all --check \
  && cargo clippy --all-targets -- -D warnings \
  && cargo test -j 8 --workspace --no-fail-fast
```

本机上测试二进制偶发的异常退出（`STATUS_HEAP_CORRUPTION`，无失败用例）来自开发机的终端管控软件（M8-D4），单独重跑那个二进制。

---

## 计划期决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P1 | 失败摘要用什么写 | Python 脚本（`.github/scripts/report-test-failures.py`）：三个平台的 runner 都自带 Python 3（Windows 上叫 `python`，其余叫 `python3`）；解析 `cargo test` 的人读输出，不依赖不稳定的 JSON 输出。测试一步在三个平台上都改用 bash 跑（要 `pipefail` 与 `tee`） |
| P2 | 注解的数量 | GitHub 每一步每个级别留 10 条：依次用 `error`、`warning`、`notice` 共 29 条，第 30 条列出装不下的名字；每条至多 60 行 / 3000 字符。全文另写进任务摘要 |
| P3 | "不引用凭据"的两种写法 | `without_credentials`：`[凭据@]地址` 形的取值（API 的 key、监听口令），只留最后一个 `@` 之后的部分；`url_without_credentials`：URL，另把 scheme 留在前面——是"能当 scheme 的东西"（字母、数字、`+ - .`）才留，写错的 scheme 也留（那正是要告诉使用者的）。两者都按**最后一个** `@` 切，全角 `＠` 也算 |
| P4 | 没有 `@` 的 `http-api` 取值 | 不引用：它可能就是 key 本身。文本给出期望的写法 `key@address:port` |
| P5 | 配置对象的 `Debug` | 手写、用解构列出全部字段（以后加字段而忘了改 `Debug` 会编译不过）；原文一律经 `redact_profile` 的同一套规则显示，不另立规则。`wifi_access_http_auth` 的口令改成 `Secret<String>`（引擎读它的地方 `expose()`） |
| P6 | 判据 | 一条"标记"用例：每个凭据字段都带同一个标记串的配置，`Debug`、诊断、脱敏输出里都找不到它。新的凭据字段以后写进这份配置即可 |
| P7 | DoH 的凭据 | 构造上游时从 URL 里取出、百分号解码、做成 `Authorization: Basic`（标记为敏感头），URL 本身去掉用户名口令。依赖只加边：`base64` 与 `percent-encoding`（都已在锁文件里） |
| P8 | `https://` 上的全角 `＠` | 拒绝并说明：URL 的解析器不认它作分隔符，用户名口令会被读成主机的一部分 |
| P9 | 环境变量的读法 | clap 的 `BoolishValueParser`，不用 `FalseyValueParser`：后者把一切非否定词当成真，写错的值会悄悄打开开关 |
| P10 | 资源的 URL 在 `Debug` 里 | 只显示到主机与端口（`https://host:port/…`）；文件来源照旧显示路径 |
| P11 | `LazyHead` 修在哪里（P11 ～ P17 是 2026-10-18 展开任务 6 时定的） | 修在类型里：两半交给两个任务轮询是 `tokio::io::split` 本来允许的用法；生产的转发循环在一个任务里轮询两半，VMess 的 UDP 载体（`vmess/udp.rs`）却在拨号时起一个任务读、由 `send_to` 的任务写（写这一条时漏看了它，记录第 15 行），类型不该靠调用方怎样轮询才对。用例另外限时等写完（P15） |
| P12 | masque-server 的 1 MiB | 节制用例的上行（至多领先回读 512 KiB），不缩短 9 MiB + 777 字节的传送：这个长度是为了越过 rurge 每条流 8 MiB 的接收窗口。参照服务端的这个限制登记进兼容性清单（设计 3.2：修不了的登记，不让它留着红） |
| P13 | macOS 上 rurge 自己的 SOCKS5 UDP 中继 socket（缺省的发送缓冲，发给客户端的数据报大于 9216 字节时以 `EMSGSIZE` 失败、只丢那一个） | 这一轮不改，记进延后事项：没有哪条失败的用例由它而来；改法是知道的（JDK-8239355：把发送缓冲调到至少 65 507 / 65 527 字节），改了也只有 macOS 的 CI 验证得了 |
| P14 | `MockDns` 在 Windows CI 上找不到端口 | 原因没有证据：两种绑定次序轮流试、轮数从 16 加到 64、找不到时说出两种绑定各自最后的错误；不换到 20000–29999 的号段（测试约定把它留给"要先于主人写进配置的端口"） |
| P15 | 在别的任务里写、在用例的任务里等它写完的地方 | 18 处都限时 30 秒，第 1 类那样的挂住从此是一条失败的用例，而不是测试一步的 60 分钟超时；两条吞吐基准（忽略项，不进 CI）与假服务端里的一处不改 |
| P16 | 第 5 类之外、预料中的同一种失败 | `rurge_proto::testing::udp_echo_server` 与 `FakeSnell` 的 UDP 会话在 macOS 上要发 16 374 字节的数据报，这次没有跑到（所在的二进制停住了），一并调高发送缓冲，不等下一次运行；`rurge-proto` 因此多一条到 `socket2` 的依赖边（锁文件仍是 517 个包） |
| P17 | 文档里"第一次 CI 运行时核对""互操作用例还没有对真实的二进制跑过"的说法 | 兼容性清单、两份 README、需求文档与验收文档逐处按这次运行改写；CLAUDE.md 的历史段落不逐处改，在状态一段里写一句以这次运行为准（跑到的范围写清）；各份计划「延后事项」里去向是"第一次 CI 运行"的行，等三个平台的全部测试二进制都跑完的那一次运行之后逐行写结论 |

## 承接事项

| 出处 | 事项 | 任务 |
| ---- | ---- | ---- |
| M7c 延后事项 #24 | `[General]` 的 `W0012` 与 `redact_profile` 里 DNS 服务器 URL / `http-api` 的凭据 | 2 |
| M7c #25 | `EncryptedDns` 的 `Debug` | 2 |
| M7c #34 | DoH URL 里的用户名口令不起认证作用 | 3 |
| M7c #36 | 全角 `＠`；`invalid port` 的回显 | 3 |
| M7c #35，M3a #4，M3b #7，M2b | 测试二进制偶发的异常退出 | 设计 3.5（查明来源，关闭；没有代码改动） |
| M2b（计划期） | `Secret<T>` 没包到的字段：`WsOpts`、`KeystoreItem` | 2 |
| M3a #19 | `<random-string(…)>` 的长度错误引用括号里的文字 | 2 |
| M3a #23 | 资源管理器的 `Debug` | 5 |
| M3a #31 | `ProxyPolicy` 与原样保留的行的 `Debug` | 2 |
| M3b #21 | `CommonOpts` 的 `Debug` 打印 `test_url` | 2 |
| M6a #7 | `ObfsOpts` 的 `Debug` 打印 `obfs-host` | 2 |
| M3a #5 | 布尔环境变量只认 `true` / `false` | 4 |
| 各计划"首次推送后看 CI"的条目 | 见设计 3.2 | 6 |

---

## 任务 1：CI 的失败摘要

**文件**：`.github/workflows/ci.yml`、`.github/scripts/report-test-failures.py`（新）。

**做法**：
- 测试一步：`shell: bash`、`timeout-minutes: 60`、`set -o pipefail`，`cargo test --workspace --no-fail-fast 2>&1 | tee "$RUNNER_TEMP/test.log"`。
- 其后一步 `if: failure()`：按 `RUNNER_OS` 选 `python` / `python3`，运行脚本读那份日志。
- 脚本：按 `Running …` / `Doc-tests …` 行切出各个测试二进制；`---- <用例> stdout ----` 到下一个这样的行、`failures:` 或 `test result:` 为止是一个失败用例的输出（去掉回溯提示与首尾空行）；`error: test failed, to rerun pass …` 给出这个二进制的名字，其后几行里的 `process didn't exit successfully` 是异常退出的原因；第一个二进制之前的 `error:` / `error[E…]:` 连同其后到空行为止的几行是编译错误；日志结束时最后一个二进制没有 `test result:` 也没有异常退出，是没跑完，列出报过"has been running for over 60 seconds"而没有完成的用例。输出：注解（P2）与 `$GITHUB_STEP_SUMMARY`。任何情形下退出 0。

**核对**（脚本没有进 `cargo test`，对着真实的日志跑）：一份全部通过的门禁日志——"没有失败的用例"一条 `notice`，且不把 `rurge` 的用例打印的 `error[E0022]: …` 当成编译错误；一份有失败用例的日志——每个用例一条，标题是 `-p <包> --test <名> :: <用例>`；一份有异常退出的日志——带出退出码；合成的三份——35 个失败（10 + 10 + 9 + 1 条）、编译错误、没跑完。

**提交**：`ci: 测试一步跑完全部二进制；失败的用例摘要发成注解与任务摘要`（a965db1，已先行合并到本地 main，好让下一次推送就带上它）。

## 任务 2：`[General]` 的诊断、`redact_profile`、配置对象的 `Debug`

**文件**：`crates/rurge-config/src/{redact,general,policy,keystore}.rs`、`src/spec/{common,obfs,ws,http}.rs`、`src/text/mod.rs`、`tests/secrets.rs`（新）；`crates/rurge-engine/src/engine.rs`（读 `wifi_access_http_auth` 的一处）。

**先写的用例**（`tests/secrets.rs`，一份带标记 `S3CR3T` 的配置——`[General]` 五个带凭据的键、21 条策略行覆盖每种协议的凭据参数、带 `policy-path` 与 `external-policy-modifier` 的组、`[Keystore]`、`[WireGuard]`、`[Tailscale]`、`[MITM]`）：

- `the_profile_loads`：这份配置没有错误、每条该有 spec 的行都有（凭据确实被读了）。
- `the_debug_of_a_loaded_profile_holds_no_secret`、`the_diagnostics_of_a_valid_profile_hold_no_secret`、`the_redacted_profile_holds_no_secret`（另核对行数不变）。
- `a_diagnostic_of_a_mistyped_general_value_holds_no_secret`：八个写错的 `[General]` 取值，诊断里没有标记，但仍说得出是哪一项（`htps://***@dns.example/dns-query`、`***@api.example:6171`、`` `wifi-access-http-auth` `` 等）。

另：`redact::tests::what_stands_before_the_last_at_sign_goes`（两个函数的取法、key 里有 `@` 或 `://` 的情形、DNS 列表逐项、`auth-key`），`spec::http::tests::a_placeholder_whose_length_is_none_is_not_quoted`。

**看到的红**：`Debug` 漏出 60 处（`EncryptedDns.url`、两个 `ControllerAccess.key`、`wifi` 口令、每条 `ProxyPolicy` 的 `definition` / `positional` / `params`、`WsOpts`、`ObfsOpts`、`CommonOpts.test_url`、`KeystoreItem`、`[Tailscale]` 与 `[MITM]` 的原文行）；脱敏输出漏出 DNS URL 的口令与 `auth-key`；八条诊断原样引用取值；把 `redact_body` 改回按第一个 `@` 切时，`http-api = <标记>-a@<标记>-pi@…` 露出后半截。

**做法**：
- `redact.rs`：`without_credentials` 与 `url_without_credentials`（P3）；`SECRET_KEYS` 加 `auth-key`；`KEY_AT_KEYS` 的四个键逐项走前者、`dns-server` / `encrypted-dns-server` / `doh-server` 逐项走后者；`redact_line` 改为 crate 内可见。
- `general.rs`：DNS 三个键与监听两个键的诊断引用处理过的取值；`http-api` / `external-controller-access` 走 `invalid_controller`（P4）；`wifi-access-http-auth` 不引用；`EncryptedDns`、`ControllerAccess` 手写 `Debug`；`wifi_access_http_auth: Option<(String, Secret<String>)>`。
- `ProxyPolicy`（同 `PolicyGroup` 已有的做法：略去 `positional` / `params`，`definition` 经 `redact_definition`）、`KeystoreItem`、`WsOpts`、`ObfsOpts`、`CommonOpts`、`Entry`（`raw` 经 `redact_line`）手写 `Debug`（P5）。
- `spec/http.rs`：长度不是数字时的错误不再引用括号里的文字；超出范围的（那是数字）照旧引用。

**提交**：`fix(config): 诊断、脱敏输出与 Debug 不再带出凭据`（b09b126）。

## 任务 3：DNS 上游 URL

**文件**：`crates/rurge-dns/src/upstream/{doh,mod}.rs`、`crates/rurge-dns/Cargo.toml`、根 `Cargo.toml`（`percent-encoding` 进工作区依赖）、`Cargo.lock`（两条依赖边）。

**先写的用例**：
- `doh::tests::a_urls_user_name_and_password_go_as_basic_authorization`：HTTP/1.1 与 HTTP/2（TLS）各一遍——URL 带 `al ice` / `s3cret:doh` 时请求有 `authorization: Basic YWwgaWNlOnMzY3JldDpkb2g=`，不带时没有这个头；任何别的头里都没有口令与 `@`。
- `doh::tests::a_redirect_is_not_followed`：服务端回 307 指向另一台，查询以 HTTP 错误结束，另一台没有收到任何请求（钉住既有行为：凭据不会被带到别处）。
- `upstream::tests::a_fullwidth_at_sign_hides_no_credentials`：五种 scheme × 两种用户信息 × 三种服务器写法，带全角 `＠` 的 URL 一律是错误且文本不含凭据；`tls://dns.example:8x53` 的错误是 `` `tls://dns.example:8x53`: invalid port ``。

**看到的红**：没有 `authorization` 头；`` `quic://al1ce:hunt3r2＠dns.example`: invalid port `hunt3r2＠dns.example` ``。

**做法**：`DohUpstream::new` 取出凭据、做成敏感的 `HeaderValue`、清掉 URL 里的用户名口令，`query` 带上这个头（P7）；`upstream/mod.rs` 里判断 `@` 的五处与 `quoted` 都认全角 `＠`，`https://` 上遇到它直接拒绝（P8），`host_port` 的端口错误不再引用。

**提交**：`fix(dns): DoH 的 URL 凭据以 Basic 认证发出；全角 ＠ 与端口回显`（0178a13）。

## 任务 4：布尔环境变量

**文件**：`crates/rurge/src/cli/{mod,run,runtime}.rs`、`crates/rurge/tests/cli.rs`。

**先写的用例**：`rule_match::a_switchs_environment_variable_takes_the_usual_spellings`——`RURGE_NO_NETWORK` 取 `1`、`true`、`yes`、`on`、`TRUE`、`y` 时 `rule match` 照常给出结果，取 `maybe` 时失败且错误点名 `--no-network`（否定的写法不在这里试：没有 `--no-network` 的 `rule match` 会去取资源）；`run::an_empty_group_goes_direct_unless_told_to_reject` 加三行（`1` 拒绝，`0`、`off` 直连）。

**看到的红**：`error: invalid value '1' for '--no-network'`。

**做法**：`cli::switch()` 返回 `BoolishValueParser`（P9），四个开关的 `#[arg]` 加 `value_parser = switch()`。

**提交**：`fix(cli): 开关的环境变量接受 1 / 0、yes / no、on / off`（aa8837d）。

## 任务 5：资源管理器的 `Debug`

**文件**：`crates/rurge-net/src/resource/{mod,cache}.rs`。

**先写的用例**：`resource::tests::the_debug_of_a_resource_shows_neither_its_url_nor_its_body`——`ResourceSpec`、`ResourceState` 的两个带正文的变体、`Meta` 的 `Debug` 里没有 URL 的路径与查询、没有正文，但有 `https://sub.example:8443/` 与 `32 bytes`。

**做法**：`ResourceSource`、`ResourceState`、`Meta` 手写 `Debug`（P10）。

**提交**：`fix(net): 资源的 Debug 不显示 URL 的路径与正文`（8fb7bf7）。

## 任务 6：CI 首跑的修正

写计划时：带着任务 1 的提交推送之后，从注解里读出三个平台各自失败的用例，逐条：定因 → 本机能复现的先补失败用例 → 修 → 记进下面的「执行期修正记录」。设计 3.2 列出的"首次推送后看 CI"的事项一并核对。

第一次推送（`b41930a`，2026-10-14，运行号 38536637023）：fmt、clippy 与全部参照服务端的安装步骤三平台都通过；`cargo test` 一步 Linux 以 101 退出（测试跑完了，有失败），macOS 与 Windows 用满了 60 分钟。注解没有说出失败的是哪些用例（记录第 5 行）；修正之后要再推送一次，失败清单才读得到。

第二次推送（`7bd6589`，2026-10-17，项目所有者推送的，运行号 39022550224；带着记录第 5、6 行的修正）：`bench` 通过；`check` 三个平台都在 `cargo test` 一步失败。Linux 6 分半跑完了全部测试二进制，以 101 退出；macOS 与 Windows 又用满了 60 分钟，都停在 `rurge-proto` 的单元测试二进制里（各有三条用例没有结束）。测试二进制按包名的次序一个接一个地跑，所以排在它后面的 `rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard` 与 `rurge-rules` 四个包在这两个平台上一个测试也没有跑（`rurge-proto` 没有集成测试；各包的文档测试排在全部测试二进制之后，整个工作区一个也没有）；`rurge-proto` 单元测试里别的用例在这两个平台上的结果也不知道（失败用例的输出要到二进制结束时才打印，那时的失败摘要也不列没跑完的二进制里已经失败的用例——`e499061` 起列出，记录第 17 行——，任务日志又要登录才能读）。失败摘要这一次认出了跑完的二进制里的全部失败与没跑完的二进制：Linux 5 条，macOS 6 条加一个没跑完的二进制，Windows 7 条加一个没跑完的二进制，都在每一步 10 条的上限之内。按根因分六类（读代码、参照服务端的源码与注解得出；本机能复现的只有第 1 类）：

| 类 | 平台 | 失败的用例 | 根因 | 子任务 |
| -- | ---- | ---------- | ---- | ------ |
| 1 | macOS、Windows | `shadowsocks::tests::{a_large_payload_crosses_many_chunks_both_ways, ss_2022_sends_names_and_crosses_chunks_of_0xffff}`、`snell::tests::{a_large_payload_crosses_many_chunks_both_ways_on_v1_to_v3, a_large_payload_crosses_many_records_both_ways}`（没有结束；每个平台三条） | `LazyHead` 丢了写端的唤醒。用例把流 `tokio::io::split` 成两半、在另一个任务里 `write_all` 1 MiB：第一次写把整段载荷并进请求头，读端与写端都去推它；底层的流对写方向只记一个唤醒者（后来者顶掉先来者），读端推完头之后只唤醒它自己记下的读端，写端那次 `poll_write` 欠着的 `Ready(n)` 再没有人来取，而用例等写端的 `writer.await` 没有时限。一个手动轮询、计数唤醒的探针在本机复现了它：写端被唤醒 0 次。同源的第二处：读端推完头之后的那次 `poll_flush` 是 `Pending` 时，下一次读不再补它（`head` 已经是 `None`）。生产的转发循环在同一个任务里轮询两半，碰不到；VMess 的 UDP 载体（`vmess/udp.rs`）在拨号时起一个任务读、由 `send_to` 的任务写，碰得到：第一个数据报的写过了 100 毫秒的宽限仍是 `Pending` 时，读端推完头只唤醒自己，`send_to` 要等到引擎 15 秒的 `SEND_TIMEOUT` 到时才完成（写这一行时漏看了它，记录第 15 行） | 6a |
| 2 | 三个平台 | `shadowsocks::{ss_against_sing_box_aead_2022_and_a_user, ss_2022_against_ssserver_with_one_key_and_as_a_user}` | `udp_roundtrip` 用 64 字节的缓冲收回包。`ss` 的载体把整个包收进调用方的缓冲再就地解开（`PacketSocket::recv_from` 的约定是给 64 KiB），2022 方法的回包是 65 字节（16 字节的分离头，1 + 8 + 8 + 2 字节的头，7 字节的地址，7 字节的载荷，16 字节的标签）：Linux 与 macOS 上被截断、解不开而丢掉（`Elapsed`），Windows 上接收直接失败（`WSAEMSGSIZE`，10040）。AEAD 方法的回包不到 64 字节 | 6b |
| 3 | Linux | `snell::{udp_against_snell_server, obfs_http_against_snell_server}` | snell-server 的双栈 socket 把 IPv4 的来源写成 `::ffff:127.0.0.1`，Snell 出站照原样报回——这是设计：出站照原样报，比较的一方经 `HostName::canonical` 归一，引擎的 UDP 流水线就是这样比的——，而 `udp_roundtrip` 比较时没有归一 | 6b |
| 4 | Linux | `masque::tcp_against_masque_server` | masque-server v0.13.0（MIT）每条 TCP 隧道里，客户端发来、还没写给目标的字节至多排 `MAX_BUFFERED_CLIENT_BYTES`（1 MiB），它不对 QUIC 施加背压，超出就以 H3_CONNECT_ERROR 重置那条流。`transfer` 两个方向同时各送 9 MiB + 777 字节，上行只受 QUIC 流量窗口的约束（masque-server 给每条流 4 ～ 16 MiB），目标那一侧一慢，排着的字节就过了 1 MiB。参照服务端的限制，不是 rurge 的缺陷 | 6b |
| 5 | macOS | `udp::{an_answer_too_long_for_the_client_is_dropped_alone, what_waits_for_an_outbound_is_bounded_in_bytes, a_refused_flow_holds_no_room}`、`udp_tls_family::a_datagram_too_long_for_vmess_is_dropped_alone` | 设计 3.2 要核对的"macOS 回环 UDP 数据报 9216 字节的上限"：macOS 上 UDP socket 的缺省发送缓冲是 9216 字节（`net.inet.udp.maxdgram`），一次 `send_to` 不能超过发送缓冲，大于它的数据报以 `EMSGSIZE`（40）失败。四条都失败在用例**自己**的 socket 上（发 20 000 ～ 65 500 字节的那一次） | 6b |
| 6 | Windows | `rurge-api` 的五条：`a_late_body_does_not_cost_a_stop_its_answer`、`a_group_is_tested_on_request`、`a_body_larger_than_what_is_read_away_does_not_hold_its_sender_up`、`a_late_body_does_not_cost_a_dns_flush_its_answer`、`a_late_body_does_not_cost_a_banned_source_its_403` | `MockDns::start` 先在 TCP 上 `bind` 0 号端口、再在同一个号上 `bind` UDP，16 次都失败（`could not find a port free for both TCP and UDP`）。为什么失败没有证据——失败的那几次的错误没有记下来；最可能的是 Windows 为 UDP 保留的成段端口（Hyper-V / WinNAT 的端口排除范围），而 TCP 的临时端口是一个挨一个地分配的，没有核对 | 6b |

设计 3.2 的事项在这次运行里的结论（只看跑到了的测试二进制；"通过"说的是这一次运行）：`rurge-platform` 的 Unix 分支在 Linux 与 macOS 上通过；`ExternalOutbound` 真实拉起程序的用例（`rurge-external-tests`）三个平台都通过，它在 `rurge-proto` 里的单元用例只在 Linux 上跑完；`sshd`（Linux、macOS）与各参照服务端的夹具都拉起来了，互操作用例除了第 2 ～ 4 类的五条，在装了参照的平台上都通过——这是第一次读得到互操作用例对真实二进制的结果；Linux 上 UDP 缓冲封顶在 `rmem_max`：用到大缓冲的 `rurge-proto-wireguard` 与 `rurge-proto-quic` 在 Linux 上通过；macOS 回环 UDP 9216 字节的上限：见第 5 类；sing-box 的 anytls 与 Shadow TLS 互操作用例三个平台都通过；Snell `reuse=false` 的半关闭：Snell 的 TCP 互操作用例都通过。

### 6a：`LazyHead` 的唤醒；限时等写完

**文件**：`crates/rurge-proto/src/transport/lazy_head.rs`；下面"限时等写完"列出的 18 处所在的文件。

**先写的用例**（`lazy_head::tests`；手动轮询；计数唤醒的 `Count`，实现 `std::task::Wake`；内层流 `OneSlot` 像 socket 一样对写方向只留一个唤醒者：写与 flush 各有一道闸，闸关着时记下（顶掉）调用者的唤醒者并返回 `Pending`，开着时收下全部字节、或给 flush 计一次数；读永远是 `Pending`；`grace` 取 30 秒，不起作用；前两条用例的 flush 闸一直开着）：

- `a_writer_parked_on_the_head_is_woken_when_a_read_finishes_it`：写端 `poll_write(b"payload")` 是 `Pending`；读端 `poll_read` 是 `Pending`；开写闸，取出 `OneSlot` 记下的唤醒者并唤醒；读端再 `poll_read` 仍是 `Pending`，`OneSlot` 收到 `HEAD|payload`；写端被唤醒过（计数 ≥ 1）。
- `a_writer_is_woken_though_the_reader_that_parked_last_never_polls_again`：前两步同上；开写闸、唤醒记下的唤醒者，此后不再轮询读端；写端被唤醒过，它再 `poll_write(b"payload")` 得 `Ready(Ok(7))`，`OneSlot` 收到 `HEAD|payload`。
- `a_reader_whose_flush_is_pending_is_woken_though_a_write_parks_after_it`：两道闸都关；写端 `poll_write(b"payload")`、读端 `poll_read` 都是 `Pending`；只开写闸，唤醒记下的唤醒者；读端 `poll_read` 推完头、停在 flush 上（`Pending`），记下读端此时的唤醒计数；写端 `poll_write(b"payload")` 得 `Ready(Ok(7))`；关上写闸，写端 `poll_write(b"more")` 是 `Pending`；两道闸都开，唤醒记下的唤醒者；读端的唤醒计数比记下的多；读端再 `poll_read` 是 `Pending`，flush 计数 ≥ 1；写端 `poll_write(b"more")` 得 `Ready(Ok(4))`；`OneSlot` 收到 `HEAD|payloadmore`。

**看到的红**：三条在今天的代码上都失败（第一条：写端被唤醒 0 次；第三条：读端没有再被唤醒，flush 计数是 0）。

**做法**：

- 新的两格唤醒者 `Both`（`Mutex<[Option<Waker>; 2]>`，下标是 `Half::Read` / `Half::Write`），实现 `std::task::Wake`：被唤醒时取出两格、放开锁之后逐个唤醒。`park(half, waker)` 把自己这一格换成 `waker`（那一格已有的唤醒者 `will_wake` 它时不换）；`release(half)` 清掉自己这一格，取出另一格并唤醒。锁中毒时照用（`PoisonError::into_inner`）。
- `LazyHead` 加 `both: Arc<Both>`、由它做成的 `woken: Waker` 与 `owed_flush: bool`，去掉 `reader`。
- 头还没出去（`head.is_some()`）或读端欠着 flush（`owed_flush`）时，对内层写方向的每一次轮询——`poll_head` 里的写、读端欠的 flush、写端的 `poll_write` / `poll_flush` / `poll_shutdown`——先 `park` 自己这一半，再以 `woken` 为唤醒者去轮询内层：内层只记得住一个唤醒者，它记下的总是能叫醒两半的那一个。别的时候照旧用调用者自己的 `cx`。
- 头写完（`written == head.len()`）或以错误结束时，`release` 推它的那一半；读端停在宽限的定时器上时 `park(Read)`（取代今天的 `self.reader`）。
- 读端推完头时置 `owed_flush`；flush 完成后清掉它、`release(Read)`，之后才读内层；下一次 `poll_read` 先补欠着的 flush。
- 写端的直通（今天的 `head.is_none() && self.coalesced == 0`）另要求 `!owed_flush`。
- `HEAD_GRACE`、并进头的载荷与各个方法返回的字节数都不变；原有的 6 条用例照旧通过。

**限时等写完**（第 1 类那样挂住的写端从此是一条失败的用例，不再吃掉测试一步的 60 分钟）：下面 18 处在别的任务里写、在用例的任务里等它写完的 `writer.await` / `sender.await` 改成 `tokio::time::timeout(Duration::from_secs(30), writer).await.expect("the writer finishes within the bound")`，原来对 `JoinHandle` 结果的 `.unwrap()` 留着——`crates/rurge-proto/src/` 下的 `h2connect/mod.rs:429`、`h2pool/mod.rs:926`、`shadowsocks/mod.rs:473` 与 `:941`、`shadowsocks/stream.rs:1223`、`snell/mod.rs:562` 与 `:1339`、`transport/obfs/mod.rs:658` 与 `:745`、`transport/shadow_tls/framed.rs:442` 与 `:464`、`transport/shadow_tls/mod.rs:530`、`transport/shadow_tls/record.rs:132`、`transport/ws.rs:333`，`crates/rurge-proto-quic/src/` 下的 `hysteria2/mod.rs:369`、`masque/mod.rs:373`、`tuic/mod.rs:388`，`crates/rurge-proto-wireguard/src/outbound.rs:478`。这 18 处都不用暂停的时钟（逐处核过）。两条吞吐基准（`hysteria2` 与 `wireguard` 的 `throughput`，忽略项，不进 CI）与假服务端里等自己任务的一处（`testing/anytls.rs:138`）不改。

**判据**：三条新用例先红后绿；`lazy_head` 的 9 条、`shadowsocks` 与 `snell` 的单元用例全过；三条新用例与第 1 类的四条用例用编好的测试二进制各单独循环跑 20 次，全过；门禁。

**提交**：`fix(proto): LazyHead 的两半在不同的任务里轮询时不再丢掉对方的唤醒`；`test: 在别的任务里写的用例限时等写完`。

### 6b：只改用例与测试夹具的五类

本机都复现不了（参照服务端本机一个也没有，第 5、6 类在别的平台上）：判据是门禁通过、改到的用例在本机照常通过或照常跳过，结论要等下一次 CI 运行。每一类一个提交。

1. 第 2 类：`tests/interop/tests/common/mod.rs` 的 `udp_roundtrip`，与 `masque.rs`、`sing_box.rs`、`xray.rs` 里收载体回包的三处（今天都是 `[0u8; 64]`），改成 `vec![0u8; 65536]`（与 `udp_side_by_side` 一致）。提交 `test(interop): 收 UDP 载体的回包给 64 KiB 的缓冲`。
2. 第 3 类：`udp_roundtrip`、`udp_side_by_side` 与上一条的那三处比较回包的来源时，两边都先经 `HostName::canonical` 归一（`common/mod.rs` 里一个把 `Target` 的主机归一的函数，这五处共用）；`tuic.rs` 里 `v4_udp_against_tuic_server` 的注释改说：服务端把 IPv4 的来源写成 IPv4 映射的形式，比较时两边都归一；rurge 自己把这个形式读作 IPv4，由 `tuic::addr::tests::an_ipv4_address_in_ipv6_form_reads_as_the_ipv4_address` 钉住。提交 `test(interop): 比较 UDP 回包的来源时把 IPv4 映射的地址当作 IPv4`。
3. 第 4 类：`tests/interop/tests/masque.rs` 的 `tcp_against_masque_server` 不再用 `transfer`，改用这个文件自己的有节制的双向传送：两个方向在同一个 `join!` 里；上行每次写至多 64 KiB，写之前从一个初值为 512 KiB 的 `tokio::sync::Semaphore` 取够这一次的许可并 `forget`；回读每读到 n 字节就 `add_permits(n)`——上行至多领先回读 512 KiB，masque-server 为这条隧道排着的客户端字节因此不超过它的 1 MiB。总长仍是 `(9 << 20) + 777`（大于 rurge 每条流 8 MiB 的接收窗口，这条用例要过的就是这道坎）；整个传送以 60 秒为限，超时或出错时的文本带上 `server.log_text()`；注释写明 masque-server 的 `MAX_BUFFERED_CLIENT_BYTES`（1 MiB，不施加背压）。`transfer` 本身不改（`sing_box_hysteria2.rs` 与 `tuic.rs` 照用）。提交 `test(interop): 对 masque-server 的大传送节制上行`。
4. 第 5 类：会发大于 9216 字节的数据报的测试 socket 都把发送缓冲调到 256 KiB（`socket2::SockRef::from(&socket).set_send_buffer_size(256 * 1024)`，失败就让用例失败）：`crates/rurge-engine/tests/common/mod.rs` 的 `udp_associate` 里的 socket（第 5 类的三条经它发 20 000 ～ 60 000 字节）；`crates/rurge-engine/tests/udp.rs` 的 `an_answer_too_long_for_the_client_is_dropped_alone` 里的 `remote`（发 65 500 字节）；还有预料中的同一种失败（它们所在的 `rurge-proto` 单元测试二进制这次在 macOS 上没有跑完）——`rurge_proto::testing::udp_echo_server` 的 socket 与 `FakeSnell` 每个 UDP 会话的 socket（`crates/rurge-proto/src/testing/snell.rs` 的 `Shared::udp`），`snell::tests` 的两条用例经它们往返 16 374 字节的数据报。`rurge-proto` 为此多一条依赖边：`socket2` 作 `testing` 特性带进来的可选依赖，另加进 `[dev-dependencies]`（同 `rcgen` 的写法；锁文件仍是 517 个包）。实现时再搜一遍别的测试 socket 有没有发大于 9216 字节的，搜法与结果写进报告。rurge 自己的 socket 不在这一轮改（P13）。提交 `test: macOS 上要发大数据报的测试 socket 调高发送缓冲`。
5. 第 6 类：`crates/rurge-dns/src/testing.rs` 的 `MockDns::start` 至多试 64 轮：第 1、3、5……轮照今天先 TCP 后 UDP，第 2、4、6……轮先在 UDP 上 `bind` 0 号端口、再在同一个号上 `bind` TCP；记下两种绑定各自最后一次的错误，都失败时 panic 的文本是 `could not find a port free for both TCP and UDP (last TCP error: <错误>; last UDP error: <错误>)`（没有发生过的一种写 `none`）。不用 20000–29999 的号段（P14）。提交 `test(dns): MockDns 找端口时两种次序轮流试，找不到时说出最后的错误`。

### 6c：文档

- 本计划：「执行期修正记录」每一类一行，另一行记失败摘要第二次用上的结果（认出了全部失败与两个没跑完的二进制）；「延后事项」第 5 行的去向补上第二次用上的结果，另加：macOS 上 rurge 自己的 SOCKS5 UDP 中继 socket 的 9216 字节（P13；做法同 JDK-8239355，把发送缓冲调到至少 65 507 / 65 527 字节）；masque-server 每条隧道 1 MiB 的客户端字节（参照服务端的限制，不处理，已登记兼容性清单）；`MockDns` 在 Windows CI 上为什么找不到端口（下一次失败时看文本里的两种错误再定）；macOS 与 Windows 上没有跑到的四个包（`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-rules`）的测试与 `rurge-proto` 单元测试其余用例的结果（下一次推送）；各份计划「延后事项」里去向是"第一次 CI 运行"的行（P17）。
- 兼容性清单：4.2 节 `masque` 一行的互操作参照补一句 masque-server 每条 TCP 隧道至多排 1 MiB 客户端发来而还没写给目标的字节、超出即以 H3_CONNECT_ERROR 重置（经它的上传比目标收得快时会断；参照服务端的限制，用例按此节制上行）；写着互操作用例"还没有对真实的二进制跑过"的格子按这次运行改写（P17）。
- `README.md` 与 `README_en.md`、需求文档第 7 节的状态行、`docs/acceptance/phase2-acceptance.md`（第 8 节加一段注明日期的更新，表里对应的一行随之改）：CI 仍然没有绿过；互操作用例对真实的二进制跑过了一次（`7bd6589`），除第 2 ～ 4 类的五条都通过；macOS 与 Windows 上有四个包的测试没有跑到；六类失败修在哪些提交、要下一次推送才知道修好了没有。
- CLAUDE.md：状态一段里"CI 从没绿过"那几句换成这次运行的结论（运行号、跑到的范围、失败的六类与修正的提交、还不知道的），并写一句：此前各段里"第一次 CI 运行时核对""还没有对真实的二进制跑过"的说法，凡这次运行跑到了的（Linux 上全部测试二进制，macOS 与 Windows 上排在 `rurge-proto` 单元测试之前的各包；这次运行的提交里还没有阶段 3 / M2），以这一句为准；"接下来的工作"一句随之改。

**判据**：禁词检查；README 两份同样的节与特性表；改写的每一处都指到这次运行与它跑到的范围，不把"跑过一次、通过"写成"CI 绿了"。

**提交**：`docs: CI 第二次运行的结论与修正；兼容性清单、README、验收文档与 CLAUDE.md 同步`。

## 任务 7：文档

兼容性清单（6.2 节 `https://` 一行：DoH 的 Basic 认证、全角 `＠`、端口回显；10.4 节 `profiles/current`：`auth-key`、DNS 服务器 URL、按最后一个 `@` 切；10.3 节：环境变量的取值）、`docs/api/phase1.md`（脱敏名单的同样两处）、总设计（M8 一行的指引、`W0007` / `Unsupported` 的归宿、第 14 节第 1、8 条）、CLAUDE.md（状态、文档索引、命令）。

**提交**：`docs: M8 细化设计、M8a 计划与可行性报告；兼容性清单、API 说明与 CLAUDE.md 同步`（一次提交，排在任务 2 ～ 5 的代码之后）。

---

## 执行期修正记录

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 1 | 失败摘要的第一版把 `rurge` 的用例打印的 `error[E0022]: …` 认成了编译错误（对着一份全部通过的日志跑出来的）：只在第一个测试二进制开始之前认编译错误 | a965db1 |
| 2 | 2 | `without_credentials` 的第一版对任何取值都保留 `://` 之前的部分：`http-api = pa://ss@…` 会露出 key 的前半截。拆成两个函数（P3），key 与监听口令不走带 scheme 的那一个 | b09b126 |
| 3 | 2 | 标记配置里 WireGuard peer 的 `public-key` 起初也用了带标记的值，脱敏输出"漏出"的其实是公钥——公钥不是凭据，换成不带标记的值；`shadow-tls-version=3` 的行补上必填的 `shadow-tls-sni` | b09b126 |
| 4 | 2 ～ 5 | 门禁（任务 1 ～ 5 做完的树，2026-10-08）：格式与 clippy 通过；全量 69 个结果行、2175 个用例通过、3 个忽略。两轮各有一次与改动无关的偶发：第一轮 `rurge-engine` 的 `auto_groups` 以 `STATUS_HEAP_CORRUPTION` 异常退出，第二轮 `rurge` 的 `cli` 里一条用例拉起的 `rurge` 子进程在启动瞬间无声退出（同一类：被注入的进程在多个线程同时开始用 socket 时崩溃，M8-D4）；各自单独重跑通过（`cli` 连跑三次都是 57 / 57） | — |
| 5 | 1、6 | 第一次真正用上（`b41930a` 的运行）时，摘要一个失败的用例也没有认出：工具链的 action（`dtolnay/rust-toolchain`）给整个任务设 `CARGO_TERM_COLOR=always`，cargo 写的 `Running …` 与 `error: test failed, …` 带颜色码、`Finished …` 带链接，`Running …` 一行认不出，测试二进制一个也没切出来，`rurge` 的用例让守护进程打印的 `error: …` 被当成了编译错误（三个平台各两段）。读每一行之前先去掉控制序列（`ESC [ …`）与操作系统命令（`ESC ] …`）。核对：本机打开颜色跑出来的一份日志（认出 14 个二进制、没有失败），与由它合成的三份——一个失败的用例连同守护进程打印的 `error: …`（只报那个用例）、日志在一个二进制中间结束（报没跑完与还在跑的用例） | 1e7c644 |
| 6 | 6 | `check` 任务的缓存只在成功时保存，而这个任务从没成功过：每次运行都从头编译全部依赖。改为失败时也保存（`cache-on-failure: true`）。macOS 与 Windows 用满 60 分钟是不是因为编译慢，要看下一次运行的摘要（没跑完的二进制与还在跑的用例） | 1e7c644 |
| 7 | 6 | 第二次运行（`7bd6589`，运行号 39022550224）的第 1 类：`rurge-proto` 里 `shadowsocks` 与 `snell` 的四条单元用例，macOS 与 Windows 上每个平台有三条没有结束，原因是 `LazyHead` 的两半交给两个任务轮询时丢了写端的唤醒。底层的流对写方向只记一个唤醒者，读端推完请求头之后只唤醒它自己记下的读端，写端欠着的 `Ready(n)` 再没有人来取；同源的第二处：读端推完头之后的 `poll_flush` 是 `Pending` 时，下一次读不再补它。修在类型里（P11）：两半的唤醒者放进一个两格的 `Both`，头还没出去或读端欠着 flush 时，对内层写方向的每一次轮询都先登记自己这一半、再以能同时叫醒两半的唤醒者去轮询内层，读端欠着的 flush 由下一次读补完；三条手动轮询的用例先红后绿（`f8d0389`）。在别的任务里写、在用例的任务里等它写完的 18 处限时 30 秒，这一类的挂住从此是一条失败的用例，不再吃掉测试一步的 60 分钟（`4fc1bad`，P15）。再加四条用例：三条钉住唤醒另一半的几条路径（读端自己把头推完、推头出错、读端欠着的 flush 完成），一条用 `tokio::io::duplex(8)` 的真实管道、两半各在一个任务里，在旧代码上红、现在绿——这是第 1 类的机制第一次在本机用真实的任务调度复现出来；CI 上失败的那四条用例本身在本机用旧代码也过（本机的回环 socket 从不让写变成 `Pending`），所以它们没有被复现（`cae06c3`）。换下的唤醒者在放开锁之后才丢弃，`get_mut` 的文档写明它绕过共用的唤醒者（`26be5e2`）。修好没有，要下一次推送才知道 | `f8d0389`、`4fc1bad`、`cae06c3`、`26be5e2` |
| 8 | 6 | 第 2 类（三个平台）：`ss` 的 SS 2022 对 sing-box 与对 `ssserver` 的互操作用例失败。`udp_roundtrip`，以及 masque、sing-box、xray 三个文件里收载体回包的地方，缓冲只有 64 字节，而载体的 `recv_from` 要把数据报按它在线上到达的样子整个收进调用方的缓冲（`PacketSocket::recv_from` 的约定是给 64 KiB），SS 2022 的回包在线上是 65 字节：Linux 与 macOS 上被截断、解不开而丢掉（用例等到超时），Windows 上接收直接失败（`WSAEMSGSIZE`）；AEAD 方法的回包不到 64 字节，一直没碰到。四处都改成 64 KiB，与 `udp_side_by_side` 一致。根因读自代码；本机没有这两个参照服务端，复现不了，修好没有要下一次推送才知道 | `c048315` |
| 9 | 6 | 第 3 类（Linux）：`snell` 对 snell-server 的 `udp_against_snell_server` 与 `obfs_http_against_snell_server` 失败。snell-server 的双栈 socket 把 IPv4 的来源写成 `::ffff:127.0.0.1`，Snell 出站照原样报回——这是设计：出站照原样报，比较的一方经 `HostName::canonical` 归一，引擎的 UDP 流水线就是这样比的——，而用例比较回包的来源时没有归一。`common::canonical` 把 `Target` 的主机归一，`udp_side_by_side`、`udp_roundtrip` 与 masque、sing-box、xray 三个文件里收载体回包的比较，两边都先经它；`tuic.rs` 里 `v4_udp_against_tuic_server` 的注释改说服务端也把 IPv4 的来源写成映射的形式，rurge 自己把这个形式读作 IPv4（`tuic::addr::tests::an_ipv4_address_in_ipv6_form_reads_as_the_ipv4_address` 钉住）。根因读自代码；本机没有 snell-server，复现不了，修好没有要下一次推送才知道 | `2519b3f` |
| 10 | 6 | 第 4 类（Linux）：`masque` 对 masque-server 的 `tcp_against_masque_server` 失败。masque-server v0.13.0 对每条 TCP 隧道把客户端发来而还没写给目标的字节至多排 1 MiB（`MAX_BUFFERED_CLIENT_BYTES`），不对 QUIC 施加背压，超出就以 H3_CONNECT_ERROR 重置那条流；`transfer` 两个方向同时各送 9 MiB + 777 字节，上行只受 QUIC 流量窗口的约束，目标那一侧一慢，排着的字节就过了 1 MiB。这是参照服务端的限制，不是 rurge 的缺陷，修不了的登记（P12）：用例改用这个文件自己的 `paced_transfer`，上行至多领先回读 512 KiB、每次写至多 64 KiB，总长仍是 `(9 << 20) + 777`（这个长度是为了越过 rurge 每条流 8 MiB 的接收窗口），整个传送限时 60 秒、失败文本带上服务端的日志；另加一条不需要参照服务端的自测，对着回环上的回显传 `(3 << 20) + 777` 字节，三个平台都跑；限制登记在兼容性清单 4.2 节 `masque` 一行。根因读自参照服务端的源码与那次运行的注解；本机没有 masque-server，复现不了，修好没有要下一次推送才知道 | `0deba9f`；兼容性清单随任务 6 的文档提交 |
| 11 | 6 | 第 5 类（macOS）：macOS 上 UDP socket 的缺省发送缓冲是 9216 字节（`net.inet.udp.maxdgram`），一次 `send_to` 不能超过发送缓冲，大于它的数据报以 `EMSGSIZE` 失败；四条用例都失败在用例自己的 socket 上。要发大于 9216 字节的数据报的测试 socket，刚绑定就把发送缓冲调到 256 KiB（`socket2::SockRef::set_send_buffer_size`，调不了就让用例失败）：`rurge-engine` 的 `udp_associate`，`udp.rs` 里 `an_answer_too_long_for_the_client_is_dropped_alone` 的 `remote`，还有这次没有跑到、预料中的同一种失败（P16）——`rurge_proto::testing::udp_echo_server` 与 `FakeSnell` 的 UDP 会话。`rurge-proto` 因此多一条到 `socket2` 的依赖边（作 `testing` 特性带进来的可选依赖，另加进 `[dev-dependencies]`；锁文件仍是 517 个包）。别处的测试 socket 都核对过，单个数据报不超过 9216 字节，或在发出之前就被出站拒绝（搜法与结果在提交说明里）；rurge 自己的 socket 不在这一轮改（P13，延后事项第 7 行）。本机复现不了，修好没有要下一次推送才知道 | `f9377a8` |
| 12 | 6 | 第 6 类（Windows）：`rurge-api` 的五条用例失败在 `MockDns::start`——先在 TCP 上 `bind` 0 号端口、再在同一个号上 `bind` UDP，16 次都失败（`could not find a port free for both TCP and UDP`）。**原因没有证据**：失败的那几次的错误没有记下来；最可能的是 Windows 为 UDP 保留的成段端口，而 TCP 的临时端口是一个挨一个地分配的，没有核对（P14）。现在至多试 64 轮，两种绑定次序轮流（奇数轮先 TCP 后 UDP，偶数轮先 UDP 后 TCP），0 号端口自己 `bind` 失败也记下来，找不到时 panic 的文本带上两种绑定各自最后的错误（`could not find a port free for both TCP and UDP (last TCP error: …; last UDP error: …)`，没有发生过的一种写 `none`），一条用例钉住这段文本。轮流试管不管用，要下一次在 Windows 上跑到 `rurge-api` 才知道；再失败的话，文本里的两种错误会说出为什么 | `94cfd9e` |
| 13 | 1 | 失败摘要第二次真正用上（`7bd6589` 的运行，39022550224）：认出了跑完的二进制里的全部失败——Linux 5 条，macOS 6 条加一个没跑完的二进制，Windows 7 条加一个没跑完的二进制，都在每一步 10 条的上限之内——，并认出了两个没跑完的二进制（`rurge-proto` 的单元测试，各有三条用例没有结束）；记录第 5 行的修正管用。记录第 6 行留下的问题有了答案：macOS 与 Windows 用满 60 分钟，是因为 `rurge-proto` 的单元测试二进制里各有三条用例没有结束（第 1 类），测试已经跑起来了 | — |
| 14 | 6 | 门禁（任务 6 的代码做完的树，2026-10-18；最后一个代码提交是 `94cfd9e`，其后的 `bead755` 只改了注释）：格式与 clippy 通过；全量 77 个结果行、3806 个用例通过、0 个失败、10 个忽略，没有测试二进制异常退出——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI；需要参照服务端二进制的互操作用例在这台机器上立刻返回（那些服务端本机一个也没有），算在通过的里面 | — |
| 15 | 6 | 整分支终审查出：P11 与第 1 类那一行说生产碰不到第 1 类，不对。生产的转发循环在一个任务里轮询两半，VMess 的 UDP 载体（`vmess/udp.rs`）却在拨号时起一个任务读（读端从那时起等 100 毫秒的 `HEAD_GRACE`）、由 `send_to` 的任务在一把锁下写并 flush：旧代码下第一个数据报并进请求头、它的写过了宽限仍是 `Pending` 时，读端以自己的唤醒者去推头、顶掉写端记在底层的那一个，推完只唤醒自己；数据报其实已由读端推了出去，`send_to` 却没有人唤醒，要等到引擎 15 秒的 `SEND_TIMEOUT` 到时——`tokio::time::timeout` 到时先再轮询一次里面的 future，发送这才完成，记为成功——，这期间那条流后面的数据报都等着（引擎这一段读代码得出，没有经引擎跑过）。直连时第一次写几乎不会 `Pending`；经有流控的 `underlying-proxy` 或在卡住的路径上碰得到。`f8d0389` 的修正对它同样起作用。P11 与第 1 类那一行改了；`f8d0389` 的提交说明第三段"生产里的转发循环在同一个任务里轮询两半，碰不到这个问题"同样不对（提交说明不改，在这里订正）。新用例 `a_first_datagram_whose_write_is_blocked_past_the_grace_is_still_sent`：拨号经一个包住回环连接的连接器，从第一次 `poll_write` 起 200 毫秒内的写都是 `Pending`（长于宽限）、只留最后一个来轮询的唤醒者、到时唤醒它，断言第一次 `send_to` 在 5 秒的 `timeout` 之内完成、按时钟量也短于 5 秒（`timeout` 到时的那一次轮询会让丢了唤醒的发送恰好完成，只靠它，旧代码上这条用例也"通过"，只是用了 5 秒），回包在 5 秒内带着同样的字节回来——换回旧的 `lazy_head.rs` 时在 5.01 秒处失败，现在约 0.2 秒完成，单独循环跑 20 次全过 | `f0e41fd` |
| 16 | 6 | 整分支终审查出的另一处：读端推完头、它的 flush 还欠着时，写端的 `poll_flush` / `poll_shutdown` 完成并不了结它（只有读端下一次 `poll_read` 才了结），而 `get_mut` 的文档说"等 `poll_flush` 返回 `Ready` 之后"就可以自己轮询写方向：照文档在两个任务里这样做的调用方，会把底层记着的共用唤醒者换成写端自己的，停在欠着的 flush 上的读端再没人唤醒；写端 `poll_shutdown` 之后，读端下一次读还会去 flush 一条写端已经关了的流。今天碰不到（`get_mut` 唯一的调用者是 Snell 的 `poll_end`，`SnellTunnel` 在生产里只在一个任务里被轮询）。现在写端的 `poll_flush` / `poll_shutdown` 在底层完成时（不论成败）也了结欠着的 flush、唤醒读端（与读端那条路共用 `settle_flush`；完成的 `poll_write` 不了结什么）；`get_mut` 的文档写成"`poll_flush` 返回 `Ready(Ok(()))` 之后"（推头出错时 `poll_flush` 返回 `Ready(Err)` 而头还在），这个条件因此够了；`Half` 的文档写明一半指方法属于哪一边、不是哪个任务（`SnellTunnel::drive` 在读里调写方向）。`a_reader_waiting_on_its_flush_is_woken_when_the_writers_flush_finishes_it`、`a_reader_waiting_on_its_flush_is_woken_when_the_writers_shutdown_finishes_it` 与 `a_reader_waiting_on_its_flush_is_woken_when_the_writers_flush_fails` 三条手动轮询的用例先红后绿（改动之前读端被唤醒 0 次），各单独循环跑 20 次全过 | `199e584` |
| 17 | 1 | 整分支终审的建议：没跑完的二进制里，每条用例一结束日志里就有一行它的结果，失败用例的输出却要到二进制结束时才打印，失败摘要只报"没跑完"与还在跑的用例，其中已经失败的用例没有报出（所以第二次运行里 macOS 与 Windows 上 `rurge-proto` 的单元测试有没有已经失败的用例，不知道）。现在那一条注解在还在跑的用例之后接 `; failed before the log ends: <用例>`（照日志里的次序）；对着两份合成的日志核对过：一份最后一个二进制有一条 `FAILED`、一条 `ok`、一条还在跑而没有结果行，一份是跑完了的普通失败——后者的输出与改动之前逐字相同。仍没有自动化测试（延后第 5 行） | `e499061` |
| 18 | 6 | 整分支终审查出的文档几处：第 6 类的原因不知道，改的是找端口的做法与诊断，两份 README、需求文档第 7 节的状态行、验收文档与 CLAUDE.md 的几处概括却说六类"已修"——改为五类已修、第 6 类换了做法并补了诊断（原因仍不知道）；兼容性清单 1.4 节四个平台变量一行说两条用例真读系统，是三条，漏了读不到版本就失败的 `this_system_says_its_version`；CLAUDE.md 里阶段 3 / M2 那一句全量的"自那以后只有文档变过"在这一轮的代码之后不对了，补上这一轮的全量；第 1 类在生产里的作用（记录第 15 行）写进 CLAUDE.md、README 与验收文档；"`rurge-proto` 单元测试里别的用例的结果也不知道"的原因补上失败摘要那一半（记录第 17 行），随之把五处"认出了全部失败"改为"认出了跑完的二进制里的全部失败"（第二次运行里没跑完的二进制有没有已经失败的用例，不知道） | 这一行所在的提交 |
| 19 | 6 | 门禁（整分支终审之后的修正做完的树，2026-10-18；在 `e499061` 上，最后一个改 Rust 代码的提交是 `f0e41fd`）：格式与 clippy 通过；全量 77 个结果行、3810 个用例通过、0 个失败、10 个忽略（比记录第 14 行多的 4 个是这一轮的新用例），没有测试二进制异常退出——一台 Windows 11 x86-64 的开发机上的一次运行，不是 CI；需要参照服务端二进制的互操作用例在这台机器上立刻返回，算在通过的里面 | — |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | `h3://` 的 DoH3 URL 带用户名口令仍是 `W0026`（拒绝），而 `https://` 的现在以 Basic 认证发出：两种写法不对称 | 有用户报告再说（做法同 DoH：加一个头） |
| 2 | 缓存目录 `meta.json` 里资源的完整 URL（含订阅 token，只写不读） | 不处理：与缓存的订阅正文、配置文件同在使用者的数据目录里 |
| 3 | `[General]` 里不认识的键（`W0001`）的取值在 `General.unknown` 的 `Debug` 里原样可见——把凭据写在一个拼错的键名下时 | 有用户报告再说；`redact_profile` 对拼错的键名同样无从判断 |
| 4 | `General` 的 `proxy_test_url` / `internet_test_url` 在 `Debug` 里原样可见（它们的诊断不引用取值；策略自己的 `test-url` 已不显示） | 顺手时改成与 `CommonOpts.test_url` 一致 |
| 5 | 失败摘要的脚本没有自动化测试，只对着几份真实与合成的日志手工核对过；注解单条的长度上限（取了 3000 字符）没有文档依据 | 第一次真正用上时核对；需要时给脚本配一组样本日志。第一次用上（`b41930a`）时认不出带颜色码的日志，修正见记录第 5 行；第二次用上（`7bd6589`，运行号 39022550224）时认出了跑完的二进制里的全部失败（Linux 5 条，macOS 6 条加一个没跑完的二进制，Windows 7 条加一个没跑完的二进制）和两个没跑完的二进制，见记录第 13 行；没跑完的二进制那一条注解自 `e499061` 起也列出其中已经失败的用例（记录第 17 行）；仍没有自动化测试 |
| 6 | `url_without_credentials` 对查询串里带 `@` 而没有用户名口令的 URL（`https://host/path?x=a@b`）过度脱敏成 `https://***@b` | 接受：偏安全一侧；只出现在诊断与脱敏输出里 |
| 7 | macOS 上 rurge 自己的 SOCKS5 UDP 中继 socket 用缺省的发送缓冲（9216 字节）：发给客户端的数据报大于它时以 `EMSGSIZE` 失败，只丢那一个。第 5 类的四条用例失败在测试自己的 socket 上，没有哪条失败的用例由它而来 | 这一轮不改（P13）：改法是知道的——同 JDK-8239355，把发送缓冲调到至少 65 507 / 65 527 字节——，只有 macOS 的 CI 验证得了 |
| 8 | masque-server v0.13.0 对每条 TCP 隧道把客户端发来而还没写给目标的字节至多排 1 MiB，不对 QUIC 施加背压，超出就以 H3_CONNECT_ERROR 重置那条流：经它的上传比目标收得快时连接会断（第 4 类） | 不处理（P12）：参照服务端的限制，不是 rurge 的缺陷；已登记兼容性清单 4.2 节 `masque` 一行，互操作用例按此节制上行 |
| 9 | `MockDns` 在 Windows CI 上为什么找不到端口（第 6 类：16 次都失败，失败的那几次的错误没有记下来） | 下一次失败时看 panic 文本里两种绑定各自最后的错误再定（P14） |
| 10 | macOS 与 Windows 上没有跑到的四个包的测试（`rurge-proto-quic`、`rurge-proto-ssh`、`rurge-proto-wireguard`、`rurge-rules`），以及 `rurge-proto` 单元测试里别的用例在这两个平台上的结果（失败用例的输出要到二进制结束时才打印，而第 1 类的三条没有结束；那时的失败摘要也不列没跑完的二进制里已经失败的用例，`e499061` 起列出，记录第 17 行） | 下一次推送：那四条用例等写端的地方限时 30 秒（`4fc1bad`），不论第 1 类的修正管不管用，`rurge-proto` 的单元测试二进制都不再因它们挂住，排在它后面的四个包随之跑到 |
| 11 | 各份计划「延后事项」里去向是"第一次 CI 运行"的行 | 等三个平台的全部测试二进制都跑完的那一次运行之后逐行写结论（P17）。第二次运行（`7bd6589`）没有跑到那一步：macOS 与 Windows 停在 `rurge-proto` 的单元测试里，阶段 3 / M2 的提交也不在里面 |
