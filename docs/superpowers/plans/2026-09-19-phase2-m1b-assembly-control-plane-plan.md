# 阶段 2 / M1b「装配与控制面」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 M1a 做好的 `http` `https` `socks5` `socks5-tls` 出站接进引擎：出站工厂、能装下真实出站的策略注册表、`underlying-proxy` 链、`select` 组的运行期选择表与 API、明文 HTTP 的绝对 URI 转发、`use-local-host-item-for-proxy`、干构建（`E0022`）、能力表翻转，以及对 sing-box 的互操作测试与 CI。

**Architecture:** `rurge-policy` 定义 `OutboundFactory`，`rurge-engine::outbounds::EngineFactory` 实现它（解析器 + `SocketHook` + Keystore + 根证书）。注册表按 spec 经工厂构建出站；带 `underlying-proxy` 的策略拿到的连接器是 `ChainConnector`，它经跨代稳定的 `RegistryCell` 按**名字**解析下一跳，所以组的选择一变、链的入口跟着变。`select` 组的选择放在引擎持有的 `SelectionTable` 里（不再烤进每一代注册表），经 `Engine::select_group` 修改并写进 `state.json`。平台相关的 socket 操作仍在 `rurge-platform`，由 bin 的 `PlatformSockets` 适配成 `SocketHook` 注入。

**Tech Stack:** Rust 1.89 / edition 2024、tokio、hyper 1（入站与转发）、axum（API）、arc-swap、rustls 0.23（ring）、idna 1、sha2 0.10、rcgen 0.14（仅测试）、sing-box 1.14.1（仅互操作测试，回环子进程）。

**Spec:** `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md`（M1 设计；第 6–10 节是本计划的依据）；上位文档 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`（阶段 2 总设计）；前一份计划 `docs/superpowers/plans/2026-09-19-phase2-m1a-outbound-library-plan.md`（末尾「延后事项」里标给 M1b 的条目由本计划接手，见下面的"承接事项"）。

## Global Constraints

- 工具链：Rust stable，`rust-version = "1.89"`，edition 2024（let-chains 可用；clippy 要求能用就用）。
- `unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` 且只有 `sysproxy::windows::notify_wininet` 一个函数 `#[allow]`。**本计划不新增任何 unsafe。**
- 新增依赖只有这些，版本照抄：工作区 `idna = "1"`（`Cargo.lock` 里已有 1.1.0，经 `url` 引入），用于 `rurge-proto`。其余都是工作区已有的 crate 的新用法：`rurge-policy` 用 `arc-swap` 与 `rurge-net`（由 dev 依赖转为正式依赖），dev 依赖加 `tokio`；`rurge-engine` 用 `rustls` 与 `sha2`（正式依赖），dev 依赖加 `rurge-proto`（feature `testing`）；`rurge`（bin）用 `socket2`（正式依赖，`SocketHook` 的签名里有它的类型），dev 依赖加 `rurge-proto`（`testing`）；新的工作区成员 `tests/interop`（包名 `rurge-interop`，`publish = false`）用 `serde_json`，dev 依赖用 `tempfile` `tokio` 与内部 crate。不引入其它 crate，不跑 `cargo update`。`Cargo.lock` 的变化要提交。
- 平台相关代码只允许出现在 `rurge-platform`（AR-02）；`rurge-platform` 不依赖任何内部 crate；`rurge-engine` / `rurge-api` / `rurge-policy` 不依赖 `rurge-platform`。
- 依赖方向不变：`rurge (bin) → rurge-api → rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`；`rurge-policy` 不依赖 `rurge-engine`。
- 诊断码永不重编号。本计划启用 M1a 已定义的 `E0022`（`codes::E_POLICY_BUILD`），不新增诊断码。
- 凭据（用户名、密码、Keystore 材料、自定义 header 的值）与来自对端 / 客户端的原始文本不得出现在任何错误文本、诊断消息与日志里（对端文本要经 `rurge_proto::outbound::untrusted_text` 那样的净化与限长）。
- **出站不信任调用方，入站不信任客户端**（M1a 终审的教训）：每个把外部字符串写进文本协议的点，都要能指出是谁校验过它。
- 测试只用回环地址 + 端口 0（或先绑 0 号端口取得空闲端口）+ 有界等待，绝不访问公网；不得修改本机的系统代理、注册表、网络设置，不注册服务。**互操作夹具渲染的 sing-box 配置里绝不能出现 `set_system_proxy`、`tun`、`auto_route` 这些键，入站只监听 `127.0.0.1`。** 夹具不下载任何东西；本机是否安装 sing-box 由项目所有者决定，没装就跳过。
- 语言：代码、注释、日志、CLI 输出用英文；文档用中文；提交标题用中文（仓库风格见 `git log --oneline -8`）。
- 提交：在分支 `phase2-m1b-assembly-control-plane`（从 `main` 切出）上工作；不 push、不 merge、不 amend、不改写历史。每条提交消息以会话提示给出的署名行结尾，并且必须含这一行：`Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。
- 每个任务结束跑门禁：`RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --no-fail-fast`。测试二进制在没有失败用例的情况下异常退出是已知的偶发基建问题：重跑一次并保留两次输出。已知两个偶发的时序用例（`rurge-dns` 的 `fanout::tests::empty_answer_rules`、bin 的 `run::watch_reloads_rules_on_change`）：与改动无关地失败一次就重跑；**同一个用例在本计划执行期间第二次出现，就在当前任务里顺手把它的时序断言放宽并记录。**
- 本机 bash 里超过约 8 KB 的 heredoc 会失败：大文件用编辑器工具写，不用 shell 重定向。

## 计划期决定（写计划时核对源码 / 文档得出；与 M1 设计不一致处在 Task 12 里同步订正设计文档）

| 编号 | 决定 | 依据 |
| ---- | ---- | ---- |
| P1 | **M1 设计 O4 已结**：转发模式不需要手写请求行。入站把请求 URI 重建成 `http://<host[:port]><path?query>`（去掉 userinfo）后原样交给 hyper | hyper 1.11.1 的 `client::conn::http1::SendRequest::send_request` 在 `proto/h1/role.rs` 里把 `msg.head.subject.1`（请求 URI）原样写出；其文档写明"发给 HTTP 代理时用 absolute-form" |
| P2 | **M1 设计 O3 已结**：互操作测试固定 sing-box **1.14.1**（2026-09-15 发布的稳定版）。CI 用的三个包与 SHA256：`sing-box-1.14.1-linux-amd64.tar.gz` = `12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f`；`sing-box-1.14.1-windows-amd64.zip` = `5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89`；`sing-box-1.14.1-darwin-arm64.tar.gz` = `b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4` | GitHub 发布 API（`releases/tags/v1.14.1`）各资产的 `digest` 字段；`macos-latest` 是 arm64 |
| P3 | sing-box 的 `socks` 与 `mixed` 入站没有 `tls` 字段，所以 **`socks5-tls` 没有 sing-box 互操作用例**，只由 M1a 的回环假上游覆盖；`https` 用 sing-box 的 `http` 入站 + `tls`，p12 客户端证书用其 `client_authentication = "require-and-verify"`（1.13.0 起） | sing-box 文档 `configuration/inbound/{http,socks,mixed}` 与 `configuration/shared/tls` |
| P4 | 跨代存活的对象放进 `rurge_engine::EngineShared { cell, selections }`：由调用方在第一次 `Runtime::build` 之前创建，经 `RuntimeOptions.shared` 传入（取代 `selections: GroupSelections`），`Runtime.policies` 改为 `Arc<PolicyRegistry>`；引擎在 `Engine::new` 与 `swap_runtime` 里把注册表存进 cell，在 `Drop` 里清空；重载时调用方用 `engine.shared()` 构建下一代，`swap_runtime` 断言两者是同一个 cell | 设计 6.2 / 6.3 说这两样东西"由引擎持有、跨代稳定"，但注册表在 `Runtime::build` 里构建、早于 `Engine::new`，所以必须先于引擎存在 |
| P5 | `ProxyPolicy` 与 `PolicyGroup` 各加一个字段 `definition: String`（`名字 =` 右边的原文） | `/v1/policies/detail` 与 `lineHash` 需要定义行原文；请求时再读文件拿到的可能是加载之后被改过的行。设计 4.1 的"`ProxyPolicy` 不变"指的是 spec 工作不重构它；语料库快照基于 `ConfigSummary`，不受影响 |
| P6 | `/v1/policies/detail` 的值是**脱敏后的定义**（不含 `名字 =` 前缀）；`lineHash` = `SHA-256("<名字> = <定义原文>")` 的前 16 个十六进制字符，内置策略则对名字本身取哈希 | 设计 6.6 标了"暂定"，这里把没写明的细节定下来，登记进 `docs/api/phase2.md` |
| P7 | 干构建的入口是 `rurge_engine::outbounds::load_checked(path, opts)`（= `rurge_config::config::load` + 追加 `dry_build` 的诊断），四个调用点（`check`、`run`、`reload`、`POST /v1/profiles/check`）统一改用它 | 设计 6.4 列了这四个点；一个入口免得漏掉哪个 |
| P8 | 网络来的主机名经 `HostName::from_wire` 进入系统：含控制字符或空白的名字被拒绝，**非 ASCII（IDN）原样通过**（规则匹配的语义不变）；`http` / `socks5` 出站在写线之前把非 ASCII 域名转成 A-label（`idna::domain_to_ascii`） | M1a 延后表里的两条："入站侧拒绝含控制字符的主机名""`http` 出站的 IDN 转 A-label"。在入站就转 A-label 会改变规则匹配的对象，所以放在出站 |
| P9 | `[Host]` 的 IP 替换（FR-DNS-07）命中时，这条会话**不走绝对 URI 转发**而走 CONNECT | 转发模式下目标由请求 URI 携带，引擎不把目标交给出站；要让代理连到本地指定的 IP 又保留 `Host:`，走隧道是语义正确且最简单的做法 |
| P10 | `hybrid`（iOS 专属）不再校验取值：桌面端一律只报 `W0004`；空的 `interface=` 仍是 `E0018`；`http` / `socks5` 策略上没有 spec 读取的参数照旧报 `W0001` | M1a 终审的三处兼容性观察。手册（`policies/parameters`）对非法值 / 空值的行为没有任何说明；项目的兼容性原则是"平台不适用的配置项解析并忽略"，`hybrid` 正属于这一类；另两项保持现状并在兼容性清单里标"未与真实 Surge 核对" |
| P11 | `skip-cert-verify=true` 的提示用运行期 WARN（真正构建时每个策略一条；干构建不打），不新增诊断码 | 诊断码的新增需要设计；这只是给运维的一个信号 |
| P12 | `rurge-platform::socket` 给网卡表加 5 秒的进程内缓存（macOS / Windows 每次连接都要查表） | M1a 终审的观察：`GetAdaptersAddresses` 是毫秒级调用 |

## 承接事项（来自 M1a 计划的「延后事项」）

| 事项 | 本计划的任务 |
| ---- | ------------ |
| SOCKS5 入站 / `HostName::parse` 接受内部控制字符（根因） | 1、2 |
| `http` 出站拒绝非 ASCII 主机名（IDN 未转 A-label） | 2 |
| `check_status` 的原因短语末尾可能留空格 | 2 |
| 转发模式在写请求行与 `Host` 头之前必须先 `valid_target`；`request_headers()` 每条连接只调一次 | 6 |
| 生产环境的 DIRECT 仍用 `NoopSocketHook` | 5（`StackOptions.socket_hook`）、10（`PlatformSockets`） |
| `bind_interface` 每次连接都查网卡表 | 10 |
| `skip-cert-verify=true` 没有任何信号 | 4 |
| `[Keystore]` 里空的 `base64` 不报 `E0021` | 4（干构建报 `E0022`，有测试） |
| 三处配置兼容性观察 | 1（`hybrid`）、12（清单登记） |
| 兼容性清单的"未核对"标注、`tfo`、`headers=` 的控制字符规则 | 12 |
| DNS 应答为空时的误导文本、超时文本里 IPv6 没有方括号 | 2 |

## File Structure

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/policy.rs` | `ProxyPolicy.definition`、`PolicyGroup.definition` | 1 |
| `crates/rurge-config/src/redact.rs` | `redact_definition` | 1 |
| `crates/rurge-config/src/types.rs` | `HostName::from_wire` | 1 |
| `crates/rurge-config/src/spec/common.rs` | `hybrid` 不再校验取值 | 1 |
| `crates/rurge-inbound/src/socks5.rs` | 拒绝不合法的域名 | 2 |
| `crates/rurge-inbound/src/http.rs` | 记下对 `http::Uri` 的依赖（测试）；`absolute_form` 与转发接线 | 2、6 |
| `crates/rurge-proto/src/hostname.rs` | `to_ascii`（IDN → A-label） | 2 |
| `crates/rurge-proto/src/http.rs`、`socks5.rs` | 用 `to_ascii`；`check_status` 的 `trim_end` | 2 |
| `crates/rurge-net/src/connector.rs` | 两条报错文本 | 2 |
| `crates/rurge-policy/src/selections.rs` | `SelectionTable` | 3 |
| `crates/rurge-policy/src/factory.rs` | `OutboundFactory` | 3 |
| `crates/rurge-policy/src/cell.rs` | `RegistryCell`、`ChainConnector` | 3 |
| `crates/rurge-engine/src/outbounds.rs` | `EngineFactory`、`dry_build`、`load_checked` | 4 |
| `crates/rurge-policy/src/registry.rs` | `Entry` 重构、`Resolution.terminal / note`、经工厂构建 | 5 |
| `crates/rurge-engine/src/shared.rs` | `EngineShared` | 5 |
| `crates/rurge-engine/src/{runtime,engine,reload,stack}.rs` | 装配、cell 的存取、`StackOptions.socket_hook` | 5 |
| `crates/rurge-inbound/src/session.rs` | `Dialed.forward` | 6 |
| `crates/rurge-engine/tests/outbounds.rs` | 经真实出站的端到端用例 | 6、7、8 |
| `crates/rurge-dns/src/resolver.rs` | `Resolver::host_lookup` | 7 |
| `crates/rurge-engine/src/views.rs` | `groups_view` `policy_detail` `group_selection` `select_group` | 8 |
| `crates/rurge-api/src/routes/policy_groups.rs` | 四个端点 | 9 |
| `crates/rurge-platform/src/socket.rs` | 网卡表缓存 | 10 |
| `crates/rurge/src/cli/runtime.rs` | `PlatformSockets` | 10 |
| `crates/rurge/src/capabilities.rs`、`cli/{check,run}.rs` | 能力表翻转、`load_checked` | 10 |
| `tests/interop/` | sing-box 夹具与用例（新的工作区成员 `rurge-interop`） | 11 |
| `.github/workflows/ci.yml` | 安装并校验 sing-box，`RURGE_INTEROP_REQUIRED=1` | 11 |
| `docs/`、`README.md`、`CLAUDE.md` | 兼容性清单、`docs/api/phase2.md`、设计订正、状态 | 12 |

---

### Task 1: `rurge-config` 的四处地基改动

**Files:**
- Modify: `crates/rurge-config/src/policy.rs`（两个结构体与 `parse_policy` / `parse_group`）
- Modify: `crates/rurge-config/src/redact.rs`
- Modify: `crates/rurge-config/src/types.rs`
- Modify: `crates/rurge-config/src/spec/common.rs`

**Interfaces:**
- Consumes: 无。
- Produces:
  - `ProxyPolicy.definition: String`、`PolicyGroup.definition: String`——`名字 =` 右边的原文，两端去空白。
  - `rurge_config::redact::redact_definition(definition: &str) -> String`。
  - `HostName::from_wire(s: &str) -> Option<HostName>`。
  - `hybrid=<任意值>` 不再产生 `E0018`；出现即记入 `Notes.ios_only`（调用方据此报 `W0004`）。

- [ ] **Step 1: 确认分支**

```bash
git rev-parse --abbrev-ref HEAD    # 期望：phase2-m1b-assembly-control-plane
git status --short                 # 期望：空
```

分支不对就停下来报告，不要自己切分支。

- [ ] **Step 2: 写失败的测试**

`crates/rurge-config/src/policy.rs` 的 `mod tests` 末尾加：

```rust
    #[test]
    fn the_definition_text_is_kept_as_written() {
        let p = parse_policy(
            "Up",
            "  http, proxy.test, 8080, alice, s3cret, skip-cert-verify=true ",
            &span(),
        )
        .unwrap();
        assert_eq!(
            p.definition,
            "http, proxy.test, 8080, alice, s3cret, skip-cert-verify=true"
        );
        let g = parse_group("Pick", " select, Up, DIRECT, hidden=true", &span()).unwrap();
        assert_eq!(g.definition, "select, Up, DIRECT, hidden=true");
    }
```

（`span()` 是该测试模块里已有的辅助函数；没有的话照 `Span::new(Arc::from(Path::new("t.conf")), 1)` 写一个。）

`crates/rurge-config/src/redact.rs` 的 `mod tests` 末尾加：

```rust
    #[test]
    fn a_definition_is_redacted_like_its_profile_line() {
        for def in [
            "http, proxy.test, 8080, alice, s3cret, skip-cert-verify=true",
            "socks5, proxy.test, 1080, username=bob, password=hunter2",
            "ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x",
            "direct, interface=eth0",
        ] {
            let alone = redact_definition(def);
            // one rule, two entry points: the profile endpoint must agree
            assert_eq!(
                format!("X = {alone}"),
                redact_profile(&format!("X = {def}")),
                "{def}"
            );
            for secret in ["s3cret", "hunter2", "alice", "bob"] {
                assert!(!alone.contains(secret), "{def} -> {alone}");
            }
        }
        assert!(redact_definition("http, h, 1, u, p").contains("***"));
        assert_eq!(redact_definition("direct, interface=eth0"), "direct, interface=eth0");
    }
```

`crates/rurge-config/src/types.rs` 的 `mod tests` 末尾加：

```rust
    #[test]
    fn names_from_the_wire_may_not_hold_control_characters_or_whitespace() {
        for bad in [
            "",
            ".",
            "a.test\r\nX-Evil: 1",
            "a\0b.test",
            "a b.test",
            " a.test",
            "a.test\t",
            "a\u{7f}.test",
            "a\u{85}.test",
            "a\u{2028}.test",
        ] {
            assert_eq!(HostName::from_wire(bad), None, "{bad:?}");
        }
        assert_eq!(
            HostName::from_wire("Example.TEST."),
            Some(HostName::Domain("example.test".into()))
        );
        assert_eq!(
            HostName::from_wire("[::1]"),
            Some(HostName::Ip("::1".parse().unwrap()))
        );
        assert_eq!(
            HostName::from_wire("192.0.2.7"),
            Some(HostName::Ip("192.0.2.7".parse().unwrap()))
        );
        // IDN names pass through: rules keep matching what the client sent
        assert_eq!(
            HostName::from_wire("bücher.example"),
            Some(HostName::Domain("bücher.example".into()))
        );
    }
```

`crates/rurge-config/src/spec/common.rs`：在 `invalid_values_are_errors` 的表里**删掉** `("http, h, 1, hybrid=sometimes", "hybrid"),` 这一行，并在 `mod tests` 末尾加：

```rust
    #[test]
    fn hybrid_is_ios_only_so_its_value_is_never_checked() {
        for def in ["http, h, 1, hybrid=sometimes", "http, h, 1, hybrid=on", "direct, hybrid="] {
            let applies = if def.starts_with("direct") { Applies::Direct } else { Applies::Proxy };
            let (_, notes, diags) = read(def, applies);
            assert!(diags.is_empty(), "{def}: {diags:?}");
            assert_eq!(notes.ios_only, ["hybrid"], "{def}");
        }
    }
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p rurge-config --lib the_definition_text_is_kept_as_written a_definition_is_redacted names_from_the_wire hybrid_is_ios_only 2>&1 | tail -20`
Expected: 编译失败——`no field definition`、`cannot find function redact_definition`、`no function or associated item named from_wire`。（`cargo test` 只接受一个过滤词；分四次跑，或直接 `cargo test -p rurge-config --lib`。）

- [ ] **Step 4: 实现**

`crates/rurge-config/src/policy.rs`：两个结构体各加一个字段（放在 `span` 之前），并在 `parse_policy` / `parse_group` 构造返回值的地方填上。

```rust
pub struct ProxyPolicy {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub positional: Vec<String>,
    pub params: ParamMap,
    /// The text right of `name =` as written (API policy detail, `lineHash`).
    pub definition: String,
    pub span: Span,
}
```

```rust
pub struct PolicyGroup {
    pub name: String,
    pub kind: GroupKind,
    pub members: Vec<String>,
    pub params: ParamMap,
    pub conditions: Vec<(SubnetExpr, String)>,
    pub legacy_keyword: bool,
    /// The text right of `name =` as written (API `lineHash`).
    pub definition: String,
    pub span: Span,
}
```

两处构造都加 `definition: definition.trim().to_string(),`。工作区里只有这两个函数构造这两个类型（`grep -rn "ProxyPolicy {\|PolicyGroup {" crates` 核对；若有别处，同样补上）。

`crates/rurge-config/src/redact.rs`：把 `redact_body` 末尾处理策略行的三行抽成公共函数，`redact_body` 改为调用它。

```rust
/// A policy definition (the text right of `name =`) with its secrets blanked:
/// positional credentials of `http` / `https` / `socks5` / `socks5-tls`, and
/// every secret `name=value` parameter.
pub fn redact_definition(definition: &str) -> String {
    let mut redacted = redact_positional_credentials(definition);
    for param in SECRET_PARAMS {
        redacted = redact_param(&redacted, param);
    }
    redacted
}
```

`redact_body` 的结尾改成：

```rust
    // policy lines: `name = type, host, port, password=..., psk=...`, or
    // `name = http/https/socks5/socks5-tls, host, port, username, password`
    // (those types carry credentials positionally, not as `password=...`).
    format!("{key}={}", redact_definition(value))
```

`crates/rurge-config/src/types.rs`，`impl HostName` 里加：

```rust
    /// A name that came off the network (a SOCKS5 request, a CONNECT
    /// authority). `None` when it is empty or holds a control character or
    /// whitespace: nothing a resolver, a rule or a proxy request line can
    /// carry safely. Non-ASCII (IDN) names pass through unchanged, so rules
    /// keep matching what the client sent; outbounds convert them to A-labels.
    pub fn from_wire(s: &str) -> Option<HostName> {
        if s.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }
        match HostName::parse(s) {
            HostName::Domain(d) if d.is_empty() => None,
            host => Some(host),
        }
    }
```

`crates/rurge-config/src/spec/common.rs`：把

```rust
    let hybrid_present = r.has("hybrid");
    let _ = r.choice("hybrid", &TRISTATES);
```

换成

```rust
    // iOS only: never applicable here, so the value is not ours to judge
    let hybrid_present = r.has("hybrid");
    r.touch("hybrid");
```

若 `TRISTATES` 因此只剩别处在用、或某个 `use` 变成未使用，按 clippy 的提示清理（只清理本改动造成的）。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-config 2>&1 | tail -15`
Expected: 全部通过，包括语料库快照测试（`definition` 不进 `ConfigSummary`，快照不变；若 `cargo insta` 报有待审阅的快照，说明改动误伤了摘要，停下来查）。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): 定义行原文（definition）、redact_definition、HostName::from_wire；hybrid 不再校验取值"
```

---

### Task 2: 主机名卫生与承接的三处文本小项

**Files:**
- Modify: `crates/rurge-inbound/src/socks5.rs`
- Modify: `crates/rurge-inbound/src/http.rs`（只加一个测试）
- Create: `crates/rurge-proto/src/hostname.rs`
- Modify: `crates/rurge-proto/src/lib.rs`、`crates/rurge-proto/src/http.rs`、`crates/rurge-proto/src/socks5.rs`
- Modify: `Cargo.toml`（工作区依赖）、`crates/rurge-proto/Cargo.toml`
- Modify: `crates/rurge-net/src/connector.rs`（两条报错文本）

**Interfaces:**
- Consumes: Task 1 的 `HostName::from_wire`。
- Produces:
  - SOCKS5 入站：ATYP = 3 的名字不合法 → 应答 `0x01`，不拨号。
  - `rurge_proto::hostname::to_ascii(name: &str) -> Option<String>`（`pub(crate)`）。
  - `rurge_proto::http::valid_target` 的规则变为"IP 字面量，或转成 A-label 之后非空且每个字节都在 `0x21..=0x7e` 的域名"；签名不变。
  - `check_status` 的原因短语在净化之后再 `trim_end`。
  - `DirectConnector`：DNS 应答为空 → `no address found for <name>`；应答被 `ip-version` 全部滤掉 → `no usable address for <name>: every answer was filtered out by ip-version`；超时文本里的 IPv6 字面量带方括号。错误种类不变（`NotFound` / `TimedOut`）。

- [ ] **Step 1: 写失败的测试**

`crates/rurge-inbound/src/socks5.rs` 的 `mod tests`，放在 `an_empty_domain_name_is_answered_without_dialing` 后面：

```rust
    /// A name with a line break or a space is never a host name. Passing it on
    /// would make every text-protocol outbound responsible for it.
    #[tokio::test]
    async fn a_domain_name_with_control_characters_is_answered_without_dialing() {
        let (running, dialer) = listener(Duration::from_secs(30)).await;
        for name in ["echo.test\r\nX-Evil: 1", "echo .test", "echo.test\0"] {
            let mut s = negotiate(running.local_addr).await;
            s.write_all(&domain_request(name, 7)).await.unwrap();
            assert_eq!(read_reply(&mut s).await[1], REP_GENERAL_FAILURE, "{name:?}");
        }
        assert!(dialer.sessions().is_empty(), "nothing was dialled");
    }
```

`crates/rurge-inbound/src/http.rs` 的 `mod tests` 末尾：

```rust
    /// `connect` and `forward` hand `authority.host()` to `HostName::parse`
    /// without further checks. That is sound only because `http::Uri` cannot
    /// hold these bytes; this test pins the guarantee we rely on.
    #[test]
    fn an_http_authority_cannot_carry_control_characters_or_spaces() {
        for bad in ["a b.test:80", "a.test\r\nX-Evil: 1:80", "a\0.test:80", "a\t.test:80"] {
            assert!(bad.parse::<http::uri::Authority>().is_err(), "{bad:?}");
            assert!(format!("http://{bad}/").parse::<Uri>().is_err(), "{bad:?}");
        }
    }
