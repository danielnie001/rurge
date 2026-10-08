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

待展开。带着任务 1 的提交推送之后，从注解里读出三个平台各自失败的用例，逐条：定因 → 本机能复现的先补失败用例 → 修 → 记进下面的「执行期修正记录」。设计 3.2 列出的"首次推送后看 CI"的事项一并核对。

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

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | `h3://` 的 DoH3 URL 带用户名口令仍是 `W0026`（拒绝），而 `https://` 的现在以 Basic 认证发出：两种写法不对称 | 有用户报告再说（做法同 DoH：加一个头） |
| 2 | 缓存目录 `meta.json` 里资源的完整 URL（含订阅 token，只写不读） | 不处理：与缓存的订阅正文、配置文件同在使用者的数据目录里 |
| 3 | `[General]` 里不认识的键（`W0001`）的取值在 `General.unknown` 的 `Debug` 里原样可见——把凭据写在一个拼错的键名下时 | 有用户报告再说；`redact_profile` 对拼错的键名同样无从判断 |
| 4 | `General` 的 `proxy_test_url` / `internet_test_url` 在 `Debug` 里原样可见（它们的诊断不引用取值；策略自己的 `test-url` 已不显示） | 顺手时改成与 `CommonOpts.test_url` 一致 |
| 5 | 失败摘要的脚本没有自动化测试，只对着几份真实与合成的日志手工核对过；注解单条的长度上限（取了 3000 字符）没有文档依据 | 第一次真正用上时核对；需要时给脚本配一组样本日志 |
| 6 | `url_without_credentials` 对查询串里带 `@` 而没有用户名口令的 URL（`https://host/path?x=a@b`）过度脱敏成 `https://***@b` | 接受：偏安全一侧；只出现在诊断与脱敏输出里 |