```

创建 `crates/rurge-proto/src/hostname.rs`，先只放测试：

```rust
//! Host names as they are written onto the wire.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_names_pass_and_idn_names_become_a_labels() {
        assert_eq!(to_ascii("example.test").as_deref(), Some("example.test"));
        assert_eq!(
            to_ascii("bücher.example").as_deref(),
            Some("xn--bcher-kva.example")
        );
        assert_eq!(to_ascii("例子.test").as_deref(), Some("xn--fsqu00a.test"));
    }

    #[test]
    fn nothing_unprintable_survives() {
        for bad in ["", "a.test\r\nX: 1", "a b.test", "a\0.test", "a\u{7f}.test"] {
            assert_eq!(to_ascii(bad), None, "{bad:?}");
        }
    }
}
```

`crates/rurge-proto/src/http.rs` 的 `mod tests`：

```rust
    #[tokio::test]
    async fn an_idn_target_is_sent_as_its_a_label() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            connect_to: Some(echo),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(&format!("http, 127.0.0.1, {}", proxy.addr().port()), no_roots());
        let t = Target::new(HostName::Domain("bücher.example".into()), 443);
        let mut stream = out.connect_tcp(&t, &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"idn").await;
        let head = &proxy.heads()[0];
        assert_eq!(head.request_line, "CONNECT xn--bcher-kva.example:443 HTTP/1.1");
        assert_eq!(head.header("Host"), Some("xn--bcher-kva.example:443"));
        assert!(valid_target(&t));
    }
```

（`roundtrip`、`no_roots`、`outbound` 是该测试模块已有的辅助函数。）

并在已有的用例 `check_status_sanitizes_and_bounds_the_reason_phrase` 末尾加一条断言：

```rust
        // a control character at the very end must not leave a trailing space
        let Err(OutboundError::Proxy(m)) = check_status(b"HTTP/1.1 403 Forbidden \x1b\r\n\r\n") else {
            panic!("expected a proxy error");
        };
        assert_eq!(m, "http proxy answered 403 Forbidden");
```

`crates/rurge-proto/src/socks5.rs` 的 `mod tests`：

```rust
    #[tokio::test]
    async fn an_idn_target_is_sent_as_its_a_label() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(&format!("socks5, 127.0.0.1, {}", server.addr().port()), no_roots());
        let t = Target::new(HostName::Domain("bücher.example".into()), 443);
        let mut stream = out.connect_tcp(&t, &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"idn").await;
        assert_eq!(server.requests()[0].host, "xn--bcher-kva.example");
    }
```

`crates/rurge-net/src/connector.rs` 的 `mod tests` 末尾（`Fixed` 是该模块已有的解析器桩）：

```rust
    #[tokio::test]
    async fn an_empty_answer_and_a_filtered_answer_read_differently() {
        let empty = DirectConnector::new(Arc::new(Fixed(Vec::new())));
        let e = empty
            .connect(&Target::new(HostName::parse("empty.test"), 80), &ConnectOpts::default())
            .await
            .err()
            .expect("no address, no connection");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.to_string(), "no address found for empty.test");

        let v6_only = DirectConnector::with_opts(
            Arc::new(Fixed(vec![ip("127.0.0.1")])),
            SocketOpts {
                ip_version: IpVersion::V6Only,
                ..SocketOpts::default()
            },
            Arc::new(NoopSocketHook),
        );
        let e = v6_only
            .connect(&Target::new(HostName::parse("v4.test"), 80), &ConnectOpts::default())
            .await
            .err()
            .expect("the only answer is filtered out");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            e.to_string(),
            "no usable address for v4.test: every answer was filtered out by ip-version"
        );
    }

    #[test]
    fn targets_are_displayed_the_way_they_are_dialled() {
        assert_eq!(display_target(&Target::new(HostName::parse("::1"), 80)), "[::1]:80");
        assert_eq!(display_target(&Target::new(HostName::parse("192.0.2.1"), 80)), "192.0.2.1:80");
        assert_eq!(display_target(&Target::new(HostName::parse("a.test"), 443)), "a.test:443");
    }
```

（该模块里若已有断言旧文本 `(ip-version)` 的用例，把期望文本改成上面的新文本。需要的 `use`——`IpVersion`、`SocketOpts`、`NoopSocketHook`——按编译器提示补。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-inbound a_domain_name_with_control_characters 2>&1 | tail -8`
Expected: FAIL——应答码是 `0x00`（拨号成功）或连接失败的码，且 `dialer.sessions()` 非空。

Run: `cargo test -p rurge-proto hostname 2>&1 | tail -8`
Expected: 编译失败，`cannot find function to_ascii`。

- [ ] **Step 3: 实现**

`crates/rurge-inbound/src/socks5.rs`，`read_request` 的 `ATYP_DOMAIN` 分支里，把 `HostName::parse(&s)` 换成：

```rust
            // A name with a control character or whitespace is never a host
            // name. Refuse it here, once, instead of trusting every outbound
            // that writes names into a text protocol to catch it.
            match HostName::from_wire(&s) {
                Some(host) => host,
                None => {
                    stream.read_u16().await?;
                    return Ok(Err(REP_GENERAL_FAILURE));
                }
            }
```

（前面的 `len == 0` 分支与 `non-utf8 domain` 的错误保持原样。）

根 `Cargo.toml` 的 `[workspace.dependencies]` 加 `idna = "1"`；`crates/rurge-proto/Cargo.toml` 的 `[dependencies]` 加 `idna.workspace = true`。确认 `Cargo.lock` 里 `idna` 仍是已有的 1.1.0、没有新增包：`git diff Cargo.lock` 只应出现 `rurge-proto` 依赖列表里多一行 `idna`。

`crates/rurge-proto/src/hostname.rs`（测试上方）：

```rust
//! Host names as they are written onto the wire.

/// `name` in the form a proxy request carries: ASCII as it is, an IDN as its
/// A-labels. `None` when the result is empty or holds anything outside
/// `0x21..=0x7e` — a space, a control character, a line break — which could
/// break out of a request line or a header.
pub(crate) fn to_ascii(name: &str) -> Option<String> {
    let ascii = if name.is_ascii() {
        name.to_string()
    } else {
        idna::domain_to_ascii(name).ok()?
    };
    (!ascii.is_empty() && ascii.bytes().all(|b| (0x21..=0x7e).contains(&b))).then_some(ascii)
}
```

`crates/rurge-proto/src/lib.rs` 加 `mod hostname;`（私有模块，按字母序放）。

`crates/rurge-proto/src/http.rs`：`valid_target`、`authority` 与 `tunnel` 改为经 `to_ascii`。

```rust
/// The target's host as it goes into a request line: an IP literal (IPv6 in
/// brackets), or the ASCII form of a domain. `None` = not safe to write.
fn wire_host(target: &Target) -> Option<String> {
    match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => Some(format!("[{v6}]")),
        HostName::Ip(ip) => Some(ip.to_string()),
        HostName::Domain(name) => crate::hostname::to_ascii(name),
    }
}

/// Whether `target` can be written into a request line and a `Host` header:
/// an IP literal, or a domain whose ASCII form (IDNs become A-labels) is not
/// empty and made of bytes `0x21..=0x7e` only. Anything else could break out
/// of the request line of a text protocol. The CONNECT path checks this
/// itself; a caller that writes requests of its own (`HttpForward`) must
/// check it first.
pub fn valid_target(target: &Target) -> bool {
    wire_host(target).is_some()
}
```

`tunnel` 开头改为取出 `wire_host` 并把它传给 `connect_request`（`connect_request(&self, host: &str, port: u16)`，其内部的 authority 就是 `format!("{host}:{port}")`）；原来的 `authority(target)` 函数随之删除，已有的 IPv6 方括号用例必须保持通过。拒绝时的错误文本不变：`the target host name is not valid for an HTTP proxy request`。

`check_status` 里渲染原因短语的那一行改为：

```rust
            let reason = untrusted_text(reason, 64);
            let reason = reason.trim_end();
```

`crates/rurge-proto/src/socks5.rs` 的 `connect_request`，域名分支改为先转 ASCII：

```rust
        // the proxy resolves the name (remote resolution); an IDN goes out as A-labels
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name)
                .ok_or_else(|| proxy("the host name cannot be sent to a SOCKS5 proxy"))?;
            let len = u8::try_from(name.len())
                .map_err(|_| proxy("the host name is longer than 255 bytes"))?;
            request.push(3);
            request.push(len);
            request.extend_from_slice(name.as_bytes());
        }
```

`crates/rurge-net/src/connector.rs`：在 `DirectConnector::connect` 里区分两种"没有地址"，并给超时文本用一个带方括号的显示函数。

```rust
/// `host:port` the way it is dialled: an IPv6 literal goes in brackets.
fn display_target(target: &Target) -> String {
    match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => format!("[{v6}]:{}", target.port),
        host => format!("{host}:{}", target.port),
    }
}
```

域名分支改为：

```rust
                    HostName::Domain(d) => {
                        let addrs = self.resolver.resolve(d).await?;
                        if addrs.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!("no address found for {d}"),
                            ));
                        }
                        let planned =
                            plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
                        if planned.0.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!(
                                    "no usable address for {d}: every answer was filtered out by ip-version"
                                ),
                            ));
                        }
                        planned
                    }
```

超时分支的文本改为 `format!("connect to {} timed out", display_target(target))`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-net connector && cargo test -p rurge-inbound && cargo test -p rurge-proto 2>&1 | tail -12`
Expected: 全部通过；M1a 的 `a_target_with_control_characters_never_reaches_the_proxy`、IPv6 方括号、超长域名三个用例保持通过。

- [ ] **Step 5: 门禁与提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-inbound crates/rurge-proto crates/rurge-net
git commit -m "fix(inbound,proto,net): SOCKS5 入站拒绝含控制字符 / 空白的域名；出站把 IDN 转成 A-label；三处报错文本"
```

---

### Task 3: `rurge-policy` 的三个新部件——`SelectionTable`、`OutboundFactory`、`RegistryCell` / `ChainConnector`

本任务只**新增**东西，不改 `PolicyRegistry` 的现有接口（那是 Task 5），所以工作区其它 crate 不受影响。

**Files:**
- Modify: `crates/rurge-policy/Cargo.toml`
- Modify: `crates/rurge-policy/src/lib.rs`、`crates/rurge-policy/src/selections.rs`
- Create: `crates/rurge-policy/src/factory.rs`
- Create: `crates/rurge-policy/src/cell.rs`
- Create: `crates/rurge-policy/src/testing.rs`（`#[cfg(test)]`，本 crate 单元测试共用的桩）

**Interfaces:**
- Consumes: `rurge_net::connector::{Connector, ConnectOpts, Target, BoxedStream}`；`rurge_proto::{OutboundRef, OutboundError, BuildError, Direct}`；`rurge_config::spec::{CommonOpts, PolicySpec}`；现有的 `PolicyRegistry::{build, resolve, contains}`。
- Produces:
  - `rurge_policy::SelectionTable`：`new(initial: GroupSelections) -> SelectionTable`、`get(&self, group: &str) -> Option<String>`、`set(&self, group: &str, member: &str)`、`snapshot(&self) -> GroupSelections`；`Default`。
  - `rurge_policy::OutboundFactory`（trait）：`direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector>`、`build(&self, spec: &PolicySpec, connector: Arc<dyn Connector>) -> Result<OutboundRef, BuildError>`；`rurge_policy::BuildError`（= `rurge_proto::BuildError` 的再导出）。
  - `rurge_policy::RegistryCell`：`new() -> Arc<RegistryCell>`、`store(&self, Arc<PolicyRegistry>)`、`clear(&self)`、`load(&self) -> Option<Arc<PolicyRegistry>>`。
  - `rurge_policy::ChainConnector`：`new(cell: Arc<RegistryCell>, name: impl Into<String>) -> ChainConnector`；`impl Connector`。错误文本一律以 `via <name>: ` 开头。
  - 测试桩 `crate::testing::RecordingConnector`（`pub(crate)`）：Task 5 继续用。

- [ ] **Step 1: 依赖**

`crates/rurge-policy/Cargo.toml`：

```toml
[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-proto.workspace = true
arc-swap.workspace = true
tracing.workspace = true

[dev-dependencies]
tokio.workspace = true
```

（`rurge-net` 从 dev 依赖挪到正式依赖；`tokio` 只给 `#[tokio::test]` 用，工作区的特性集已含 `macros` `rt-multi-thread` `io-util`。）

- [ ] **Step 2: 写失败的测试**

`crates/rurge-policy/src/selections.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_starts_from_a_snapshot_and_can_be_saved_again() {
        let mut saved = GroupSelections::new();
        saved.set("Pick", "HK");
        let table = SelectionTable::new(saved.clone());
        assert_eq!(table.get("Pick").as_deref(), Some("HK"));
        assert_eq!(table.get("Other"), None);
        table.set("Pick", "JP");
        table.set("Other", "DIRECT");
        assert_eq!(table.get("Pick").as_deref(), Some("JP"));
        let snapshot = table.snapshot();
        assert_eq!(snapshot.get("Pick"), Some("JP"));
        assert_eq!(snapshot.get("Other"), Some("DIRECT"));
        assert_ne!(snapshot, saved);
        assert_eq!(SelectionTable::default().get("Pick"), None);
    }
}
```

创建 `crates/rurge-policy/src/testing.rs`：

```rust
//! Test doubles shared by this crate's unit tests.

use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use std::io;
use std::sync::{Arc, Mutex};

/// Records every target it is asked to reach; hands out one end of an
/// in-memory pipe, or refuses when `fail` is set.
#[derive(Default)]
pub(crate) struct RecordingConnector {
    pub log: Arc<Mutex<Vec<String>>>,
    pub fail: bool,
}

impl RecordingConnector {
    pub(crate) fn seen(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }
}

impl Connector for RecordingConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        _opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            self.log
                .lock()
                .expect("log")
                .push(format!("dial {}:{}", target.host, target.port));
            if self.fail {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "refused by the test",
                ));
            }
            let (near, _far) = tokio::io::duplex(64);
            Ok(Box::new(near) as BoxedStream)
        })
    }
}
```

创建 `crates/rurge-policy/src/cell.rs`，先只放测试：

```rust
//! The registry as seen by things that outlive a config generation
//! (M1 design 6.2).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selections::GroupSelections;
    use crate::testing::RecordingConnector;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_proto::Direct;
    use std::path::Path;

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\nD = direct\nBlock = reject\n[Proxy Group]\nPick = select, D, DIRECT\n[Rule]\nFINAL,DIRECT\n";

    fn registry(connector: Arc<RecordingConnector>) -> Arc<PolicyRegistry> {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        Arc::new(PolicyRegistry::build(
            &loaded.config,
            &GroupSelections::new(),
            Arc::new(Direct::new(connector)),
        ))
    }

    fn server() -> Target {
        Target::new(HostName::parse("proxy.example"), 8080)
    }

    #[tokio::test]
    async fn an_empty_cell_is_an_error() {
        let chain = ChainConnector::new(RegistryCell::new(), "D");
        let e = chain
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("nothing to resolve against");
        assert_eq!(e.to_string(), "via D: no policy registry is active");
    }

    #[tokio::test]
    async fn a_name_that_is_gone_is_an_error() {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector::default())));
        let e = ChainConnector::new(cell, "Ghost")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("the name does not exist");
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert_eq!(e.to_string(), "via Ghost: the policy no longer exists");
    }

    /// The policy's own server goes to the underlying policy as it is: the
    /// name is never resolved locally (FR-OUT-08).
    #[tokio::test]
    async fn the_target_is_handed_over_unchanged_also_through_a_group() {
        let connector = Arc::new(RecordingConnector::default());
        let cell = RegistryCell::new();
        cell.store(registry(connector.clone()));
        for name in ["D", "Pick"] {
            ChainConnector::new(cell.clone(), name)
                .connect(&server(), &ConnectOpts::default())
                .await
                .unwrap_or_else(|e| panic!("{name}: {e}"));
        }
        assert_eq!(
            connector.seen(),
            ["dial proxy.example:8080", "dial proxy.example:8080"]
        );
    }

    #[tokio::test]
    async fn failures_name_the_hop_and_keep_their_kind() {
        let cell = RegistryCell::new();
        cell.store(registry(Arc::new(RecordingConnector {
            fail: true,
            ..RecordingConnector::default()
        })));
        let e = ChainConnector::new(cell.clone(), "D")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("the connector refuses");
        assert_eq!(e.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(e.to_string(), "via D: refused by the test");
        // a reject policy underneath is an error too, not a silent hang
        let e = ChainConnector::new(cell, "Block")
            .connect(&server(), &ConnectOpts::default())
            .await
            .err()
            .expect("REJECT cannot carry a connection");
        assert_eq!(e.to_string(), "via Block: rejected by REJECT");
    }

    #[test]
    fn clearing_the_cell_lets_the_registry_go() {
        let cell = RegistryCell::new();
        let reg = registry(Arc::new(RecordingConnector::default()));
        cell.store(reg.clone());
        assert!(cell.load().is_some());
        cell.clear();
        assert!(cell.load().is_none());
        assert_eq!(Arc::strong_count(&reg), 1);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p rurge-policy 2>&1 | tail -15`
Expected: 编译失败——`cannot find type SelectionTable`、`ChainConnector`、`RegistryCell`。

- [ ] **Step 4: 实现**

`crates/rurge-policy/src/selections.rs`（`GroupSelections` 之后）：

```rust
use std::sync::RwLock;

/// The live `select` choices of the running profile (M1 design 6.3). One
/// table serves every config generation: `resolve` reads it on each call, the
/// API changes it, and `GroupSelections` stays the type that is loaded from
/// and saved to `state.json`.
#[derive(Debug, Default)]
pub struct SelectionTable {
    map: RwLock<HashMap<String, String>>,
}

impl SelectionTable {
    pub fn new(initial: GroupSelections) -> SelectionTable {
        SelectionTable {
            map: RwLock::new(initial.map),
        }
    }

    /// Owned, so no lock is held while the caller walks on through the groups.
    pub fn get(&self, group: &str) -> Option<String> {
        self.map.read().expect("selection table").get(group).cloned()
    }

    pub fn set(&self, group: &str, member: &str) {
        self.map
            .write()
            .expect("selection table")
            .insert(group.to_string(), member.to_string());
    }

    pub fn snapshot(&self) -> GroupSelections {
        GroupSelections::from_map(self.map.read().expect("selection table").clone())
    }
}
```

创建 `crates/rurge-policy/src/factory.rs`：

```rust
//! How the registry gets real outbounds without knowing how they are made
//! (M1 design 6.1). `rurge-engine` implements this over the resolver, the
//! socket hook, the keystore and the root certificates; tests use a fake.

use rurge_config::spec::{CommonOpts, PolicySpec};
use rurge_net::connector::Connector;
use rurge_proto::OutboundRef;
use std::sync::Arc;

pub use rurge_proto::BuildError;

pub trait OutboundFactory: Send + Sync {
    /// What a policy without `underlying-proxy` dials through: a direct
    /// connector carrying that policy's own socket options.
    fn direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector>;

    /// The outbound of `spec`, reaching its server through `connector`.
    /// Synchronous and offline: whatever can fail without the network
    /// (a broken p12, an unusable name) fails here, not at dial time.
    fn build(
        &self,
        spec: &PolicySpec,
        connector: Arc<dyn Connector>,
    ) -> Result<OutboundRef, BuildError>;
}
```

`crates/rurge-policy/src/cell.rs`（测试上方）：

```rust
//! The registry as seen by things that outlive a config generation
//! (M1 design 6.2).

use crate::registry::PolicyRegistry;
use arc_swap::ArcSwapOption;
use rurge_config::rule::PolicyRef;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rurge_proto::OutboundError;
use std::io;
use std::sync::Arc;

/// Where the current generation's registry can be found. The engine stores
/// every new generation here and clears the cell when it goes away: the
/// registry owns outbounds, an outbound may own a `ChainConnector`, and that
/// points back here — clearing is what breaks the cycle.
#[derive(Default)]
pub struct RegistryCell(ArcSwapOption<PolicyRegistry>);

impl RegistryCell {
    pub fn new() -> Arc<RegistryCell> {
        Arc::new(RegistryCell::default())
    }

    pub fn store(&self, registry: Arc<PolicyRegistry>) {
        self.0.store(Some(registry));
    }

    pub fn clear(&self) {
        self.0.store(None);
    }

    pub fn load(&self) -> Option<Arc<PolicyRegistry>> {
        self.0.load_full()
    }
}

/// The connector of a policy with `underlying-proxy = <name>`: reaches the
/// policy's own server through whatever `<name>` resolves to *now* — a group
/// follows its current selection, a reload follows the new generation. The
/// server's host name travels as it is, so the underlying proxy resolves it
/// (FR-OUT-08).
pub struct ChainConnector {
    cell: Arc<RegistryCell>,
    name: String,
}

impl ChainConnector {
    pub fn new(cell: Arc<RegistryCell>, name: impl Into<String>) -> ChainConnector {
        ChainConnector {
            cell,
            name: name.into(),
        }
    }

    fn via(&self, e: OutboundError) -> io::Error {
        match e {
            OutboundError::Io(e) => io::Error::new(e.kind(), format!("via {}: {e}", self.name)),
            OutboundError::Timeout => io::Error::new(
                io::ErrorKind::TimedOut,
                format!("via {}: connect timed out", self.name),
            ),
            other => io::Error::other(format!("via {}: {other}", self.name)),
        }
    }
}

impl Connector for ChainConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let Some(registry) = self.cell.load() else {
                return Err(io::Error::other(format!(
                    "via {}: no policy registry is active",
                    self.name
                )));
            };
            if !registry.contains(&self.name) {
                // a reload removed the name this policy was built against
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("via {}: the policy no longer exists", self.name),
                ));
            }
            let resolution = registry.resolve(&PolicyRef::Named(self.name.clone()));
            resolution
                .outbound
                .connect_tcp(target, opts)
                .await
                .map_err(|e| self.via(e))
        })
    }
}
```

`crates/rurge-policy/src/lib.rs`：

```rust
//! Policy registry (M3 design §5, M1 design 6.1 – 6.3): resolves a `PolicyRef`
//! through aliases and groups to a concrete `Outbound`, recording the chain
//! it took; the factory trait real outbounds come from; the cell and the
//! selection table that outlive a config generation.

pub mod cell;
pub mod factory;
pub mod registry;
pub mod selections;
#[cfg(test)]
pub(crate) mod testing;

pub use cell::{ChainConnector, RegistryCell};
pub use factory::{BuildError, OutboundFactory};
pub use registry::{PolicyRegistry, Resolution};
pub use selections::{GroupSelections, SelectionTable};
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-policy 2>&1 | tail -15`
Expected: 全部通过（原有 4 个 + 新增 6 个）。`rejected by REJECT` 是 `OutboundError::Reject` 现有的 Display 文本；若实际文本不同，以 `crates/rurge-proto/src/outbound.rs` 为准改断言，不要改产品代码。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-policy Cargo.lock
git commit -m "feat(policy): SelectionTable、OutboundFactory、RegistryCell 与按名字解析下一跳的 ChainConnector"
```

---

### Task 4: `EngineFactory`、干构建与 `load_checked`

**Files:**
- Modify: `crates/rurge-engine/Cargo.toml`
- Create: `crates/rurge-engine/src/outbounds.rs`
- Modify: `crates/rurge-engine/src/lib.rs`

**Interfaces:**
- Consumes: Task 3 的 `OutboundFactory` / `BuildError`；M1a 的 `HttpOutbound::from_spec` / `Socks5Outbound::from_spec`（`(spec, keystore, roots, connector)`）、`Direct::new(connector)`、`DirectConnector::with_opts(resolver, SocketOpts, hook)`、`rurge_net::tls::root_store()`、`rurge_config::diagnostic::codes::E_POLICY_BUILD`。
- Produces（`rurge_engine::outbounds`）：
  - `EngineFactory::new(cfg: &Config, resolver: Arc<dyn Resolve>, hook: Arc<dyn SocketHook>) -> EngineFactory`（信任系统根证书）；`EngineFactory::with_roots(cfg, resolver, hook, roots: Arc<rustls::RootCertStore>) -> EngineFactory`（测试注入私有 CA；Task 11 用）；`impl OutboundFactory`。
  - `dry_build(cfg: &Config) -> Diagnostics`——每个构建不出来的策略一条 `E0022`，位置是策略自己的行。
  - `load_checked(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError>`。
  - `rurge_engine::{EngineFactory, dry_build, load_checked}` 再导出。

- [ ] **Step 1: 依赖**

`crates/rurge-engine/Cargo.toml`：`[dependencies]` 加 `rustls.workspace = true`（从 dev 依赖里删掉同名那行，避免重复）；`[dev-dependencies]` 加 `rurge-proto = { workspace = true, features = ["testing"] }`。

- [ ] **Step 2: 写失败的测试**

创建 `crates/rurge-engine/src/outbounds.rs`，先只放测试：

```rust
//! Real outbounds for the policy registry, and the dry build that turns a
//! policy which cannot be built into a load error (M1 design 6.1, 6.4).

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_config::diagnostic::codes;
    use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
    use rurge_net::socket::NoopSocketHook;
    use rurge_proto::testing::{FakeSocks5, Socks5Script, echo_server};
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config(text: &str) -> Config {
        let loaded = from_text(text, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded.diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        loaded.config
    }

    fn factory(cfg: &Config) -> EngineFactory {
        EngineFactory::new(cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook))
    }

    #[test]
    fn every_m1_protocol_builds() {
        let cfg = config(
            "[Proxy]\nH = http, proxy.test, 8080, alice, s3cret\nHS = https, proxy.test, 443, sni=edge.test\n\
S = socks5, proxy.test, 1080\nST = socks5-tls, proxy.test, 1443, skip-cert-verify=true\n\
Corp = direct, interface=eth9, allow-other-interface=true\nBlock = reject\n[Rule]\nFINAL,DIRECT\n",
        );
        let f = factory(&cfg);
        for (name, outbound_name) in [("H", "H"), ("HS", "HS"), ("S", "S"), ("ST", "ST"), ("Corp", "DIRECT")] {
            let spec = cfg.spec(name).unwrap_or_else(|| panic!("no spec for {name}"));
            let out = f
                .build(spec, f.direct_connector(&spec.common))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(out.name(), outbound_name);
        }
        let block = cfg.spec("Block").expect("reject aliases have a spec");
        let e = f
            .build(block, f.direct_connector(&block.common))
            .err()
            .expect("a reject alias has no outbound of its own");
        assert_eq!(e.message, "policy `Block` is a reject alias and has no outbound of its own");
    }

    #[tokio::test]
    async fn a_built_outbound_really_connects() {
        let echo = echo_server().await;
        let upstream = FakeSocks5::spawn(Socks5Script::default()).await;
        let cfg = config(&format!(
            "[Proxy]\nS = socks5, 127.0.0.1, {}\n[Rule]\nFINAL,DIRECT\n",
            upstream.addr().port()
        ));
        let f = factory(&cfg);
        let spec = cfg.spec("S").unwrap();
        let out = f.build(spec, f.direct_connector(&spec.common)).unwrap();
        let target = Target::new(rurge_config::HostName::Ip(echo.ip()), echo.port());
        let mut stream = out.connect_tcp(&target, &ConnectOpts::default()).await.unwrap();
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
        assert_eq!(upstream.requests().len(), 1);
    }

    const BROKEN: &str = "[Proxy]\nGood = http, proxy.test, 8080\nUp = https, proxy.test, 443, client-cert=cert1\nUp2 = socks5-tls, proxy.test, 443, client-cert=empty\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\nempty = type=p12, base64=, password=hunter2\n[Rule]\nFINAL,DIRECT\n";

    #[test]
    fn the_dry_build_reports_what_cannot_be_built_at_the_policys_own_line() {
        let cfg = config(BROKEN);
        let diags = dry_build(&cfg).sorted();
        let found: Vec<(&str, u32)> = diags
            .iter()
            .map(|d| (d.code, d.span.as_ref().map(|s| s.line).unwrap_or(0)))
            .collect();
        assert_eq!(found, [(codes::E_POLICY_BUILD, 3), (codes::E_POLICY_BUILD, 4)]);
        let first = diags.iter().next().unwrap().message.clone();
        assert!(
            first.starts_with("policy `Up` cannot be built: keystore item `cert1`"),
            "{first}"
        );
        for d in diags.iter() {
            assert!(!d.message.contains("hunter2") && !d.message.contains("QUJD"), "{}", d.message);
        }
        // nothing to say about a sound profile
        assert!(dry_build(&config("[Proxy]\nH = http, h.test, 80\n[Rule]\nFINAL,DIRECT\n")).is_empty());
    }

    #[test]
    fn load_checked_is_load_plus_the_dry_build() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.conf");
        std::fs::write(&path, BROKEN).unwrap();
        let loaded = load_checked(&path, &LoadOptions::for_tests()).unwrap();
        assert!(loaded.diagnostics.has_errors());
        assert_eq!(
            loaded
                .diagnostics
                .iter()
                .filter(|d| d.code == codes::E_POLICY_BUILD)
                .count(),
            2
        );
        assert!(load_checked(&dir.path().join("missing.conf"), &LoadOptions::for_tests()).is_err());
    }

    #[test]
    fn only_a_really_unverified_policy_is_flagged() {
        let cfg = config(
            "[Proxy]\nA = https, h.test, 443, skip-cert-verify=true\n\
B = https, h.test, 443, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000\n\
C = https, h.test, 443\nD = http, h.test, 80\n[Rule]\nFINAL,DIRECT\n",
        );
        let flagged: Vec<&str> = cfg
            .specs
            .iter()
            .filter(|s| skips_verification(s))
            .map(|s| s.name.as_str())
            .collect();
        assert_eq!(flagged, ["A"]);
    }
}
```

（`config()` 里 `B` 会带一条 `W0012` 告警，不是错误，`has_errors()` 仍为假。）

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p rurge-engine --lib outbounds 2>&1 | tail -12`
Expected: 编译失败，`cannot find type EngineFactory` / `function dry_build`。

- [ ] **Step 4: 实现**

`crates/rurge-engine/src/outbounds.rs`（测试上方）：

```rust
//! Real outbounds for the policy registry, and the dry build that turns a
//! policy which cannot be built into a load error (M1 design 6.1, 6.4).

use rurge_config::config::{LoadError, LoadOptions, Loaded, load};
use rurge_config::diagnostic::codes;
use rurge_config::spec::{CommonOpts, PolicySpec, ProtoSpec, TlsOpts};
use rurge_config::{Config, Diagnostic, Diagnostics, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, DirectConnector, Resolve};
use rurge_net::socket::{NoopSocketHook, SocketHook, SocketOpts};
use rurge_policy::{BuildError, OutboundFactory};
use rurge_proto::http::HttpOutbound;
use rurge_proto::socks5::Socks5Outbound;
use rurge_proto::{Direct, OutboundRef};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

pub struct EngineFactory {
    resolver: Arc<dyn Resolve>,
    hook: Arc<dyn SocketHook>,
    keystore: Vec<KeystoreItem>,
    roots: Arc<RootCertStore>,
    /// `[General] ipv6`: which family leads when a policy says `dual`.
    v6_first: bool,
    /// A dry build only wants the errors: no warnings, no system roots.
    dry: bool,
}

impl EngineFactory {
    /// Trusts the operating system's root certificates.
    pub fn new(
        cfg: &Config,
        resolver: Arc<dyn Resolve>,
        hook: Arc<dyn SocketHook>,
    ) -> EngineFactory {
        EngineFactory::with_roots(cfg, resolver, hook, rurge_net::tls::root_store())
    }

    /// Trusts `roots` instead (tests bring their own CA).
    pub fn with_roots(
        cfg: &Config,
        resolver: Arc<dyn Resolve>,
        hook: Arc<dyn SocketHook>,
        roots: Arc<RootCertStore>,
    ) -> EngineFactory {
        EngineFactory {
            resolver,
            hook,
            keystore: cfg.keystore.clone(),
            roots,
            v6_first: cfg.general.ipv6,
            dry: false,
        }
    }

    fn dry(cfg: &Config) -> EngineFactory {
        EngineFactory {
            resolver: Arc::new(NeverResolve),
            hook: Arc::new(NoopSocketHook),
            keystore: cfg.keystore.clone(),
            roots: Arc::new(RootCertStore::empty()),
            v6_first: cfg.general.ipv6,
            dry: true,
        }
    }
}

/// A dry build never dials, so this is never asked.
struct NeverResolve;

impl Resolve for NeverResolve {
    fn resolve<'a>(&'a self, _host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(std::future::ready(Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a dry build does not resolve names",
        ))))
    }
}

fn tls_of(spec: &PolicySpec) -> Option<&TlsOpts> {
    match &spec.proto {
        ProtoSpec::Http(http) => http.tls.as_ref(),
        ProtoSpec::Socks5(socks) => socks.tls.as_ref(),
        _ => None,
    }
}

/// `skip-cert-verify` without a pinned fingerprint: the proxy is not
/// authenticated at all (with a fingerprint, the pin takes over — W0012).
fn skips_verification(spec: &PolicySpec) -> bool {
    tls_of(spec).is_some_and(|tls| tls.skip_cert_verify && tls.fingerprint_sha256.is_none())
}

impl OutboundFactory for EngineFactory {
    fn direct_connector(&self, common: &CommonOpts) -> Arc<dyn Connector> {
        Arc::new(DirectConnector::with_opts(
            self.resolver.clone(),
            SocketOpts {
                interface: common.interface.clone(),
                allow_other_interface: common.allow_other_interface,
                ip_version: common.ip_version,
                v6_first: self.v6_first,
                tos: common.tos,
            },
            self.hook.clone(),
        ))
    }

    fn build(
        &self,
        spec: &PolicySpec,
        connector: Arc<dyn Connector>,
    ) -> Result<OutboundRef, BuildError> {
        let outbound: OutboundRef = match &spec.proto {
            ProtoSpec::Direct => Arc::new(Direct::new(connector)),
            ProtoSpec::Reject(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}` is a reject alias and has no outbound of its own",
                    spec.name
                )));
            }
            ProtoSpec::Http(_) => Arc::new(HttpOutbound::from_spec(
                spec,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::Socks5(_) => Arc::new(Socks5Outbound::from_spec(
                spec,
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
        };
        if !self.dry && skips_verification(spec) {
            tracing::warn!(
                policy = %spec.name,
                "skip-cert-verify is on: the proxy server is not authenticated"
            );
        }
        Ok(outbound)
    }
}

/// Builds every policy that has an outbound of its own and throws the result
/// away: what cannot be built is a load error at the policy's own line.
/// Offline and quick — no name is resolved, no socket opened.
pub fn dry_build(cfg: &Config) -> Diagnostics {
    let factory = EngineFactory::dry(cfg);
    let mut diagnostics = Diagnostics::default();
    for spec in &cfg.specs {
        if matches!(spec.proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
            continue;
        }
        if let Err(e) = factory.build(spec, factory.direct_connector(&spec.common)) {
            diagnostics.push(
                Diagnostic::error(
                    codes::E_POLICY_BUILD,
                    format!("policy `{}` cannot be built: {}", spec.name, e.message),
                )
                .at(spec.span.clone()),
            );
        }
    }
    diagnostics
}

/// `rurge_config::config::load` plus the dry build: the one way `check`,
/// `run`, a reload and `POST /v1/profiles/check` read a profile.
pub fn load_checked(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError> {
    let mut loaded = load(path, opts)?;
    loaded.diagnostics.extend(dry_build(&loaded.config));
    Ok(loaded)
}
```

`crates/rurge-engine/src/lib.rs`：加 `pub mod outbounds;` 与 `pub use outbounds::{EngineFactory, dry_build, load_checked};`（按该文件现有的排列方式放）。

若 `ProtoSpec` 的某个变体名 / 载荷与上面不符（例如 `Reject` 的载荷类型），以 `crates/rurge-config/src/spec/mod.rs` 为准调整 `match`，行为不变。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-engine --lib outbounds 2>&1 | tail -12`
Expected: 5 个用例通过。`cert1`（`QUJD` = `ABC`）与 `empty`（空 Base64）都在解码 p12 时失败，各得一条 `E0022`——后一条就是 M1a 留下的"空的 `base64` 不报 `E0021`"由干构建兜住的证明。若加载期已经把空的 `base64=` 判成错误（那样 `config(BROKEN)` 的断言会先失败），把 `empty` 条目的值换成 `base64=AA==`（合法 Base64、不是 p12），并在报告里记下这一点——那说明空值在更早的地方就被拦住了，延后表里那一条可以直接关掉。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-engine Cargo.lock
git commit -m "feat(engine): EngineFactory（真实出站的工厂）、干构建 dry_build（E0022）与 load_checked"
```

---

### Task 5: 换轨——注册表经工厂构建，`EngineShared`，引擎装配

本任务改 `PolicyRegistry::build` 的签名，所以同一个提交里必须把所有调用方一起改完：`Runtime::build`、引擎、bin 的 `build_engine_runtime`、三个测试夹具。完成之后，`http` / `socks5` 策略在引擎里就是真实出站了（能力表到 Task 10 才翻转，期间 `W0007` 告警与实际行为不一致，只存在于本分支内部）。

**Files:**
- Modify: `crates/rurge-policy/src/registry.rs`、`crates/rurge-policy/src/lib.rs`、`crates/rurge-policy/src/testing.rs`、`crates/rurge-policy/src/cell.rs`（只改测试里的 `registry()` 辅助函数）
- Create: `crates/rurge-engine/src/shared.rs`
- Modify: `crates/rurge-engine/src/{lib,runtime,engine,reload,stack}.rs`
- Modify: `crates/rurge/src/cli/runtime.rs`、`crates/rurge/src/cli/run.rs`
- Modify: `crates/rurge-engine/tests/pipeline.rs`、`crates/rurge-api/tests/api.rs`，以及其它构造 `StackOptions` / `RuntimeOptions` 的地方（`grep -rn "StackOptions {\|RuntimeOptions {" crates` 列出）
- Create: `crates/rurge-engine/tests/outbounds.rs`

**Interfaces:**
- Consumes: Task 3（`SelectionTable` `OutboundFactory` `RegistryCell` `ChainConnector` `RecordingConnector`）、Task 4（`EngineFactory`）。
- Produces:
  - `rurge_policy::registry::{TerminalKind, Note}`（也从 crate 根再导出）；`Resolution { chain, outbound, terminal: TerminalKind, note: Option<Note> }`（**`unsupported` 字段删除**）。
  - `PolicyRegistry::build(cfg: &Config, factory: &dyn OutboundFactory, cell: &Arc<RegistryCell>, selections: Arc<SelectionTable>) -> Result<PolicyRegistry, BuildError>`；`PolicyRegistry::current_member(&self, group: &str) -> Option<String>`。
  - `rurge_engine::EngineShared { pub cell: Arc<RegistryCell>, pub selections: Arc<SelectionTable> }`：`new(initial: GroupSelections)`、`Default`、`Clone`。
  - `RuntimeOptions.shared: EngineShared`（**取代** `selections`）；`Runtime.policies: Arc<PolicyRegistry>`；`StackOptions.socket_hook: Arc<dyn SocketHook>`。
  - `Engine::shared(&self) -> EngineShared`；`impl Drop for Engine` 清空 cell。
  - `crates/rurge-engine/tests/outbounds.rs` 的夹具：`Profile { general, proxies, groups, hosts, rules }`（`Default`）、`harness(Profile) -> Harness`（字段 `dir` `engine` `listeners` `dns`，方法 `http()` `socks()`）、`runtime(dir, text, shared) -> Runtime`、`connect_via_http(proxy, authority) -> TcpStream`、`get(&mut stream, host, path) -> String`、`wait_until(what, check)`。Task 6–8 继续往这个文件里加用例。

- [ ] **Step 1: 写失败的测试——注册表**

`crates/rurge-policy/src/testing.rs` 追加假工厂与假出站（假出站像真的代理出站一样：先经自己的连接器够到**自己的服务器**，并记下它被要求去的目标）：

```rust
use crate::factory::{BuildError, OutboundFactory};
use rurge_config::spec::{CommonOpts, PolicySpec};
use rurge_proto::{Outbound, OutboundError, OutboundRef};

pub(crate) struct FakeOutbound {
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Outbound for FakeOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            self.log
                .lock()
                .expect("log")
                .push(format!("{} -> {}:{}", self.name, target.host, target.port));
            self.connector
                .connect(&self.server, opts)
                .await
                .map_err(OutboundError::from)
        })
    }
}

/// Every direct connector it hands out is the same `RecordingConnector`, and
/// every outbound it builds writes into the same log.
pub(crate) struct FakeFactory {
    pub connector: Arc<RecordingConnector>,
    /// The policy whose build fails.
    pub broken: Option<&'static str>,
}

impl FakeFactory {
    pub(crate) fn new() -> FakeFactory {
        FakeFactory {
            connector: Arc::new(RecordingConnector::default()),
            broken: None,
        }
    }
}

impl OutboundFactory for FakeFactory {
    fn direct_connector(&self, _common: &CommonOpts) -> Arc<dyn Connector> {
        self.connector.clone()
    }

    fn build(
        &self,
        spec: &PolicySpec,
        connector: Arc<dyn Connector>,
    ) -> Result<OutboundRef, BuildError> {
        if self.broken == Some(spec.name.as_str()) {
            return Err(BuildError::new("boom"));
        }
        if matches!(spec.proto, rurge_config::spec::ProtoSpec::Direct) {
            return Ok(Arc::new(rurge_proto::Direct::new(connector)));
        }
        let server = Target::new(
            spec.server.clone().expect("a proxy policy has a server"),
            spec.port.expect("and a port"),
        );
        Ok(Arc::new(FakeOutbound {
            name: spec.name.clone(),
            server,
            connector,
            log: self.connector.log.clone(),
        }))
    }
}
```

`crates/rurge-policy/src/registry.rs` 的 `mod tests` 整个换成下面这份（旧用例的断言保留，只是改用新的构造方式与字段）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::selections::GroupSelections;
    use crate::testing::FakeFactory;
    use rurge_config::HostName;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{ConnectOpts, Target};
    use std::path::Path;

    const PROFILE: &str = "[General]\nloglevel = notify\n[Proxy]\n\
HK = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n\
D = direct\nCorp = direct, interface=eth9\nBlock = reject-tinygif\n\
EntryA = socks5, a.example, 1080\nEntryB = socks5, b.example, 1080\n\
Exit = http, exit.example, 8080, underlying-proxy=Hop\n\
[Proxy Group]\nAuto = url-test, HK, D\nPick = select, HK, D, DIRECT\nOuter = select, Pick, Auto\n\
Emptyish = select, Block\nHop = select, EntryA, EntryB\n[Rule]\nFINAL,Pick\n";

    struct Built {
        registry: Arc<PolicyRegistry>,
        factory: FakeFactory,
        table: Arc<SelectionTable>,
    }

    fn built(selections: GroupSelections) -> Built {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(
            !loaded.diagnostics.has_errors(),
            "{:?}",
            loaded.diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>()
        );
        let factory = FakeFactory::new();
        let cell = RegistryCell::new();
        let table = Arc::new(SelectionTable::new(selections));
        let registry = Arc::new(
            PolicyRegistry::build(&loaded.config, &factory, &cell, table.clone()).expect("builds"),
        );
        cell.store(registry.clone());
        Built {
            registry,
            factory,
            table,
        }
    }

    fn chain(r: &Resolution) -> Vec<&str> {
        r.chain.iter().map(String::as_str).collect()
    }

    #[test]
    fn builtins_and_aliases() {
        let reg = built(GroupSelections::new()).registry;
        let d = reg.resolve(&PolicyRef::Builtin(Builtin::Direct));
        assert_eq!(
            (chain(&d), d.outbound.name(), d.terminal, d.note.clone()),
            (vec!["DIRECT"], "DIRECT", TerminalKind::Direct, None)
        );
        let r = reg.resolve(&PolicyRef::Builtin(Builtin::RejectTinyGif));
        assert_eq!(
            (chain(&r), r.outbound.name(), r.terminal),
            (vec!["REJECT-TINYGIF"], "REJECT-TINYGIF", TerminalKind::Reject)
        );
        let cell = reg.resolve(&PolicyRef::Builtin(Builtin::Cellular));
        assert_eq!(
            (chain(&cell), cell.outbound.name()),
            (vec!["CELLULAR", "DIRECT"], "DIRECT")
        );
        // a plain alias shares the built-in DIRECT …
        let alias = reg.resolve(&PolicyRef::parse("D"));
        assert_eq!(chain(&alias), vec!["D", "DIRECT"]);
        assert!(Arc::ptr_eq(&alias.outbound, &reg.direct()));
        // … an alias with socket options owns its outbound
        let corp = reg.resolve(&PolicyRef::parse("Corp"));
        assert_eq!(
            (chain(&corp), corp.outbound.name(), corp.terminal),
            (vec!["Corp", "DIRECT"], "DIRECT", TerminalKind::Direct)
        );
        assert!(!Arc::ptr_eq(&corp.outbound, &reg.direct()));
        let block = reg.resolve(&PolicyRef::parse("Block"));
        assert_eq!(
            (chain(&block), block.outbound.name(), block.terminal),
            (vec!["Block", "REJECT-TINYGIF"], "REJECT-TINYGIF", TerminalKind::Reject)
        );
        assert_eq!(
            reg.names(),
            vec![
                "HK", "D", "Corp", "Block", "EntryA", "EntryB", "Exit", "Auto", "Pick", "Outer",
                "Emptyish", "Hop"
            ]
        );
        assert!(reg.contains("HK") && !reg.contains("Nope"));
    }

    #[test]
    fn a_proxy_policy_resolves_to_its_own_outbound() {
        let reg = built(GroupSelections::new()).registry;
        let a = reg.resolve(&PolicyRef::parse("EntryA"));
        assert_eq!(
            (chain(&a), a.outbound.name(), a.terminal, a.note.clone()),
            (vec!["EntryA"], "EntryA", TerminalKind::Proxy, None)
        );
        let hop = reg.resolve(&PolicyRef::parse("Hop"));
        assert_eq!((chain(&hop), hop.terminal), (vec!["Hop", "EntryA"], TerminalKind::Proxy));
    }

    #[test]
    fn unsupported_protocols_and_devices_reject_with_a_note() {
        let reg = built(GroupSelections::new()).registry;
        let hk = reg.resolve(&PolicyRef::parse("HK"));
        assert_eq!(chain(&hk), vec!["HK", "!unsupported:ss", "REJECT"]);
        assert_eq!(
            (hk.outbound.name(), hk.terminal, hk.note.clone()),
            ("REJECT", TerminalKind::Reject, Some(Note::Unsupported("ss".into())))
        );
        let dev = reg.resolve(&PolicyRef::parse("DEVICE:Living Room"));
        assert_eq!(chain(&dev), vec!["DEVICE:Living Room", "REJECT"]);
        assert_eq!(dev.note, Some(Note::Unsupported("DEVICE".into())));
        let missing = reg.resolve(&PolicyRef::Named("Nope".to_string()));
        assert_eq!((chain(&missing), missing.note.clone()), (vec!["Nope", "REJECT"], None));
    }

    #[test]
    fn groups_read_the_live_table() {
        let mut saved = GroupSelections::new();
        saved.set("Pick", "DIRECT");
        saved.set("Outer", "Pick");
        saved.set("Auto", "D"); // ignored: not a select group
        saved.set("Emptyish", "Gone"); // not a member any more
        let b = built(saved);
        let reg = &b.registry;
        assert_eq!(chain(&reg.resolve(&PolicyRef::parse("Pick"))), vec!["Pick", "DIRECT"]);
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Outer"))),
            vec!["Outer", "Pick", "DIRECT"]
        );
        assert_eq!(chain(&reg.resolve(&PolicyRef::parse("Auto")))[1], "HK");
        assert_eq!(
            chain(&reg.resolve(&PolicyRef::parse("Emptyish"))),
            vec!["Emptyish", "Block", "REJECT-TINYGIF"]
        );
        assert_eq!(reg.current_member("Pick").as_deref(), Some("DIRECT"));
        assert_eq!(reg.current_member("Emptyish").as_deref(), Some("Block"));
        assert_eq!(reg.current_member("Auto").as_deref(), Some("HK"));
        assert_eq!(reg.current_member("D"), None, "not a group");
        // no rebuild: the very next resolve sees the change
        b.table.set("Pick", "D");
        assert_eq!(chain(&reg.resolve(&PolicyRef::parse("Pick"))), vec!["Pick", "D", "DIRECT"]);
        assert_eq!(reg.current_member("Pick").as_deref(), Some("D"));
    }

    /// M1 design §8: switch the selection between two dials and the entry
    /// node of the chain follows.
    #[tokio::test]
    async fn the_entry_of_a_chain_follows_the_group_selection() {
        let b = built(GroupSelections::new());
        let exit = b.registry.resolve(&PolicyRef::parse("Exit"));
        assert_eq!((chain(&exit), exit.terminal), (vec!["Exit"], TerminalKind::Proxy));
        let target = Target::new(HostName::parse("site.example"), 443);
        exit.outbound.connect_tcp(&target, &ConnectOpts::default()).await.unwrap();
        b.table.set("Hop", "EntryB");
        exit.outbound.connect_tcp(&target, &ConnectOpts::default()).await.unwrap();
        assert_eq!(
            b.factory.connector.seen(),
            [
                "Exit -> site.example:443",
                // the exit's server is reached through the entry, by name
                "EntryA -> exit.example:8080",
                "dial a.example:1080",
                "Exit -> site.example:443",
                "EntryB -> exit.example:8080",
                "dial b.example:1080",
            ]
        );
    }

    #[test]
    fn a_policy_that_cannot_be_built_fails_the_whole_registry() {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        let factory = FakeFactory {
            broken: Some("EntryB"),
            ..FakeFactory::new()
        };
        let e = PolicyRegistry::build(
            &loaded.config,
            &factory,
            &RegistryCell::new(),
            Arc::new(SelectionTable::default()),
        )
        .err()
        .expect("EntryB does not build");
        assert_eq!(e.message, "policy `EntryB`: boom");
    }

    #[test]
    fn selections_api() {
        let mut s = GroupSelections::new();
        assert!(s.is_empty());
        s.set("G", "A");
        assert_eq!(s.get("G"), Some("A"));
        assert_eq!(s.get("X"), None);
        let s2 = GroupSelections::from_map(HashMap::from([("G".to_string(), "B".to_string())]));
        assert_eq!(s2.get("G"), Some("B"));
    }
}
```

`crates/rurge-policy/src/cell.rs` 测试里的 `registry()` 辅助函数改成新的构造方式（其余用例不动）：

```rust
    fn registry(connector: Arc<RecordingConnector>) -> Arc<PolicyRegistry> {
        let loaded = from_text(PROFILE, Path::new("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let factory = crate::testing::FakeFactory {
            connector,
            broken: None,
        };
        Arc::new(
            PolicyRegistry::build(
                &loaded.config,
                &factory,
                &RegistryCell::new(),
                Arc::new(crate::selections::SelectionTable::default()),
            )
            .expect("builds"),
        )
    }
```

（`cell.rs` 测试里因此不再需要的 `use`——`GroupSelections`、`Direct`——删掉。）

- [ ] **Step 2: 写失败的测试——引擎端到端**

`crates/rurge-engine/Cargo.toml` 的 dev 依赖已在 Task 4 加了 `rurge-proto`（`testing`）。创建 `crates/rurge-engine/tests/outbounds.rs`：

```rust
//! Sessions that leave through real proxy outbounds: profile text → Runtime →
//! Engine → loopback listeners → scripted loopback upstreams
//! (`rurge_proto::testing`) → `TestServer` (M1 design §8).

use rurge_config::config::{LoadOptions, from_text};
use rurge_config::session::ListenerKind;
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_engine::stack::StackOptions;
use rurge_engine::{Engine, EngineShared, ListenerSpec, Runtime, RuntimeOptions};
use rurge_inbound::Running;
use rurge_net::socket::NoopSocketHook;
use rurge_net::testing::TestServer;
use rurge_proto::testing::{FakeHttpProxy, HttpProxyScript};
use rurge_rules::{GeoUrls, OutboundMode};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

struct Harness {
    dir: tempfile::TempDir,
    engine: Arc<Engine>,
    listeners: Vec<(ListenerSpec, Running)>,
    dns: MockDns,
}

impl Harness {
    fn addr_of(&self, kind: ListenerKind) -> SocketAddr {
        self.listeners
            .iter()
            .find(|(spec, _)| spec.kind == kind)
            .map(|(_, running)| running.local_addr)
            .unwrap_or_else(|| panic!("no {kind:?} listener"))
    }
    fn http(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Http)
    }
    fn socks(&self) -> SocketAddr {
        self.addr_of(ListenerKind::Socks5)
    }
}

fn stack_options(dir: &std::path::Path) -> StackOptions {
    StackOptions {
        data_dir: dir.to_path_buf(),
        no_network: true,
        geo_urls: GeoUrls::default(),
        dns_cache_size: 2000,
        system: Arc::new(StaticSystemDns::default()),
        wait: Duration::ZERO,
        dns_connector: None,
        socket_hook: Arc::new(NoopSocketHook),
    }
}

async fn runtime(dir: &std::path::Path, profile: &str, shared: EngineShared) -> Runtime {
    std::fs::write(dir.join("t.conf"), profile).unwrap();
    let loaded = from_text(profile, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded.diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
    Runtime::build(
        loaded.config,
        RuntimeOptions {
            stack: stack_options(dir),
            outbound_mode: OutboundMode::Rule,
            idle_timeout: Duration::from_secs(600),
            shared,
            request_log_size: 1000,
        },
    )
    .await
    .unwrap()
}

/// The variable parts of a test profile; everything else is fixed.
#[derive(Default)]
struct Profile<'a> {
    general: &'a str,
    proxies: &'a str,
    groups: &'a str,
    hosts: &'a str,
    /// Inserted before `FINAL,DIRECT`.
    rules: &'a str,
}

impl Profile<'_> {
    fn text(&self, dns: SocketAddr) -> String {
        format!(
            "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\ndns-server = {dns}\nipv6 = false\n{}\n\
[Proxy]\n{}\n[Proxy Group]\n{}\n[Host]\n{}\n[Rule]\n{}\nFINAL,DIRECT\n",
            self.general, self.proxies, self.groups, self.hosts, self.rules
        )
    }
}

async fn harness(p: Profile<'_>) -> Harness {
    let dns = MockDns::spawn().await;
    for name in ["target.test", "alt.test"] {
        dns.set(name, &["127.0.0.1"], &[], 60);
    }
    let dir = tempfile::tempdir().unwrap();
    let text = p.text(dns.addr());
    let engine = Engine::new(runtime(dir.path(), &text, EngineShared::default()).await);
    let listeners = engine.bind_listeners().await.unwrap();
    Harness {
        dir,
        engine,
        listeners,
        dns,
    }
}

/// `CONNECT host:port` through rurge's HTTP listener; returns the tunnel.
async fn connect_via_http(proxy: SocketAddr, authority: &str) -> TcpStream {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut byte))
            .await
            .expect("the proxy answers")
            .unwrap();
        assert!(n > 0, "closed before the CONNECT response: {:?}", String::from_utf8_lossy(&head));
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    s
}

/// One `GET` over an established tunnel (or any stream to an origin).
async fn get(stream: &mut TcpStream, host: &str, path: &str) -> String {
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_connect_leaves_through_the_http_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let origin_addr: SocketAddr = format!("127.0.0.1:{}", origin.url("/").port().unwrap())
        .parse()
        .unwrap();
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "s3cret".into())),
        connect_to: Some(origin_addr),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!("Up = http, 127.0.0.1, {}, alice, s3cret", upstream.addr().port()),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    let head = &upstream.heads()[0];
    // the proxy resolves the name: rurge never looked it up
    assert_eq!(head.request_line, "CONNECT target.test:8080 HTTP/1.1");
    assert!(h.dns.queries().is_empty(), "a domain rule and a remote-resolving proxy need no DNS");
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    assert_eq!(log.recent(10)[0].policy, ["Up"]);
}

#[tokio::test]
async fn dropping_the_engine_empties_the_cell() {
    let h = harness(Profile {
        proxies: "Up = http, 127.0.0.1, 9",
        ..Profile::default()
    })
    .await;
    let shared = h.engine.shared();
    assert!(shared.cell.load().is_some());
    let Harness {
        engine,
        listeners,
        dir,
        ..
    } = h;
    drop(listeners);
    drop(engine);
    // the accept loops hold the last references and are aborted asynchronously
    wait_until("the engine to go away", || shared.cell.load().is_none()).await;
    drop(dir); // the profile outlives the engine that read it
}
```

（`MockDns::queries()` 返回收到的全部查询；`RequestLog::recent(n)` 返回 `RequestRecord`，策略链在它的 `policy` 字段里。）

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p rurge-policy 2>&1 | tail -8`
Expected: 编译失败——`TerminalKind` / `Note` 不存在、`build` 的参数不对。

- [ ] **Step 4: 实现——注册表**

`crates/rurge-policy/src/registry.rs` 非测试部分整体换成：

```rust
//! Name → outbound resolution (M3 design §5, M1 design 6.2). Built once per
//! config generation; `resolve` is a table walk with no allocation beyond the
//! chain and the group selections it reads.

use crate::cell::{ChainConnector, RegistryCell};
use crate::factory::{BuildError, OutboundFactory};
use crate::selections::SelectionTable;
use rurge_config::rule::PolicyRef;
use rurge_config::spec::{CommonOpts, IpVersion, PolicySpec};
use rurge_config::{Builtin, Config, GroupKind, PolicyKind};
use rurge_net::connector::Connector;
use rurge_proto::{Direct, OutboundRef, Reject, RejectKind};
use std::collections::HashMap;
use std::sync::Arc;

/// Deeper chains than this are treated as a defect (group cycles are load errors).
pub const MAX_DEPTH: usize = 16;

/// What kind of outbound a resolution ended at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    Direct,
    Reject,
    Proxy,
}

/// Why a resolution ended where it did, when that needs saying.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Note {
    /// The terminal policy's protocol keyword (or `DEVICE`) is not
    /// implemented in this version: the outbound is REJECT.
    Unsupported(String),
}

#[derive(Clone)]
pub struct Resolution {
    pub chain: Vec<String>,
    pub outbound: OutboundRef,
    pub terminal: TerminalKind,
    pub note: Option<Note>,
}

enum Terminal {
    Direct,
    Reject(RejectKind),
}

enum Entry {
    /// A `direct` / `reject*` alias without options of its own.
    Alias(Terminal),
    /// A built proxy, or a `direct` alias with socket options.
    Outbound { outbound: OutboundRef, proxy: bool },
    /// The protocol is not implemented yet: REJECT (W0007 at load).
    Unsupported { kind: PolicyKind },
    Group { kind: GroupKind, members: Vec<String> },
}

pub struct PolicyRegistry {
    entries: HashMap<String, Entry>,
    order: Vec<String>,
    direct: OutboundRef,
    rejects: [OutboundRef; 4],
    selections: Arc<SelectionTable>,
}

fn reject_slot(kind: RejectKind) -> usize {
    match kind {
        RejectKind::Reject => 0,
        RejectKind::Drop => 1,
        RejectKind::NoDrop => 2,
        RejectKind::TinyGif => 3,
    }
}

fn alias_terminal(kind: PolicyKind) -> Option<Terminal> {
    match kind {
        PolicyKind::Direct => Some(Terminal::Direct),
        PolicyKind::Reject => Some(Terminal::Reject(RejectKind::Reject)),
        PolicyKind::RejectDrop => Some(Terminal::Reject(RejectKind::Drop)),
        PolicyKind::RejectNoDrop => Some(Terminal::Reject(RejectKind::NoDrop)),
        PolicyKind::RejectTinyGif => Some(Terminal::Reject(RejectKind::TinyGif)),
        _ => None,
    }
}

/// Whether a `direct` alias needs a connector of its own.
fn has_socket_opts(common: &CommonOpts) -> bool {
    common.interface.is_some() || common.tos != 0 || common.ip_version != IpVersion::default()
}

fn build_one(
    spec: &PolicySpec,
    factory: &dyn OutboundFactory,
    cell: &Arc<RegistryCell>,
) -> Result<OutboundRef, BuildError> {
    let connector: Arc<dyn Connector> = match spec.common.underlying_proxy.as_deref() {
        Some(name) => Arc::new(ChainConnector::new(cell.clone(), name)),
        None => factory.direct_connector(&spec.common),
    };
    factory
        .build(spec, connector)
        .map_err(|e| BuildError::new(format!("policy `{}`: {}", spec.name, e.message)))
}

impl PolicyRegistry {
    /// `cell` is where the chain connectors built here will look the
    /// registry up at dial time; the caller stores the result into it.
    pub fn build(
        cfg: &Config,
        factory: &dyn OutboundFactory,
        cell: &Arc<RegistryCell>,
        selections: Arc<SelectionTable>,
    ) -> Result<PolicyRegistry, BuildError> {
        let direct: OutboundRef =
            Arc::new(Direct::new(factory.direct_connector(&CommonOpts::default())));
        let mut entries = HashMap::new();
        let mut order = Vec::new();
        for p in &cfg.policies {
            let entry = match (alias_terminal(p.kind), cfg.spec(&p.name)) {
                (Some(Terminal::Direct), Some(spec)) if has_socket_opts(&spec.common) => {
                    Entry::Outbound {
                        outbound: build_one(spec, factory, cell)?,
                        proxy: false,
                    }
                }
                (Some(terminal), _) => Entry::Alias(terminal),
                (None, Some(spec)) => Entry::Outbound {
                    outbound: build_one(spec, factory, cell)?,
                    proxy: true,
                },
                // no spec: a protocol of a later milestone
                (None, None) => Entry::Unsupported { kind: p.kind },
            };
            entries.insert(p.name.clone(), entry);
            order.push(p.name.clone());
        }
        for g in &cfg.groups {
            entries.insert(
                g.name.clone(),
                Entry::Group {
                    kind: g.kind,
                    members: g.members.clone(),
                },
            );
            order.push(g.name.clone());
        }
        let rejects = [
            Arc::new(Reject::new(RejectKind::Reject)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::Drop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::NoDrop)) as OutboundRef,
            Arc::new(Reject::new(RejectKind::TinyGif)) as OutboundRef,
        ];
        Ok(PolicyRegistry {
            entries,
            order,
            direct,
            rejects,
            selections,
        })
    }

    pub fn direct(&self) -> OutboundRef {
        self.direct.clone()
    }

    pub fn reject(&self, kind: RejectKind) -> OutboundRef {
        self.rejects[reject_slot(kind)].clone()
    }

    pub fn names(&self) -> Vec<String> {
        self.order.clone()
    }

    /// Non-allocating membership check: every configured policy / group name
    /// (not builtins). Used on the per-connection dial path, where cloning
    /// the whole table via `names()` would allocate for every session.
    pub fn contains(&self, name: &str) -> bool {
        self.order.iter().any(|n| n == name)
    }

    /// The member `group` points at right now: the live selection of a
    /// `select` group when it still names a member, else the first member.
    /// `None` when `group` is not a group or has no members.
    pub fn current_member(&self, group: &str) -> Option<String> {
        let Some(Entry::Group { kind, members }) = self.entries.get(group) else {
            return None;
        };
        let selected = (*kind == GroupKind::Select)
            .then(|| self.selections.get(group))
            .flatten()
            .filter(|m| members.contains(m));
        selected.or_else(|| members.first().cloned())
    }

    pub fn resolve(&self, policy: &PolicyRef) -> Resolution {
        let mut chain = Vec::new();
        match policy {
            PolicyRef::Builtin(b) => self.builtin(*b, &mut chain),
            PolicyRef::Device(name) => self.device(name, &mut chain),
            PolicyRef::Named(name) => self.named(name, &mut chain, 0),
        }
    }

    fn device(&self, name: &str, chain: &mut Vec<String>) -> Resolution {
        chain.push(format!("DEVICE:{name}"));
        self.rejected(chain, Some(Note::Unsupported("DEVICE".to_string())))
    }

    fn builtin(&self, b: Builtin, chain: &mut Vec<String>) -> Resolution {
        chain.push(b.name().to_string());
        if b == Builtin::Direct {
            return self.done(chain, self.direct(), TerminalKind::Direct, None);
        }
        if let Some(kind) = RejectKind::from_builtin(b) {
            return self.done(chain, self.reject(kind), TerminalKind::Reject, None);
        }
        // CELLULAR / CELLULAR-ONLY / HYBRID / NO-HYBRID: iOS-only, DIRECT on desktop (W0009 at load).
        chain.push("DIRECT".to_string());
        self.done(chain, self.direct(), TerminalKind::Direct, None)
    }

    fn named(&self, name: &str, chain: &mut Vec<String>, depth: usize) -> Resolution {
        chain.push(name.to_string());
        if depth > MAX_DEPTH {
            tracing::error!(
                policy = name,
                "policy chain deeper than {MAX_DEPTH}; treating as REJECT"
            );
            return self.rejected(chain, None);
        }
        match self.entries.get(name) {
            None => {
                tracing::error!(
                    policy = name,
                    "policy not found in registry; treating as REJECT"
                );
                self.rejected(chain, None)
            }
            Some(Entry::Alias(Terminal::Direct)) => {
                chain.push("DIRECT".to_string());
                self.done(chain, self.direct(), TerminalKind::Direct, None)
            }
            Some(Entry::Alias(Terminal::Reject(kind))) => {
                chain.push(kind.name().to_string());
                self.done(chain, self.reject(*kind), TerminalKind::Reject, None)
            }
            Some(Entry::Outbound {
                outbound,
                proxy: true,
            }) => self.done(chain, outbound.clone(), TerminalKind::Proxy, None),
            Some(Entry::Outbound {
                outbound,
                proxy: false,
            }) => {
                chain.push("DIRECT".to_string());
                self.done(chain, outbound.clone(), TerminalKind::Direct, None)
            }
            Some(Entry::Unsupported { kind }) => {
                chain.push(format!("!unsupported:{}", kind.keyword()));
                self.rejected(chain, Some(Note::Unsupported(kind.keyword().to_string())))
            }
            Some(Entry::Group { .. }) => match self.current_member(name) {
                Some(member) => match PolicyRef::parse(&member) {
                    PolicyRef::Builtin(b) => self.builtin(b, chain),
                    PolicyRef::Device(d) => self.device(&d, chain),
                    PolicyRef::Named(n) => self.named(&n, chain, depth + 1),
                },
                None => {
                    tracing::error!(
                        group = name,
                        "policy group has no members; treating as REJECT"
                    );
                    self.rejected(chain, None)
                }
            },
        }
    }

    fn rejected(&self, chain: &mut Vec<String>, note: Option<Note>) -> Resolution {
        chain.push(RejectKind::Reject.name().to_string());
        self.done(
            chain,
            self.reject(RejectKind::Reject),
            TerminalKind::Reject,
            note,
        )
    }

    fn done(
        &self,
        chain: &mut Vec<String>,
        outbound: OutboundRef,
        terminal: TerminalKind,
        note: Option<Note>,
    ) -> Resolution {
        Resolution {
            chain: std::mem::take(chain),
            outbound,
            terminal,
            note,
        }
    }
}
```

`crates/rurge-policy/src/lib.rs` 的再导出改为 `pub use registry::{Note, PolicyRegistry, Resolution, TerminalKind};`。

Run: `cargo test -p rurge-policy 2>&1 | tail -12`
Expected: `rurge-policy` 自己的测试全部通过（此时工作区其它 crate 还编译不过，下一步修）。

- [ ] **Step 5: 实现——`EngineShared`、`StackOptions`、`Runtime`**

创建 `crates/rurge-engine/src/shared.rs`：

```rust
//! What outlives a config generation (M1 design 6.2, 6.3).

use rurge_policy::{GroupSelections, RegistryCell, SelectionTable};
use std::sync::Arc;

/// Created once per engine — before the first `Runtime::build`, because the
/// registry built there already needs both — and handed to every later
/// `Runtime::build` of the same engine (`Engine::shared`).
#[derive(Clone)]
pub struct EngineShared {
    /// Where chain connectors find the current generation's registry.
    pub cell: Arc<RegistryCell>,
    /// The live `select` choices of the running profile.
    pub selections: Arc<SelectionTable>,
}

impl EngineShared {
    pub fn new(initial: GroupSelections) -> EngineShared {
        EngineShared {
            cell: RegistryCell::new(),
            selections: Arc::new(SelectionTable::new(initial)),
        }
    }
}

impl Default for EngineShared {
    fn default() -> EngineShared {
        EngineShared::new(GroupSelections::new())
    }
}
```

`crates/rurge-engine/src/lib.rs`：加 `pub mod shared;` 与 `pub use shared::EngineShared;`。

`crates/rurge-engine/src/stack.rs`：`StackOptions` 加一个字段（放在 `dns_connector` 之后）：

```rust
    /// Interface binding and TOS for outbound sockets (the bin injects
    /// `rurge-platform`; everything else uses `NoopSocketHook`).
    pub socket_hook: Arc<dyn rurge_net::socket::SocketHook>,
```

`crates/rurge-engine/src/runtime.rs`：

```rust
pub struct RuntimeOptions {
    pub stack: StackOptions,
    pub outbound_mode: OutboundMode,
    pub idle_timeout: Duration,
    /// The engine's generation-independent objects: `EngineShared::new` for
    /// the first build, `Engine::shared()` for every later one.
    pub shared: EngineShared,
    pub request_log_size: usize,
}

pub struct Runtime {
    pub config: Arc<Config>,
    pub stack: Stack,
    pub rules: RuleEngine,
    pub policies: Arc<PolicyRegistry>,
    pub outbound_mode: OutboundMode,
    pub idle_timeout: Duration,
    pub request_log_size: usize,
    pub(crate) shared: EngineShared,
    /// Present only when `encrypted-dns-follow-outbound-mode` is on; the engine
    /// attaches itself to it so DNS upstream connections take the dial pipeline.
    pub(crate) dns_pipeline: Option<Arc<crate::dns_pipeline::PipelineConnector>>,
}
```

`Runtime::build` 里，把构造 `direct` 与 `PolicyRegistry::build(&config, &opts.selections, direct)` 的那一段换成：

```rust
        let factory = crate::outbounds::EngineFactory::new(
            &config,
            stack.resolver.clone(),
            opts.stack.socket_hook.clone(),
        );
        // The dry build has already turned every build failure into a load
        // error (`load_checked`), so this only fails for a caller that skipped it.
        let policies = Arc::new(
            PolicyRegistry::build(
                &config,
                &factory,
                &opts.shared.cell,
                opts.shared.selections.clone(),
            )
            .map_err(|e| anyhow::anyhow!("cannot build the policies: {e}"))?,
        );
```

并在返回的 `Runtime { .. }` 里加 `shared: opts.shared,`；删掉因此不再使用的 `use`（`Direct`、`OutboundRef`、`GroupSelections`）。

- [ ] **Step 6: 实现——引擎**

`crates/rurge-engine/src/engine.rs`：

1. `Engine` 加字段 `shared: EngineShared,`（`use crate::shared::EngineShared;`）。
2. `Engine::new` 开头：

```rust
        // Publish the first generation before anything can dial through it.
        let shared = runtime.shared.clone();
        shared.cell.store(runtime.policies.clone());
```

   构造 `Engine { .. }` 时带上 `shared,`。
3. 加两个方法与 `Drop`：

```rust
impl Engine {
    /// The generation-independent objects; every `Runtime` swapped into this
    /// engine must have been built with them.
    pub fn shared(&self) -> EngineShared {
        self.shared.clone()
    }

    /// Makes `next`'s registry the one chain connectors resolve against.
    pub(crate) fn publish_registry(&self, next: &Runtime) {
        assert!(
            Arc::ptr_eq(&self.shared.cell, &next.shared.cell),
            "the next generation must be built with `Engine::shared()`"
        );
        self.shared.cell.store(next.policies.clone());
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        // registry → outbound → chain connector → cell → registry
        self.shared.cell.clear();
    }
}
```

4. `dial` 里把 `if let Some(kind) = &resolution.unsupported {` 换成 `if let Some(rurge_policy::Note::Unsupported(kind)) = &resolution.note {`（函数体不变）。

`crates/rurge-engine/src/reload.rs` 的 `swap_runtime`：在 `self.store_runtime(next);` 之前加 `self.publish_registry(&next);`。

- [ ] **Step 7: 实现——调用方**

`crates/rurge/src/cli/runtime.rs` 的 `stack_options`：加 `socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),`（Task 10 换成平台适配器）。

`crates/rurge/src/cli/run.rs`：

- `build_engine_runtime` 的最后一个参数由 `state: &State` 改为 `shared: &EngineShared`，函数体里删掉 `let selections = …;`，`RuntimeOptions` 里用 `shared: shared.clone(),` 取代 `selections,`。
- 启动处：在调用它之前创建 `let shared = EngineShared::new(state.selections_for(&profile_key(&cfg.source.main)));`（注意 `cfg` 在这之后被移走，所以这一行要放在 `build_engine_runtime(cfg, …)` 之前），传 `&shared`。
- `reload()` 里：删掉只为取选择而存在的 `let state = d.store.snapshot().await;`，传 `&d.engine.shared()`。若 `Daemon.store` 因此不再被读，保留字段（Task 8 之后仍由引擎经 `attach_state` 写入），必要时按 clippy 的提示处理未使用警告。
- 补上 `use rurge_engine::EngineShared;`，删掉不再使用的 `use`。

其余构造点：`crates/rurge-engine/tests/pipeline.rs`、`crates/rurge-api/tests/api.rs`（以及 `grep` 列出的其它位置，如基准或 bin 的离线命令）——`StackOptions { .. }` 里加 `socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),`，`RuntimeOptions { .. }` 里把 `selections: GroupSelections::new(),` 换成 `shared: rurge_engine::EngineShared::default(),`，并清理不再使用的 `use rurge_policy::GroupSelections;`。

- [ ] **Step 8: 跑测试确认通过**

Run: `cargo test -p rurge-policy && cargo test -p rurge-engine && cargo test -p rurge-api 2>&1 | tail -20`
Expected: 全部通过，包括 `tests/pipeline.rs` 的全部旧用例（策略链的表示没变：`!unsupported:ss` 标记还在）与 `tests/outbounds.rs` 的两个新用例。

- [ ] **Step 9: 门禁与提交**

```bash
git add crates Cargo.lock
git commit -m "feat(policy,engine): 注册表经工厂构建真实出站；Resolution.terminal / note；EngineShared（RegistryCell + SelectionTable）与引擎装配"
```

---

### Task 6: 明文 HTTP 的绝对 URI 转发

**Files:**
- Modify: `crates/rurge-inbound/src/session.rs`（`Dialed.forward`）
- Modify: `crates/rurge-inbound/src/http.rs`（`absolute_form`、`forward()` 的分支）
- Modify: `crates/rurge-inbound/src/testing.rs`（`FakeDialer` 构造 `Dialed` 的地方）
- Modify: `crates/rurge-engine/src/engine.rs`（`dial`）
- Test: `crates/rurge-engine/tests/outbounds.rs`

**Interfaces:**
- Consumes: M1a 的 `Outbound::http_forward() -> Option<&dyn HttpForward>`、`HttpForward::{connect(&ConnectOpts), request_headers()}`、`rurge_proto::http::valid_target`；Task 5 的夹具。
- Produces:
  - `rurge_inbound::Dialed { stream, handle, forward: Option<Vec<(String, String)>> }`——`Some` 表示 `stream` 通向一个接受绝对 URI 请求的 HTTP 代理，里面是要套到请求上的头（替换同名头），**为这条连接渲染一次**。
  - 引擎：会话来自 HTTP 入站的明文请求（`listener == Http` 且 `session.url.is_some()`）且终端出站的 `http_forward()` 为 `Some` 时走转发；其余一切情形照旧走 `connect_tcp`。
  - 入站：`Dialed.forward` 为 `Some` 时保留绝对 URI（重建为 `http://<host[:port]><path?query>`，去掉 userinfo）。

`HttpForward` 的两条契约（写在 trait 的文档注释上）由本任务履行：写请求行之前先 `valid_target`；`request_headers()` 每条连接只调一次。

- [ ] **Step 1: 写失败的测试**

`crates/rurge-inbound/src/http.rs` 的 `mod tests` 末尾：

```rust
    #[test]
    fn absolute_form_keeps_the_uri_and_applies_the_upstream_headers() {
        let mut req = Request::builder()
            .method("GET")
            .uri("http://user:pw@example.test:8080/a?b=1")
            .header("Host", "lying.internal")
            .header("Proxy-Authorization", "Basic Y2xpZW50")
            .header("Proxy-Connection", "keep-alive")
            .header("Connection", "X-Hop")
            .header("X-Hop", "1")
            .header("X-Keep", "1")
            .body(())
            .unwrap();
        absolute_form(
            &mut req,
            &[
                ("Proxy-Authorization".to_string(), "Basic dXA=".to_string()),
                ("X-Pad".to_string(), "abc".to_string()),
            ],
        )
        .unwrap();
        // still absolute, without the userinfo
        assert_eq!(req.uri().to_string(), "http://example.test:8080/a?b=1");
        let h = req.headers();
        assert_eq!(h.get("host").unwrap(), "example.test:8080");
        // the client's credentials for this hop are gone, the upstream's are on
        assert_eq!(h.get("proxy-authorization").unwrap(), "Basic dXA=");
        assert_eq!(h.get("x-pad").unwrap(), "abc");
        assert_eq!(h.get("x-keep").unwrap(), "1");
        for gone in ["proxy-connection", "connection", "x-hop"] {
            assert!(h.get(gone).is_none(), "{gone}");
        }
    }

    #[test]
    fn the_upstream_headers_replace_same_name_headers_including_host() {
        let mut req = Request::builder()
            .uri("http://example.test/")
            .header("User-Agent", "client/1")
            .body(())
            .unwrap();
        absolute_form(
            &mut req,
            &[
                ("Host".to_string(), "edge.example".to_string()),
                ("User-Agent".to_string(), "rurge".to_string()),
                ("Bad Name".to_string(), "dropped".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(req.headers().get("host").unwrap(), "edge.example");
        assert_eq!(req.headers().get_all("user-agent").iter().count(), 1);
        assert_eq!(req.headers().get("user-agent").unwrap(), "rurge");
        assert_eq!(req.headers().len(), 2, "the malformed header is dropped, not sent");
    }
```

`crates/rurge-engine/tests/outbounds.rs` 追加（`use rurge_proto::testing::{FakeSocks5, Socks5Script};` 补到文件头）：

```rust
/// One request/response over a fresh connection to rurge's HTTP listener.
async fn plain_get(proxy: SocketAddr, url: &str, host: &str) -> String {
    let mut s = TcpStream::connect(proxy).await.unwrap();
    s.write_all(
        format!(
            "GET {url} HTTP/1.1\r\nHost: {host}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    String::from_utf8_lossy(&buf).into_owned()
}

fn origin_addr(origin: &TestServer) -> SocketAddr {
    format!("127.0.0.1:{}", origin.url("/").port().unwrap())
        .parse()
        .unwrap()
}

#[tokio::test]
async fn a_plain_request_goes_to_an_http_upstream_in_absolute_form() {
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "s3cret".into())),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Up = http, 127.0.0.1, {}, alice, s3cret, headers=X-Client:rurge",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let response = plain_get(h.http(), "http://u:p@target.test:8080/hello?x=1", "lying.internal").await;
    // the fake answers absolute-form requests itself
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("forwarded"), "{response}");
    let heads = upstream.heads();
    assert_eq!(heads.len(), 1, "one connection, one request, no CONNECT");
    assert_eq!(heads[0].request_line, "GET http://target.test:8080/hello?x=1 HTTP/1.1");
    assert_eq!(heads[0].header("Host"), Some("target.test:8080"));
    // base64("alice:s3cret")
    assert_eq!(heads[0].header("Proxy-Authorization"), Some("Basic YWxpY2U6czNjcmV0"));
    assert_eq!(heads[0].header("X-Client"), Some("rurge"));
    assert_eq!(heads[0].header("Proxy-Connection"), None);
}

#[tokio::test]
async fn always_use_connect_and_other_protocols_tunnel_plain_requests() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let http_up = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    let socks_up = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(origin_addr(&origin)),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Tunnel = http, 127.0.0.1, {}, always-use-connect=true\nSocks = socks5, 127.0.0.1, {}",
            http_up.addr().port(),
            socks_up.addr().port()
        ),
        rules: "DOMAIN,target.test,Tunnel\nDOMAIN,alt.test,Socks",
        ..Profile::default()
    })
    .await;
    let response = plain_get(h.http(), "http://target.test:8080/hello", "target.test:8080").await;
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(http_up.heads()[0].request_line, "CONNECT target.test:8080 HTTP/1.1");
    let response = plain_get(h.http(), "http://alt.test:8080/hello", "alt.test:8080").await;
    assert!(response.ends_with("hi there"), "{response}");
    let seen = &socks_up.requests()[0];
    assert_eq!((seen.atyp, seen.host.as_str(), seen.port), (3, "alt.test", 8080));
    // inside a tunnel the origin sees an ordinary origin-form request
    assert_eq!(origin.hits("/hello"), 2);
}

#[tokio::test]
async fn a_refusing_upstream_is_a_502_that_quotes_the_proxy() {
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        auth: Some(("alice".into(), "right".into())),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!("Up = http, 127.0.0.1, {}, alice, wrong", upstream.addr().port()),
        rules: "DOMAIN,target.test,Up",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:443 HTTP/1.1\r\nHost: target.test:443\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut buf)).await;
    let response = String::from_utf8_lossy(&buf).into_owned();
    assert!(response.starts_with("HTTP/1.1 502"), "{response}");
    let log = h.engine.request_log();
    wait_until("the failed session", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert_eq!(error, "http proxy answered 407 Proxy Authentication Required");
    assert!(!response.contains("wrong") && !error.contains("wrong"), "no credential leaks");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-inbound absolute_form 2>&1 | tail -8`
Expected: 编译失败，`cannot find function absolute_form`。

Run: `cargo test -p rurge-engine --test outbounds a_plain_request_goes 2>&1 | tail -12`
Expected: FAIL——假上游收到的是 `CONNECT target.test:8080 HTTP/1.1`（还没有转发模式）。

- [ ] **Step 3: 实现——入站**

`crates/rurge-inbound/src/session.rs`：

```rust
pub struct Dialed {
    pub stream: BoxedStream,
    pub handle: Arc<SessionHandle>,
    /// `Some` when `stream` leads to an HTTP proxy that takes plain requests
    /// in absolute form (no CONNECT was sent): the headers to put on the
    /// request — they replace same-name headers — rendered once for this
    /// connection. Only ever set for a plain request of the HTTP listener.
    pub forward: Option<Vec<(String, String)>>,
}
```

`crates/rurge-inbound/src/testing.rs`：`FakeDialer` 里每个 `Dialed { .. }` 加 `forward: None,`（`grep -rn "Dialed {" crates` 列出全部构造点，一并补上）。

`crates/rurge-inbound/src/http.rs`，放在 `origin_form` 后面：

```rust
/// Keeps a proxy request in absolute form for an upstream HTTP proxy
/// (`always-use-connect = false`): the URI is rebuilt without userinfo, the
/// hop-by-hop headers go, `Host` follows the request target, and `extra` —
/// the upstream's `Proxy-Authorization` and configured headers — replaces
/// same-name headers, `Host` included (manual: Policies › HTTP).
pub(crate) fn absolute_form<B>(
    req: &mut Request<B>,
    extra: &[(String, String)],
) -> Result<(), http::Error> {
    let authority = req.uri().authority().map(host_port).unwrap_or_default();
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".to_string());
    *req.uri_mut() = format!("http://{authority}{path}").parse::<Uri>()?;
    strip_hop_by_hop(req.headers_mut());
    if let Ok(v) = HeaderValue::from_str(&authority) {
        req.headers_mut().insert(header::HOST, v);
    }
    for (name, value) in extra {
        let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::from_str(value),
        ) else {
            // the outbound validated its templates; whatever the http crate
            // still refuses is dropped rather than sent malformed
            tracing::debug!(listener = "http", "dropped an upstream proxy header");
            continue;
        };
        req.headers_mut().insert(name, value);
    }
    Ok(())
}
```

`forward()` 里：

1. 拨号成功后把三个字段拆开——把 `let handle = dialed.handle.clone();` 与 `Counting::new(dialed.stream, …)` 改成：

```rust
    let Dialed {
        stream: upstream,
        handle,
        forward: upstream_headers,
    } = dialed;
    let io = TokioIo::new(Counting::new(upstream, handle.clone()));
```

   （`use crate::session::Dialed;` 补到文件头的 `use` 里。）
2. 把 `if let Err(e) = origin_form(&mut req) {` 换成：

```rust
    // To an HTTP proxy the request stays in absolute form; to anything else
    // (an origin, a tunnel) it goes in origin form.
    let rewritten = match &upstream_headers {
        Some(headers) => absolute_form(&mut req, headers),
        None => origin_form(&mut req),
    };
    if let Err(e) = rewritten {
```

- [ ] **Step 4: 实现——引擎**

`crates/rurge-engine/src/engine.rs` 的 `dial`，把

```rust
            match resolution.outbound.connect_tcp(&target, &opts).await {
                Ok(stream) => Ok(Dialed { stream, handle }),
```

换成：

```rust
            // A plain request of the HTTP listener can go to an HTTP proxy in
            // absolute form instead of through a tunnel (M1 design 6.5). The
            // inbound writes the request line, so the target is checked here
            // first, and the headers are rendered once for this connection
            // (the two obligations `HttpForward` puts on its caller).
            let plain_http = handle.session().listener == ListenerKind::Http
                && handle.session().url.is_some();
            let forward = if plain_http {
                resolution.outbound.http_forward()
            } else {
                None
            };
            let connected = match forward {
                Some(proxy) if rurge_proto::http::valid_target(&target) => proxy
                    .connect(&opts)
                    .await
                    .map(|stream| (stream, Some(proxy.request_headers()))),
                Some(_) => Err(OutboundError::Proxy(
                    "the target host name is not valid for an HTTP proxy request".to_string(),
                )),
                None => resolution
                    .outbound
                    .connect_tcp(&target, &opts)
                    .await
                    .map(|stream| (stream, None)),
            };
            match connected {
                Ok((stream, forward)) => Ok(Dialed {
                    stream,
                    handle,
                    forward,
                }),
```

其余 `Err(..)` 分支原样保留。`dial_internal` 不动（它的会话是 `Internal` 监听类型，永远不转发）。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-inbound && cargo test -p rurge-engine 2>&1 | tail -15`
Expected: 全部通过。`tests/pipeline.rs` 里经 DIRECT 的明文转发用例不受影响（`http_forward()` 对 DIRECT 是 `None`）。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-inbound crates/rurge-engine
git commit -m "feat(inbound,engine): 明文 HTTP 经 http / https 上游按绝对 URI 转发（always-use-connect=true 时走 CONNECT）"
```

---

### Task 7: `use-local-host-item-for-proxy`、两级链与 rurge → rurge

**Files:**
- Modify: `crates/rurge-dns/src/resolver.rs`（`Resolver::host_lookup`）
- Modify: `crates/rurge-engine/src/engine.rs`（`dial`）
- Test: `crates/rurge-engine/tests/outbounds.rs`

**Interfaces:**
- Consumes: `rurge_dns::hosts::{HostAction, HostLookup}`、`HostMap::lookup`；Task 5 的 `Resolution.terminal`；Task 6 的转发分支。
- Produces:
  - `Resolver::host_lookup(&self, name: &str) -> Option<HostLookup>`——只查 `[Host]` 与 hosts 文件，不发任何网络查询。
  - 引擎（FR-DNS-07）：`resolution.terminal == Proxy`、目标是域名、`[General] use-local-host-item-for-proxy = true` 且命中 `HostAction::Ips` 时，把**第一个 IP** 而不是域名交给代理；别名 / 指定服务器类条目不改变目标。命中时这条会话不走绝对 URI 转发（计划期决定 P9）。

- [ ] **Step 1: 写失败的测试**

`crates/rurge-dns/src/resolver.rs` 的 `mod tests` 末尾（构造解析器的辅助函数用该模块已有的；没有现成的就照模块里别的用例的写法造一个带 `[Host]` 的解析器）：

```rust
    #[tokio::test]
    async fn host_lookup_reads_the_host_section_without_any_query() {
        let dns = MockDns::spawn().await;
        let resolver = resolver_for(
            &format!(
                "[General]\ndns-server = {}\n[Host]\npinned.test = 10.1.2.3, 10.1.2.4\nalias.test = other.test\n[Rule]\nFINAL,DIRECT\n",
                dns.addr()
            ),
        );
        let hit = resolver.host_lookup("Pinned.Test.").expect("a [Host] item");
        assert_eq!(
            hit.action,
            HostAction::Ips(vec!["10.1.2.3".parse().unwrap(), "10.1.2.4".parse().unwrap()])
        );
        assert!(matches!(
            resolver.host_lookup("alias.test").map(|h| h.action),
            Some(HostAction::Alias(_))
        ));
        assert!(resolver.host_lookup("unknown.test").is_none());
        assert!(dns.queries().is_empty(), "no query leaves the process");
    }
```

（`resolver_for(profile_text) -> Arc<Resolver>`：若模块里的辅助函数名字 / 形状不同，用实际的那个；要点是解析器来自一份带 `[Host]` 的配置。）

`crates/rurge-engine/tests/outbounds.rs` 追加：

```rust
#[tokio::test]
async fn a_local_host_item_reaches_the_proxy_as_an_ip() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    for (flag, expected) in [("true", (1u8, "10.1.2.3")), ("false", (3u8, "pinned.test"))] {
        let upstream = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(origin_addr(&origin)),
            ..Socks5Script::default()
        })
        .await;
        let h = harness(Profile {
            general: &format!("use-local-host-item-for-proxy = {flag}"),
            proxies: &format!("Up = socks5, 127.0.0.1, {}", upstream.addr().port()),
            hosts: "pinned.test = 10.1.2.3\nalias.test = pinned.test",
            rules: "DOMAIN-SUFFIX,test,Up",
            ..Profile::default()
        })
        .await;
        let mut tunnel = connect_via_http(h.http(), "pinned.test:80").await;
        assert!(get(&mut tunnel, "pinned.test", "/hello").await.ends_with("hi there"));
        let seen = &upstream.requests()[0];
        assert_eq!((seen.atyp, seen.host.as_str()), expected, "flag = {flag}");
        // an alias item never changes what the proxy is asked for
        let _ = connect_via_http(h.http(), "alias.test:80").await;
        wait_until("the second request", || upstream.requests().len() == 2).await;
        assert_eq!(upstream.requests()[1].host, "alias.test", "flag = {flag}");
    }
}

/// With an IP substituted for the name, a plain request is tunnelled: the
/// request inside keeps its `Host`, the proxy connects where `[Host]` says.
#[tokio::test]
async fn a_pinned_host_turns_forwarding_into_a_tunnel() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let upstream = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    let h = harness(Profile {
        general: "use-local-host-item-for-proxy = true",
        proxies: &format!("Up = http, 127.0.0.1, {}", upstream.addr().port()),
        hosts: "pinned.test = 10.1.2.3",
        rules: "DOMAIN,pinned.test,Up",
        ..Profile::default()
    })
    .await;
    let response = plain_get(h.http(), "http://pinned.test/hello", "pinned.test").await;
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(upstream.heads()[0].request_line, "CONNECT 10.1.2.3:80 HTTP/1.1");
    assert_eq!(origin.requests()[0].header("host"), Some("pinned.test"));
}

/// M1 design §9.1: a two-level chain whose lower level is a `select` group.
#[tokio::test]
async fn a_chain_enters_through_the_groups_current_member() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let exit = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(origin_addr(&origin)),
        ..HttpProxyScript::default()
    })
    .await;
    let entry = |to: SocketAddr| Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let entry_a = FakeSocks5::spawn(entry(exit.addr())).await;
    let entry_b = FakeSocks5::spawn(entry(exit.addr())).await;
    let h = harness(Profile {
        proxies: &format!(
            "EntryA = socks5, 127.0.0.1, {}\nEntryB = socks5, 127.0.0.1, {}\nExit = http, exit.example, 8080, underlying-proxy=Hop",
            entry_a.addr().port(),
            entry_b.addr().port()
        ),
        groups: "Hop = select, EntryA, EntryB",
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(get(&mut tunnel, "target.test", "/hello").await.ends_with("hi there"));
    // the entry is asked for the exit's server by name; the exit for the target by name
    let first = &entry_a.requests()[0];
    assert_eq!((first.atyp, first.host.as_str(), first.port), (3, "exit.example", 8080));
    assert_eq!(exit.heads()[0].request_line, "CONNECT target.test:8080 HTTP/1.1");
    assert!(entry_b.requests().is_empty());

    h.engine.shared().selections.set("Hop", "EntryB");
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(get(&mut tunnel, "target.test", "/hello").await.ends_with("hi there"));
    assert_eq!(entry_b.requests().len(), 1, "the next connection follows the selection");
    assert_eq!(entry_a.requests().len(), 1);
}

#[tokio::test]
async fn a_broken_hop_is_named_in_the_error() {
    // a port nothing listens on: bind one, note it, let it go
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let h = harness(Profile {
        proxies: &format!(
            "Entry = socks5, 127.0.0.1, {closed}\nExit = http, exit.example, 8080, underlying-proxy=Entry"
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:443 HTTP/1.1\r\nHost: target.test:443\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), s.read_to_end(&mut buf)).await;
    let log = h.engine.request_log();
    wait_until("the failed session", || !log.recent(10).is_empty()).await;
    let error = log.recent(10)[0].error.clone().unwrap_or_default();
    assert!(error.starts_with("via Entry: "), "{error}");
}

/// Two independent implementations check each other: engine A's upstreams
/// are engine B's HTTP and SOCKS5 listeners.
#[tokio::test]
async fn rurge_talks_to_rurge() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let port = origin.url("/").port().unwrap();
    let b = harness(Profile::default()).await; // everything DIRECT, resolves *.test itself
    let a = harness(Profile {
        proxies: &format!(
            "ViaHttp = http, 127.0.0.1, {}\nViaSocks = socks5, 127.0.0.1, {}",
            b.http().port(),
            b.socks().port()
        ),
        rules: "DOMAIN,target.test,ViaHttp\nDOMAIN,alt.test,ViaSocks",
        ..Profile::default()
    })
    .await;
    // CONNECT through A → CONNECT to B's HTTP listener → origin
    let mut tunnel = connect_via_http(a.http(), &format!("target.test:{port}")).await;
    assert!(get(&mut tunnel, "target.test", "/hello").await.ends_with("hi there"));
    // CONNECT through A → B's SOCKS5 listener → origin
    let mut tunnel = connect_via_http(a.http(), &format!("alt.test:{port}")).await;
    assert!(get(&mut tunnel, "alt.test", "/hello").await.ends_with("hi there"));
    // a plain request: absolute form from A to B, origin form from B to the origin
    let response = plain_get(
        a.http(),
        &format!("http://target.test:{port}/hello"),
        &format!("target.test:{port}"),
    )
    .await;
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(origin.hits("/hello"), 3);
    let log = b.engine.request_log();
    wait_until("B to record three sessions", || log.recent(10).len() == 3).await;
    let via: Vec<ListenerKind> = log.recent(10).iter().map(|r| r.listener).collect();
    assert_eq!(via.iter().filter(|k| **k == ListenerKind::Http).count(), 2);
    assert_eq!(via.iter().filter(|k| **k == ListenerKind::Socks5).count(), 1);
    // only B resolved anything: A handed the names over
    assert!(a.dns.queries().is_empty());
    assert!(!b.dns.queries().is_empty());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-engine --test outbounds a_local_host_item 2>&1 | tail -10`
Expected: FAIL——`flag = true` 时上游收到的仍是 `(3, "pinned.test")`。（`a_chain_enters…`、`a_broken_hop…`、`rurge_talks_to_rurge` 在 Task 5 / 6 之后应当已经能过：它们在这里落地，是为了把设计第 8、9 节点名的场景钉成回归测试。先单独跑它们，哪一个不过就是前面任务的缺陷，停下来报告。）

- [ ] **Step 3: 实现**

`crates/rurge-dns/src/resolver.rs`，`impl Resolver` 里：

```rust
    /// What `[Host]` (or the hosts file) says about `name`, without sending a
    /// query: the engine uses it for `use-local-host-item-for-proxy`.
    pub fn host_lookup(&self, name: &str) -> Option<crate::hosts::HostLookup> {
        self.hosts.lookup(name)
    }
```

`crates/rurge-engine/src/engine.rs` 的 `dial`：把 `let target = Target::new(…);` 改成可变，并在 `let opts = …;` 之后、Task 6 的 `plain_http` 之前加：

```rust
            // FR-DNS-07: a proxy normally gets the name and resolves it itself;
            // with `use-local-host-item-for-proxy`, an address pinned in [Host]
            // goes to the proxy instead. Alias / server items leave the name alone.
            let pinned_ip = match &target.host {
                HostName::Domain(name)
                    if resolution.terminal == rurge_policy::TerminalKind::Proxy
                        && rt.config.general.use_local_host_item_for_proxy =>
                {
                    rt.stack
                        .resolver
                        .host_lookup(name)
                        .and_then(|hit| match hit.action {
                            rurge_dns::hosts::HostAction::Ips(ips) => ips.first().copied(),
                            _ => None,
                        })
                }
                _ => None,
            };
            let pinned = pinned_ip.is_some();
            if let Some(ip) = pinned_ip {
                target = Target::new(HostName::Ip(ip), target.port);
            }
```

并把 Task 6 的 `let forward = if plain_http {` 改成 `let forward = if plain_http && !pinned {`，在它上面补一行注释：`// a pinned address only reaches the proxy through a tunnel: in absolute form the request URI carries the name`。

`use rurge_config::HostName;` 补到文件头。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-dns host_lookup && cargo test -p rurge-engine --test outbounds 2>&1 | tail -15`
Expected: 全部通过。

- [ ] **Step 5: 门禁与提交**

```bash
git add crates/rurge-dns crates/rurge-engine
git commit -m "feat(engine,dns): use-local-host-item-for-proxy（FR-DNS-07）；两级链、链上报错与 rurge → rurge 的端到端用例"
```

---

### Task 8: `select` 组的控制面——视图、切换与持久化

**Files:**
- Modify: `crates/rurge-engine/Cargo.toml`（`sha2.workspace = true`）
- Create: `crates/rurge-engine/src/views.rs`
- Modify: `crates/rurge-engine/src/lib.rs`
- Test: `crates/rurge-engine/tests/outbounds.rs`

**Interfaces:**
- Consumes: Task 1 的 `definition` 字段与 `redact_definition`；Task 5 的 `EngineShared`、`PolicyRegistry::current_member`；现有的 `StateStore::update`、`state::profile_key`、`Engine::attach_state`。
- Produces（都从 `rurge_engine` 根再导出）：

```rust
pub struct MemberView { pub name: String, pub is_group: bool, pub type_description: String, pub line_hash: String }
pub struct GroupView { pub name: String, pub kind: GroupKind, pub hidden: bool, pub members: Vec<MemberView>, pub selected: Option<String> }
pub enum SelectError { UnknownGroup(String), NotSelectable(String), NotAMember { group: String, member: String } }   // Display + Error
impl Engine {
    pub fn groups_view(&self) -> Vec<GroupView>;
    pub fn policy_detail(&self, name: &str) -> Option<String>;
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError>;
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError>;
}
```

  `type_description`：策略是类型关键字（`http`、`ss`…），组是组类型关键字（`select`、`url-test`…），内置策略是它的名字。`line_hash`：`SHA-256("<名字> = <定义原文>")` 的前 16 个十六进制字符；内置策略对名字取哈希（计划期决定 P6）。

- [ ] **Step 1: 写失败的测试**

`crates/rurge-engine/tests/outbounds.rs` 追加（文件头补 `use rurge_engine::state::{STATE_FILE, StateStore, profile_key};` 与 `use rurge_engine::SelectError;`）：

```rust
const PICK: &str = "Pick = select, A, B, DIRECT\nAuto = url-test, A, B, hidden=true";

async fn two_entries(origin: &TestServer) -> (FakeSocks5, FakeSocks5, String) {
    let script = || Socks5Script {
        connect_to: Some(origin_addr(origin)),
        ..Socks5Script::default()
    };
    let (a, b) = (FakeSocks5::spawn(script()).await, FakeSocks5::spawn(script()).await);
    let proxies = format!(
        "A = socks5, 127.0.0.1, {}, alice, s3cret\nB = socks5, 127.0.0.1, {}",
        a.addr().port(),
        b.addr().port()
    );
    (a, b, proxies)
}

#[tokio::test]
async fn a_selection_applies_to_the_next_connection_and_survives_a_restart() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (a, b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: PICK,
        rules: "DOMAIN,target.test,Pick",
        ..Profile::default()
    })
    .await;
    let state_path = h.dir.path().join(STATE_FILE);
    let (store, _) = StateStore::open(state_path.clone()).await;
    h.engine.attach_state(store);

    assert_eq!(h.engine.group_selection("Pick").unwrap(), "A", "the first member by default");
    let mut t = connect_via_http(h.http(), "target.test:80").await;
    assert!(get(&mut t, "target.test", "/hello").await.ends_with("hi there"));
    assert_eq!((a.requests().len(), b.requests().len()), (1, 0));

    h.engine.select_group("Pick", "B").await.unwrap();
    assert_eq!(h.engine.group_selection("Pick").unwrap(), "B");
    let mut t = connect_via_http(h.http(), "target.test:80").await;
    assert!(get(&mut t, "target.test", "/hello").await.ends_with("hi there"));
    assert_eq!((a.requests().len(), b.requests().len()), (1, 1));

    // written under the profile's file name
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).unwrap()).unwrap();
    assert_eq!(saved["group_selections"]["t.conf"]["Pick"], "B");

    // "restart": a fresh engine seeded from state.json, as `rurge run` does
    let text = std::fs::read_to_string(h.dir.path().join("t.conf")).unwrap();
    let (_, state) = StateStore::open(state_path).await;
    let key = profile_key(&h.dir.path().join("t.conf"));
    let shared = EngineShared::new(state.selections_for(&key));
    let restarted = Engine::new(runtime(h.dir.path(), &text, shared).await);
    assert_eq!(restarted.group_selection("Pick").unwrap(), "B");
}

#[tokio::test]
async fn only_a_member_of_a_select_group_can_be_selected() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: PICK,
        ..Profile::default()
    })
    .await;
    assert_eq!(
        h.engine.select_group("Nope", "A").await,
        Err(SelectError::UnknownGroup("Nope".into()))
    );
    assert_eq!(
        h.engine.select_group("A", "B").await,
        Err(SelectError::UnknownGroup("A".into())),
        "a policy is not a group"
    );
    assert_eq!(
        h.engine.select_group("Auto", "A").await,
        Err(SelectError::NotSelectable("Auto".into()))
    );
    assert_eq!(
        h.engine.select_group("Pick", "C").await,
        Err(SelectError::NotAMember {
            group: "Pick".into(),
            member: "C".into()
        })
    );
    assert_eq!(h.engine.group_selection("Pick").unwrap(), "A", "nothing changed");
    assert_eq!(
        h.engine.group_selection("Nope"),
        Err(SelectError::UnknownGroup("Nope".into()))
    );
    assert_eq!(h.engine.group_selection("Auto").unwrap(), "A", "readable for every group kind");
    // the messages are what the API will show
    assert_eq!(SelectError::UnknownGroup("G".into()).to_string(), "unknown policy group `G`");
    assert_eq!(
        SelectError::NotSelectable("G".into()).to_string(),
        "`G` is not a select group"
    );
    assert_eq!(
        SelectError::NotAMember { group: "G".into(), member: "M".into() }.to_string(),
        "`M` is not a member of `G`"
    );
}

#[tokio::test]
async fn views_describe_groups_and_redact_policy_details() {
    let origin = TestServer::spawn().await;
    let (_a, _b, proxies) = two_entries(&origin).await;
    let h = harness(Profile {
        proxies: &proxies,
        groups: PICK,
        ..Profile::default()
    })
    .await;
    let groups = h.engine.groups_view();
    assert_eq!(groups.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Pick", "Auto"]);
    let pick = &groups[0];
    assert_eq!((pick.kind.keyword(), pick.hidden, pick.selected.as_deref()), ("select", false, Some("A")));
    assert!(groups[1].hidden);
    let described: Vec<(&str, bool, &str)> = pick
        .members
        .iter()
        .map(|m| (m.name.as_str(), m.is_group, m.type_description.as_str()))
        .collect();
    assert_eq!(described, [("A", false, "socks5"), ("B", false, "socks5"), ("DIRECT", false, "DIRECT")]);
    for m in &pick.members {
        assert_eq!(m.line_hash.len(), 16, "{}", m.name);
        assert!(m.line_hash.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    assert_ne!(pick.members[0].line_hash, pick.members[1].line_hash);

    let detail = h.engine.policy_detail("A").expect("a configured policy");
    assert!(detail.starts_with("socks5, 127.0.0.1, "), "{detail}");
    assert!(!detail.contains("s3cret") && !detail.contains("alice"), "{detail}");
    assert_eq!(h.engine.policy_detail("direct").as_deref(), Some("DIRECT"));
    assert_eq!(h.engine.policy_detail("Pick").as_deref(), Some("select, A, B, DIRECT"));
    assert_eq!(h.engine.policy_detail("Nope"), None);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-engine --test outbounds a_selection_applies 2>&1 | tail -8`
Expected: 编译失败，`no method named select_group`。

- [ ] **Step 3: 实现**

`crates/rurge-engine/Cargo.toml` 的 `[dependencies]` 加 `sha2.workspace = true`。

创建 `crates/rurge-engine/src/views.rs`：

```rust
//! Policy groups as the control plane sees them, and the one thing it may
//! change: the selection of a `select` group (M1 design 6.3).

use crate::engine::Engine;
use crate::state::profile_key;
use rurge_config::rule::PolicyRef;
use rurge_config::{Config, GroupKind};
use sha2::{Digest, Sha256};
use std::fmt;

pub struct MemberView {
    pub name: String,
    pub is_group: bool,
    /// A policy's type keyword, a group's kind keyword, a built-in's name.
    pub type_description: String,
    /// First 16 hex digits of the SHA-256 of the definition line.
    pub line_hash: String,
}

pub struct GroupView {
    pub name: String,
    pub kind: GroupKind,
    pub hidden: bool,
    /// In profile order.
    pub members: Vec<MemberView>,
    /// The member the group points at right now.
    pub selected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectError {
    UnknownGroup(String),
    NotSelectable(String),
    NotAMember { group: String, member: String },
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectError::UnknownGroup(g) => write!(f, "unknown policy group `{g}`"),
            SelectError::NotSelectable(g) => write!(f, "`{g}` is not a select group"),
            SelectError::NotAMember { group, member } => {
                write!(f, "`{member}` is not a member of `{group}`")
            }
        }
    }
}

impl std::error::Error for SelectError {}

fn line_hash(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn member_view(cfg: &Config, name: &str) -> MemberView {
    if let Some(p) = cfg.policies.iter().find(|p| p.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: false,
            type_description: p.kind.keyword().to_string(),
            line_hash: line_hash(&format!("{} = {}", p.name, p.definition)),
        };
    }
    if let Some(g) = cfg.groups.iter().find(|g| g.name == name) {
        return MemberView {
            name: name.to_string(),
            is_group: true,
            type_description: g.kind.keyword().to_string(),
            line_hash: line_hash(&format!("{} = {}", g.name, g.definition)),
        };
    }
    // a built-in (or a DEVICE: reference): nothing but its name describes it
    let shown = match PolicyRef::parse(name) {
        PolicyRef::Builtin(b) => b.name().to_string(),
        _ => name.to_string(),
    };
    MemberView {
        name: name.to_string(),
        is_group: false,
        line_hash: line_hash(&shown),
        type_description: shown,
    }
}

impl Engine {
    pub fn groups_view(&self) -> Vec<GroupView> {
        let rt = self.runtime();
        rt.config
            .groups
            .iter()
            .map(|g| GroupView {
                name: g.name.clone(),
                kind: g.kind,
                hidden: g
                    .params
                    .get("hidden")
                    .is_some_and(|v| v.eq_ignore_ascii_case("true")),
                members: g.members.iter().map(|m| member_view(&rt.config, m)).collect(),
                selected: rt.policies.current_member(&g.name),
            })
            .collect()
    }

    /// The definition of a policy or group with its secrets blanked; a
    /// built-in is described by its own name.
    pub fn policy_detail(&self, name: &str) -> Option<String> {
        let rt = self.runtime();
        if let Some(p) = rt.config.policies.iter().find(|p| p.name == name) {
            return Some(rurge_config::redact::redact_definition(&p.definition));
        }
        if let Some(g) = rt.config.groups.iter().find(|g| g.name == name) {
            return Some(rurge_config::redact::redact_definition(&g.definition));
        }
        match PolicyRef::parse(name) {
            PolicyRef::Builtin(b) => Some(b.name().to_string()),
            _ => None,
        }
    }

    /// The member `group` points at right now (any kind of group).
    pub fn group_selection(&self, group: &str) -> Result<String, SelectError> {
        let rt = self.runtime();
        if !rt.config.groups.iter().any(|g| g.name == group) {
            return Err(SelectError::UnknownGroup(group.to_string()));
        }
        Ok(rt.policies.current_member(group).unwrap_or_default())
    }

    /// Takes effect for the next connection and is written to `state.json`
    /// under the profile's file name.
    pub async fn select_group(&self, group: &str, member: &str) -> Result<(), SelectError> {
        let rt = self.runtime();
        let Some(g) = rt.config.groups.iter().find(|g| g.name == group) else {
            return Err(SelectError::UnknownGroup(group.to_string()));
        };
        if g.kind != GroupKind::Select {
            return Err(SelectError::NotSelectable(group.to_string()));
        }
        if !g.members.iter().any(|m| m == member) {
            return Err(SelectError::NotAMember {
                group: group.to_string(),
                member: member.to_string(),
            });
        }
        self.shared().selections.set(group, member);
        if let Some(store) = self.state_store() {
            let profile = profile_key(&rt.config.source.main);
            let (group, member) = (group.to_string(), member.to_string());
            store
                .update(move |s| {
                    s.group_selections
                        .entry(profile)
                        .or_default()
                        .insert(group, member);
                })
                .await;
        }
        Ok(())
    }
}
```

`crates/rurge-engine/src/engine.rs` 加一个给兄弟模块用的取值方法（`state` 字段是私有的）：

```rust
    /// The attached state store, if any (`attach_state`).
    pub(crate) fn state_store(&self) -> Option<&Arc<StateStore>> {
        self.state.get()
    }
```

`crates/rurge-engine/src/lib.rs`：`pub mod views;` 与 `pub use views::{GroupView, MemberView, SelectError};`。

`Builtin::name()`、`PolicyRef::parse` 对大小写的处理以现有实现为准：`policy_detail("direct")` 期望得到规范名 `DIRECT`；若 `PolicyRef::parse` 不接受小写，测试里改用 `"DIRECT"`，不要为此改解析器。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-engine 2>&1 | tail -15`
Expected: 全部通过。

- [ ] **Step 5: 门禁与提交**

```bash
git add crates/rurge-engine Cargo.lock
git commit -m "feat(engine): select 组的视图、切换与持久化（groups_view / policy_detail / group_selection / select_group）"
```

---

### Task 9: API——四个策略 / 组端点，`profiles/check` 带干构建

**Files:**
- Create: `crates/rurge-api/src/routes/policy_groups.rs`
- Modify: `crates/rurge-api/src/routes/mod.rs`、`crates/rurge-api/src/lib.rs`（路由）、`crates/rurge-api/src/routes/profiles.rs`（`check`）
- Test: `crates/rurge-api/tests/api.rs`

**Interfaces:**
- Consumes: Task 8 的 `Engine::{groups_view, policy_detail, group_selection, select_group}`、`SelectError`；Task 4 的 `rurge_engine::load_checked`；现有的 `ApiError` / `ApiResult` / `json_body` / `query_params`。
- Produces（形状登记在 Task 12 的 `docs/api/phase2.md`；前两个标"暂定"）：

| 端点 | 成功 | 失败 |
| ---- | ---- | ---- |
| `GET /v1/policies/detail?policy_name=X` | `{"X": "<脱敏后的定义>"}`；内置策略的值是它的规范名 | 未知 → 404；缺参数 → 400 |
| `GET /v1/policy_groups` | `{"<组名>": [{"name", "typeDescription", "isGroup", "enabled": true, "lineHash"}, …], …}`，成员顺序同配置 | — |
| `GET /v1/policy_groups/select?group_name=G` | `{"policy": "<当前生效的成员>"}` | 未知组 → 404；缺参数 → 400 |
| `POST /v1/policy_groups/select`，请求体 `{"group_name", "policy"}` | `{}` | 组或成员无效、不是 `select` 组、请求体不合法 → 400 |

- [ ] **Step 1: 写失败的测试**

`crates/rurge-api/tests/api.rs` 末尾追加：

```rust
#[tokio::test]
async fn policy_groups_are_listed_and_a_policy_detail_is_redacted() {
    let api = api().await;
    let (status, body) = get(&api, "/v1/policy_groups").await;
    assert_eq!(status, 200);
    let members = body["Pick"].as_array().expect("the group's members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["name"], "HK");
    assert_eq!(members[0]["typeDescription"], "ss");
    assert_eq!(members[0]["isGroup"], false);
    assert_eq!(members[0]["enabled"], true);
    let hash = members[0]["lineHash"].as_str().unwrap();
    assert!(hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()), "{hash}");
    assert_eq!(members[1]["name"], "DIRECT");
    assert_eq!(members[1]["typeDescription"], "DIRECT");

    let (status, body) = get(&api, "/v1/policies/detail?policy_name=HK").await;
    assert_eq!(status, 200);
    let detail = body["HK"].as_str().expect("the definition");
    assert!(detail.starts_with("ss, 1.2.3.4, 8388"), "{detail}");
    assert!(detail.contains("***") && !detail.contains("password=x"), "{detail}");
    let (status, body) = get(&api, "/v1/policies/detail?policy_name=DIRECT").await;
    assert_eq!((status, body), (200, json!({ "DIRECT": "DIRECT" })));
    let (status, body) = get(&api, "/v1/policies/detail?policy_name=Nope").await;
    assert_eq!((status, body), (404, json!({ "error": "unknown policy `Nope`" })));
    let (status, _) = get(&api, "/v1/policies/detail").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn a_select_group_is_switched_for_the_next_request_and_persisted() {
    let api = api_with("DOMAIN,target.test,Pick").await;
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Pick").await,
        (200, json!({ "policy": "HK" }))
    );
    // HK is a protocol rurge does not speak yet: the request is refused
    let (head, _) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(!head.starts_with("HTTP/1.1 200"), "{head}");

    let (status, body) = post(
        &api,
        "/v1/policy_groups/select",
        json!({ "group_name": "Pick", "policy": "DIRECT" }),
    )
    .await;
    assert_eq!((status, body), (200, json!({})));
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Pick").await,
        (200, json!({ "policy": "DIRECT" }))
    );
    let (head, body) = get_via_proxy(api.http(), &api.target_url()).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hi there");

    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&api.state_path).unwrap()).unwrap();
    assert_eq!(saved["group_selections"]["t.conf"]["Pick"], "DIRECT");
}

#[tokio::test]
async fn select_refuses_what_is_not_a_member_of_a_select_group() {
    let api = api().await;
    for (body, message) in [
        (json!({ "group_name": "Nope", "policy": "DIRECT" }), "unknown policy group `Nope`"),
        (json!({ "group_name": "HK", "policy": "DIRECT" }), "unknown policy group `HK`"),
        (json!({ "group_name": "Pick", "policy": "Block" }), "`Block` is not a member of `Pick`"),
    ] {
        let (status, answer) = post(&api, "/v1/policy_groups/select", body).await;
        assert_eq!((status, answer), (400, json!({ "error": message })));
    }
    let (status, _) = post(&api, "/v1/policy_groups/select", json!({ "group_name": "Pick" })).await;
    assert_eq!(status, 400, "a body without `policy`");
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Nope").await,
        (404, json!({ "error": "unknown policy group `Nope`" }))
    );
    let (status, _) = get(&api, "/v1/policy_groups/select").await;
    assert_eq!(status, 400);
    assert_eq!(
        get(&api, "/v1/policy_groups/select?group_name=Pick").await,
        (200, json!({ "policy": "HK" })),
        "nothing changed"
    );
}

#[tokio::test]
async fn profile_check_includes_the_dry_build() {
    let api = api().await;
    let text = std::fs::read_to_string(&api.conf).unwrap().replace(
        "[Proxy Group]",
        "Up = https, proxy.test, 443, client-cert=cert1\n[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Proxy Group]",
    );
    std::fs::write(&api.conf, text).unwrap();
    let (status, body) = post(&api, "/v1/profiles/check", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(body["ok"], false);
    let codes: Vec<&str> = body["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|d| d["code"].as_str())
        .collect();
    assert!(codes.contains(&"E0022"), "{codes:?}");
    assert!(!body.to_string().contains("hunter2"));
}
```

（`get` / `post` / `get_via_proxy` / `api_with` 是该文件已有的辅助函数；`get_via_proxy` 返回 `(响应头文本, 响应体字节)`。`POST /v1/profiles/check` 现有用例怎么传请求体就怎么传。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-api policy_groups 2>&1 | tail -10`
Expected: FAIL——`/v1/policy_groups` 返回 404（`not_found` 兜底路由）。

- [ ] **Step 3: 实现**

创建 `crates/rurge-api/src/routes/policy_groups.rs`：

```rust
//! `GET /v1/policies/detail`, `GET /v1/policy_groups` and
//! `GET/POST /v1/policy_groups/select` (M1 design 6.6). The first two have no
//! response sample in the manual: their shapes are provisional.

use crate::App;
use crate::error::{ApiError, ApiResult, json_body, query_params};
use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

#[derive(Deserialize)]
pub struct DetailQuery {
    pub policy_name: String,
}

pub async fn detail(
    State(app): State<App>,
    query: Result<Query<DetailQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let query = query_params(query)?;
    let detail = app
        .engine
        .policy_detail(&query.policy_name)
        .ok_or_else(|| ApiError::not_found(format!("unknown policy `{}`", query.policy_name)))?;
    let mut body = Map::new();
    body.insert(query.policy_name, Value::String(detail));
    Ok(Json(Value::Object(body)))
}

#[derive(Serialize)]
struct MemberJson {
    name: String,
    #[serde(rename = "typeDescription")]
    type_description: String,
    #[serde(rename = "isGroup")]
    is_group: bool,
    /// Always `true`: rurge has no way to disable a member.
    enabled: bool,
    #[serde(rename = "lineHash")]
    line_hash: String,
}

pub async fn groups(State(app): State<App>) -> Json<Value> {
    let mut body = Map::new();
    for group in app.engine.groups_view() {
        let members: Vec<MemberJson> = group
            .members
            .into_iter()
            .map(|m| MemberJson {
                name: m.name,
                type_description: m.type_description,
                is_group: m.is_group,
                enabled: true,
                line_hash: m.line_hash,
            })
            .collect();
        body.insert(group.name, json!(members));
    }
    Json(Value::Object(body))
}

#[derive(Deserialize)]
pub struct SelectionQuery {
    pub group_name: String,
}

pub async fn selection(
    State(app): State<App>,
    query: Result<Query<SelectionQuery>, QueryRejection>,
) -> ApiResult<Json<Value>> {
    let query = query_params(query)?;
    let policy = app
        .engine
        .group_selection(&query.group_name)
        .map_err(|e| ApiError::not_found(e.to_string()))?;
    Ok(Json(json!({ "policy": policy })))
}

#[derive(Deserialize)]
pub struct Select {
    pub group_name: String,
    pub policy: String,
}

pub async fn select(
    State(app): State<App>,
    body: Result<Json<Select>, JsonRejection>,
) -> ApiResult<Json<Value>> {
    let body = json_body(body)?;
    app.engine
        .select_group(&body.group_name, &body.policy)
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    tracing::info!(group = %body.group_name, policy = %body.policy, "policy group selection changed via http-api");
    Ok(Json(json!({})))
}
```

`crates/rurge-api/src/routes/mod.rs`：加 `pub mod policy_groups;`。

`crates/rurge-api/src/lib.rs` 的路由表，在 `/v1/policies` 那一行后面加：

```rust
        .route("/v1/policies/detail", get(routes::policy_groups::detail))
        .route("/v1/policy_groups", get(routes::policy_groups::groups))
        .route(
            "/v1/policy_groups/select",
            get(routes::policy_groups::selection).post(routes::policy_groups::select),
        )
```

`crates/rurge-api/src/routes/profiles.rs` 的 `check`：把 `load(&path, &opts)` 换成 `rurge_engine::load_checked(&path, &opts)`（对应的 `use` 一并调整），并把函数上方的文档注释补一句 `The dry build runs too, so a policy that cannot be built is reported here exactly as a reload would report it.`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-api 2>&1 | tail -12`
Expected: 全部通过（原有用例 + 4 个新用例）。

- [ ] **Step 5: 门禁与提交**

```bash
git add crates/rurge-api
git commit -m "feat(api): /v1/policies/detail、/v1/policy_groups、GET / POST /v1/policy_groups/select；profiles/check 带干构建"
```

---

### Task 10: bin——平台 socket 适配器、干构建接线、能力表翻转

**Files:**
- Modify: `crates/rurge-platform/src/socket.rs`（网卡表缓存）
- Modify: `crates/rurge/Cargo.toml`（`socket2.workspace = true`；dev 依赖 `rurge-proto`（`testing`））
- Modify: `crates/rurge/src/cli/runtime.rs`（`PlatformSockets`、`stack_options`）
- Modify: `crates/rurge/src/capabilities.rs`
- Modify: `crates/rurge/src/cli/check.rs`、`crates/rurge/src/cli/run.rs`（`load_checked`）
- Test: `crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: M1a 的 `rurge_platform::socket::{bind_interface, set_tos, Family}`、`rurge_net::socket::{SocketHook, Family}`；Task 4 的 `load_checked`；Task 5 的 `StackOptions.socket_hook`。
- Produces:
  - `rurge_platform::socket` 内部的 `Cached<T>`（5 秒 TTL），macOS / Windows 的 `bind_interface` 经它取网卡表。
  - bin 的 `PlatformSockets`（`impl SocketHook`），经 `stack_options()` 注入——从这里起，`interface` / `allow-other-interface` / `tos` 在 `rurge run` 里真正生效。
  - `capabilities::current()` 含 `Http` `Https` `Socks5` `Socks5Tls`：这四种不再报 `W0007`。
  - `rurge check`、`rurge run` 的启动与重载都经 `load_checked`：构建不出来的策略 = `E0022` = `check` 退出 2、`run` 退出 2、重载保留旧一代。

- [ ] **Step 1: 写失败的测试**

`crates/rurge-platform/src/socket.rs` 的 `mod tests` 末尾：

```rust
    #[test]
    fn the_interface_table_is_cached_for_a_while() {
        use std::cell::Cell;
        let loads = Cell::new(0u32);
        let load = || -> io::Result<u32> {
            loads.set(loads.get() + 1);
            Ok(loads.get())
        };
        let cache = Cached::new(std::time::Duration::from_secs(60));
        assert_eq!(*cache.get(load).unwrap(), 1);
        assert_eq!(*cache.get(load).unwrap(), 1, "served from the cache");
        assert_eq!(loads.get(), 1);

        let never = Cached::new(std::time::Duration::ZERO);
        assert_eq!(*never.get(load).unwrap(), 2);
        assert_eq!(*never.get(load).unwrap(), 3, "a zero TTL always reloads");

        // a failure is reported, not remembered
        let failing: Cached<u32> = Cached::new(std::time::Duration::from_secs(60));
        assert!(failing.get(|| Err(io::Error::other("no table"))).is_err());
        assert_eq!(*failing.get(|| Ok(7)).unwrap(), 7);
    }
```

`crates/rurge/src/cli/runtime.rs` 末尾加测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::socket::{Family, SocketHook};

    /// Socket-level only: nothing about this machine's network is changed.
    #[test]
    fn platform_sockets_delegates_to_rurge_platform() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        let hook = PlatformSockets;
        assert!(hook.bind_interface(&socket, "rurge-no-such-if0", Family::V4).is_err());
        hook.set_tos(&socket, Family::V4, 0x28).unwrap();
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
    }
}
```

`crates/rurge/tests/cli.rs`——顶层（`check` 的用例旁边）加：

```rust
const PROXIES: &str = "[General]\n[Proxy]\nH = http, proxy.test, 8080\nS = socks5-tls, proxy.test, 443\nOld = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
const BROKEN_P12: &str = "[General]\n[Proxy]\nUp = https, proxy.test, 443, client-cert=cert1\n[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n";

#[test]
fn check_knows_the_m1_protocols_and_runs_the_dry_build() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "proxies.conf", PROXIES))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // only the protocol of a later milestone is still "not implemented"
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("Old"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "broken.conf", BROKEN_P12))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0022"))
        .stdout(predicate::str::contains("broken.conf:3"))
        .stdout(predicate::str::contains("hunter2").not());
}
```

`mod run` 里（`run_exits_2_on_a_broken_profile` 后面）加：

```rust
    #[test]
    fn run_refuses_a_policy_that_cannot_be_built() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.conf"), super::BROKEN_P12).unwrap();
        let output = rurge_run(&dir.path().join("t.conf"), &dir.path().join("data"))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("E0022"), "{text}");
    }

    /// M1 design 6.4: a reload whose profile holds a policy that cannot be
    /// built keeps the running generation.
    #[test]
    fn a_reload_with_an_unbuildable_policy_keeps_the_current_config() {
        let dir = tempfile::tempdir().unwrap();
        let conf = write_conf(dir.path(), API_GENERAL);
        let daemon = spawn_daemon(&conf, &dir.path().join("data"));
        let port = api_port(&daemon);
        let good = std::fs::read_to_string(&conf).unwrap();
        let broken = good.replace(
            "[Proxy]\n",
            "[Proxy]\nUp = https, proxy.test, 443, client-cert=cert1\n[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n",
        );
        assert_ne!(good, broken);
        std::fs::write(&conf, broken).unwrap();
        let (status, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"ok\":false"), "{body}");
        // the daemon is still up and takes the repaired profile
        std::fs::write(&conf, good).unwrap();
        let (_, body) = api_call(port, "POST", "/v1/profiles/reload", "k", Some("{}"));
        assert!(body.contains("\"ok\":true"), "{body}");
    }

    /// The whole way: `rurge run` → HTTP listener → a real `http` policy in
    /// forward mode → a scripted loopback upstream.
    #[tokio::test]
    async fn run_routes_through_an_http_upstream() {
        use rurge_proto::testing::{FakeHttpProxy, HttpProxyScript};
        let upstream = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let dir = tempfile::tempdir().unwrap();
        let conf = dir.path().join("t.conf");
        std::fs::write(
            &conf,
            format!(
                "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\nloglevel = warning\n\
[Proxy]\nUp = http, 127.0.0.1, {}\n[Rule]\nDOMAIN,via.test,Up\nFINAL,DIRECT\n",
                upstream.addr().port()
            ),
        )
        .unwrap();
        let daemon = tokio::task::spawn_blocking({
            let (conf, data) = (conf.clone(), dir.path().join("data"));
            move || spawn_daemon(&conf, &data)
        })
        .await
        .unwrap();
        let port = daemon.http;
        let answer = tokio::task::spawn_blocking(move || http_get(port, "http://via.test/hello"))
            .await
            .unwrap();
        assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("forwarded"), "{answer}");
        assert_eq!(upstream.heads()[0].request_line, "GET http://via.test/hello HTTP/1.1");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-platform the_interface_table 2>&1 | tail -6`
Expected: 编译失败，`cannot find type Cached`。

Run: `cargo test -p rurge --test cli check_knows 2>&1 | tail -12`
Expected: FAIL——`W0007` 出现 3 次（能力表还没翻转），`broken.conf` 退出码是 0。

- [ ] **Step 3: 实现——`rurge-platform`**

`crates/rurge-platform/src/socket.rs`：

```rust
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A value worth keeping for a short while: the interface table costs a
/// system call in the millisecond range, and `bind_interface` runs once per
/// connection attempt.
#[cfg(any(target_os = "macos", windows, test))]
struct Cached<T> {
    ttl: Duration,
    slot: Mutex<Option<(Instant, Arc<T>)>>,
}

#[cfg(any(target_os = "macos", windows, test))]
impl<T> Cached<T> {
    const fn new(ttl: Duration) -> Cached<T> {
        Cached {
            ttl,
            slot: Mutex::new(None),
        }
    }

    /// The cached value while it is fresh, else whatever `load` returns — a
    /// failure is returned and not remembered.
    fn get(&self, load: impl FnOnce() -> io::Result<T>) -> io::Result<Arc<T>> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, value)) = slot.as_ref()
            && at.elapsed() < self.ttl
        {
            return Ok(value.clone());
        }
        let value = Arc::new(load()?);
        *slot = Some((Instant::now(), value.clone()));
        Ok(value)
    }
}

/// An interface that appears or changes its address is seen within this long.
#[cfg(any(target_os = "macos", windows))]
const INTERFACE_TABLE_TTL: Duration = Duration::from_secs(5);

#[cfg(any(target_os = "macos", windows))]
fn interfaces() -> io::Result<Arc<Vec<if_addrs::Interface>>> {
    static TABLE: Cached<Vec<if_addrs::Interface>> = Cached::new(INTERFACE_TABLE_TTL);
    TABLE.get(if_addrs::get_if_addrs)
}
```

macOS 与 Windows 的 `bind_interface` 里，把 `if_addrs::get_if_addrs()?` 换成 `interfaces()?`，并把随后的 `.into_iter()` 改成 `.iter()`（`Arc<Vec<_>>` 不能按值迭代；`find` / `map` 的闭包相应改成按引用取字段，`i.name.clone()`、`i.ip()` 不变）。`use` 的 cfg 条件要让 Linux 上没有未使用的导入：把上面的两行 `use` 也放进 `#[cfg(any(target_os = "macos", windows, test))]`。Linux 分支不动。本机只能编译 Windows 分支，macOS 分支照 Windows 分支的改法写，并在报告里说明未编译。

- [ ] **Step 4: 实现——bin**

`crates/rurge/Cargo.toml`：`[dependencies]` 加 `socket2.workspace = true`；`[dev-dependencies]` 加 `rurge-proto = { workspace = true, features = ["testing"] }`。

`crates/rurge/src/cli/runtime.rs`（`PlatformSystemDns` 后面）：

```rust
/// `rurge-platform::socket` behind the `SocketHook` trait (AR-02: platform
/// code stays in rurge-platform; rurge-net only sees the trait).
pub struct PlatformSockets;

fn platform_family(family: rurge_net::socket::Family) -> rurge_platform::socket::Family {
    match family {
        rurge_net::socket::Family::V4 => rurge_platform::socket::Family::V4,
        rurge_net::socket::Family::V6 => rurge_platform::socket::Family::V6,
    }
}

impl rurge_net::socket::SocketHook for PlatformSockets {
    fn bind_interface(
        &self,
        socket: &socket2::Socket,
        interface: &str,
        family: rurge_net::socket::Family,
    ) -> std::io::Result<()> {
        rurge_platform::socket::bind_interface(socket, interface, platform_family(family))
    }

    fn set_tos(
        &self,
        socket: &socket2::Socket,
        family: rurge_net::socket::Family,
        tos: u8,
    ) -> std::io::Result<()> {
        rurge_platform::socket::set_tos(socket, platform_family(family), tos)
    }
}
```

`stack_options()` 里把 Task 5 放的 `socket_hook: Arc::new(rurge_net::socket::NoopSocketHook),` 换成 `socket_hook: Arc::new(PlatformSockets),`。

`crates/rurge/src/capabilities.rs`：`policy_kinds` 加四项，并更新文件头注释：

```rust
//! What this build of rurge actually implements: the built-in alias
//! policies, the HTTP / SOCKS5 proxy family (phase 2 M1) and `select` groups.
```

```rust
        policy_kinds: HashSet::from([
            PolicyKind::Direct,
            PolicyKind::Reject,
            PolicyKind::RejectDrop,
            PolicyKind::RejectNoDrop,
            PolicyKind::RejectTinyGif,
            PolicyKind::Http,
            PolicyKind::Https,
            PolicyKind::Socks5,
            PolicyKind::Socks5Tls,
        ]),
```

`crates/rurge/src/cli/check.rs`：`let loaded = load(&args.config, &opts)?;` → `let loaded = rurge_engine::load_checked(&args.config, &opts)?;`。

`crates/rurge/src/cli/run.rs`：`run()` 开头的 `load(&args.config, &opts)?` 与 `reload()` 里的 `load(d.config, d.load_opts)` 都换成 `rurge_engine::load_checked(..)`。两处随后的"有错误就退出 2 / 保留旧一代"逻辑不用改——干构建的错误已经在诊断里。`use` 相应调整。

其它测试夹具里若写死了"`http` / `socks5` 会产生 `W0007`"的假设，按新的能力表改（`grep -rn "W0007" crates/*/tests crates/rurge/tests`）；`tests/pipeline.rs` 与 `tests/api.rs` 用的是去掉 `Shadowsocks` 的测试能力表，不受影响。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-platform && cargo test -p rurge 2>&1 | tail -20`
Expected: 全部通过。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates Cargo.lock
git commit -m "feat(cli,platform): PlatformSockets 注入、网卡表缓存、check / run / reload 走干构建、能力表加入 http / https / socks5 / socks5-tls"
```

---

### Task 11: 对 sing-box 的互操作夹具、用例与 CI

**本机没有安装 sing-box**（写计划时已查过 `PATH`），所以本任务的用例在本机会打印说明并跳过；它们第一次真正运行是在首次推送后的 CI 上。这一点要如实写进报告：夹具自身的单元测试（渲染、端口、定位二进制）在本机必须通过，六个互操作用例在本机只验证"缺二进制时跳过、`RURGE_INTEROP_REQUIRED=1` 时失败"。**不要为了让用例跑起来而下载或安装 sing-box**——装不装由项目所有者决定。

**Files:**
- Modify: `Cargo.toml`（`members = ["crates/*", "tests/interop"]`）
- Create: `tests/interop/Cargo.toml`、`tests/interop/src/lib.rs`、`tests/interop/tests/sing_box.rs`、`tests/interop/README.md`
- Modify: `crates/rurge-proto/src/testing/tls.rs`（PEM 与 p12 的取值方法）
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: Task 4 的 `EngineFactory::{new, with_roots}`；`rurge_proto::testing::{TlsFixture, echo_server}`；`rurge_net::testing::TestServer`。
- Produces:
  - `TlsFixture::{ca_pem, leaf_pem, leaf_key_pem}() -> String`、`TlsFixture::client_p12_base64(common_name: &str) -> String`（密码固定为 `pw`）。
  - `rurge_interop::{locate, sing_box_or_skip, free_port, render, Inbound, InboundKind, TlsFiles, SingBox}`。
  - 环境变量：`RURGE_TEST_SING_BOX`（二进制路径，优先于 `PATH`）、`RURGE_INTEROP_REQUIRED=1`（缺二进制即失败）。

安全约束（Global Constraints 的具体化）：渲染出的配置只有 `log` / `inbounds` / `outbounds` 三个顶层键；入站只监听 `127.0.0.1`；出站只有一个 `direct`；**所有用例的目标都是回环的 IP 字面量**，sing-box 因此既不解析域名也不访问外网；任何地方都不出现 `set_system_proxy`、`tun`、`auto_route`。

- [ ] **Step 1: `TlsFixture` 的取值方法（先写测试）**

`crates/rurge-proto/src/testing/tls.rs` 没有自己的测试模块就新建一个；加：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pem_and_p12_exports_are_well_formed() {
        let fixture = TlsFixture::new(&["localhost", "127.0.0.1"]);
        for (pem, label) in [
            (fixture.ca_pem(), "CERTIFICATE"),
            (fixture.leaf_pem(), "CERTIFICATE"),
            (fixture.leaf_key_pem(), "PRIVATE KEY"),
        ] {
            assert!(pem.starts_with(&format!("-----BEGIN {label}-----\n")), "{pem}");
            assert!(pem.ends_with(&format!("-----END {label}-----\n")), "{pem}");
            assert!(pem.lines().all(|l| l.len() <= 64), "lines are wrapped at 64");
        }
        let item = rurge_config::KeystoreItem {
            name: "mtls".into(),
            kind: rurge_config::KeystoreType::P12,
            base64: fixture.client_p12_base64("interop client"),
            password: Some("pw".into()),
            unknown: Vec::new(),
            span: rurge_config::Span::new(std::sync::Arc::from(std::path::Path::new("t.conf")), 1),
        };
        crate::keystore::decode_p12(&item).expect("our own p12 decodes");
    }
}
```

实现（`impl TlsFixture` 里；`base64` 与 `p12-keystore` 已是本 crate 的正式依赖）：

```rust
    pub fn ca_pem(&self) -> String {
        pem("CERTIFICATE", self.ca.as_ref())
    }

    pub fn leaf_pem(&self) -> String {
        pem("CERTIFICATE", self.leaf.as_ref())
    }

    /// PKCS#8.
    pub fn leaf_key_pem(&self) -> String {
        pem("PRIVATE KEY", &self.leaf_key)
    }

    /// A client certificate signed by the fixture's CA as a Base64 PKCS#12
    /// with the password `pw`: the value of a `[Keystore]` item.
    pub fn client_p12_base64(&self, common_name: &str) -> String {
        use p12_keystore::{Certificate, KeyStore, KeyStoreEntry, PrivateKeyChain};
        let (cert, key) = self.issue_client(common_name);
        let chain = PrivateKeyChain::new(
            key,
            [1u8, 2, 3, 4],
            [Certificate::from_der(&cert).expect("our own certificate")],
        );
        let mut store = KeyStore::new();
        store.add_entry("client", KeyStoreEntry::PrivateKeyChain(chain));
        let der = store.writer("pw").write().expect("p12");
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der)
    }
```

文件级的自由函数：

```rust
/// RFC 7468 text for a DER blob.
fn pem(label: &str, der: &[u8]) -> String {
    let body = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, der);
    let mut out = format!("-----BEGIN {label}-----\n");
    for line in body.as_bytes().chunks(64) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push('\n');
    }
    out.push_str(&format!("-----END {label}-----\n"));
    out
}
```

（`p12-keystore` 0.2.1 的写法照抄 `crates/rurge-proto/src/keystore.rs` 测试里的 `p12_for`；`writer("pw").write()` 用库的默认算法，若默认算法 `decode_p12` 读不了，就像 `p12_for` 那样显式指定 `EncryptionAlgorithm::PbeWithHmacSha256AndAes256` 与 `MacAlgorithm::HmacSha256`。）

Run: `cargo test -p rurge-proto pem_and_p12 2>&1 | tail -6` → PASS。

- [ ] **Step 2: 夹具 crate（先写它自己的测试）**

根 `Cargo.toml`：`members = ["crates/*", "tests/interop"]`。

`tests/interop/Cargo.toml`：

```toml
[package]
name = "rurge-interop"
description = "Interoperability tests against reference proxy implementations (test-only, never published)"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[dependencies]
serde_json.workspace = true

[dev-dependencies]
rurge-config.workspace = true
rurge-engine.workspace = true
rurge-net = { workspace = true, features = ["testing"] }
rurge-policy.workspace = true
rurge-proto = { workspace = true, features = ["testing"] }
tempfile.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

`tests/interop/src/lib.rs`：

```rust
//! A sing-box child process on the loopback, for interoperability tests
//! (M1 design §8). Nothing here downloads, installs or configures anything
//! outside a temporary directory: the rendered configuration listens on
//! 127.0.0.1 only, its single outbound is `direct`, and it never holds a key
//! that touches the machine (`set_system_proxy`, `tun`, `auto_route`).

use serde_json::{Value, json};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const BINARY_ENV: &str = "RURGE_TEST_SING_BOX";
pub const REQUIRED_ENV: &str = "RURGE_INTEROP_REQUIRED";
const READY_TIMEOUT: Duration = Duration::from_secs(15);

/// `RURGE_TEST_SING_BOX`, else the first `sing-box` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) { "sing-box.exe" } else { "sing-box" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure instead.
pub fn sing_box_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if std::env::var(REQUIRED_ENV).as_deref() == Ok("1") {
        panic!("{REQUIRED_ENV}=1 but no sing-box was found ({BINARY_ENV} or PATH)");
    }
    eprintln!("skipping {test}: no sing-box ({BINARY_ENV} or PATH); see tests/interop/README.md");
    None
}

/// A port that was free a moment ago.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("a free loopback port")
        .port()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InboundKind {
    Http,
    Socks,
    Mixed,
}

pub struct TlsFiles {
    /// PEM paths.
    pub certificate: PathBuf,
    pub key: PathBuf,
    /// Require a client certificate signed by this CA (PEM path).
    pub client_ca: Option<PathBuf>,
}

pub struct Inbound {
    pub kind: InboundKind,
    /// Empty = no authentication.
    pub users: Vec<(String, String)>,
    /// Only the `http` inbound of sing-box speaks TLS.
    pub tls: Option<TlsFiles>,
}

/// The whole configuration for `inbounds`, each on its loopback port.
pub fn render(inbounds: &[(Inbound, u16)]) -> Value {
    let rendered: Vec<Value> = inbounds
        .iter()
        .enumerate()
        .map(|(i, (inbound, port))| {
            let mut v = json!({
                "type": match inbound.kind {
                    InboundKind::Http => "http",
                    InboundKind::Socks => "socks",
                    InboundKind::Mixed => "mixed",
                },
                "tag": format!("in-{i}"),
                "listen": "127.0.0.1",
                "listen_port": port,
            });
            if !inbound.users.is_empty() {
                v["users"] = inbound
                    .users
                    .iter()
                    .map(|(u, p)| json!({ "username": u, "password": p }))
                    .collect();
            }
            if let Some(tls) = &inbound.tls {
                assert_eq!(inbound.kind, InboundKind::Http, "only sing-box's http inbound has tls");
                let mut t = json!({
                    "enabled": true,
                    "certificate_path": tls.certificate,
                    "key_path": tls.key,
                });
                if let Some(ca) = &tls.client_ca {
                    t["client_authentication"] = json!("require-and-verify");
                    t["client_certificate_path"] = json!([ca]);
                }
                v["tls"] = t;
            }
            v
        })
        .collect();
    json!({
        "log": { "level": "warn", "timestamp": false },
        "inbounds": rendered,
        "outbounds": [{ "type": "direct", "tag": "direct" }],
    })
}

/// A running sing-box; killed and reaped on drop.
pub struct SingBox {
    child: Child,
    ports: Vec<u16>,
    log: PathBuf,
}

impl SingBox {
    /// Writes the configuration into `dir`, starts `binary` there and waits
    /// until every inbound accepts connections.
    pub fn spawn(binary: &Path, dir: &Path, inbounds: Vec<Inbound>) -> SingBox {
        let with_ports: Vec<(Inbound, u16)> =
            inbounds.into_iter().map(|i| (i, free_port())).collect();
        let ports: Vec<u16> = with_ports.iter().map(|(_, p)| *p).collect();
        let config = dir.join("sing-box.json");
        std::fs::write(&config, render(&with_ports).to_string()).expect("write the config");
        let log = dir.join("sing-box.log");
        let out = std::fs::File::create(&log).expect("create the log");
        let child = Command::new(binary)
            .arg("run")
            .arg("-c")
            .arg(&config)
            .arg("-D")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(out.try_clone().expect("clone the log handle"))
            .stderr(out)
            .spawn()
            .unwrap_or_else(|e| panic!("cannot start {}: {e}", binary.display()));
        let mut running = SingBox { child, ports, log };
        running.wait_ready();
        running
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + READY_TIMEOUT;
        for port in self.ports.clone() {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            loop {
                if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
                    break;
                }
                if let Ok(Some(status)) = self.child.try_wait() {
                    panic!("sing-box exited early ({status}):\n{}", self.log_text());
                }
                assert!(
                    Instant::now() < deadline,
                    "sing-box never listened on {addr}:\n{}",
                    self.log_text()
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }

    /// The loopback port of the `index`-th inbound.
    pub fn port(&self, index: usize) -> u16 {
        self.ports[index]
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for SingBox {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> Vec<(Inbound, u16)> {
        vec![
            (
                Inbound {
                    kind: InboundKind::Http,
                    users: vec![("alice".into(), "s3cret".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: Some("ca.pem".into()),
                    }),
                },
                1001,
            ),
            (Inbound { kind: InboundKind::Socks, users: Vec::new(), tls: None }, 1002),
            (Inbound { kind: InboundKind::Mixed, users: Vec::new(), tls: None }, 1003),
        ]
    }

    #[test]
    fn the_configuration_never_touches_the_machine() {
        let config = render(&every_kind());
        let text = config.to_string();
        for forbidden in ["set_system_proxy", "tun", "auto_route", "0.0.0.0", "::"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let top: Vec<&String> = config.as_object().unwrap().keys().collect();
        assert_eq!(top, ["inbounds", "log", "outbounds"]);
        for inbound in config["inbounds"].as_array().unwrap() {
            assert_eq!(inbound["listen"], "127.0.0.1");
        }
        assert_eq!(config["outbounds"], json!([{ "type": "direct", "tag": "direct" }]));
    }

    #[test]
    fn inbounds_are_rendered_as_sing_box_spells_them() {
        let config = render(&every_kind());
        let http = &config["inbounds"][0];
        assert_eq!((&http["type"], &http["listen_port"]), (&json!("http"), &json!(1001)));
        assert_eq!(http["users"], json!([{ "username": "alice", "password": "s3cret" }]));
        assert_eq!(http["tls"]["enabled"], true);
        assert_eq!(http["tls"]["client_authentication"], "require-and-verify");
        assert_eq!(http["tls"]["client_certificate_path"], json!(["ca.pem"]));
        assert_eq!(config["inbounds"][1]["type"], "socks");
        assert!(config["inbounds"][1].get("users").is_none());
        assert_eq!(config["inbounds"][2]["type"], "mixed");
    }

    #[test]
    fn free_ports_are_usable() {
        let port = free_port();
        assert!(port > 0);
        TcpListener::bind(("127.0.0.1", port)).expect("the port is free again");
    }
}
```

Run: `cargo test -p rurge-interop --lib 2>&1 | tail -8`
Expected: 3 个用例通过（不需要 sing-box）。

- [ ] **Step 3: 互操作用例**

`tests/interop/tests/sing_box.rs`：

```rust
//! rurge's outbounds against a real sing-box on the loopback. Every target
//! is a loopback IP literal, so sing-box neither resolves names nor leaves
//! the machine. Without a sing-box binary each test prints why it is skipped
//! (`RURGE_INTEROP_REQUIRED=1` turns that into a failure).

use rurge_config::config::{LoadOptions, from_text};
use rurge_engine::EngineFactory;
use rurge_interop::{Inbound, InboundKind, SingBox, TlsFiles, sing_box_or_skip};
use rurge_net::connector::{ConnectOpts, SystemResolve, Target};
use rurge_net::socket::NoopSocketHook;
use rurge_net::testing::TestServer;
use rurge_policy::OutboundFactory;
use rurge_proto::testing::{TlsFixture, echo_server};
use rurge_proto::{OutboundError, OutboundRef};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The outbound of policy `name` in `profile`, built the way the engine
/// builds it; `fixture`'s CA is trusted when given.
fn outbound(profile: &str, name: &str, fixture: Option<&Arc<TlsFixture>>) -> OutboundRef {
    let loaded = from_text(profile, Path::new("interop.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded.diagnostics.iter().map(|d| d.to_string()).collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let factory = match fixture {
        Some(f) => EngineFactory::with_roots(
            &cfg,
            Arc::new(SystemResolve),
            Arc::new(NoopSocketHook),
            f.roots(),
        ),
        None => EngineFactory::new(&cfg, Arc::new(SystemResolve), Arc::new(NoopSocketHook)),
    };
    let spec = cfg.spec(name).expect("the policy has a spec");
    factory
        .build(spec, factory.direct_connector(&spec.common))
        .expect("the policy builds")
}

fn target(addr: SocketAddr) -> Target {
    Target::new(rurge_config::HostName::Ip(addr.ip()), addr.port())
}

async fn roundtrip(out: &OutboundRef, echo: SocketAddr) {
    let mut stream = out
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("the tunnel is established");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    stream.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"interop");
}

fn plain(kind: InboundKind, users: &[(&str, &str)]) -> Inbound {
    Inbound {
        kind,
        users: users.iter().map(|(u, p)| (u.to_string(), p.to_string())).collect(),
        tls: None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::test]
async fn http_connect_with_and_without_credentials() {
    let Some(bin) = sing_box_or_skip("http_connect_with_and_without_credentials") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![plain(InboundKind::Http, &[]), plain(InboundKind::Http, &[("alice", "s3cret")])],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nOpen = http, 127.0.0.1, {}\nAuth = http, 127.0.0.1, {}, alice, s3cret\nWrong = http, 127.0.0.1, {}, alice, nope\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1),
        sb.port(1)
    );
    roundtrip(&outbound(&profile, "Open", None), echo).await;
    roundtrip(&outbound(&profile, "Auth", None), echo).await;
    let refused = outbound(&profile, "Wrong", None)
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .err()
        .expect("wrong credentials are refused");
    assert!(
        matches!(&refused, OutboundError::Proxy(m) if m.starts_with("http proxy answered 407")),
        "{refused}"
    );
}

#[tokio::test]
async fn a_plain_request_in_absolute_form_is_served_by_sing_box() {
    let Some(bin) = sing_box_or_skip("a_plain_request_in_absolute_form_is_served_by_sing_box") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(&bin, dir.path(), vec![plain(InboundKind::Http, &[("alice", "s3cret")])]);
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let port = origin.url("/").port().unwrap();
    let profile = format!(
        "[Proxy]\nUp = http, 127.0.0.1, {}, alice, s3cret\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    let out = outbound(&profile, "Up", None);
    let forward = out.http_forward().expect("forward mode is the default");
    let mut stream = forward.connect(&ConnectOpts::default()).await.unwrap();
    let mut request = format!("GET http://127.0.0.1:{port}/hello HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n");
    for (name, value) in forward.request_headers() {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read_to_end(&mut response)).await;
    let response = String::from_utf8_lossy(&response);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}\n{}", sb.log_text());
    assert!(response.ends_with("hi there"), "{response}");
    assert_eq!(origin.hits("/hello"), 1);
}

#[tokio::test]
async fn https_with_a_private_ca_a_pinned_fingerprint_and_a_client_certificate() {
    let Some(bin) = sing_box_or_skip("https_with_a_private_ca_a_pinned_fingerprint_and_a_client_certificate")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["localhost", "127.0.0.1"]);
    let write = |name: &str, text: String| {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    let (cert, key, ca) = (
        write("leaf.pem", fixture.leaf_pem()),
        write("leaf.key", fixture.leaf_key_pem()),
        write("ca.pem", fixture.ca_pem()),
    );
    let tls = |client_ca: Option<std::path::PathBuf>| Inbound {
        kind: InboundKind::Http,
        users: Vec::new(),
        tls: Some(TlsFiles {
            certificate: cert.clone(),
            key: key.clone(),
            client_ca,
        }),
    };
    let sb = SingBox::spawn(&bin, dir.path(), vec![tls(None), tls(Some(ca))]);
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nCa = https, 127.0.0.1, {open}\nNamed = https, 127.0.0.1, {open}, sni=localhost\n\
Pinned = https, 127.0.0.1, {open}, server-cert-fingerprint-sha256={pin}\n\
WrongPin = https, 127.0.0.1, {open}, server-cert-fingerprint-sha256={zero}\n\
Mutual = https, 127.0.0.1, {mtls}, client-cert=mtls\nNoCert = https, 127.0.0.1, {mtls}\n\
[Keystore]\nmtls = type=p12, password=pw, base64={p12}\n[Rule]\nFINAL,DIRECT\n",
        open = sb.port(0),
        mtls = sb.port(1),
        pin = hex(&fixture.leaf_fingerprint()),
        zero = "0".repeat(64),
        p12 = fixture.client_p12_base64("interop client"),
    );
    for name in ["Ca", "Named", "Mutual"] {
        roundtrip(&outbound(&profile, name, Some(&fixture)), echo).await;
    }
    // pinned: no CA is trusted at all
    roundtrip(&outbound(&profile, "Pinned", None), echo).await;
    for (name, trusted) in [("WrongPin", None), ("Ca", None), ("NoCert", Some(&fixture))] {
        let e = outbound(&profile, name, trusted)
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .err()
            .unwrap_or_else(|| panic!("{name} must not get through"));
        assert!(
            matches!(e, OutboundError::Tls(_) | OutboundError::Proxy(_) | OutboundError::Io(_)),
            "{name}: {e}"
        );
    }
}

#[tokio::test]
async fn socks5_with_and_without_credentials_and_the_mixed_inbound() {
    let Some(bin) = sing_box_or_skip("socks5_with_and_without_credentials_and_the_mixed_inbound") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            plain(InboundKind::Socks, &[]),
            plain(InboundKind::Socks, &[("alice", "s3cret")]),
            plain(InboundKind::Mixed, &[]),
        ],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nOpen = socks5, 127.0.0.1, {}\nAuth = socks5, 127.0.0.1, {}, alice, s3cret\nWrong = socks5, 127.0.0.1, {}, alice, nope\n\
MixedSocks = socks5, 127.0.0.1, {}\nMixedHttp = http, 127.0.0.1, {}\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1),
        sb.port(1),
        sb.port(2),
        sb.port(2)
    );
    for name in ["Open", "Auth", "MixedSocks", "MixedHttp"] {
        roundtrip(&outbound(&profile, name, None), echo).await;
    }
    let refused = outbound(&profile, "Wrong", None)
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .err()
        .expect("wrong credentials are refused");
    assert!(matches!(&refused, OutboundError::Proxy(_)), "{refused}");
}

/// What "skipped" means must itself be tested: without a binary the helper
/// says so and returns `None`; this machine decides which branch runs.
#[test]
fn a_missing_binary_is_a_skip_unless_interop_is_required() {
    if rurge_interop::locate().is_some() {
        return; // the real tests above are running
    }
    if std::env::var(rurge_interop::REQUIRED_ENV).as_deref() == Ok("1") {
        let caught = std::panic::catch_unwind(|| sing_box_or_skip("probe"));
        assert!(caught.is_err(), "a required but missing sing-box must fail");
    } else {
        assert!(sing_box_or_skip("probe").is_none());
    }
}
```

Run: `cargo test -p rurge-interop 2>&1 | tail -12`
Expected（本机，无 sing-box）：全部"通过"，其中四个互操作用例各打印一行 `skipping …`。再验证必需模式：

Run: `RURGE_INTEROP_REQUIRED=1 cargo test -p rurge-interop --test sing_box 2>&1 | tail -12`
Expected: 四个互操作用例 FAIL（`RURGE_INTEROP_REQUIRED=1 but no sing-box was found`），`a_missing_binary_is_a_skip_unless_interop_is_required` PASS。把两次输出都写进报告。

- [ ] **Step 4: README 与 CI**

`tests/interop/README.md`（中文）：这个 crate 是什么（仅测试、不发布）；固定的 sing-box 版本 1.14.1 与三个包的 SHA256（照抄计划期决定 P2）；本地运行方法（自行安装 sing-box，或 `RURGE_TEST_SING_BOX=<路径> cargo test -p rurge-interop`）；两个环境变量；覆盖范围与**不覆盖** `socks5-tls` 的原因（P3）；安全约束（只监听回环、目标全是回环 IP 字面量、配置里不出现 `set_system_proxy` / `tun`）。

`.github/workflows/ci.yml`：`check` 作业里，在 `cargo test --workspace` 之前加一步：

```yaml
      - name: Install sing-box for the interoperability tests
        shell: bash
        run: |
          set -euo pipefail
          version=1.14.1
          case "$RUNNER_OS" in
            Linux)   asset="sing-box-$version-linux-amd64.tar.gz";  sha=12cb2816b52febb356f6a885b740cc8758c3f30b8ae0ca8edba80f0d2d35343f ;;
            Windows) asset="sing-box-$version-windows-amd64.zip";   sha=5197f16d492d93202dc623622149a6ed040f8eca263128f91d603f2b901baa89 ;;
            macOS)   asset="sing-box-$version-darwin-arm64.tar.gz"; sha=b9024642ef7b4848252df5469b7f60ef3c18bb5e217a16a0934f0174f8ad11b4 ;;
            *) echo "unexpected runner OS: $RUNNER_OS"; exit 1 ;;
          esac
          cd "$RUNNER_TEMP"
          curl -fsSL -o "$asset" "https://github.com/SagerNet/sing-box/releases/download/v$version/$asset"
          if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$asset" | cut -d' ' -f1)
          else
            actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
          fi
          if [ "$actual" != "$sha" ]; then
            echo "sing-box checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p sing-box
          case "$asset" in
            *.zip) unzip -q "$asset" -d sing-box ;;
            *)     tar -xzf "$asset" -C sing-box ;;
          esac
          bin=$(find "$PWD/sing-box" -type f \( -name sing-box -o -name sing-box.exe \) | head -n 1)
          [ -n "$bin" ] || { echo "no sing-box binary in $asset"; exit 1; }
          chmod +x "$bin"
          if [ "$RUNNER_OS" = "Windows" ]; then bin=$(cygpath -w "$bin"); fi
          echo "RURGE_TEST_SING_BOX=$bin" >> "$GITHUB_ENV"
          echo "RURGE_INTEROP_REQUIRED=1" >> "$GITHUB_ENV"
```

这一步在仓库首次推送之前无法验证（本地 `main` 领先 origin 近 200 个提交、从未推送）；报告与 Task 12 的延后表都要如实写明。

- [ ] **Step 5: 门禁与提交**

门禁的 `cargo test --workspace` 现在包含 `rurge-interop`（本机为跳过）。

```bash
git add Cargo.toml Cargo.lock tests/interop crates/rurge-proto .github/workflows/ci.yml
git commit -m "test(interop): 对 sing-box 1.14.1 的互操作夹具与用例（http / https / mTLS / socks5 / mixed）；CI 安装并校验 sing-box"
```

---

### Task 12: 文档与计划收尾

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`
- Create: `docs/api/phase2.md`
- Modify: `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md`（新增第 13 节"M1b 实施期的订正"）
- Modify: `README.md`、`CLAUDE.md`
- Modify: `docs/superpowers/plans/2026-09-19-phase2-m1b-assembly-control-plane-plan.md`（本文件末尾两张表）

**Interfaces:**
- Consumes: 前 11 个任务的执行记录（SDD 账本 / 各任务报告，或控制者整理的汇总文件）。
- Produces: 无代码接口。

- [ ] **Step 1: 兼容性清单**

约定（看 10.4 节已有的写法）：状态列是"计划中的支持程度"，已实现的行在备注列注明"M1（阶段 2）已实现"；语义与 Surge 有差异的行状态改 🟡 并在备注里写清差异。逐行改：

| 位置 | 行 | 改动 |
| ---- | -- | ---- |
| 4.2 | `http` / `https`、`socks5` / `socks5-tls` | 备注：`M1（阶段 2）已实现（TCP）：CONNECT 隧道；明文 HTTP 经 http / https 上游默认按绝对 URI 转发，always-use-connect=true 时走 CONNECT；headers 与 <random-string> 占位；socks5 的 udp-relay 解析但未生效（M5）。非 ASCII 目标域名在写线之前转成 A-label（未与真实 Surge 核对）`；`socks5` 行另注"只有密码没有用户名时密码被忽略" |
| 4.2 | "策略指向 rurge 尚未实现的协议类型" | 备注里的"阶段 2 逐协议实现后移除"改为"随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`" |
| 4.3 | `interface` | 状态 🟡；备注改为：`M1 已实现。Lin 用 SO_BINDTODEVICE；mac 用 IP_BOUND_IF / IPV6_BOUND_IF；Win 取网卡的友好名称（如 Wi-Fi），以绑定该网卡在对应地址族上的第一个非环回、非链路本地地址实现（不用 IP_UNICAST_IF：没有安全封装），该族没有地址则视为不可用，被改成弱主机模型的接口上可能不生效。网卡表缓存 5 秒。空值（interface=）是 E0018（未与真实 Surge 核对）。WireGuard / Tailscale 不支持，与 Surge 一致` |
| 4.3 | `allow-other-interface` | 备注：`M1 已实现：网卡不可用时每个策略 WARN 一次并改用默认网卡` |
| 4.3 | `dns-follow-interface` | 状态 🟡，阶段 `2（M5）`；备注：`解析，W0029；M5 生效` |
| 4.3 | `ip-version` | 备注追加：`M1 已实现：dual 按 250 ms 交错竞速，prefer-* 3 秒后加入另一族；用于 direct 别名时同样作用于对目标的解析与连接（手册只描述了到代理服务器的连接）；目标是 IP 字面量时不过滤` |
| 4.3 | `hybrid` | 备注：`iOS 专属：出现即 W0004，取值不校验` |
| 4.3 | `tfo` | 状态 🟡；备注改为：`解析并校验，W0029，三平台都不生效：socket2 0.6 没有 TFO 的安全封装，不为此引入 unsafe；有安全封装后再评估` |
| 4.3 | `tos` | 状态 🟡；备注：`M1 已实现；Windows 上对 IPv6 不生效` |
| 4.3 | `ecn` `block-quic` `test-url` `test-timeout` `test-udp` | 备注各加：`M1 解析并校验取值，W0029；随后续里程碑生效`（`test-*` 写 M3，`ecn` / `block-quic` 写 M5 / M7） |
| 4.3 | `underlying-proxy` | 备注追加：`M1 已实现（TCP）：底层可以是策略或组，按名字在拨号时解析，组的选择变了链的入口随之变；成环是 E0019；链上某一跳失败时错误文本带 via <名字>: 前缀` |
| 4.4 | 六个 TLS 参数 | 各行备注加 `M1 已实现`；另加两条"未与真实 Surge 核对"：自定义 `sni` 同时成为证书校验名（除非给了 `server-cert-verify-name`）；`https` / `socks5-tls` 握手不带 ALPN（除非写了 `alpn`）。`skip-cert-verify=true` 行补：`构建出站时打一条 WARN`；指纹行补：`与 skip-cert-verify 同时出现时指纹优先（W0012）` |
| 4.6（或 4.2 的 http 行） | `headers` | `值里允许 HTAB，拒绝其它一切控制字符（E0018）；Surge 大概原样发送（未核对）` |
| 1.6 | Keystore `p12` | 备注：`M1 已实现：加载期校验 Base64（E0021）与引用（E0020），干构建期解码（E0022）；OpenSSL 3 默认加密与 -legacy（RC2-40 + 3DES）两种都能读` |
| 2.1 | `use-local-host-item-for-proxy` | 备注：`M1 已实现（FR-DNS-07）：只对 [Host] 里指向 IP 的条目生效，取第一个 IP；命中时明文 HTTP 不走绝对 URI 转发而走 CONNECT` |
| 5.1 | `select` | 备注追加：`M1：选择经 API 切换，对下一条连接生效，按 Profile 文件名持久化到 state.json` |
| 10.4 | `/v1/policies/detail`、`/v1/policy_groups` | 状态 🟡；备注：`M1 已实现；响应形状手册未给出，暂定结构见 docs/api/phase2.md` |
| 10.4 | `GET/POST /v1/policy_groups/select` | 备注：`M1 已实现` |
| 10.4 | `POST /v1/profiles/check`（多配置管理那一行） | 备注追加：`自 M1 起含干构建（E0022）` |
| 10.3（CLI） | `rurge check` 所在的行 | 备注追加：`自阶段 2 / M1 起含干构建：构建不出来的策略（如 p12 解不开）是带行号的 E0022` |

行的确切位置与现有措辞以文件为准；找不到对应行的条目（例如 `headers` 没有单独一行）就加在最贴近的行的备注里，并在报告里说明放在了哪里。

- [ ] **Step 2: `docs/api/phase2.md`**

照 `docs/api/phase1.md` 的体例（先读它），内容：适用范围（阶段 2 新增的端点，鉴权 / 错误体 / `{}` 成功体沿用阶段 1）；四个端点各一节——方法与路径、参数、成功响应的 JSON 示例、每种失败的状态码与 `{"error": …}` 文本（取自 `SelectError` 的 Display：`unknown policy group `G``、`` `G` is not a select group ``、`` `M` is not a member of `G` ``；`unknown policy `X``）；**"暂定"标注**：`/v1/policies/detail` 与 `/v1/policy_groups` 的响应形状手册没有给出，按社区已知形状实现，拿到 Surge 实例的样本后对齐（M1 设计 O2）；字段说明：`typeDescription`（策略 = 类型关键字，组 = 组类型关键字，内置 = 名字）、`isGroup`、`enabled`（恒 `true`）、`lineHash`（`SHA-256("<名字> = <定义原文>")` 的前 16 个十六进制字符；内置策略对名字取哈希）；`/v1/policies/detail` 的值是脱敏后的定义（不含 `名字 =`），脱敏规则同 `GET /v1/profiles/current`；选择的生效时机（下一条连接）与持久化位置（`state.json` 的 `group_selections[<Profile 文件名>][<组名>]`）；`POST /v1/profiles/check` 自 M1 起包含干构建。

- [ ] **Step 3: M1 设计文档的订正**

在 `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md` 末尾加第 13 节「M1b 实施期的订正」，一张表，逐条登记本计划「计划期决定」里与设计文字不同的地方（P4 `EngineShared` 与 `Runtime.policies: Arc<_>`；P5 `definition` 字段；P6 两个暂定细节；P7 `load_checked`；P8 `from_wire` 与出站侧的 A-label；P9 `[Host]` 命中时不转发；P10 `hybrid`；P11 WARN；P12 缓存），以及执行期发生的偏差；并把第 12 节 O3、O4 的"处理"列改为"已结"+ 结论（照抄 P1、P2）。6.2 节的 `PolicyRegistry::build` 签名、`Entry`、`Resolution` 与实现核对一遍，有出入就订正设计文字。

- [ ] **Step 4: `README.md` 与 `CLAUDE.md`**

`README.md`（中英双语，两种语言都要改）：状态段与特性表里把 `http` / `https` / `socks5` / `socks5-tls` 出站、`underlying-proxy` 链、`select` 组的 API 切换标为已可用；路线图里阶段 2 标"进行中（M1 完成）"。措辞与 PRD（`docs/requirements.md` 第 7 节）保持一致；PRD 里若有阶段 2 的进度标记也同步。

`CLAUDE.md`：
- 「当前状态」里关于 M1a 的那一句之后接上 M1b：`M1b（装配与控制面）已完成——…`，列出：`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法，绝对 URI 转发，`use-local-host-item-for-proxy`，`Resolver::host_lookup`，四个 API 端点，bin 的 `PlatformSockets` 与能力表翻转，`tests/interop`；并删掉"这些还没有接进引擎（四种协议在运行期仍是 `W0007` + REJECT），装配属于 M1b"这半句。
- 「先读这些文档」加两条：本计划文件（12 个任务；开头「计划期决定」P1–P12 与「承接事项」；末尾两张表）、`docs/api/phase2.md`。
- 「计划中的架构」的依赖方向一段补一句：`tests/interop`（`rurge-interop`，`publish = false`）是仅测试用的工作区成员。
- 「常用命令」加：

```bash
cargo test -p rurge-engine --test outbounds     # 经真实 http / socks5 出站的端到端用例（回环假上游）
cargo test -p rurge-interop                     # 对 sing-box 的互操作测试；没装 sing-box 就跳过（RURGE_TEST_SING_BOX / RURGE_INTEROP_REQUIRED=1）
```

- [ ] **Step 5: 填写本计划末尾的两张表**

按执行记录如实填写「执行期修正记录」（每一处与计划文字不同的实现：是什么、为什么、在哪个提交）与「延后事项」（评审里标为 minor 而未修的、以及预先列出的几条的最终状态）。没有偏差就写"无"，不要留空表。

- [ ] **Step 6: 门禁与提交**

```bash
git add docs README.md CLAUDE.md
git commit -m "docs: 阶段 2 / M1b 装配与控制面：兼容性清单、API 参考（phase2）、设计订正、README 与 CLAUDE.md"
```

---

## 验收对照（M1 设计第 9 节）

| 验收标准 | 由谁证明 |
| -------- | -------- |
| 1. 四种策略可用；两级链（含底层是 `select` 组）转发通过 | Task 5、6、7 的 `tests/outbounds.rs`（`a_connect_leaves_through…`、`always_use_connect_and_other_protocols…`、`a_chain_enters_through_the_groups_current_member`、`rurge_talks_to_rurge`）；Task 10 的 `run_routes_through_an_http_upstream` |
| 2. 明文 HTTP 按绝对 URI 转发；`always-use-connect=true` 改走 CONNECT | Task 6 |
| 3. 六个 TLS 参数与 p12、`interface` / `allow-other-interface` / `ip-version` 各有测试 | M1a（库级）；Task 11（对 sing-box 的 TLS / mTLS） |
| 4. `POST /v1/policy_groups/select` 切换后下一条连接生效、重启后保留；其余三个端点返回登记的形状 | Task 8、9 |
| 5. `rurge check` 对 `E0018`–`E0022` 给出带行号的诊断；语料库不新增错误 | M1a（`E0018`–`E0021`）；Task 4、10（`E0022`） |
| 6. 对 sing-box 的互操作测试通过（本机装了就跑） | Task 11（本机未装：首次推送后的 CI 证明） |
| 7. 门禁全绿 | 每个任务的最后一步 |

## 执行期修正记录

| 任务 | 偏差 | 原因 | 提交 |
| ---- | ---- | ---- | ---- |
| 1 | 无 | — | d525787 |
| 2 | `rurge_proto::http::wire_host` 公开；`valid_target` 定义为 `wire_host(..).is_some()`；`HttpForward` 的契约改写为"自己写请求行 / `Host` 头的调用方必须写 `wire_host` 返回的文本，不得写 `target.host`；转发不是自己构造的 URI 的调用方用 `valid_target` 把关" | 任务评审的 Important（由计划文字引起）：计划把 `valid_target` 的规则改成"转成 A-label 之后可写"，却没有同步 trait 上的契约，A-label 形式在 crate 外也拿不到 | c8d8698 |
| 2 | 出站的主机名字母表收紧：转换之后只允许 ASCII 字母、数字、`-`、`.`、`_` | 控制者裁定（评审实测的观察）：UTS-46 会把全角 `＠` `／` `：` 映射成 `@` `/` `:`，普通 ASCII `@` 一直被放行；宽松的上游（Go 的 URL 解析器）把 `CONNECT a@b.test:443` 读成 userinfo + 主机 `b.test`，客户端就能经上游代理够到 rurge 的域名规则从未见过的主机 | c8d8698 |
| 2 | `check_status` 用 `trim()` 而不是计划写的 `trim_end()`；`DirectConnector` 的空应答文本沿用两个生产解析器已有的措辞 `no addresses for <name>`（计划写的是 `no address found for <name>`），并注明该分支只为"返回空列表而不是错误"的 `Resolve` 实现兜底 | 评审的 Minor，修复轮里顺手处理：状态码后的双空格不该保留；同一情形不该有两种措辞（`SystemResolve` 与 `rurge_dns::Resolver` 自己就会把空应答变成 `NotFound`） | c8d8698 |
| 3 | 无（代码与计划逐字一致，仅 rustfmt 换行） | — | 357b14b |
| 4 | 无；`[Keystore]` 里空的 `base64=` 在加载期确实不报错，干构建在解码 p12 时报 `E0022`（计划预留的 `base64=AA==` 退路没有用上）——M1a 延后表里"空的 `base64` 不报 `E0021`"那一条就此关闭 | — | b20973d |
| 5 | `tests/outbounds.rs` 的 CONNECT 用例在等请求记录之前先 `drop(tunnel)` | 会话记录要等中继的两个方向都关闭才产生；计划里的用例没有关闭客户端一侧，会一直等到超时 | bf84ed8 |
| 5 | `tests/pipeline.rs` 的 `build_runtime` 辅助函数多了一个 `shared` 参数，重载类用例用 `engine.shared()` 构建下一代 | 新加的 `publish_registry` 断言在四个重载 / 重新绑定用例里触发：它们确实在用另一个 cell 构建下一代——修法与 bin 的 `reload()` 一致 | bf84ed8 |
| 5 | `Daemon.store`（bin）与 `Harness::socks()`（测试夹具）各加了 `#[allow(dead_code)]` | 前者是计划要求保留的字段（评审指出它其实已是死代码，Task 10 删除）；后者到 Task 7 才被用到 | bf84ed8 |
| 6 | 无（`Dialed { .. }` 的构造点实际只有两处：引擎的 `dial` 与 `FakeDialer`） | — | 4ea463b |
| 7 | `rurge_talks_to_rurge` 里每条隧道用完即 `drop(tunnel)`；去掉 `Harness::socks()` 上的 `#[allow(dead_code)]` | 派发时的控制者订正：会话记录要等隧道两个方向都关闭才产生，重新绑定变量并不会关闭前一条隧道 | c62b631 |
| 6、7 | 两个"CONNECT 失败"的端到端用例（`a_refusing_upstream_is_a_502_that_quotes_the_proxy`、`a_broken_hop_is_named_in_the_error`）的请求带 `Connection: close`，并把"读到 EOF"从可有可无改成必须（超时即失败） | 任务评审的 Important（出在计划自己的测试代码里）：失败的 CONNECT 得到的是保持连接的 502 页面，`read_to_end` 等不到 EOF，每次都白等满 15 秒 / 5 秒 | deb5c26 |
| 8 | 测试里用 `policy_detail("DIRECT")` 而不是计划写的 `"direct"` | 内置策略名区分大小写（`Builtin::parse` 是精确匹配）；解析器未动 | f1cb39e |
| 8 | **计划期决定 P6 被修订**：`lineHash` 对**脱敏后**的定义取哈希（`SHA-256("<名字> = <redact_definition(定义)>")` 的前 16 个十六进制字符；内置策略仍对名字取哈希） | 任务评审的 Important（出在计划文字里）：对未脱敏的定义行取哈希再经 API 暴露，等于给持有 API key 的人一个离线验证凭据的途径；项目规则是任何由凭据派生的东西都不出进程。名字唯一且参与哈希，脱敏不会让两个成员撞哈希；代价是只改了凭据时 `lineHash` 不变 | 9877025 |
| 8 | `hidden` 按项目的布尔约定读取（`g.params.bool("hidden")`：true / 1 / yes），不是计划写的只认字面 `true` | 任务评审的 Important（出在计划文字里） | 9877025 |
| 9 | 无（仅 rustfmt 重排；`GET /v1/policy_groups/select` 的处理函数多了一行注释：没有成员的组返回 `{"policy": ""}`） | 派发时的控制者说明 | d0eaa9a |
| 10（修的是 Task 4 的计划代码） | **干构建的工厂改用真实根证书**（`rurge_net::tls::root_store()`，进程内缓存、只读本机证书库、永不为空），不再用空的 `RootCertStore` | 计划缺陷，Task 10 把干构建接进 `rurge check` 时由 CLI 用例暴露：rustls 的 WebPKI 校验器拒绝空的根证书库，于是任何走标准校验的 `https` / `socks5-tls` 策略都会被干构建判成 `E0022`。Task 4 的用例只让坏 p12（更早失败）和明文 `http` 走过干构建工厂，没有暴露 | bf2899e |
| 10 | CLI 用例断言输出里含 `` `ss` `` 而不是策略名 `Old` | `W0007` 按协议类型去重，消息里没有策略名 | bf2899e |
| 10 | 删除 bin 里的死字段 `Daemon.store` 及其 `#[allow(dead_code)]` | Task 5 评审指出、派发时路由到本任务：选择是经引擎挂载的状态库写入的，从不经过这个字段 | bf2899e |
| 10 | 给干构建的根证书修复补了单元测试 `an_ordinary_tls_policy_passes_the_dry_build`（把空证书库改回去，它会对两条标准校验的策略报 `E0022` 而失败）；两个 CLI 用例补上"输出 / 响应里没有 Keystore 密码"的断言；订正 `api.rs` / `pipeline.rs` 里"bin 不声明任何代理协议"的注释 | 任务评审的 Important：修复在 `rurge-engine` 自己的测试里没有钉子——正是让缺陷漏过 Task 4 的那个缺口 | 1d9cc26 |
| 11 | 无（代码与计划逐字一致，仅 rustfmt 重排）。本机未装 sing-box：四个互操作用例在本机只验证了"跳过"与"`RURGE_INTEROP_REQUIRED=1` 时失败"两条路径 | — | e68d1bd |

## 延后事项

标为"整分支终审时分诊"的条目，其最终去向由终审后的修复提交更新到本表。

| 事项 | 去向 |
| ---- | ---- |
| 互操作用例与 CI 里安装 sing-box 的步骤在本机无法运行（未装 sing-box、仓库从未推送） | 首次推送后的 CI；项目所有者也可在本机装 sing-box 后 `cargo test -p rurge-interop` |
| `socks5-tls` 没有参考实现可对测（sing-box 的 socks / mixed 入站不支持 TLS） | 保持由回环假上游覆盖；出现支持 SOCKS5-over-TLS 的参考实现时再补 |
| `/v1/policies/detail`、`/v1/policy_groups` 的真实响应形状（M1 设计 O2） | 拿到 Surge 实例的样本后对齐 |
| 出站按指纹跨代复用 | M2 |
| `dns-follow-interface`；`udp-relay`、UDP 载体 | M5 |
| `test-url` / `test-timeout` 的使用、请求记录的计时字段、组的环降级、空组回退、"策略存在但不可用 → REJECT" | M3 |
| `race` 的 3 秒分支被 `queue.is_empty()` 挡住；`server-cert-verify-name` 只在标准分支解析 | 前者保持现状（M1a 终审裁定）；后者 M2 |
| 两个出站的测试辅助函数近乎相同；两个假上游各有一份 accept 循环；`http` 出站的 CONNECT 写没有 EOF / reset 归一 | 第三个出站（M2）到位时一并处理 |
| Linux / macOS 的平台分支（含本计划的网卡表缓存）本机无法编译 | 首次推送后的 CI |
| `HostName::from_wire` 放行 Unicode 格式字符（零宽空格 / 连接符、BOM、双向控制符）：不是注入向量（出站转成 A-label 后只允许字母、数字与 `- . _`），是显示混淆向量；ZWJ / ZWNJ 在部分文字的合法 IDN 里会出现，不能一概拒绝 | 整分支终审时分诊（至多拒绝 BOM 与双向控制符） |
| SOCKS5 入站拒绝一个名字时不打任何日志（与空名字分支一致）；非 UTF-8 的名字仍是直接断开连接而不是回 `0x01` | 不处理 / 需要运维信号时加一条净化过的 debug 日志 |
| `socks5` 出站的"超过 255 字节"提示量的是 A-label 而不是用户输入的名字（措辞） | 不处理 |
| 运行期跨多跳 `ChainConnector` 的递归不受 `MAX_DEPTH` 约束（每一跳都是深度 0 的一次新 `resolve`）；它的有界性完全依赖"注册表只从通过了 `E0019` 的配置构建"——M1 里成立（`run` / `reload` 拒绝带错误的配置，组成员是静态的，选择只能落在静态图已覆盖的边上） | M3：订阅引入动态成员时加运行期的深度兜底（M1 设计 6.2 已预告） |
| `cell.rs` 里 `via <name>: ` 前缀出现四处（每处一行） | 不处理 |
| 注册表单元测试重写时少带了三条旧断言（成员不受支持的组的完整策略链；选择之后 `outbound.name() == "DIRECT"`；`Emptyish` 没有 note）；`Direct::with_socket_opts` 在工作区里已无调用方；`has_socket_opts` 缺一句"为什么不含 `allow_other_interface`"的注释 | 整分支终审时分诊 |
| 1 Hz 采样任务持有 `Arc<Engine>` 直到 `stop_accepting`，在那之前 `Drop for Engine`（清空 cell）不会运行 | 不处理：守护进程退出路径先 `stop_accepting`；测试夹具不启动采样器 |
| 转发模式下上游的拒绝（对绝对 URI 请求回 407 / 502）原样交还客户端（`Proxy-Authenticate` 已剥掉），会话记为 Completed、没有 error；CONNECT 模式下同样的配置错误是 502 页面 + 会话日志里的 `http proxy answered 407 …`。没有用例覆盖 | 整分支终审时分诊（控制者建议处理：转发模式下把上游的 407 映射成失败会话 + 502 页面）；不处理则登记进兼容性清单 |
| 拒绝文本 `the target host name is not valid for an HTTP proxy request` 在 `rurge-engine` 与 `rurge-proto` 各写了一遍；隧道类端到端用例里"origin 看到的是 origin-form"的注释没有被断言证明；入站丢弃上游头的 debug 行不说是哪个头 | 整分支终审时分诊 |
| `dry_build` 的文档注释把"跳过 Direct"说成"没有自己的出站"（实际是 `Direct::new` 不会失败；只有 Reject 没有自己的出站） | 不处理（措辞） |
| HTTP 入站的"tripwire"测试钉的是 `http::Uri::from_str`，不是 hyper 的请求行解析器（两者共用 `Uri::parse`，间接但有效） | 不处理 |
| `[Host]` 给同一个名字列了多个地址时，交给代理的总是第一个，不看该策略的 `ip-version`（FR-DNS-07 没有规定） | 登记进兼容性清单（见下面的文档要点）；需要时再按 `ip-version` 挑选 |
| 三条新路由没有各自的 401 用例（路由器只有一条 `.layer(require_key)` 链，既有的"兜底路由也在鉴权之后"用例在结构上已覆盖）；API 端到端用例里"切换前不是 200"可以收紧成"状态行为空" | 不处理 |
| `Cached::get` 在持锁期间调用 `get_if_addrs`（tokio 工作线程上的阻塞系统调用）：不是回退——以前每条连接都内联调用一次，现在每 5 秒一次，并发的连接排在一次刷新后面；新出现的网卡最多晚 5 秒可见；`PlatformSockets` 的用例只测了 `Family::V4` | 不处理 |
| **`rurge run` 遇到有加载错误的配置，会在系统代理的崩溃恢复之前就退出 2**（`sysproxy.recover()` 在 tokio 块里，加载错误的退出点在它前面）。阶段 1 就是如此；干构建让"能解析但构建不出来"的配置也走这条早退路径。后果：上一次崩溃留下系统代理指向 rurge，而这次启动的配置又是坏的，系统代理会一直指向一个没人监听的端口，直到配置被修好 | 整分支终审时分诊；不在本计划范围内处理的话，列为阶段 1 的遗留缺陷单独修（把崩溃恢复提前到加载错误的退出点之前，它只需要数据目录） |
| 互操作层的防抖小项：TLS 反例的匹配不含 `OutboundError::Timeout`（runner 负载高时握手超时会把"正确地没通过"变成红）；`wait_ready` 先连接后查子进程是否已退出（端口被别的进程抢走且 sing-box 已死时会误判就绪）；转发用例丢弃了超时结果；守卫夹具没有渲染"不带客户端 CA 的 TLS 入站"；必需模式的探针用例会打印一段 panic 回溯；README 说本地单元测试覆盖"定位二进制"而 `locate()` 其实没有测试；CI 步骤的 `curl` 没有 `--retry` | 整分支终审时分诊（都便宜，且影响 CI 的首次运行） |
| sing-box 1.14.1 是否接受夹具渲染的配置（`tls.client_authentication` / `client_certificate_path`、没有 `route` 节、`log.timestamp`）、三个 SHA-256 本身、CI 步骤在真实 runner 上的行为 | 首次推送后的 CI（失败方式是干净的：`spawn` 带着 sing-box 的日志 panic） |
