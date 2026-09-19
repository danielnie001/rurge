# 阶段 2 / M1a「配置与出站库」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `[Proxy]` 的参数类型化并校验，建好带 socket 选项的竞速连接层、TLS 层与 p12 解码，并在库层面实现 `http` `https` `socks5` `socks5-tls` 四种出站（TCP，含明文 HTTP 的绝对 URI 转发能力）；引擎装配留给 M1b。

**Architecture:** `rurge-config::spec` 在无类型的 `ParamMap` 之上加一层 `PolicySpec`（`ParamReader` 记住读过的键，没读过的告警）。`rurge_net::socket` 提供 `SocketOpts` / `SocketHook` 与按 `ip-version` 竞速的建连，`DirectConnector` 建立在它上面；平台相关的网卡绑定与 TOS 是 `rurge-platform::socket` 里的自由函数。`rurge-proto` 增加 TLS 层（三种证书校验模式）、p12 解码、两个协议出站与一套可编排的回环假上游（cargo feature `testing`）。

**Tech Stack:** Rust 1.89 / edition 2024、tokio、rustls 0.23（ring）+ tokio-rustls、socket2 0.6（`all`）、p12-keystore 0.2、getrandom 0.3、base64 0.22、sha2 0.10、rcgen 0.14（仅测试）。

**Spec:** `docs/superpowers/specs/2026-09-19-phase2-m1-outbound-foundation-design.md`（M1 设计；第 4、5 节是本计划的依据）；上位文档 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`（阶段 2 总设计）。

## Global Constraints

- 工具链：Rust stable，`rust-version = "1.89"`，edition 2024（let-chains 可用；clippy 要求能用就用）。
- `unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 是 `deny` 且只有 `sysproxy::windows::notify_wininet` 一个函数 `#[allow]`。**本计划不新增任何 unsafe**；某个平台缺安全封装的能力按"该平台不支持"处理。
- 新增依赖只有这些，版本照抄：工作区 `socket2 = { version = "0.6", features = ["all"] }`、`p12-keystore = "0.2"`、`getrandom = "0.3"`；`base64`（工作区已有 `"0.22"`）新用于 `rurge-config` 与 `rurge-proto`；`rcgen`（已有 `"0.14"`）作为 `rurge-proto` 的可选依赖（feature `testing`）与 dev 依赖。不引入其它 crate，不跑 `cargo update`。`Cargo.lock` 的变化要提交。
- 平台相关代码只允许出现在 `rurge-platform`（AR-02）；`rurge-platform` 不依赖任何内部 crate；`rurge-engine` / `rurge-api` 不依赖 `rurge-platform`。
- 诊断码永不重编号。本计划新增：`E0018` `E0019` `E0020` `E0021` `E0022`、`W0028` `W0029`，含义照抄 M1 设计 4.3。
- **`capabilities::current()` 在本计划里不动**（M1 设计 T6）：四种协议在 M1a 结束时仍然产生 `W0007` 并在运行期按 REJECT 处理。
- 凭据（用户名、密码、Keystore 材料）不得出现在任何错误文本、诊断消息与日志里。
- 测试只用回环地址 + 端口 0 + 有界等待，绝不访问公网；不得修改本机的系统代理、注册表、网络设置，不注册服务；需要 `[::1]` 的用例在绑定失败时跳过而不是失败。
- 语言：代码、注释、日志、CLI 输出用英文；文档用中文；提交标题用中文（仓库风格见 `git log --oneline -8`）。
- 提交：在分支 `phase2-m1a-outbound-library`（从 `main` 切出）上工作；不 push、不 merge、不 amend、不改写历史。每条提交消息以会话提示给出的署名行结尾，并且必须含这一行：`Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。
- 每个任务结束跑门禁：`RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --no-fail-fast`。测试二进制在没有失败用例的情况下异常退出是已知的偶发基建问题：重跑一次并保留两次输出。
- 本机 bash 里超过约 8 KB 的 heredoc 会失败：大文件用编辑器工具写，不用 shell 重定向。

## 计划期决定（写计划时核对源码得出，与 M1 设计不一致处已同步订正设计文档）

| 编号 | 决定 | 依据 |
| ---- | ---- | ---- |
| P1 | `tfo` 在 M1 三平台都不生效：`SocketOpts` 与 `SocketHook` 里没有 TFO，`tfo=true` 归入 `W0029` | `socket2` 0.6.5 源码里没有任何 TCP Fast Open 的封装；M1 设计的规则是"缺安全封装即不支持，不为此引入 unsafe" |
| P2 | PKCS#12 用 `p12-keystore` **0.2**（不是 0.3） | 0.3.2 会给 `Cargo.lock` 新增约 47 个 crate 版本（整套新一代 RustCrypto），其中 `cms 0.3.0-pre.1` 与 `pkcs12 0.2.0-pre.0` 是预发布版；0.2.1 建在工作区已在用的那一代上，同样支持 PBES2-AES256 / RC2-40 / 3DES，并自带 writer |
| P3 | macOS 的"网卡名 → 索引"取自 `if_addrs::Interface::index` | `socket2` 文档建议的 `libc::if_nametoindex` 是 unsafe FFI |
| P4 | `to_spec` 返回 `SpecOutcome { spec, diagnostics, inert, ios_only }` | `W0029` / `W0004` 要"每个参数名每次加载只报一次"，去重只能在调用方做，所以把出现过的参数名带出来 |
| P5 | `BuildError` 定义在 `rurge-proto`（`rurge_proto::BuildError`） | 协议的构造函数在 M1a 就要返回它；`rurge-policy::factory` 在 M1b 直接复用 |
| P6 | "语料库不新增错误"由现有的 `valid_corpus_loads_without_errors_and_matches_snapshots` 承担；新增的告警进快照并逐条核对；另加一份 `invalid` 样本覆盖四个新错误码 | 该测试已经断言 `valid` 语料库零错误 |
| P7 | M1a 必须同步改 `rurge-engine` 的几行：`ConnectOpts` 的两个构造点、`Runtime::build` 里用 `Direct::with_socket_opts` 保住 `[General] ipv6` 的排序语义、`OutboundError` 新变体的 `match` 分支 | 不改就编译不过，或者在 M1a 与 M1b 之间丢掉 v6 在前的行为 |
| P8 | `IpVersion` 定义在 `rurge_config::spec`，`rurge_net::socket` 直接复用 | `rurge-net` 本来就依赖 `rurge-config`；避免两个同构的枚举互相转换 |
| P9 | 随机串用 `getrandom` 0.3 的 `getrandom::fill` | 0.3.4 已在 `Cargo.lock` 里 |
| P10 | 真实 OpenSSL 生成的两份 p12（PBES2-AES256 与旧式 RC2-40 + 3DES）以 Base64 常量的形式放进测试；它们的证书带 `CA:TRUE`，webpki 不接受它当终端实体，所以只用来验证解码，双向 TLS 握手用 `p12-keystore` 的 writer 现场生成的 p12（证书由测试 CA 签发） | OpenSSL 3.2.4 `openssl pkcs12 -export [-legacy]` 的产物；密钥是测试专用的 |

## File Structure

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/diagnostic.rs` | 新诊断码 | 1 |
| `crates/rurge-config/src/spec/mod.rs` | `PolicySpec` `ProtoSpec` `SpecEnv` `NameKind` `SpecOutcome` `to_spec` | 1, 2 |
| `crates/rurge-config/src/spec/reader.rs` | `ParamReader` | 1 |
| `crates/rurge-config/src/spec/common.rs` | `CommonOpts` `IpVersion` `Tristate` `Applies` 与读取 | 2 |
| `crates/rurge-config/src/spec/tls.rs` | `TlsOpts` `Sni` 与读取 | 2 |
| `crates/rurge-config/src/spec/http.rs` | `HttpSpec` `HeaderTemplate` `HeaderPart` | 2 |
| `crates/rurge-config/src/spec/socks5.rs` | `Socks5Spec` | 2 |
| `crates/rurge-config/src/config.rs` | `Config.specs`、`Config::spec`、`validate` 接线、环检测、Keystore Base64 | 3 |
| `crates/rurge-config/tests/policy_spec.rs` | 接线的集成测试 | 3 |
| `tests/corpus/invalid/policy-params.conf` + `.expect` | 四个新错误码的语料 | 3 |
| `crates/rurge-net/src/socket.rs` | `SocketOpts` `Family` `SocketHook` `NoopSocketHook` `plan_addresses` `race` | 4 |
| `crates/rurge-net/src/connector.rs` | `ConnectOpts`（去掉 `prefer_v6`）、新的 `DirectConnector` | 5 |
| `crates/rurge-platform/src/socket.rs` | `bind_interface` `set_tos` `pick_source` | 6 |
| `crates/rurge-proto/src/outbound.rs` | `OutboundError` 新变体、`HttpForward`、`http_forward()` | 7 |
| `crates/rurge-proto/src/build.rs` | `BuildError` | 7 |
| `crates/rurge-proto/src/transport/{mod,head,prefixed}.rs` | 读响应头、带前缀字节的流 | 8 |
| `crates/rurge-proto/src/testing/{mod,tls,http_proxy,socks5}.rs` | 回环假上游（feature `testing`） | 8 |
| `crates/rurge-net/src/tls.rs` | `root_store()` | 9 |
| `crates/rurge-proto/src/transport/tls.rs` | `TlsClient` `ClientIdentity` | 9 |
| `crates/rurge-proto/src/keystore.rs` | `decode_p12` | 10 |
| `crates/rurge-proto/src/http.rs` | `HttpOutbound` | 11 |
| `crates/rurge-proto/src/socks5.rs` | `Socks5Outbound` | 12 |

---

### Task 1: 诊断码与 `ParamReader`

**Files:**
- Modify: `crates/rurge-config/src/diagnostic.rs`（`codes` 模块）
- Modify: `crates/rurge-config/src/lib.rs`
- Create: `crates/rurge-config/src/spec/mod.rs`
- Create: `crates/rurge-config/src/spec/reader.rs`

**Interfaces:**
- Consumes: `rurge_config::policy::{ProxyPolicy, parse_policy}`、`rurge_config::value::parse_bool`、`Diagnostic::{error, warning}` + `.at(span)`。
- Produces:
  - `codes::{E_INVALID_POLICY_PARAM = "E0018", E_UNDERLYING_PROXY_CYCLE = "E0019", E_KEYSTORE_REF = "E0020", E_KEYSTORE_BASE64 = "E0021", E_POLICY_BUILD = "E0022", W_PARAM_NOT_APPLICABLE = "W0028", W_PARAM_NOT_EFFECTIVE = "W0029"}`
  - `spec::ParamReader<'a>`：`new(&'a ProxyPolicy)`、`policy()`、`has(&str) -> bool`、`touch(&str)`、`str(&str) -> Option<&'a str>`、`positional(usize) -> Option<&'a str>`、`bool(&str) -> Option<bool>`、`number::<T>(&str, expected: &str) -> Option<T>`、`choice::<T: Copy>(&str, &[(&str, T)]) -> Option<T>`、`invalid(key, value, expected)`、`error(code, String)`、`warn(code, String)`、`has_errors() -> bool`、`finish(self) -> Vec<Diagnostic>`

- [ ] **Step 1: 确认所在分支**

执行方在开始时已从 `main` 切出 `phase2-m1a-outbound-library`，并把本计划（连同写计划时对 M1 设计文档的订正）作为该分支的第一个提交。

Run: `git branch --show-current && git status --short | head -n 3`
Expected: `phase2-m1a-outbound-library`，工作区干净。不在这个分支上就停下来报告，不要自己切分支。

- [ ] **Step 2: 加诊断码**

在 `crates/rurge-config/src/diagnostic.rs` 的 `codes` 模块里，`E_INVALID_DEFINITION` 之后加：

```rust
    /// A known policy parameter has a value that cannot be used.
    pub const E_INVALID_POLICY_PARAM: &str = "E0018";
    /// An `underlying-proxy` chain that leads back to the policy itself.
    pub const E_UNDERLYING_PROXY_CYCLE: &str = "E0019";
    /// A `[Keystore]` reference that names a missing item or one of the wrong type.
    pub const E_KEYSTORE_REF: &str = "E0020";
    /// A `[Keystore]` item whose `base64` does not decode.
    pub const E_KEYSTORE_BASE64: &str = "E0021";
    /// A policy that cannot be built (reported by the engine's dry build, M1b).
    pub const E_POLICY_BUILD: &str = "E0022";
```

`W_HOST_SCRIPT_SKIPPED` 之后加：

```rust
    /// A parameter that does not apply to this policy type; ignored.
    pub const W_PARAM_NOT_APPLICABLE: &str = "W0028";
    /// A parameter that is parsed but has no effect in this version.
    pub const W_PARAM_NOT_EFFECTIVE: &str = "W0029";
```

- [ ] **Step 3: 写失败的测试（`reader.rs` 的测试模块）**

创建 `crates/rurge-config/src/spec/mod.rs`：

```rust
//! Typed view of `[Proxy]` policy parameters (phase 2 M1 design §4).

pub mod reader;

pub use reader::ParamReader;
```

在 `crates/rurge-config/src/lib.rs` 的 `pub mod session;` 之后加 `pub mod spec;`。

创建 `crates/rurge-config/src/spec/reader.rs`，先只放测试（实现在 Step 5）：

```rust
//! Typed reads over one policy's parameters. Remembers which keys were read
//! so that `finish` can warn about the rest.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Severity, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    fn policy(def: &str) -> crate::policy::ProxyPolicy {
        parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 7)).unwrap()
    }

    #[test]
    fn typed_reads_validate_and_remember_the_key() {
        let p = policy("http, 1.2.3.4, 80, tfo=true, tos=0x10, ip-version=V4-Only, bad=maybe");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.bool("tfo"), Some(true));
        assert_eq!(r.str("tos"), Some("0x10"));
        let table = [("dual", 0u8), ("v4-only", 1u8)];
        assert_eq!(r.choice("ip-version", &table), Some(1));
        assert_eq!(r.bool("bad"), None);
        assert_eq!(r.bool("absent"), None);
        assert!(r.has_errors());
        let diags = r.finish();
        assert_eq!(diags.len(), 1, "{diags:?}");
        assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
        assert_eq!(diags[0].severity, Severity::Error);
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `maybe` for `bad` (expected true or false)"
        );
        assert_eq!(diags[0].span.as_ref().unwrap().line, 7);
    }

    #[test]
    fn unread_parameters_and_extra_positionals_are_warned_about_once() {
        let p = policy("http, 1.2.3.4, 80, user, pass, surplus, mystery=1, mystery=2, known=x");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.positional(0), Some("user"));
        assert_eq!(r.positional(1), Some("pass"));
        assert_eq!(r.positional(5), None);
        r.touch("known");
        assert!(!r.has_errors());
        let messages: Vec<String> = r.finish().into_iter().map(|d| d.message).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: unknown parameter `mystery` ignored",
                // the value itself is never echoed: it may be a secret
                "policy `P`: unexpected positional value #3 ignored",
            ]
        );
    }

    #[test]
    fn numbers_report_what_was_expected() {
        let p = policy("http, 1.2.3.4, 80, test-timeout=soon");
        let mut r = ParamReader::new(&p);
        assert_eq!(r.number::<u32>("test-timeout", "seconds"), None);
        let diags = r.finish();
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid value `soon` for `test-timeout` (expected seconds)"
        );
    }
}
```

- [ ] **Step 4: 跑测试确认失败**

Run: `cargo test -p rurge-config spec::`
Expected: 编译失败——`ParamReader` 未定义。

- [ ] **Step 5: 实现 `ParamReader`**

在 `crates/rurge-config/src/spec/reader.rs` 的模块注释之后、测试模块之前加：

```rust
use crate::diagnostic::{Diagnostic, Severity, codes};
use crate::policy::ProxyPolicy;
use crate::value::parse_bool;
use std::collections::HashSet;
use std::str::FromStr;

pub struct ParamReader<'a> {
    policy: &'a ProxyPolicy,
    used: HashSet<String>,
    positional_used: usize,
    diags: Vec<Diagnostic>,
}

impl<'a> ParamReader<'a> {
    pub fn new(policy: &'a ProxyPolicy) -> ParamReader<'a> {
        ParamReader {
            policy,
            used: HashSet::new(),
            positional_used: 0,
            diags: Vec::new(),
        }
    }

    pub fn policy(&self) -> &'a ProxyPolicy {
        self.policy
    }

    /// `true` when the parameter is written on the policy line.
    pub fn has(&self, key: &str) -> bool {
        self.policy.params.contains(key)
    }

    /// Marks `key` as known without reading it.
    pub fn touch(&mut self, key: &str) {
        self.used.insert(key.to_ascii_lowercase());
    }

    /// The first value of `key`; marks it as read.
    pub fn str(&mut self, key: &str) -> Option<&'a str> {
        self.touch(key);
        let policy = self.policy;
        policy.params.get(key)
    }

    /// The positional value at `index` (after `type, server, port`).
    pub fn positional(&mut self, index: usize) -> Option<&'a str> {
        let policy = self.policy;
        let value = policy.positional.get(index).map(String::as_str);
        if value.is_some() {
            self.positional_used = self.positional_used.max(index + 1);
        }
        value
    }

    pub fn bool(&mut self, key: &str) -> Option<bool> {
        let value = self.str(key)?;
        let parsed = parse_bool(value);
        if parsed.is_none() {
            self.invalid(key, value, "true or false");
        }
        parsed
    }

    pub fn number<T: FromStr>(&mut self, key: &str, expected: &str) -> Option<T> {
        let value = self.str(key)?;
        let parsed = value.trim().parse().ok();
        if parsed.is_none() {
            self.invalid(key, value, expected);
        }
        parsed
    }

    /// Case-insensitive lookup of the value in `table`.
    pub fn choice<T: Copy>(&mut self, key: &str, table: &[(&str, T)]) -> Option<T> {
        let value = self.str(key)?;
        let found = table
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(value.trim()))
            .map(|(_, v)| *v);
        if found.is_none() {
            let names: Vec<&str> = table.iter().map(|(name, _)| *name).collect();
            self.invalid(key, value, &names.join(" / "));
        }
        found
    }

    /// `E0018`. Never call this for a parameter whose value is a secret.
    pub fn invalid(&mut self, key: &str, value: &str, expected: &str) {
        self.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("invalid value `{value}` for `{key}` (expected {expected})"),
        );
    }

    pub fn error(&mut self, code: &'static str, message: String) {
        let message = format!("policy `{}`: {message}", self.policy.name);
        self.diags
            .push(Diagnostic::error(code, message).at(self.policy.span.clone()));
    }

    pub fn warn(&mut self, code: &'static str, message: String) {
        let message = format!("policy `{}`: {message}", self.policy.name);
        self.diags
            .push(Diagnostic::warning(code, message).at(self.policy.span.clone()));
    }

    pub fn has_errors(&self) -> bool {
        self.diags.iter().any(|d| d.severity == Severity::Error)
    }

    /// Warns about every parameter and positional value nobody read.
    pub fn finish(mut self) -> Vec<Diagnostic> {
        let policy = self.policy;
        let mut reported = HashSet::new();
        for (key, _) in policy.params.iter() {
            if !self.used.contains(key) && reported.insert(key.to_string()) {
                self.warn(
                    codes::W_UNKNOWN_KEY,
                    format!("unknown parameter `{key}` ignored"),
                );
            }
        }
        for index in self.positional_used..policy.positional.len() {
            self.warn(
                codes::W_UNKNOWN_KEY,
                format!("unexpected positional value #{} ignored", index + 1),
            );
        }
        self.diags
    }
}
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test -p rurge-config spec::`
Expected: PASS（`reader` 3 个）。

- [ ] **Step 7: 门禁与提交**

Run: Global Constraints 里的门禁命令。Expected: 全绿。

```bash
git add crates/rurge-config/src/diagnostic.rs crates/rurge-config/src/lib.rs crates/rurge-config/src/spec
git commit -m "feat(config): 策略参数的类型化读取器 ParamReader 与阶段 2 M1 的诊断码"
```

---

### Task 2: 通用参数、TLS 参数、HTTP / SOCKS5 的 spec 与 `to_spec`

**Files:**
- Create: `crates/rurge-config/src/spec/common.rs`
- Create: `crates/rurge-config/src/spec/tls.rs`
- Create: `crates/rurge-config/src/spec/http.rs`
- Create: `crates/rurge-config/src/spec/socks5.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`
- Modify: `crates/rurge-config/src/lib.rs`（re-export）

**Interfaces:**
- Consumes: Task 1 的 `ParamReader` 与诊断码；`rurge_config::general::UdpTest { hostname: String, server: Ipv4Addr }`；`rurge_config::keystore::{KeystoreItem, KeystoreType}`；`rurge_config::policy::{Builtin, PolicyKind, ProxyPolicy}`；`rurge_config::types::HostName`。
- Produces:
  - `spec::{IpVersion, Tristate, Applies, CommonOpts}`（字段见 M1 设计 4.1）；crate 内部的 `spec::common::{Notes, read_common}`
  - `spec::{Sni, TlsOpts}`：`TlsOpts { skip_cert_verify: bool, sni: Sni, verify_name: Option<String>, fingerprint_sha256: Option<[u8; 32]>, alpn: Vec<String>, client_cert: Option<String> }`，`Sni::{Default, Off, Name(String)}`
  - `spec::{HeaderPart, HeaderTemplate, HttpSpec, Socks5Spec}`：`HeaderPart::{Literal(String), Random { min: usize, max: usize }}`，`HeaderTemplate { name: String, value: Vec<HeaderPart> }`，`HeaderTemplate::parse_list(&str) -> Result<Vec<HeaderTemplate>, String>`
  - `spec::{NameKind, SpecEnv, ProtoSpec, PolicySpec, SpecOutcome, to_spec}`（签名见 M1 设计 4.1）
  - 顶层 re-export：`rurge_config::{PolicySpec, ProtoSpec}`

- [ ] **Step 1: 写失败的测试**

创建 `crates/rurge-config/src/spec/common.rs`，先只放测试：

```rust
//! The 14 common policy parameters (compatibility matrix §4.3).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::codes;
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::net::Ipv4Addr;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;

    fn read(def: &str, applies: Applies) -> (CommonOpts, Notes, Vec<crate::Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let mut notes = Notes::default();
        let common = read_common(&mut r, applies, &mut notes);
        (common, notes, r.finish())
    }

    #[test]
    fn every_common_parameter_is_parsed() {
        let (c, notes, diags) = read(
            "http, h, 1, interface=en0, allow-other-interface=true, dns-follow-interface=true, \
             no-error-alert=true, ip-version=prefer-v6, tfo=true, tos=0x28, ecn=on, block-quic=off, \
             test-url=http://t.example/, test-timeout=3, test-udp=apple.com@1.1.1.1, \
             underlying-proxy=Entry, hybrid=auto",
            Applies::Proxy,
        );
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            c,
            CommonOpts {
                interface: Some("en0".into()),
                allow_other_interface: true,
                dns_follow_interface: true,
                no_error_alert: true,
                ip_version: IpVersion::PreferV6,
                tfo: true,
                tos: 0x28,
                ecn: Tristate::On,
                block_quic: Tristate::Off,
                test_url: Some("http://t.example/".into()),
                test_timeout: Some(Duration::from_secs(3)),
                test_udp: Some(crate::general::UdpTest {
                    hostname: "apple.com".into(),
                    server: Ipv4Addr::new(1, 1, 1, 1),
                }),
                underlying_proxy: Some("Entry".into()),
            }
        );
        assert_eq!(
            notes.inert,
            ["dns-follow-interface", "tfo", "test-url", "test-timeout", "test-udp", "block-quic", "ecn"]
        );
        assert_eq!(notes.ios_only, ["hybrid"]);
    }

    #[test]
    fn defaults_and_false_booleans_leave_no_notes() {
        let (c, notes, diags) = read("http, h, 1, tfo=false, dns-follow-interface=false", Applies::Proxy);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(c, CommonOpts::default());
        assert!(notes.inert.is_empty() && notes.ios_only.is_empty());
        assert_eq!(c.ip_version, IpVersion::Dual);
        assert_eq!(c.tos, 0);
    }

    #[test]
    fn invalid_values_are_errors() {
        for (def, key) in [
            ("http, h, 1, tos=300", "tos"),
            ("http, h, 1, tos=0xZZ", "tos"),
            ("http, h, 1, ip-version=v5", "ip-version"),
            ("http, h, 1, interface=", "interface"),
            ("http, h, 1, test-url=ftp://x/", "test-url"),
            ("http, h, 1, test-timeout=0", "test-timeout"),
            ("http, h, 1, test-udp=apple.com", "test-udp"),
            ("http, h, 1, test-udp=apple.com@::1", "test-udp"),
            ("http, h, 1, ecn=maybe", "ecn"),
            ("http, h, 1, hybrid=sometimes", "hybrid"),
        ] {
            let (_, _, diags) = read(def, Applies::Proxy);
            assert_eq!(diags.len(), 1, "{def}: {diags:?}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM, "{def}");
            assert!(diags[0].message.contains(&format!("`{key}`")), "{def}: {}", diags[0].message);
        }
    }

    #[test]
    fn proxy_only_parameters_do_not_apply_to_direct() {
        let (c, notes, diags) = read(
            "direct, interface=utun0, underlying-proxy=Entry, ecn=on, no-error-alert=true",
            Applies::Direct,
        );
        assert_eq!(c.interface.as_deref(), Some("utun0"));
        assert_eq!((c.underlying_proxy, c.ecn, c.no_error_alert), (None, Tristate::Auto, false));
        let codes_seen: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(codes_seen, [codes::W_PARAM_NOT_APPLICABLE; 3]);
        assert_eq!(
            diags[0].message,
            "policy `P`: `underlying-proxy` does not apply to `direct` policies; ignored"
        );
        assert!(notes.inert.is_empty(), "{:?}", notes.inert);
    }

    #[test]
    fn reject_aliases_only_have_their_values_checked() {
        let (_, notes, diags) = read("reject, underlying-proxy=Entry, tfo=true, hybrid=on", Applies::Reject);
        assert!(diags.is_empty(), "{diags:?}");
        assert!(notes.inert.is_empty() && notes.ios_only.is_empty());
        let (_, _, diags) = read("reject, tos=999", Applies::Reject);
        assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
    }
}
```

创建 `crates/rurge-config/src/spec/tls.rs`，先只放测试：

```rust
//! TLS parameters shared by every TLS-carried protocol (matrix §4.4).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::codes;
    use crate::keystore::{KeystoreItem, KeystoreType};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::ParamReader;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 1)
    }

    fn keystore() -> Vec<KeystoreItem> {
        let item = |name: &str, kind| KeystoreItem {
            name: name.into(),
            kind,
            base64: "AAAA".into(),
            password: None,
            unknown: Vec::new(),
            span: span(),
        };
        vec![item("cert1", KeystoreType::P12), item("key1", KeystoreType::OpensshPrivateKey)]
    }

    fn read(def: &str) -> (TlsOpts, Vec<crate::Diagnostic>) {
        let p = parse_policy("P", def, &span()).unwrap();
        let mut r = ParamReader::new(&p);
        let tls = read_tls(&mut r, &keystore());
        (tls, r.finish())
    }

    #[test]
    fn all_six_parameters() {
        let fp = "ab".repeat(32);
        let (tls, diags) = read(&format!(
            "https, h, 443, sni=cdn.example.com, server-cert-verify-name=real.example.com, \
             server-cert-fingerprint-sha256={fp}, alpn=\"h2, http/1.1\", client-cert=cert1"
        ));
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(tls.sni, Sni::Name("cdn.example.com".into()));
        assert_eq!(tls.verify_name.as_deref(), Some("real.example.com"));
        assert_eq!(tls.fingerprint_sha256, Some([0xab; 32]));
        assert_eq!(tls.alpn, ["h2", "http/1.1"]);
        assert_eq!(tls.client_cert.as_deref(), Some("cert1"));
        assert!(!tls.skip_cert_verify);
    }

    #[test]
    fn sni_off_and_defaults() {
        let (tls, _) = read("https, h, 443, sni=OFF, skip-cert-verify=true");
        assert_eq!(tls.sni, Sni::Off);
        assert!(tls.skip_cert_verify);
        let (tls, _) = read("https, h, 443");
        assert_eq!(tls, TlsOpts::default());
    }

    #[test]
    fn a_fingerprint_wins_over_skip_cert_verify_with_a_warning() {
        let (tls, diags) = read(&format!(
            "https, h, 443, skip-cert-verify=true, server-cert-fingerprint-sha256={}",
            "00".repeat(32)
        ));
        assert!(tls.fingerprint_sha256.is_some());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, codes::W_INVALID_VALUE);
        assert_eq!(
            diags[0].message,
            "policy `P`: `skip-cert-verify` is ignored because `server-cert-fingerprint-sha256` is set"
        );
    }

    #[test]
    fn invalid_values_and_keystore_references() {
        for (def, code) in [
            ("https, h, 443, server-cert-fingerprint-sha256=abcd", codes::E_INVALID_POLICY_PARAM),
            ("https, h, 443, sni=", codes::E_INVALID_POLICY_PARAM),
            ("https, h, 443, server-cert-verify-name=", codes::E_INVALID_POLICY_PARAM),
            ("https, h, 443, client-cert=nope", codes::E_KEYSTORE_REF),
            ("https, h, 443, client-cert=key1", codes::E_KEYSTORE_REF),
        ] {
            let (_, diags) = read(def);
            assert_eq!(diags.len(), 1, "{def}: {diags:?}");
            assert_eq!(diags[0].code, code, "{def}");
        }
        let (_, diags) = read("https, h, 443, client-cert=key1");
        assert_eq!(
            diags[0].message,
            "policy `P`: `client-cert` needs a `p12` keystore item, but `key1` is `openssh-private-key`"
        );
    }

    #[test]
    fn tls_parameters_on_a_plain_protocol_do_not_apply() {
        let p = parse_policy("P", "http, h, 80, sni=x.example, skip-cert-verify=true", &span()).unwrap();
        let mut r = ParamReader::new(&p);
        refuse_tls(&mut r);
        let diags = r.finish();
        let codes_seen: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert_eq!(codes_seen, [codes::W_PARAM_NOT_APPLICABLE; 2]);
        assert_eq!(
            diags[0].message,
            "policy `P`: `skip-cert-verify` does not apply to `http` policies; ignored"
        );
    }
}
```

创建 `crates/rurge-config/src/spec/http.rs`，先只放测试：

```rust
//! `http` / `https` policy parameters (manual: Policies › HTTP and HTTP/2).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_templates() {
        let list = HeaderTemplate::parse_list(
            "X-Client:rurge; X-Pad: a<random-string(8)>b<random-string(2-5)>;Host:edge.example",
        )
        .unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], HeaderTemplate { name: "X-Client".into(), value: vec![HeaderPart::Literal("rurge".into())] });
        assert_eq!(
            list[1].value,
            [
                HeaderPart::Literal("a".into()),
                HeaderPart::Random { min: 8, max: 8 },
                HeaderPart::Literal("b".into()),
                HeaderPart::Random { min: 2, max: 5 },
            ]
        );
        assert_eq!(list[2].name, "Host");
        assert!(HeaderTemplate::parse_list("").unwrap().is_empty());
    }

    #[test]
    fn malformed_header_templates() {
        for bad in [
            "NoColon",
            ": value",
            "Bad Name: v",
            "X: <random-string(0)>",
            "X: <random-string(5-2)>",
            "X: <random-string(9999)>",
            "X: <random-string(3",
            "X: <random-string(a)>",
        ] {
            assert!(HeaderTemplate::parse_list(bad).is_err(), "{bad}");
        }
    }
}
```

把 `crates/rurge-config/src/spec/mod.rs` 整个换成（实现与测试一起；`to_spec` 的实现在 Step 3 才补上，这里先让测试出现）：

```rust
//! Typed view of `[Proxy]` policy parameters (phase 2 M1 design §4).

pub mod common;
pub mod http;
pub mod reader;
pub mod socks5;
pub mod tls;

pub use common::{Applies, CommonOpts, IpVersion, Tristate};
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
pub use reader::ParamReader;
pub use socks5::Socks5Spec;
pub use tls::{Sni, TlsOpts};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::codes;
    use crate::keystore::{KeystoreItem, KeystoreType};
    use crate::policy::{Builtin, PolicyKind, parse_policy};
    use crate::span::Span;
    use crate::types::HostName;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 3)
    }

    fn outcome(name: &str, def: &str) -> SpecOutcome {
        let p = parse_policy(name, def, &span()).unwrap();
        let keystore = vec![KeystoreItem {
            name: "cert1".into(),
            kind: KeystoreType::P12,
            base64: "AAAA".into(),
            password: Some("x".into()),
            unknown: Vec::new(),
            span: span(),
        }];
        let lookup = |n: &str| match n {
            "Entry" => Some(NameKind::Policy(PolicyKind::Socks5)),
            "Pick" => Some(NameKind::Group),
            "DIRECT" => Some(NameKind::Builtin(Builtin::Direct)),
            "REJECT" => Some(NameKind::Builtin(Builtin::Reject)),
            _ => None,
        };
        to_spec(&p, &SpecEnv { keystore: &keystore, lookup: &lookup })
    }

    #[test]
    fn manual_examples() {
        let o = outcome("ProxyHTTPS", "https, 1.2.3.4, 443, username, password");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let spec = o.spec.unwrap();
        assert_eq!(spec.name, "ProxyHTTPS");
        assert_eq!(spec.kind, PolicyKind::Https);
        assert_eq!(spec.server, Some(HostName::parse("1.2.3.4")));
        assert_eq!(spec.port, Some(443));
        let ProtoSpec::Http(http) = &spec.proto else { panic!("{:?}", spec.proto) };
        assert_eq!(http.username.as_deref(), Some("username"));
        assert_eq!(http.password.as_deref(), Some("password"));
        assert_eq!(http.tls, Some(TlsOpts::default()));
        assert!(!http.always_use_connect);

        let o = outcome("ProxySOCKS5TLS", "socks5-tls, 1.2.3.4, 443, username, password, skip-cert-verify=false");
        let ProtoSpec::Socks5(s) = &o.spec.unwrap().proto else { panic!() };
        assert!(s.tls.is_some() && !s.udp_relay);

        let o = outcome("Plain", "http, proxy.example.com, 8080, always-use-connect=true, headers=X-A:1;X-B:2");
        let ProtoSpec::Http(http) = &o.spec.unwrap().proto else { panic!() };
        assert!(http.tls.is_none() && http.always_use_connect);
        assert_eq!(http.headers.len(), 2);
    }

    #[test]
    fn named_credentials_win_over_positional_ones() {
        let o = outcome("P", "socks5, h, 1080, posuser, pospass, username=named, password=secret");
        let ProtoSpec::Socks5(s) = &o.spec.unwrap().proto else { panic!() };
        assert_eq!((s.username.as_deref(), s.password.as_deref()), (Some("named"), Some("secret")));
    }

    #[test]
    fn aliases_get_a_spec_too() {
        let o = outcome("Corp", "direct, interface=utun0");
        let spec = o.spec.unwrap();
        assert_eq!(spec.proto, ProtoSpec::Direct);
        assert_eq!(spec.common.interface.as_deref(), Some("utun0"));
        assert_eq!((spec.server, spec.port), (None, None));
        let o = outcome("Block", "reject-tinygif");
        assert_eq!(o.spec.unwrap().proto, ProtoSpec::Reject(Builtin::RejectTinyGif));
    }

    #[test]
    fn protocols_without_a_spec_are_left_alone() {
        let o = outcome("SS", "ss, h, 8388, encrypt-method=aes-128-gcm, password=x, mystery=1");
        assert!(o.spec.is_none() && o.diagnostics.is_empty() && o.inert.is_empty());
    }

    #[test]
    fn underlying_proxy_references() {
        assert_eq!(
            outcome("P", "http, h, 80, underlying-proxy=Pick").spec.unwrap().common.underlying_proxy.as_deref(),
            Some("Pick")
        );
        // DIRECT means "no chain"
        assert_eq!(outcome("P", "http, h, 80, underlying-proxy=DIRECT").spec.unwrap().common.underlying_proxy, None);
        let o = outcome("P", "http, h, 80, underlying-proxy=Ghost");
        assert!(o.spec.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_UNKNOWN_POLICY_REF);
        assert_eq!(o.diagnostics[0].message, "policy `P`: `underlying-proxy` references unknown policy `Ghost`");
        let o = outcome("P", "http, h, 80, underlying-proxy=REJECT");
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
    }

    #[test]
    fn notes_unknowns_and_limits() {
        let o = outcome("P", "socks5, h, 1080, udp-relay=true, shadow-tls-password=pw, mystery=1");
        assert_eq!(o.inert, ["udp-relay", "shadow-tls-password"]);
        let warnings: Vec<&str> = o.diagnostics.iter().map(|d| d.code).collect();
        assert_eq!(warnings, [codes::W_UNKNOWN_KEY]);
        assert!(o.spec.is_some(), "warnings do not drop the spec");

        let long = "u".repeat(256);
        let o = outcome("P", &format!("socks5, h, 1080, {long}, pw"));
        assert!(o.spec.is_none());
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
        assert!(!o.diagnostics[0].message.contains(&long), "credentials are never echoed");

        let o = outcome("P", "http, h, 80, headers=Broken");
        assert_eq!(o.diagnostics[0].code, codes::E_INVALID_POLICY_PARAM);
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-config spec::`
Expected: 编译失败——`CommonOpts`、`read_common`、`TlsOpts`、`read_tls`、`HeaderTemplate`、`to_spec` 等未定义；`spec/socks5.rs` 不存在。

- [ ] **Step 3: 实现**

在 `crates/rurge-config/src/spec/common.rs` 的模块注释之后、测试模块之前加：

```rust
use super::reader::ParamReader;
use crate::diagnostic::codes;
use crate::general::UdpTest;
use std::net::Ipv4Addr;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum IpVersion {
    #[default]
    Dual,
    V4Only,
    V6Only,
    PreferV4,
    PreferV6,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tristate {
    #[default]
    Auto,
    On,
    Off,
}

/// Which kind of policy the parameters are written on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applies {
    Proxy,
    Direct,
    /// `reject*` aliases accept the common parameters; none has any effect.
    Reject,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommonOpts {
    pub interface: Option<String>,
    pub allow_other_interface: bool,
    pub dns_follow_interface: bool,
    pub no_error_alert: bool,
    pub ip_version: IpVersion,
    pub tfo: bool,
    pub tos: u8,
    pub ecn: Tristate,
    pub block_quic: Tristate,
    pub test_url: Option<String>,
    pub test_timeout: Option<Duration>,
    pub test_udp: Option<UdpTest>,
    pub underlying_proxy: Option<String>,
}

/// Parameter names the caller reports once per load: parsed but without
/// effect in this version (`W0029`), and iOS-only (`W0004`).
#[derive(Debug, Default)]
pub(crate) struct Notes {
    pub inert: Vec<&'static str>,
    pub ios_only: Vec<&'static str>,
}

const IP_VERSIONS: [(&str, IpVersion); 5] = [
    ("dual", IpVersion::Dual),
    ("v4-only", IpVersion::V4Only),
    ("v6-only", IpVersion::V6Only),
    ("prefer-v4", IpVersion::PreferV4),
    ("prefer-v6", IpVersion::PreferV6),
];

/// `true` / `false` are accepted wherever `on` / `off` are (manual, `hybrid` and `ecn`).
const TRISTATES: [(&str, Tristate); 5] = [
    ("auto", Tristate::Auto),
    ("on", Tristate::On),
    ("off", Tristate::Off),
    ("true", Tristate::On),
    ("false", Tristate::Off),
];

/// Parameters the manual marks "proxy policies only".
const PROXY_ONLY: [&str; 3] = ["underlying-proxy", "ecn", "no-error-alert"];

fn parse_tos(value: &str) -> Option<u8> {
    let value = value.trim();
    match value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        Some(hex) => u8::from_str_radix(hex, 16).ok(),
        None => value.parse().ok(),
    }
}

fn parse_udp_test(value: &str) -> Option<UdpTest> {
    let (hostname, server) = value.split_once('@')?;
    let hostname = hostname.trim();
    if hostname.is_empty() {
        return None;
    }
    Some(UdpTest {
        hostname: hostname.to_string(),
        server: server.trim().parse::<Ipv4Addr>().ok()?,
    })
}

pub(crate) fn read_common(r: &mut ParamReader<'_>, applies: Applies, notes: &mut Notes) -> CommonOpts {
    let mut interface = None;
    if let Some(v) = r.str("interface") {
        if v.trim().is_empty() {
            r.invalid("interface", v, "a network interface name");
        } else {
            interface = Some(v.trim().to_string());
        }
    }
    let allow_other_interface = r.bool("allow-other-interface").unwrap_or(false);
    let dns_follow_interface = r.bool("dns-follow-interface").unwrap_or(false);
    let mut no_error_alert = r.bool("no-error-alert").unwrap_or(false);
    let ip_version = r.choice("ip-version", &IP_VERSIONS).unwrap_or_default();
    let tfo = r.bool("tfo").unwrap_or(false);
    let mut tos = 0;
    if let Some(v) = r.str("tos") {
        match parse_tos(v) {
            Some(n) => tos = n,
            None => r.invalid("tos", v, "0-255 or 0x00-0xff"),
        }
    }
    let ecn_present = r.has("ecn");
    let mut ecn = r.choice("ecn", &TRISTATES).unwrap_or_default();
    let block_quic_present = r.has("block-quic");
    let block_quic = r.choice("block-quic", &TRISTATES).unwrap_or_default();
    let mut test_url = None;
    if let Some(v) = r.str("test-url") {
        let lower = v.trim().to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            test_url = Some(v.trim().to_string());
        } else {
            r.invalid("test-url", v, "an http:// or https:// URL");
        }
    }
    let mut test_timeout = None;
    if let Some(v) = r.str("test-timeout") {
        match v.trim().parse::<u32>() {
            Ok(secs) if secs > 0 => test_timeout = Some(Duration::from_secs(u64::from(secs))),
            _ => r.invalid("test-timeout", v, "seconds, at least 1"),
        }
    }
    let mut test_udp = None;
    if let Some(v) = r.str("test-udp") {
        match parse_udp_test(v) {
            Some(t) => test_udp = Some(t),
            None => r.invalid("test-udp", v, "hostname@ipv4"),
        }
    }
    let mut underlying_proxy = r
        .str("underlying-proxy")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string);
    let hybrid_present = r.has("hybrid");
    let _ = r.choice("hybrid", &TRISTATES);

    if applies == Applies::Direct {
        for key in PROXY_ONLY {
            if r.has(key) {
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` does not apply to `direct` policies; ignored"),
                );
            }
        }
        underlying_proxy = None;
        ecn = Tristate::Auto;
        no_error_alert = false;
    }
    if applies != Applies::Reject {
        let inert = [
            ("dns-follow-interface", dns_follow_interface),
            ("tfo", tfo),
            ("test-url", test_url.is_some()),
            ("test-timeout", test_timeout.is_some()),
            ("test-udp", test_udp.is_some()),
            ("block-quic", block_quic_present),
            ("ecn", ecn_present && applies == Applies::Proxy),
        ];
        notes
            .inert
            .extend(inert.iter().filter(|(_, on)| *on).map(|(name, _)| *name));
        if hybrid_present {
            notes.ios_only.push("hybrid");
        }
    }
    CommonOpts {
        interface,
        allow_other_interface,
        dns_follow_interface,
        no_error_alert,
        ip_version,
        tfo,
        tos,
        ecn,
        block_quic,
        test_url,
        test_timeout,
        test_udp,
        underlying_proxy,
    }
}
```

在 `common.rs` 顶部的 `use` 之后，测试模块需要的 `ParamReader` 已由 `use super::reader::ParamReader;` 引入；测试模块里的 `use super::*;` 会带上它。

`crates/rurge-config/src/spec/tls.rs`，加在模块注释之后、测试模块之前：

```rust
use super::common::Notes;
use super::reader::ParamReader;
use crate::diagnostic::codes;
use crate::keystore::{KeystoreItem, KeystoreType};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Sni {
    /// Send the proxy's host name (nothing for an IP literal).
    #[default]
    Default,
    /// `sni = off`: no SNI extension at all.
    Off,
    Name(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TlsOpts {
    pub skip_cert_verify: bool,
    pub sni: Sni,
    /// Verify the certificate against this name instead of the SNI name.
    pub verify_name: Option<String>,
    /// SHA-256 of the pinned leaf certificate (DER); replaces chain validation.
    pub fingerprint_sha256: Option<[u8; 32]>,
    pub alpn: Vec<String>,
    /// Name of a `p12` `[Keystore]` item.
    pub client_cert: Option<String>,
}

pub(crate) const TLS_KEYS: [&str; 6] = [
    "skip-cert-verify",
    "sni",
    "server-cert-verify-name",
    "server-cert-fingerprint-sha256",
    "alpn",
    "client-cert",
];

/// Shadow TLS arrives in M2; until then the parameters are known but inert.
pub(crate) const SHADOW_TLS_KEYS: [&str; 3] =
    ["shadow-tls-password", "shadow-tls-sni", "shadow-tls-version"];

fn parse_fingerprint(value: &str) -> Option<[u8; 32]> {
    let value = value.trim();
    if value.len() != 64 || !value.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub(crate) fn read_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let skip_cert_verify = r.bool("skip-cert-verify").unwrap_or(false);
    let mut sni = Sni::Default;
    if let Some(v) = r.str("sni") {
        let v = v.trim();
        if v.is_empty() {
            r.invalid("sni", v, "a host name or off");
        } else if v.eq_ignore_ascii_case("off") {
            sni = Sni::Off;
        } else {
            sni = Sni::Name(v.to_string());
        }
    }
    let mut verify_name = None;
    if let Some(v) = r.str("server-cert-verify-name") {
        if v.trim().is_empty() {
            r.invalid("server-cert-verify-name", v, "a host name");
        } else {
            verify_name = Some(v.trim().to_string());
        }
    }
    let mut fingerprint_sha256 = None;
    if let Some(v) = r.str("server-cert-fingerprint-sha256") {
        match parse_fingerprint(v) {
            Some(fp) => fingerprint_sha256 = Some(fp),
            None => r.invalid("server-cert-fingerprint-sha256", v, "64 hexadecimal characters"),
        }
    }
    let alpn = r
        .str("alpn")
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut client_cert = None;
    if let Some(v) = r.str("client-cert") {
        let name = v.trim();
        match keystore.iter().find(|k| k.name == name) {
            None => r.error(
                codes::E_KEYSTORE_REF,
                format!("`client-cert` references unknown keystore item `{name}`"),
            ),
            Some(item) if item.kind != KeystoreType::P12 => r.error(
                codes::E_KEYSTORE_REF,
                format!("`client-cert` needs a `p12` keystore item, but `{name}` is `openssh-private-key`"),
            ),
            Some(_) => client_cert = Some(name.to_string()),
        }
    }
    if skip_cert_verify && fingerprint_sha256.is_some() {
        r.warn(
            codes::W_INVALID_VALUE,
            "`skip-cert-verify` is ignored because `server-cert-fingerprint-sha256` is set".to_string(),
        );
    }
    TlsOpts {
        skip_cert_verify,
        sni,
        verify_name,
        fingerprint_sha256,
        alpn,
        client_cert,
    }
}

/// For protocols that do not run over TLS: every TLS parameter present is `W0028`.
pub(crate) fn refuse_tls(r: &mut ParamReader<'_>) {
    let kind = r.policy().kind.keyword();
    let mut present: Vec<&str> = TLS_KEYS.iter().copied().filter(|k| r.has(k)).collect();
    present.sort_unstable();
    for key in present {
        r.touch(key);
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`{key}` does not apply to `{kind}` policies; ignored"),
        );
    }
}

pub(crate) fn note_shadow_tls(r: &mut ParamReader<'_>, notes: &mut Notes) {
    for key in SHADOW_TLS_KEYS {
        if r.has(key) {
            r.touch(key);
            notes.inert.push(key);
        }
    }
}
```

`crates/rurge-config/src/spec/http.rs`，加在模块注释之后、测试模块之前：

```rust
use super::tls::TlsOpts;

/// One piece of a header value: literal text or a random URL-safe string
/// rendered anew for every connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderPart {
    Literal(String),
    Random { min: usize, max: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderTemplate {
    pub name: String,
    pub value: Vec<HeaderPart>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpSpec {
    /// `Some` for `https`.
    pub tls: Option<TlsOpts>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub always_use_connect: bool,
    pub headers: Vec<HeaderTemplate>,
}

/// Longest random string a placeholder may ask for.
const MAX_RANDOM: usize = 1024;
const PLACEHOLDER: &str = "<random-string(";

fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn parse_value(text: &str) -> Result<Vec<HeaderPart>, String> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(PLACEHOLDER) {
        if at > 0 {
            parts.push(HeaderPart::Literal(rest[..at].to_string()));
        }
        let after = &rest[at + PLACEHOLDER.len()..];
        let end = after
            .find(")>")
            .ok_or_else(|| "unterminated <random-string(...)>".to_string())?;
        let spec = &after[..end];
        let (min, max) = match spec.split_once('-') {
            Some((a, b)) => (a.trim().parse::<usize>(), b.trim().parse::<usize>()),
            None => (spec.trim().parse::<usize>(), spec.trim().parse::<usize>()),
        };
        let (Ok(min), Ok(max)) = (min, max) else {
            return Err(format!("invalid length in <random-string({spec})>"));
        };
        if min == 0 || min > max || max > MAX_RANDOM {
            return Err(format!("length in <random-string({spec})> must be 1-{MAX_RANDOM}"));
        }
        parts.push(HeaderPart::Random { min, max });
        rest = &after[end + 2..];
    }
    if !rest.is_empty() {
        parts.push(HeaderPart::Literal(rest.to_string()));
    }
    Ok(parts)
}

impl HeaderTemplate {
    /// `Name:value;Name:value`. A value may hold `<random-string(n)>` and
    /// `<random-string(min-max)>` placeholders.
    pub fn parse_list(list: &str) -> Result<Vec<HeaderTemplate>, String> {
        let mut out = Vec::new();
        for item in list.split(';').map(str::trim).filter(|i| !i.is_empty()) {
            let (name, value) = item
                .split_once(':')
                .ok_or_else(|| format!("header `{item}` has no `:`"))?;
            let name = name.trim();
            if !is_token(name) {
                return Err(format!("`{name}` is not a valid header name"));
            }
            let value = value.trim();
            if value.contains(['\r', '\n']) {
                return Err(format!("header `{name}` holds a line break"));
            }
            out.push(HeaderTemplate {
                name: name.to_string(),
                value: parse_value(value)?,
            });
        }
        Ok(out)
    }
}
```

创建 `crates/rurge-config/src/spec/socks5.rs`：

```rust
//! `socks5` / `socks5-tls` policy parameters (manual: Policies › SOCKS5).

use super::tls::TlsOpts;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Socks5Spec {
    /// `Some` for `socks5-tls`.
    pub tls: Option<TlsOpts>,
    pub username: Option<String>,
    pub password: Option<String>,
    /// Parsed now; UDP ASSOCIATE arrives in M5.
    pub udp_relay: bool,
}

/// RFC 1929 carries each of the two in a one-byte length field.
pub(crate) const MAX_CREDENTIAL: usize = 255;
```

在 `crates/rurge-config/src/spec/mod.rs` 的 `pub use tls::{Sni, TlsOpts};` 之后、测试模块之前加：

```rust
use crate::diagnostic::{Diagnostic, codes};
use crate::keystore::KeystoreItem;
use crate::policy::{Builtin, PolicyKind, ProxyPolicy};
use crate::span::Span;
use crate::types::HostName;
use common::{Notes, read_common};

/// What a name on a policy line refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameKind {
    Policy(PolicyKind),
    Group,
    Builtin(Builtin),
}

pub struct SpecEnv<'a> {
    pub keystore: &'a [KeystoreItem],
    pub lookup: &'a dyn Fn(&str) -> Option<NameKind>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoSpec {
    Direct,
    Reject(Builtin),
    Http(HttpSpec),
    Socks5(Socks5Spec),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicySpec {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub common: CommonOpts,
    pub proto: ProtoSpec,
    pub span: Span,
}

#[derive(Debug, Default)]
pub struct SpecOutcome {
    /// `None` when the policy type has no spec yet, or when an error was found.
    pub spec: Option<PolicySpec>,
    pub diagnostics: Vec<Diagnostic>,
    /// Parameters present on the line that are parsed but do nothing yet;
    /// the caller reports each name once per load (`W0029`).
    pub inert: Vec<&'static str>,
    /// iOS-only parameters present on the line (`W0004`, once per load).
    pub ios_only: Vec<&'static str>,
}

/// Named `username=` / `password=` win over the positional pair.
fn read_credentials(r: &mut ParamReader<'_>) -> (Option<String>, Option<String>) {
    let positional = (r.positional(0), r.positional(1));
    let username = r.str("username").or(positional.0).map(str::to_string);
    let password = r.str("password").or(positional.1).map(str::to_string);
    (username, password)
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
    let Some(name) = common.underlying_proxy.clone() else {
        return;
    };
    match (env.lookup)(&name) {
        None => r.error(
            codes::E_UNKNOWN_POLICY_REF,
            format!("`underlying-proxy` references unknown policy `{name}`"),
        ),
        // DIRECT is the absence of a chain
        Some(NameKind::Builtin(Builtin::Direct)) => common.underlying_proxy = None,
        Some(NameKind::Builtin(_)) => r.invalid("underlying-proxy", &name, "a proxy policy or a policy group"),
        Some(NameKind::Policy(_) | NameKind::Group) => {}
    }
}

pub fn to_spec(policy: &ProxyPolicy, env: &SpecEnv<'_>) -> SpecOutcome {
    let mut r = ParamReader::new(policy);
    let mut notes = Notes::default();
    let (mut common, proto) = match policy.kind {
        PolicyKind::Direct => {
            let common = read_common(&mut r, Applies::Direct, &mut notes);
            tls::refuse_tls(&mut r);
            (common, ProtoSpec::Direct)
        }
        PolicyKind::Reject
        | PolicyKind::RejectDrop
        | PolicyKind::RejectNoDrop
        | PolicyKind::RejectTinyGif => {
            let common = read_common(&mut r, Applies::Reject, &mut notes);
            let builtin = match policy.kind {
                PolicyKind::RejectDrop => Builtin::RejectDrop,
                PolicyKind::RejectNoDrop => Builtin::RejectNoDrop,
                PolicyKind::RejectTinyGif => Builtin::RejectTinyGif,
                _ => Builtin::Reject,
            };
            (common, ProtoSpec::Reject(builtin))
        }
        PolicyKind::Http | PolicyKind::Https => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tls = if policy.kind == PolicyKind::Https {
                Some(tls::read_tls(&mut r, env.keystore))
            } else {
                tls::refuse_tls(&mut r);
                None
            };
            tls::note_shadow_tls(&mut r, &mut notes);
            let (username, password) = read_credentials(&mut r);
            let always_use_connect = r.bool("always-use-connect").unwrap_or(false);
            let mut headers = Vec::new();
            if let Some(v) = r.str("headers") {
                match HeaderTemplate::parse_list(v) {
                    Ok(list) => headers = list,
                    Err(why) => r.error(codes::E_INVALID_POLICY_PARAM, format!("invalid `headers`: {why}")),
                }
            }
            (
                common,
                ProtoSpec::Http(HttpSpec { tls, username, password, always_use_connect, headers }),
            )
        }
        PolicyKind::Socks5 | PolicyKind::Socks5Tls => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let tls = if policy.kind == PolicyKind::Socks5Tls {
                Some(tls::read_tls(&mut r, env.keystore))
            } else {
                tls::refuse_tls(&mut r);
                None
            };
            tls::note_shadow_tls(&mut r, &mut notes);
            let (username, password) = read_credentials(&mut r);
            for (what, value) in [("username", &username), ("password", &password)] {
                if value.as_ref().is_some_and(|v| v.len() > socks5::MAX_CREDENTIAL) {
                    // never echo the value
                    r.error(
                        codes::E_INVALID_POLICY_PARAM,
                        format!("`{what}` is longer than the {} bytes SOCKS5 allows", socks5::MAX_CREDENTIAL),
                    );
                }
            }
            let udp_relay = r.bool("udp-relay").unwrap_or(false);
            if udp_relay {
                notes.inert.insert(0, "udp-relay");
            }
            (common, ProtoSpec::Socks5(Socks5Spec { tls, username, password, udp_relay }))
        }
        _ => return SpecOutcome::default(),
    };
    if matches!(proto, ProtoSpec::Http(_) | ProtoSpec::Socks5(_)) {
        check_underlying(&mut r, &mut common, env);
    }
    let failed = r.has_errors();
    let diagnostics = r.finish();
    let spec = (!failed).then(|| PolicySpec {
        name: policy.name.clone(),
        kind: policy.kind,
        server: policy.server.clone(),
        port: policy.port,
        common,
        proto,
        span: policy.span.clone(),
    });
    SpecOutcome {
        spec,
        diagnostics,
        inert: notes.inert,
        ios_only: notes.ios_only,
    }
}
```

`notes_unknowns_and_limits` 期望 `inert == ["udp-relay", "shadow-tls-password"]`：`udp-relay` 用 `insert(0, ..)` 放在最前，是为了让协议自己的参数排在传输层参数之前；不要改成 `push`。

在 `crates/rurge-config/src/lib.rs` 的 `pub use span::Span;` 之前加：

```rust
pub use spec::{PolicySpec, ProtoSpec};
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-config spec::`
Expected: PASS（新增 `common` 5 个、`tls` 5 个、`http` 2 个、`spec::tests` 6 个）。

- [ ] **Step 5: 门禁与提交**

```bash
git add crates/rurge-config/src/spec crates/rurge-config/src/lib.rs
git commit -m "feat(config): 通用参数、TLS 参数、http / socks5 的 PolicySpec 与 to_spec 入口"
```

---

### Task 3: 把 spec 接进 `validate`：`Config.specs`、环检测、Keystore Base64、语料库

**Files:**
- Modify: `crates/rurge-config/Cargo.toml`（加 `base64.workspace = true`）
- Modify: `crates/rurge-config/src/config.rs`
- Create: `crates/rurge-config/tests/policy_spec.rs`
- Create: `tests/corpus/invalid/policy-params.conf`
- Create: `tests/corpus/invalid/policy-params.expect`
- Modify: `crates/rurge-config/tests/snapshots/corpus__kitchen-sink.snap`（由 insta 重写）

**Interfaces:**
- Consumes: Task 2 的 `to_spec` `SpecEnv` `NameKind` `PolicySpec`；`Config::resolve_policy(&self, name) -> Option<PolicyTarget<'_>>`（`PolicyTarget::{Builtin(Builtin), Proxy(&ProxyPolicy), Group(&PolicyGroup)}`）；`Diagnostics::push`。
- Produces:
  - `Config.specs: Vec<PolicySpec>`（与 `[Proxy]` 同序，只含有 spec 且无错误的策略）
  - `Config::spec(&self, name: &str) -> Option<&PolicySpec>`
  - 诊断：`W0029`（每个参数名一次，落在第一条用到它的策略行上）、`W0004`（`hybrid`）、`E0019`、`E0021`

- [ ] **Step 1: 写失败的测试**

创建 `crates/rurge-config/tests/policy_spec.rs`：

```rust
//! `[Proxy]` parameters are typed and validated at load time (phase 2 M1 design §4).

use rurge_config::config::{LoadOptions, Loaded, from_text};
use rurge_config::spec::{IpVersion, ProtoSpec};
use rurge_config::{Severity, codes};
use std::path::Path;

fn load(proxy: &str, extra: &str) -> Loaded {
    let text = format!("[Proxy]\n{proxy}\n{extra}\n[Rule]\nFINAL,DIRECT\n");
    from_text(&text, Path::new("t.conf"), &LoadOptions::for_tests())
}

fn codes_of(loaded: &Loaded, severity: Severity) -> Vec<&'static str> {
    loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .map(|d| d.code)
        .collect()
}

#[test]
fn specs_are_stored_in_proxy_order() {
    let loaded = load(
        "A = http, a.example, 80, ip-version=v6-only\nSS = ss, s.example, 8388, encrypt-method=aes-128-gcm, password=x\nB = socks5, b.example, 1080\nC = direct, interface=eth0",
        "",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let names: Vec<&str> = loaded.config.specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["A", "B", "C"], "ss has no spec yet");
    assert_eq!(loaded.config.spec("A").unwrap().common.ip_version, IpVersion::V6Only);
    assert!(matches!(loaded.config.spec("B").unwrap().proto, ProtoSpec::Socks5(_)));
    assert!(loaded.config.spec("SS").is_none() && loaded.config.spec("nope").is_none());
}

#[test]
fn inert_and_ios_only_parameters_are_reported_once_per_name() {
    let loaded = load(
        "A = socks5, a.example, 1080, udp-relay=true, tfo=true\nB = socks5, b.example, 1080, udp-relay=true, hybrid=on\nC = http, c.example, 80, hybrid=off",
        "",
    );
    let warnings: Vec<(&str, String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::W_PARAM_NOT_EFFECTIVE || d.code == codes::W_PLATFORM_IGNORED)
        .map(|d| (d.code, d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        warnings,
        [
            (codes::W_PARAM_NOT_EFFECTIVE, "policy parameter `udp-relay` is parsed but has no effect in this version".to_string(), 2),
            (codes::W_PARAM_NOT_EFFECTIVE, "policy parameter `tfo` is parsed but has no effect in this version".to_string(), 2),
            (codes::W_PLATFORM_IGNORED, "policy parameter `hybrid` is iOS-only; ignored".to_string(), 3),
        ]
    );
}

#[test]
fn an_invalid_parameter_fails_the_load_and_leaves_no_spec() {
    let loaded = load("A = http, a.example, 80, tos=300", "");
    assert_eq!(codes_of(&loaded, Severity::Error), [codes::E_INVALID_POLICY_PARAM]);
    assert!(loaded.config.spec("A").is_none());
}

#[test]
fn underlying_proxy_cycles() {
    // direct cycle
    let loaded = load("A = http, a.example, 80, underlying-proxy=B\nB = http, b.example, 80, underlying-proxy=A", "");
    assert_eq!(codes_of(&loaded, Severity::Error), [codes::E_UNDERLYING_PROXY_CYCLE; 2]);
    // through a group that lists the policy itself
    let loaded = load(
        "A = http, a.example, 80, underlying-proxy=Pick\nB = socks5, b.example, 1080",
        "[Proxy Group]\nPick = select, B, A",
    );
    let errors: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{} {}", d.code, d.message))
        .collect();
    assert_eq!(errors, ["E0019 policy `A`: `underlying-proxy` leads back to the policy itself (via `Pick`)"]);
    // a plain chain is fine
    let loaded = load(
        "Exit = http, a.example, 80, underlying-proxy=Pick\nEntry = socks5, b.example, 1080",
        "[Proxy Group]\nPick = select, Entry, DIRECT",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
}

#[test]
fn keystore_base64_is_checked_at_load() {
    let loaded = load("A = direct", "[Keystore]\ngood = type=p12, base64=QUJD, password=x\nnopad = base64=QUI\nbad = type=p12, base64=@@@, password=x");
    let errors: Vec<(String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::E_KEYSTORE_BASE64)
        .map(|d| (d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(errors, [("keystore item `bad`: `base64` is not valid Base64".to_string(), 6)]);
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-config --test policy_spec`
Expected: 编译失败——`Config` 没有 `specs` 字段与 `spec` 方法。

- [ ] **Step 3: 实现接线**

`crates/rurge-config/Cargo.toml` 的 `[dependencies]` 加一行 `base64.workspace = true`。

在 `crates/rurge-config/src/config.rs`：

1. `use` 区加：

```rust
use crate::spec::{NameKind, PolicySpec, SpecEnv, to_spec};
use base64::Engine as _;
```

2. `pub struct Config` 里，`pub policies: Vec<ProxyPolicy>,` 之后加：

```rust
    /// Typed parameters of every policy whose type has a spec (same order as
    /// `policies`; policies with errors are absent).
    pub specs: Vec<PolicySpec>,
```

`from_profile` 里构造 `Config { .. }` 的地方（`let mut config = Config {`）加 `specs: Vec::new(),`。

3. `impl Config` 里，`resolve_policy` 之后加：

```rust
    pub fn spec(&self, name: &str) -> Option<&PolicySpec> {
        self.specs.iter().find(|s| s.name == name)
    }
```

4. `from_profile` 的 `[Keystore]` 循环里，`keystore.push(k);` 之前加：

```rust
                if !is_base64(&k.base64) {
                    diags.push(
                        Diagnostic::error(
                            codes::E_KEYSTORE_BASE64,
                            format!("keystore item `{}`: `base64` is not valid Base64", k.name),
                        )
                        .at(span.clone()),
                    );
                }
```

5. 把 `validate` 里的内嵌函数 `fn group_refs(g: &PolicyGroup) -> Vec<String>` 原样挪到模块级（`validate` 之前），`validate` 与它里面的 `dfs` 继续调用它；再在它旁边加：

```rust
fn is_base64(text: &str) -> bool {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
    STANDARD.decode(text).is_ok() || STANDARD_NO_PAD.decode(text).is_ok()
}

/// `E0019`: follows `underlying-proxy` edges and group membership from each
/// chained policy; reaching the policy again is a cycle.
fn underlying_cycles(config: &Config, specs: &[PolicySpec], diags: &mut Diagnostics) {
    let mut edges: HashMap<&str, Vec<String>> = HashMap::new();
    for s in specs {
        if let Some(u) = &s.common.underlying_proxy {
            edges.insert(s.name.as_str(), vec![u.clone()]);
        }
    }
    for g in &config.groups {
        edges.insert(g.name.as_str(), group_refs(g));
    }
    for s in specs {
        let Some(first) = &s.common.underlying_proxy else {
            continue;
        };
        let mut seen: HashSet<String> = HashSet::new();
        let mut stack = vec![first.clone()];
        let mut cyclic = false;
        while let Some(name) = stack.pop() {
            if name == s.name {
                cyclic = true;
                break;
            }
            if seen.insert(name.clone())
                && let Some(next) = edges.get(name.as_str())
            {
                stack.extend(next.iter().cloned());
            }
        }
        if cyclic {
            diags.push(
                Diagnostic::error(
                    codes::E_UNDERLYING_PROXY_CYCLE,
                    format!(
                        "policy `{}`: `underlying-proxy` leads back to the policy itself (via `{first}`)",
                        s.name
                    ),
                )
                .at(s.span.clone()),
            );
        }
    }
}
```

6. `validate` 里，`// Capabilities.` 注释之前加：

```rust
    // Typed policy parameters (phase 2 M1 design §4).
    let specs = {
        let cfg: &Config = config;
        let lookup = |name: &str| -> Option<NameKind> {
            Some(match cfg.resolve_policy(name)? {
                PolicyTarget::Builtin(b) => NameKind::Builtin(b),
                PolicyTarget::Proxy(p) => NameKind::Policy(p.kind),
                PolicyTarget::Group(_) => NameKind::Group,
            })
        };
        let env = SpecEnv {
            keystore: &cfg.keystore,
            lookup: &lookup,
        };
        let mut specs = Vec::new();
        let mut inert_seen: HashSet<&'static str> = HashSet::new();
        let mut ios_seen: HashSet<&'static str> = HashSet::new();
        for p in &cfg.policies {
            let outcome = to_spec(p, &env);
            for d in outcome.diagnostics {
                diags.push(d);
            }
            for name in outcome.inert {
                if inert_seen.insert(name) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_PARAM_NOT_EFFECTIVE,
                            format!("policy parameter `{name}` is parsed but has no effect in this version"),
                        )
                        .at(p.span.clone()),
                    );
                }
            }
            for name in outcome.ios_only {
                if ios_seen.insert(name) {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_PLATFORM_IGNORED,
                            format!("policy parameter `{name}` is iOS-only; ignored"),
                        )
                        .at(p.span.clone()),
                    );
                }
            }
            specs.extend(outcome.spec);
        }
        underlying_cycles(cfg, &specs, diags);
        specs
    };
    config.specs = specs;
```

若 `config.rs` 里已有别的代码因为 `Config` 多了一个字段而编译不过（例如测试里手工构造 `Config`），照样补上 `specs: Vec::new()`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-config --test policy_spec`
Expected: PASS（5 个）。

- [ ] **Step 5: 语料库——新增 `invalid` 样本**

创建 `tests/corpus/invalid/policy-params.conf`：

```
[General]
loglevel = notify

[Proxy]
BadTos = http, proxy.example.com, 8080, tos=300
LoopA = http, a.example.com, 80, underlying-proxy=LoopB
LoopB = socks5, b.example.com, 1080, underlying-proxy=LoopA
NoCert = https, c.example.com, 443, client-cert=missing

[Keystore]
broken = type=p12, base64=@@not-base64@@, password=x

[Rule]
FINAL,DIRECT
```

创建 `tests/corpus/invalid/policy-params.expect`：

```
E0018
E0019
E0020
E0021
```

Run: `cargo test -p rurge-config --test corpus invalid_corpus_reports_expected_codes`
Expected: PASS。

- [ ] **Step 6: 语料库——核对 `valid` 快照的变化**

Run: `cargo test -p rurge-config --test corpus valid_corpus`
Expected: FAIL，只有 `corpus__kitchen-sink` 的快照不一致（`valid` 语料库仍然零错误——这条断言在快照比较之前，若它失败说明 spec 校验误伤了合法配置，必须先修校验而不是改语料）。

Run: `INSTA_UPDATE=always cargo test -p rurge-config --test corpus valid_corpus && git diff --stat crates/rurge-config/tests/snapshots`
Expected: 只有 `corpus__kitchen-sink.snap` 变化。`git diff crates/rurge-config/tests/snapshots` 里新增的行必须恰好是这两条（顺序以 `sorted()` 为准），多一条少一条都要停下来查原因：

```
warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version
warning[W0029] valid/kitchen-sink.conf:55: policy parameter `udp-relay` is parsed but has no effect in this version
```

- [ ] **Step 7: 门禁与提交**

```bash
git add crates/rurge-config Cargo.lock tests/corpus/invalid/policy-params.conf tests/corpus/invalid/policy-params.expect
git commit -m "feat(config): PolicySpec 接入 validate：Config.specs、underlying-proxy 环检测、Keystore Base64 校验与语料库"
```

---

### Task 4: `rurge_net::socket`——socket 选项、平台钩子与连接竞速

**Files:**
- Modify: `Cargo.toml`（工作区：`[workspace.dependencies]` 加 `socket2`）
- Modify: `crates/rurge-net/Cargo.toml`
- Modify: `crates/rurge-net/src/lib.rs`
- Create: `crates/rurge-net/src/socket.rs`

**Interfaces:**
- Consumes: `rurge_config::spec::IpVersion`（Task 2）；`rurge_net::connector::interleave(addrs: Vec<IpAddr>, prefer_v6: bool) -> Vec<IpAddr>`（现有）。
- Produces（都在 `rurge_net::socket`）：
  - `pub const STAGGER: Duration`（250 ms）、`pub const OTHER_FAMILY_AFTER: Duration`（3 s）
  - `pub struct SocketOpts { pub interface: Option<String>, pub allow_other_interface: bool, pub ip_version: IpVersion, pub v6_first: bool, pub tos: u8 }`（`Clone + Debug + Default + PartialEq + Eq`）
  - `pub enum Family { V4, V6 }`，`Family::of(&IpAddr) -> Family`
  - `pub trait SocketHook: Send + Sync { fn bind_interface(&self, &socket2::Socket, &str, Family) -> io::Result<()>; fn set_tos(&self, &socket2::Socket, Family, u8) -> io::Result<()>; }`，`pub struct NoopSocketHook`
  - `pub fn plan_addresses(addrs: Vec<IpAddr>, version: IpVersion, v6_first: bool) -> (Vec<IpAddr>, Vec<IpAddr>)`（先试的、3 秒后加入的）
  - `pub async fn race<T, F, Fut>(primary: Vec<SocketAddr>, secondary: Vec<SocketAddr>, connect: F) -> io::Result<T>`，其中 `T: Send + 'static`、`F: Fn(SocketAddr) -> Fut`、`Fut: Future<Output = io::Result<T>> + Send + 'static`

- [ ] **Step 1: 依赖**

工作区 `Cargo.toml` 的 `[workspace.dependencies]` 末尾（`windows-sys` 之后）加：

```toml
socket2 = { version = "0.6", features = ["all"] }
```

`crates/rurge-net/Cargo.toml`：`[dependencies]` 加 `socket2.workspace = true`；`[dev-dependencies]` 加 `tokio = { workspace = true, features = ["test-util"] }`。

`crates/rurge-net/src/lib.rs` 的 `pub mod resource;` 之后加 `pub mod socket;`。

- [ ] **Step 2: 写失败的测试**

创建 `crates/rurge-net/src/socket.rs`，先只放测试：

```rust
//! Socket options and the connection race behind every direct connection
//! (phase 2 M1 design §5.1).

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn addr(n: u8) -> SocketAddr {
        SocketAddr::from(([10, 0, 0, n], 80))
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[derive(Clone, Copy)]
    enum Script {
        OkAfter(u64),
        FailAfter(u64),
        Never,
    }

    type Launches = Arc<Mutex<Vec<(SocketAddr, u128)>>>;
    type Attempt = Pin<Box<dyn Future<Output = io::Result<SocketAddr>> + Send>>;

    /// A `connect` function that records when each attempt starts (virtual
    /// milliseconds since `started`) and then follows the script.
    fn scripted(
        scripts: Vec<(SocketAddr, Script)>,
        log: Launches,
        started: Instant,
    ) -> impl Fn(SocketAddr) -> Attempt {
        move |a| {
            log.lock().unwrap().push((a, started.elapsed().as_millis()));
            let script = scripts
                .iter()
                .find(|(x, _)| *x == a)
                .map(|(_, s)| *s)
                .unwrap_or(Script::Never);
            Box::pin(async move {
                match script {
                    Script::OkAfter(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        Ok(a)
                    }
                    Script::FailAfter(ms) => {
                        tokio::time::sleep(Duration::from_millis(ms)).await;
                        Err(io::Error::new(
                            io::ErrorKind::ConnectionRefused,
                            format!("refused by {a}"),
                        ))
                    }
                    Script::Never => std::future::pending().await,
                }
            })
        }
    }

    async fn run(
        primary: Vec<SocketAddr>,
        secondary: Vec<SocketAddr>,
        scripts: Vec<(SocketAddr, Script)>,
    ) -> (io::Result<SocketAddr>, Vec<(SocketAddr, u128)>, u128) {
        let log: Launches = Arc::default();
        let started = Instant::now();
        let result = race(primary, secondary, scripted(scripts, log.clone(), started)).await;
        let launches = log.lock().unwrap().clone();
        (result, launches, started.elapsed().as_millis())
    }

    #[tokio::test(start_paused = true)]
    async fn attempts_are_staggered_and_the_first_success_wins() {
        let (result, launches, elapsed) = run(
            vec![addr(1), addr(2), addr(3)],
            vec![],
            vec![(addr(1), Script::OkAfter(1000)), (addr(2), Script::OkAfter(100))],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 250)]);
        assert_eq!(elapsed, 350);
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_starts_the_next_attempt_at_once() {
        let (result, launches, _) = run(
            vec![addr(1), addr(2)],
            vec![],
            vec![(addr(1), Script::FailAfter(10)), (addr(2), Script::OkAfter(5))],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 10)]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_family_joins_after_three_seconds() {
        let (result, launches, _) = run(
            vec![addr(1)],
            vec![addr(2)],
            vec![(addr(1), Script::Never), (addr(2), Script::OkAfter(20))],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 3000)]);
    }

    #[tokio::test(start_paused = true)]
    async fn the_other_family_joins_early_once_the_preferred_one_has_failed() {
        let (result, launches, _) = run(
            vec![addr(1)],
            vec![addr(2)],
            vec![(addr(1), Script::FailAfter(50)), (addr(2), Script::OkAfter(20))],
        )
        .await;
        assert_eq!(result.unwrap(), addr(2));
        assert_eq!(launches, [(addr(1), 0), (addr(2), 50)]);
    }

    #[tokio::test(start_paused = true)]
    async fn when_everything_fails_the_last_error_is_returned() {
        let (result, launches, _) = run(
            vec![addr(1)],
            vec![addr(2)],
            vec![(addr(1), Script::FailAfter(10)), (addr(2), Script::FailAfter(10))],
        )
        .await;
        let err = result.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
        assert_eq!(err.to_string(), format!("refused by {}", addr(2)));
        assert_eq!(launches.len(), 2);
        let (result, launches, _) = run(vec![], vec![], vec![]).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(launches.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn the_losers_are_cancelled() {
        struct Flag(Arc<AtomicBool>);
        impl Drop for Flag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = dropped.clone();
        let winner = race(vec![addr(1), addr(2)], vec![], move |a| {
            let guard = (a == addr(1)).then(|| Flag(flag.clone()));
            Box::pin(async move {
                let _guard = guard;
                if a == addr(1) {
                    std::future::pending::<()>().await;
                }
                Ok::<SocketAddr, io::Error>(a)
            }) as Attempt
        })
        .await
        .unwrap();
        assert_eq!(winner, addr(2));
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
        assert!(dropped.load(Ordering::SeqCst), "the pending attempt was dropped");
    }

    #[test]
    fn addresses_are_planned_per_ip_version() {
        let all = || vec![ip("10.0.0.1"), ip("10.0.0.2"), ip("fd00::1"), ip("fd00::2")];
        let none: Vec<IpAddr> = Vec::new();
        assert_eq!(
            plan_addresses(all(), IpVersion::Dual, false),
            (vec![ip("10.0.0.1"), ip("fd00::1"), ip("10.0.0.2"), ip("fd00::2")], none.clone())
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::Dual, true).0,
            [ip("fd00::1"), ip("10.0.0.1"), ip("fd00::2"), ip("10.0.0.2")]
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::V4Only, true),
            (vec![ip("10.0.0.1"), ip("10.0.0.2")], none.clone())
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::V6Only, false),
            (vec![ip("fd00::1"), ip("fd00::2")], none.clone())
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::PreferV6, false),
            (vec![ip("fd00::1"), ip("fd00::2")], vec![ip("10.0.0.1"), ip("10.0.0.2")])
        );
        assert_eq!(
            plan_addresses(all(), IpVersion::PreferV4, true),
            (vec![ip("10.0.0.1"), ip("10.0.0.2")], vec![ip("fd00::1"), ip("fd00::2")])
        );
        // nothing of the preferred family: the other one goes first, at once
        assert_eq!(
            plan_addresses(vec![ip("10.0.0.1")], IpVersion::PreferV6, false),
            (vec![ip("10.0.0.1")], none.clone())
        );
        assert_eq!(plan_addresses(vec![ip("10.0.0.1")], IpVersion::V6Only, false), (none.clone(), none));
    }

    #[test]
    fn the_noop_hook_accepts_everything() {
        let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        assert!(NoopSocketHook.bind_interface(&socket, "nope0", Family::V4).is_ok());
        assert!(NoopSocketHook.set_tos(&socket, Family::V4, 0x10).is_ok());
        assert_eq!(Family::of(&ip("fd00::1")), Family::V6);
        assert_eq!(SocketOpts::default().ip_version, IpVersion::Dual);
    }
}
```

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p rurge-net socket::`
Expected: 编译失败——`race`、`plan_addresses`、`SocketOpts` 等未定义。

- [ ] **Step 4: 实现**

在 `crates/rurge-net/src/socket.rs` 的模块注释之后、测试模块之前加：

```rust
use crate::connector::interleave;
use rurge_config::spec::IpVersion;
use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::Instant;

/// Delay between the starts of two connection attempts.
pub const STAGGER: Duration = Duration::from_millis(250);
/// `prefer-v4` / `prefer-v6`: when the other address family joins the race.
pub const OTHER_FAMILY_AFTER: Duration = Duration::from_secs(3);

/// The socket-level part of a policy's common parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SocketOpts {
    pub interface: Option<String>,
    /// Fall back to the default interface when `interface` cannot be used.
    pub allow_other_interface: bool,
    pub ip_version: IpVersion,
    /// Which family leads the interleaving (`[General] ipv6`); ignored by the
    /// `prefer-*` and `*-only` modes.
    pub v6_first: bool,
    pub tos: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

impl Family {
    pub fn of(ip: &IpAddr) -> Family {
        if ip.is_ipv4() { Family::V4 } else { Family::V6 }
    }
}

/// The two socket options whose spelling differs per platform. Implemented
/// by the binary on top of `rurge-platform` (AR-02); tests use a fake.
pub trait SocketHook: Send + Sync {
    fn bind_interface(
        &self,
        socket: &socket2::Socket,
        interface: &str,
        family: Family,
    ) -> io::Result<()>;
    fn set_tos(&self, socket: &socket2::Socket, family: Family, tos: u8) -> io::Result<()>;
}

/// Does nothing: for commands that never dial (`check`, `rule match`) and tests.
pub struct NoopSocketHook;

impl SocketHook for NoopSocketHook {
    fn bind_interface(&self, _: &socket2::Socket, _: &str, _: Family) -> io::Result<()> {
        Ok(())
    }
    fn set_tos(&self, _: &socket2::Socket, _: Family, _: u8) -> io::Result<()> {
        Ok(())
    }
}

/// Splits resolved addresses into the ones to try first and the ones that
/// join after `OTHER_FAMILY_AFTER` (only the `prefer-*` modes have any).
pub fn plan_addresses(
    addrs: Vec<IpAddr>,
    version: IpVersion,
    v6_first: bool,
) -> (Vec<IpAddr>, Vec<IpAddr>) {
    let split = |addrs: Vec<IpAddr>, v6_preferred: bool| {
        let (v6, v4): (Vec<IpAddr>, Vec<IpAddr>) = addrs.into_iter().partition(IpAddr::is_ipv6);
        let (preferred, other) = if v6_preferred { (v6, v4) } else { (v4, v6) };
        if preferred.is_empty() {
            (other, Vec::new())
        } else {
            (preferred, other)
        }
    };
    match version {
        IpVersion::Dual => (interleave(addrs, v6_first), Vec::new()),
        IpVersion::V4Only => (addrs.into_iter().filter(IpAddr::is_ipv4).collect(), Vec::new()),
        IpVersion::V6Only => (addrs.into_iter().filter(IpAddr::is_ipv6).collect(), Vec::new()),
        IpVersion::PreferV4 => split(addrs, false),
        IpVersion::PreferV6 => split(addrs, true),
    }
}

/// Connects to the first address that answers. Attempts on `primary` start
/// `STAGGER` apart (at once after a failure); `secondary` joins after
/// `OTHER_FAMILY_AFTER`, or as soon as every primary attempt has failed.
/// Returning drops the attempts still in flight. The caller bounds the total
/// time.
pub async fn race<T, F, Fut>(
    primary: Vec<SocketAddr>,
    secondary: Vec<SocketAddr>,
    connect: F,
) -> io::Result<T>
where
    T: Send + 'static,
    F: Fn(SocketAddr) -> Fut,
    Fut: Future<Output = io::Result<T>> + Send + 'static,
{
    let started = Instant::now();
    let mut queue: VecDeque<SocketAddr> = primary.into();
    let mut secondary = Some(secondary).filter(|s| !s.is_empty());
    let mut attempts: JoinSet<io::Result<T>> = JoinSet::new();
    let mut next_launch = started;
    let mut last = io::Error::new(io::ErrorKind::NotFound, "no addresses to connect to");
    loop {
        if queue.is_empty() && attempts.is_empty() {
            match secondary.take() {
                Some(more) => {
                    queue = more.into();
                    next_launch = Instant::now();
                }
                None => return Err(last),
            }
        }
        tokio::select! {
            _ = tokio::time::sleep_until(next_launch), if !queue.is_empty() => {
                if let Some(addr) = queue.pop_front() {
                    attempts.spawn(connect(addr));
                }
                next_launch = Instant::now() + STAGGER;
            }
            Some(joined) = attempts.join_next(), if !attempts.is_empty() => match joined {
                Ok(Ok(value)) => return Ok(value),
                Ok(Err(e)) => {
                    last = e;
                    next_launch = Instant::now();
                }
                Err(e) => {
                    last = io::Error::other(format!("connection attempt failed: {e}"));
                    next_launch = Instant::now();
                }
            },
            _ = tokio::time::sleep_until(started + OTHER_FAMILY_AFTER),
                if secondary.is_some() && queue.is_empty() =>
            {
                if let Some(more) = secondary.take() {
                    queue = more.into();
                    next_launch = Instant::now();
                }
            }
        }
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-net socket::`
Expected: PASS（8 个）。计时断言在暂停的时钟下是确定的；若 `the_losers_are_cancelled` 失败，先确认测试运行时是 `current_thread`（`#[tokio::test]` 的默认值），不要靠加 `sleep` 来"修"。

- [ ] **Step 6: 门禁与提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-net/Cargo.toml crates/rurge-net/src/lib.rs crates/rurge-net/src/socket.rs
git commit -m "feat(net): socket 层：SocketOpts、SocketHook 与按 ip-version 竞速的建连"
```

---

### Task 5: 新的 `DirectConnector`，`ConnectOpts` 去掉 `prefer_v6`

**Files:**
- Modify: `crates/rurge-net/src/connector.rs`
- Modify: `crates/rurge-net/src/http.rs`（约 162 行的 `ConnectOpts` 构造）
- Modify: `crates/rurge-dns/src/upstream/tcp.rs`（约 111 行）
- Modify: `crates/rurge-dns/src/bootstrap.rs`（约 187 行与约 421 行）
- Modify: `crates/rurge-proto/src/direct.rs`
- Modify: `crates/rurge-engine/src/engine.rs`（约 523 行与约 681 行）
- Modify: `crates/rurge-engine/src/runtime.rs`（约 50 行）

**Interfaces:**
- Consumes: Task 4 的 `SocketOpts` `SocketHook` `NoopSocketHook` `Family` `plan_addresses` `race`。
- Produces:
  - `rurge_net::connector::ConnectOpts { pub timeout: Duration }`（`Default` = 10 秒；**不再有 `prefer_v6`**）
  - `DirectConnector::new(resolver: Arc<dyn Resolve>) -> DirectConnector`（默认 `SocketOpts` + `NoopSocketHook`）
  - `DirectConnector::with_opts(resolver: Arc<dyn Resolve>, opts: SocketOpts, hook: Arc<dyn SocketHook>) -> DirectConnector`
  - `rurge_proto::Direct::with_socket_opts(resolver: Arc<dyn Resolve>, opts: SocketOpts, hook: Arc<dyn SocketHook>) -> Direct`
  - 行为：域名目标按 `ip-version` 竞速；IP 字面量目标不过滤；总超时（含 DNS）= `ConnectOpts.timeout`，超时是 `io::ErrorKind::TimedOut`。

- [ ] **Step 1: 写失败的测试**

在 `crates/rurge-net/src/connector.rs` 的 `mod tests` 里：删掉 `per_attempt_shares_the_budget_with_a_floor`，并在模块末尾加：

```rust
    use crate::socket::{Family, SocketHook, SocketOpts};
    use rurge_config::spec::IpVersion;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingHook {
        calls: Mutex<Vec<String>>,
        refuse_interface: bool,
    }

    impl SocketHook for RecordingHook {
        fn bind_interface(&self, _: &socket2::Socket, interface: &str, family: Family) -> io::Result<()> {
            self.calls.lock().unwrap().push(format!("bind {interface} {family:?}"));
            if self.refuse_interface {
                return Err(io::Error::new(io::ErrorKind::NotFound, "no such interface"));
            }
            Ok(())
        }
        fn set_tos(&self, _: &socket2::Socket, family: Family, tos: u8) -> io::Result<()> {
            self.calls.lock().unwrap().push(format!("tos {tos:#04x} {family:?}"));
            Ok(())
        }
    }

    struct Fixed(Vec<IpAddr>);

    impl Resolve for Fixed {
        fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
            Box::pin(async move { Ok(self.0.clone()) })
        }
    }

    async fn listener() -> (tokio::net::TcpListener, u16) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        (l, port)
    }

    #[tokio::test]
    async fn the_hook_sees_the_tos_and_the_interface() {
        let (_l, port) = listener().await;
        let hook = Arc::new(RecordingHook::default());
        let connector = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("test0".into()),
                tos: 0x10,
                ..SocketOpts::default()
            },
            hook.clone(),
        );
        connector
            .connect(&Target::new(HostName::parse("127.0.0.1"), port), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(*hook.calls.lock().unwrap(), ["tos 0x10 V4", "bind test0 V4"]);
    }

    #[tokio::test]
    async fn an_unusable_interface_fails_unless_other_interfaces_are_allowed() {
        let (_l, port) = listener().await;
        let target = Target::new(HostName::parse("127.0.0.1"), port);
        let hook = || {
            Arc::new(RecordingHook {
                refuse_interface: true,
                ..RecordingHook::default()
            })
        };
        let strict = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts { interface: Some("gone0".into()), ..SocketOpts::default() },
            hook(),
        );
        let err = strict.connect(&target, &ConnectOpts::default()).await.map(|_| ()).unwrap_err();
        assert_eq!(err.to_string(), "cannot use interface gone0: no such interface");
        let lenient = DirectConnector::with_opts(
            Arc::new(SystemResolve),
            SocketOpts {
                interface: Some("gone0".into()),
                allow_other_interface: true,
                ..SocketOpts::default()
            },
            hook(),
        );
        lenient.connect(&target, &ConnectOpts::default()).await.unwrap();
    }

    #[tokio::test]
    async fn ip_version_filters_resolved_addresses_but_not_literals() {
        let (_l, port) = listener().await;
        let both = || Arc::new(Fixed(vec!["::1".parse().unwrap(), "127.0.0.1".parse().unwrap()]));
        let with = |version| {
            DirectConnector::with_opts(
                both(),
                SocketOpts { ip_version: version, ..SocketOpts::default() },
                Arc::new(crate::socket::NoopSocketHook),
            )
        };
        let name = Target::new(HostName::parse("both.test"), port);
        with(IpVersion::V4Only).connect(&name, &ConnectOpts::default()).await.unwrap();
        // only 127.0.0.1 listens, so v6-only cannot connect
        assert!(with(IpVersion::V6Only).connect(&name, &ConnectOpts::default()).await.is_err());
        // an IP literal is used as it is
        let literal = Target::new(HostName::parse("127.0.0.1"), port);
        with(IpVersion::V6Only).connect(&literal, &ConnectOpts::default()).await.unwrap();
        // nothing left after filtering
        let v4_only_name = DirectConnector::with_opts(
            Arc::new(Fixed(vec!["::1".parse().unwrap()])),
            SocketOpts { ip_version: IpVersion::V4Only, ..SocketOpts::default() },
            Arc::new(crate::socket::NoopSocketHook),
        );
        let err = v4_only_name.connect(&name, &ConnectOpts::default()).await.map(|_| ()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(err.to_string(), "no usable address for both.test (ip-version)");
    }

    #[tokio::test]
    async fn the_timeout_covers_name_resolution() {
        struct Stuck;
        impl Resolve for Stuck {
            fn resolve<'a>(&'a self, _: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
                Box::pin(std::future::pending())
            }
        }
        let connector = DirectConnector::new(Arc::new(Stuck));
        let err = connector
            .connect(
                &Target::new(HostName::parse("slow.test"), 80),
                &ConnectOpts { timeout: Duration::from_millis(50) },
            )
            .await
            .map(|_| ())
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(err.to_string(), "connect to slow.test:80 timed out");
    }
```

（`mod tests` 顶部若还没有 `use std::time::Duration;` 就加上。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-net connector::`
Expected: 编译失败——`DirectConnector::with_opts` 不存在、`ConnectOpts` 还有 `prefer_v6` 字段。

- [ ] **Step 3: 实现 `connector.rs`**

在 `crates/rurge-net/src/connector.rs`：

1. `ConnectOpts` 与它的 `Default` 换成：

```rust
#[derive(Clone, Debug)]
pub struct ConnectOpts {
    /// Covers name resolution and every connection attempt.
    pub timeout: Duration,
}

impl Default for ConnectOpts {
    fn default() -> Self {
        ConnectOpts {
            timeout: Duration::from_secs(10),
        }
    }
}
```

2. 删掉 `fn per_attempt`。`interleave` 保持不变（`plan_addresses` 与 `rurge-dns` 的 bootstrap 连接器还在用）。

3. `DirectConnector` 的结构体、`impl DirectConnector` 与 `impl Connector for DirectConnector` 整个换成：

```rust
/// Plain TCP: resolves through `Resolve`, then races the addresses the
/// policy's `ip-version` allows (`crate::socket::race`).
pub struct DirectConnector {
    resolver: Arc<dyn Resolve>,
    opts: SocketOpts,
    hook: Arc<dyn SocketHook>,
    /// The `allow-other-interface` fallback is logged once per connector.
    fallback_logged: Arc<AtomicBool>,
}

impl DirectConnector {
    pub fn new(resolver: Arc<dyn Resolve>) -> DirectConnector {
        DirectConnector::with_opts(resolver, SocketOpts::default(), Arc::new(NoopSocketHook))
    }

    pub fn with_opts(
        resolver: Arc<dyn Resolve>,
        opts: SocketOpts,
        hook: Arc<dyn SocketHook>,
    ) -> DirectConnector {
        DirectConnector {
            resolver,
            opts,
            hook,
            fallback_logged: Arc::new(AtomicBool::new(false)),
        }
    }
}

async fn connect_one(
    addr: SocketAddr,
    opts: &SocketOpts,
    hook: &dyn SocketHook,
    fallback_logged: &AtomicBool,
) -> io::Result<TcpStream> {
    let family = Family::of(&addr.ip());
    let domain = match family {
        Family::V4 => socket2::Domain::IPV4,
        Family::V6 => socket2::Domain::IPV6,
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    socket.set_nonblocking(true)?;
    if opts.tos != 0
        && let Err(e) = hook.set_tos(&socket, family, opts.tos)
    {
        tracing::debug!(error = %e, tos = opts.tos, "cannot set the IP TOS; connecting without it");
    }
    if let Some(interface) = &opts.interface
        && let Err(e) = hook.bind_interface(&socket, interface, family)
    {
        if !opts.allow_other_interface {
            return Err(io::Error::new(
                e.kind(),
                format!("cannot use interface {interface}: {e}"),
            ));
        }
        if !fallback_logged.swap(true, Ordering::Relaxed) {
            tracing::warn!(interface = %interface, error = %e, "interface unavailable; using the default one (allow-other-interface)");
        }
    }
    let std_stream: std::net::TcpStream = socket.into();
    let stream = tokio::net::TcpSocket::from_std_stream(std_stream)
        .connect(addr)
        .await?;
    let _ = stream.set_nodelay(true);
    Ok(stream)
}

impl Connector for DirectConnector {
    fn connect<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let attempt = async {
                let (primary, secondary) = match &target.host {
                    // `ip-version` only means something for a host name (manual)
                    HostName::Ip(ip) => (vec![*ip], Vec::new()),
                    HostName::Domain(d) => {
                        let addrs = self.resolver.resolve(d).await?;
                        let planned =
                            plan_addresses(addrs, self.opts.ip_version, self.opts.v6_first);
                        if planned.0.is_empty() {
                            return Err(io::Error::new(
                                io::ErrorKind::NotFound,
                                format!("no usable address for {d} (ip-version)"),
                            ));
                        }
                        planned
                    }
                };
                let port = target.port;
                let with_port = |ips: Vec<IpAddr>| -> Vec<SocketAddr> {
                    ips.into_iter().map(|ip| SocketAddr::new(ip, port)).collect()
                };
                let (socket_opts, hook, logged) =
                    (self.opts.clone(), self.hook.clone(), self.fallback_logged.clone());
                race(with_port(primary), with_port(secondary), move |addr| {
                    let (socket_opts, hook, logged) =
                        (socket_opts.clone(), hook.clone(), logged.clone());
                    async move { connect_one(addr, &socket_opts, hook.as_ref(), &logged).await }
                })
                .await
            };
            match tokio::time::timeout(opts.timeout, attempt).await {
                Ok(Ok(stream)) => Ok(Box::new(stream) as BoxedStream),
                Ok(Err(e)) => Err(e),
                Err(_) => Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("connect to {}:{} timed out", target.host, target.port),
                )),
            }
        })
    }
}
```

4. 文件顶部的 `use` 补上：

```rust
use crate::socket::{Family, NoopSocketHook, SocketHook, SocketOpts, plan_addresses, race};
use std::sync::atomic::{AtomicBool, Ordering};
```

（`TcpStream` 已经从 `tokio::net` 引入；`tracing` 是 `rurge-net` 的现有依赖。）

- [ ] **Step 4: 同步其余构造点**

逐处改（行号是写计划时的近似值，以内容为准）：

`crates/rurge-net/src/http.rs`：

```rust
            let opts = ConnectOpts { timeout };
```

`crates/rurge-dns/src/upstream/tcp.rs`：

```rust
        let opts = ConnectOpts { timeout };
```

`crates/rurge-dns/src/bootstrap.rs`（连接器里的排序；保持现状的 v4 在前）：

```rust
            for ip in interleave(ips, false) {
```

`crates/rurge-dns/src/bootstrap.rs`（测试里）：

```rust
        let quick = ConnectOpts {
            timeout: Duration::from_millis(500),
        };
```

`crates/rurge-engine/src/engine.rs`（`dial_internal` 与 `dial` 两处，写法相同）：

```rust
            let opts = ConnectOpts {
                timeout: CONNECT_TIMEOUT,
            };
```

`crates/rurge-proto/src/direct.rs`：加构造函数，并把测试里那处 `ConnectOpts { timeout: Duration::from_secs(3), prefer_v6: false }` 改成 `ConnectOpts { timeout: Duration::from_secs(3) }`：

```rust
    /// DIRECT with a policy's socket options (`direct` aliases, and the
    /// built-in DIRECT carrying `[General] ipv6` as `v6_first`).
    pub fn with_socket_opts(
        resolver: Arc<dyn Resolve>,
        opts: SocketOpts,
        hook: Arc<dyn SocketHook>,
    ) -> Direct {
        Direct::new(Arc::new(DirectConnector::with_opts(resolver, opts, hook)))
    }
```

`direct.rs` 顶部加 `use rurge_net::socket::{SocketHook, SocketOpts};`；`with_resolver` 的文档注释里"happy-eyeballs address ordering"改成 `racing the resolved addresses`。

`crates/rurge-engine/src/runtime.rs`：保住 `[General] ipv6` 原先经 `prefer_v6` 表达的"v6 在前"：

```rust
        let direct: OutboundRef = Arc::new(Direct::with_socket_opts(
            stack.resolver.clone(),
            rurge_net::socket::SocketOpts {
                v6_first: config.general.ipv6,
                ..Default::default()
            },
            Arc::new(rurge_net::socket::NoopSocketHook),
        ));
```

再确认没有漏网的：

Run: `grep -rn "prefer_v6" crates --include=*.rs`
Expected: 只剩 `crates/rurge-net/src/connector.rs` 里 `interleave` 的形参与它的测试。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-net && cargo test -p rurge-dns && cargo test -p rurge-proto && cargo test -p rurge-engine`
Expected: PASS。`rurge-net` 的 `connector::` 里新增 4 个用例，原有 3 个异步用例不变。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-net crates/rurge-dns crates/rurge-proto crates/rurge-engine
git commit -m "feat(net): DirectConnector 带 socket 选项并按 ip-version 竞速；ConnectOpts 去掉 prefer_v6"
```

---

### Task 6: `rurge-platform::socket`——网卡绑定与 TOS 的平台实现

**Files:**
- Modify: `crates/rurge-platform/Cargo.toml`
- Modify: `crates/rurge-platform/src/lib.rs`
- Create: `crates/rurge-platform/src/socket.rs`

**Interfaces:**
- Consumes: 无内部依赖（`rurge-platform` 不依赖任何内部 crate）；`socket2`（`all`）、`if-addrs`（现有依赖：`if_addrs::get_if_addrs() -> io::Result<Vec<Interface>>`，`Interface { name: String, index: Option<u32>, .. }`，`Interface::ip() -> IpAddr`）。
- Produces（`rurge_platform::socket`）：
  - `pub enum Family { V4, V6 }`
  - `pub fn bind_interface(socket: &socket2::Socket, interface: &str, family: Family) -> io::Result<()>`
  - `pub fn set_tos(socket: &socket2::Socket, family: Family, tos: u8) -> io::Result<()>`
  - `pub fn pick_source(addrs: &[(String, IpAddr)], interface: &str, family: Family) -> Option<IpAddr>`（纯函数，三平台都编译，供 Windows 实现与测试使用）
  - M1b 的 bin 适配器把它们接到 `rurge_net::socket::SocketHook` 上。

- [ ] **Step 1: 依赖与模块**

`crates/rurge-platform/Cargo.toml` 的 `[dependencies]` 加 `socket2.workspace = true`。`crates/rurge-platform/src/lib.rs` 的 `pub mod service;` 之后加 `pub mod socket;`。

- [ ] **Step 2: 写失败的测试**

创建 `crates/rurge-platform/src/socket.rs`，先只放测试：

```rust
//! Socket options whose spelling differs per platform (AR-02). Plain
//! functions: the binary adapts them to `rurge_net::socket::SocketHook`.

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<(String, IpAddr)> {
        [
            ("Wi-Fi", "fe80::1"),
            ("Wi-Fi", "169.254.10.1"),
            ("Wi-Fi", "192.168.1.20"),
            ("Wi-Fi", "2001:db8::20"),
            ("Ethernet", "10.0.0.5"),
            ("Loopback", "127.0.0.1"),
        ]
        .into_iter()
        .map(|(name, ip)| (name.to_string(), ip.parse().unwrap()))
        .collect()
    }

    #[test]
    fn the_source_address_is_the_first_routable_one_of_the_family() {
        let t = table();
        assert_eq!(pick_source(&t, "Wi-Fi", Family::V4), Some("192.168.1.20".parse().unwrap()));
        assert_eq!(pick_source(&t, "Wi-Fi", Family::V6), Some("2001:db8::20".parse().unwrap()));
        assert_eq!(pick_source(&t, "Ethernet", Family::V4), Some("10.0.0.5".parse().unwrap()));
        // no address of that family, only a loopback one, or no such interface
        assert_eq!(pick_source(&t, "Ethernet", Family::V6), None);
        assert_eq!(pick_source(&t, "Loopback", Family::V4), None);
        assert_eq!(pick_source(&t, "wi-fi", Family::V4), None, "names are matched exactly");
    }

    #[test]
    fn binding_to_an_interface_that_does_not_exist_fails() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        assert!(bind_interface(&socket, "rurge-no-such-if0", Family::V4).is_err());
    }

    #[test]
    fn the_tos_is_set_on_an_ipv4_socket() {
        let socket =
            socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
        set_tos(&socket, Family::V4, 0x28).unwrap();
        assert_eq!(socket.tos_v4().unwrap(), 0x28);
    }
}
```

`the_tos_is_set_on_an_ipv4_socket` 只改测试自己新建的 socket，不触碰任何系统设置。若 Windows 上 `tos_v4()` 读回的不是 `0x28`（微软文档说并非所有版本都支持 `IP_TOS`），把读回断言换成 `assert!(socket.tos_v4().is_ok())` 并在报告里写明读到的值。

- [ ] **Step 3: 跑测试确认失败**

Run: `cargo test -p rurge-platform socket::`
Expected: 编译失败——`pick_source`、`bind_interface`、`set_tos` 未定义。

- [ ] **Step 4: 实现**

在模块注释之后、测试模块之前加：

```rust
use socket2::Socket;
use std::io;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
}

fn routable(ip: &IpAddr, family: Family) -> bool {
    match (ip, family) {
        (IpAddr::V4(v4), Family::V4) => {
            !v4.is_loopback() && !v4.is_link_local() && !v4.is_unspecified()
        }
        (IpAddr::V6(v6), Family::V6) => {
            // fe80::/10 is link-local
            !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80
        }
        _ => false,
    }
}

/// The address to bind as the source so that traffic leaves through
/// `interface`: its first address of `family` that is neither loopback nor
/// link-local. This is how Windows binds to an interface without `unsafe`
/// (strong host model: a bound source address pins the outgoing interface).
pub fn pick_source(addrs: &[(String, IpAddr)], interface: &str, family: Family) -> Option<IpAddr> {
    addrs
        .iter()
        .filter(|(name, _)| name == interface)
        .map(|(_, ip)| *ip)
        .find(|ip| routable(ip, family))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn bind_interface(socket: &Socket, interface: &str, _family: Family) -> io::Result<()> {
    socket.bind_device(Some(interface.as_bytes()))
}

#[cfg(target_os = "macos")]
pub fn bind_interface(socket: &Socket, interface: &str, family: Family) -> io::Result<()> {
    // `Interface::index` spares us the unsafe `if_nametoindex`
    let index = if_addrs::get_if_addrs()?
        .into_iter()
        .find(|i| i.name == interface)
        .and_then(|i| i.index)
        .and_then(std::num::NonZeroU32::new)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no such interface: {interface}"))
        })?;
    match family {
        Family::V4 => socket.bind_device_by_index_v4(Some(index)),
        Family::V6 => socket.bind_device_by_index_v6(Some(index)),
    }
}

#[cfg(windows)]
pub fn bind_interface(socket: &Socket, interface: &str, family: Family) -> io::Result<()> {
    // `Interface::name` is the adapter's friendly name on Windows ("Wi-Fi")
    let addrs: Vec<(String, IpAddr)> = if_addrs::get_if_addrs()?
        .into_iter()
        .map(|i| (i.name.clone(), i.ip()))
        .collect();
    let source = pick_source(&addrs, interface, family).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("interface {interface} has no usable address of this family"),
        )
    })?;
    socket.bind(&std::net::SocketAddr::new(source, 0).into())
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
pub fn bind_interface(_socket: &Socket, _interface: &str, _family: Family) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "binding to an interface is not supported on this platform",
    ))
}

pub fn set_tos(socket: &Socket, family: Family, tos: u8) -> io::Result<()> {
    match family {
        Family::V4 => socket.set_tos_v4(u32::from(tos)),
        Family::V6 => set_traffic_class(socket, tos),
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
fn set_traffic_class(socket: &Socket, tos: u8) -> io::Result<()> {
    socket.set_tclass_v6(u32::from(tos))
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn set_traffic_class(_socket: &Socket, _tos: u8) -> io::Result<()> {
    tracing::debug!("the IPv6 traffic class cannot be set on this platform");
    Ok(())
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-platform socket::`
Expected: PASS（3 个）。本机只能编译 Windows 分支；Linux / macOS 分支靠 CI 证明，在报告里如实写明。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-platform Cargo.lock
git commit -m "feat(platform): 网卡绑定与 TOS 的平台函数（Linux SO_BINDTODEVICE、macOS IP_BOUND_IF、Windows 绑定源地址）"
```

---

### Task 7: `Outbound` 的变化——新错误变体、`HttpForward`、`BuildError`

**Files:**
- Modify: `crates/rurge-proto/src/outbound.rs`
- Create: `crates/rurge-proto/src/build.rs`
- Modify: `crates/rurge-proto/src/lib.rs`
- Modify: `crates/rurge-engine/src/engine.rs`（`dial` 与 `dial_internal` 里对 `OutboundError` 的 `match`）

**Interfaces:**
- Consumes: `rurge_net::connector::{BoxedStream, ConnectOpts}`、`rurge_net::BoxFuture`。
- Produces:
  - `OutboundError::{Proxy(String), Tls(String), Unavailable(String)}`，`Display` 分别是 `{0}`、`tls: {0}`、`policy unavailable: {0}`
  - `pub trait HttpForward: Send + Sync { fn connect<'a>(&'a self, opts: &'a ConnectOpts) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>; fn request_headers(&self) -> Vec<(String, String)>; }`
  - `Outbound::http_forward(&self) -> Option<&dyn HttpForward>`（默认 `None`）
  - `rurge_proto::BuildError { pub message: String }`，`BuildError::new(impl Into<String>)`，实现 `Display` 与 `std::error::Error`

- [ ] **Step 1: 写失败的测试**

在 `crates/rurge-proto/src/outbound.rs` 末尾加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Direct, Reject};
    use rurge_net::connector::SystemResolve;

    #[test]
    fn the_new_error_variants_render_for_the_session_log() {
        assert_eq!(
            OutboundError::Proxy("socks5: authentication failed".into()).to_string(),
            "socks5: authentication failed"
        );
        assert_eq!(
            OutboundError::Tls("certificate fingerprint mismatch".into()).to_string(),
            "tls: certificate fingerprint mismatch"
        );
        assert_eq!(
            OutboundError::Unavailable("subscription item is broken".into()).to_string(),
            "policy unavailable: subscription item is broken"
        );
    }

    #[test]
    fn only_http_proxies_forward_plain_requests() {
        let direct = Direct::with_resolver(Arc::new(SystemResolve));
        assert!(direct.http_forward().is_none());
        assert!(Reject::new(RejectKind::Reject).http_forward().is_none());
    }
}
```

创建 `crates/rurge-proto/src/build.rs`，先只放测试：

```rust
//! Why an outbound could not be built from its spec.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_errors_are_plain_messages() {
        let e = BuildError::new("keystore item `cert1` cannot be decoded");
        assert_eq!(e.to_string(), "keystore item `cert1` cannot be decoded");
        assert_eq!(e, BuildError { message: "keystore item `cert1` cannot be decoded".into() });
        let _: &dyn std::error::Error = &e;
    }
}
```

`crates/rurge-proto/src/lib.rs` 加 `pub mod build;` 与 `pub use build::BuildError;`，并把 `pub use outbound::{..}` 那一行补上 `HttpForward`。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test -p rurge-proto`
Expected: 编译失败——新变体、`http_forward`、`BuildError` 未定义。

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/outbound.rs`：`OutboundError` 加三个变体（放在 `Timeout` 之后）：

```rust
    /// The proxy refused or broke the handshake (`socks5: authentication failed`).
    Proxy(String),
    Tls(String),
    /// The policy exists but cannot be used (M3: a broken subscription item).
    Unavailable(String),
```

`Display` 的 `match` 加：

```rust
            OutboundError::Proxy(m) => f.write_str(m),
            OutboundError::Tls(m) => write!(f, "tls: {m}"),
            OutboundError::Unavailable(m) => write!(f, "policy unavailable: {m}"),
```

`Outbound` trait 之前加 `HttpForward`，trait 里加默认方法：

```rust
/// An HTTP proxy that takes plain requests in absolute form
/// (`always-use-connect = false`, the manual's default).
pub trait HttpForward: Send + Sync {
    /// Connects to the proxy itself (TCP, then TLS for `https`): no CONNECT.
    fn connect<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
    /// `Proxy-Authorization` and the configured `headers`, rendered for one request.
    fn request_headers(&self) -> Vec<(String, String)>;
}
```

```rust
    /// `Some` when plain HTTP requests may be sent to this outbound in
    /// absolute form instead of through a CONNECT tunnel.
    fn http_forward(&self) -> Option<&dyn HttpForward> {
        None
    }
```

`crates/rurge-proto/src/build.rs`，加在模块注释之后：

```rust
use std::fmt;

/// The text is shown to the user (`rurge check`): never put a secret in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildError {
    pub message: String,
}

impl BuildError {
    pub fn new(message: impl Into<String>) -> BuildError {
        BuildError {
            message: message.into(),
        }
    }
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for BuildError {}
```

- [ ] **Step 4: 让引擎的两个 `match` 重新穷尽**

`crates/rurge-engine/src/engine.rs` 的 `dial` 里，`Err(OutboundError::Timeout) => ...` 之后加：

```rust
                Err(
                    e @ (OutboundError::Proxy(_)
                    | OutboundError::Tls(_)
                    | OutboundError::Unavailable(_)),
                ) => fail(handle, FailKind::Connect, e.to_string()),
```

`dial_internal` 里，`Err(OutboundError::Timeout) => { .. }` 之后加：

```rust
            Err(
                e @ (OutboundError::Proxy(_)
                | OutboundError::Tls(_)
                | OutboundError::Unavailable(_)),
            ) => {
                let message = e.to_string();
                handle.finish(SessionOutcome::Failed(message.clone()));
                Err(io::Error::other(message))
            }
```

再确认工作区里没有别的穷尽 `match`：

Run: `grep -rn "OutboundError::Timeout" crates --include=*.rs`
Expected: 每一处命中所在的 `match` 都已带上新变体或本来就有 `_` 分支；否则同样补上。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-proto && cargo test -p rurge-engine`
Expected: PASS。

- [ ] **Step 6: 门禁与提交**

```bash
git add crates/rurge-proto crates/rurge-engine
git commit -m "feat(proto): OutboundError 的 Proxy / Tls / Unavailable、HttpForward 与 BuildError"
```

---

### Task 8: `transport::{head, prefixed}` 与 `rurge_proto::testing` 回环假上游

**Files:**
- Modify: `crates/rurge-proto/Cargo.toml`
- Modify: `crates/rurge-proto/src/lib.rs`
- Create: `crates/rurge-proto/src/transport/mod.rs`
- Create: `crates/rurge-proto/src/transport/head.rs`
- Create: `crates/rurge-proto/src/transport/prefixed.rs`
- Create: `crates/rurge-proto/src/testing/mod.rs`
- Create: `crates/rurge-proto/src/testing/tls.rs`
- Create: `crates/rurge-proto/src/testing/http_proxy.rs`
- Create: `crates/rurge-proto/src/testing/socks5.rs`

**Interfaces:**
- Consumes: `rurge_net::connector::BoxedStream`；`rcgen` 0.14（`KeyPair::generate`、`CertificateParams::{new, self_signed, signed_by}`、`Issuer::new`、`IsCa`、`BasicConstraints`、`DnType`）；`rustls` 0.23 / `tokio-rustls` 0.26；`base64`；`sha2`。
- Produces:
  - `rurge_proto::transport::head::read_head<S: AsyncRead + Unpin>(stream: &mut S, limit: usize) -> io::Result<(Vec<u8>, Vec<u8>)>`（响应头含结尾空行；第二项是多读到的字节）
  - `rurge_proto::transport::prefixed::{Prefixed<S>, boxed(prefix: Vec<u8>, inner: BoxedStream) -> BoxedStream}`
  - `rurge_proto::testing`（`#[cfg(any(test, feature = "testing"))]`）：
    - `echo_server() -> SocketAddr`
    - `TlsFixture::new(names: &[&str]) -> Arc<TlsFixture>`，方法 `roots() -> Arc<RootCertStore>`、`leaf_fingerprint() -> [u8; 32]`、`issue_client(common_name: &str) -> (Vec<u8>, Vec<u8>)`（证书 DER、PKCS#8 私钥 DER）、`acceptor(require_client_cert: bool) -> TlsAcceptor`、`accept(&self, &TlsAcceptor, TcpStream) -> io::Result<BoxedStream>`、`seen() -> Vec<SeenHandshake>`、`spawn_echo(self: &Arc<Self>, require_client_cert: bool) -> SocketAddr`；`SeenHandshake { sni: Option<String>, alpn: Option<String>, client_cert: bool }`
    - `FakeHttpProxy::{spawn(HttpProxyScript), spawn_tls(HttpProxyScript, Arc<TlsFixture>, bool)}`，`addr()`，`heads() -> Vec<RecordedHead>`；`HttpProxyScript { auth, refuse, padding, delay, trailing, truncate, connect_to }`；`RecordedHead { request_line, headers }` + `header(&str) -> Option<&str>`
    - `FakeSocks5::{spawn(Socks5Script), spawn_tls(..)}`，`addr()`，`requests() -> Vec<RecordedSocks5>`；`Socks5Script { auth, reply, connect_to, delay, hang_up_after_greeting, force_method }`；`RecordedSocks5 { methods, credentials, atyp, host, port }`
  - 假上游**从不解析域名**：目标不是 IP 字面量且脚本没给 `connect_to` 时回 502 / 应答码 4。

- [ ] **Step 1: 依赖与模块**

`crates/rurge-proto/Cargo.toml` 的依赖段改成：

```toml
[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
tokio.workspace = true
rustls.workspace = true
tokio-rustls.workspace = true
sha2.workspace = true
base64.workspace = true
rcgen = { workspace = true, optional = true }

[features]
# Scriptable loopback peers (`rurge_proto::testing`); enabled by dependants' dev-dependencies.
testing = ["dep:rcgen"]

[dev-dependencies]
rcgen.workspace = true
tokio = { workspace = true, features = ["test-util"] }
```

`crates/rurge-proto/src/lib.rs` 加：

```rust
pub mod transport;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
```

创建 `crates/rurge-proto/src/transport/mod.rs`：

```rust
//! Layers between a connector's stream and a protocol's own handshake.

pub mod head;
pub mod prefixed;
```

- [ ] **Step 2: 写失败的测试（`head` 与 `prefixed`）**

创建 `crates/rurge-proto/src/transport/head.rs`，先只放测试：

```rust
//! Reads an HTTP-style head off a stream.

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn the_head_ends_at_the_empty_line_and_the_rest_is_handed_back() {
        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            // split in awkward places on purpose
            for chunk in ["HTTP/1.1 200 OK\r", "\nX-A: 1\r\n\r", "\nTUNNEL", " BYTES"] {
                server.write_all(chunk.as_bytes()).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let (head, rest) = read_head(&mut client, 1024).await.unwrap();
        assert_eq!(head, b"HTTP/1.1 200 OK\r\nX-A: 1\r\n\r\n");
        assert!(b"TUNNEL BYTES".starts_with(&rest), "{rest:?}");
    }

    #[tokio::test]
    async fn oversized_and_truncated_heads_are_errors() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let _ = server.write_all(&[b'x'; 3000]).await;
        });
        let err = read_head(&mut client, 1024).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert_eq!(err.to_string(), "header larger than 1024 bytes");

        let (mut client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            server.write_all(b"HTTP/1.1 200 OK\r\n").await.unwrap();
        });
        let err = read_head(&mut client, 1024).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
}
```

创建 `crates/rurge-proto/src/transport/prefixed.rs`，先只放测试：

```rust
//! A stream that first yields bytes a handshake read past its own end.

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn the_prefix_is_read_first_and_writes_pass_through() {
        let (client, mut server) = tokio::io::duplex(64);
        let mut stream = boxed(b"early ".to_vec(), Box::new(client));
        server.write_all(b"late").await.unwrap();
        let mut buf = [0u8; 10];
        let mut got = Vec::new();
        while got.len() < 10 {
            let n = stream.read(&mut buf[..3]).await.unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, b"early late");
        stream.write_all(b"ping").await.unwrap();
        let mut echo = [0u8; 4];
        server.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");
    }
}
```

Run: `cargo test -p rurge-proto transport::`
Expected: 编译失败——`read_head`、`boxed` 未定义。

- [ ] **Step 3: 实现 `head` 与 `prefixed`**

`crates/rurge-proto/src/transport/head.rs`，加在模块注释之后：

```rust
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

fn end_of_head(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

/// Reads up to and including the first empty line. Returns the head and
/// whatever was read past it (those bytes belong to what follows). A head
/// larger than `limit` bytes is `InvalidData`.
pub async fn read_head<S: AsyncRead + Unpin>(
    stream: &mut S,
    limit: usize,
) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    loop {
        if let Some(end) = end_of_head(&buf) {
            let rest = buf.split_off(end);
            return Ok((buf, rest));
        }
        if buf.len() >= limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("header larger than {limit} bytes"),
            ));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed before the end of the header",
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}
```

`crates/rurge-proto/src/transport/prefixed.rs`，加在模块注释之后：

```rust
use rurge_net::connector::BoxedStream;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub struct Prefixed<S> {
    prefix: Vec<u8>,
    pos: usize,
    inner: S,
}

impl<S> Prefixed<S> {
    pub fn new(prefix: Vec<u8>, inner: S) -> Prefixed<S> {
        Prefixed {
            prefix,
            pos: 0,
            inner,
        }
    }
}

/// `inner` itself when there is nothing to put in front of it.
pub fn boxed(prefix: Vec<u8>, inner: BoxedStream) -> BoxedStream {
    if prefix.is_empty() {
        inner
    } else {
        Box::new(Prefixed::new(prefix, inner))
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.pos < self.prefix.len() {
            let n = (self.prefix.len() - self.pos).min(buf.remaining());
            let start = self.pos;
            buf.put_slice(&self.prefix[start..start + n]);
            self.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
```

Run: `cargo test -p rurge-proto transport::`
Expected: PASS（3 个）。

- [ ] **Step 4: 写失败的测试（三个假上游的自测）**

创建 `crates/rurge-proto/src/testing/mod.rs`：

```rust
//! Scriptable loopback peers for tests (phase 2 M1 design §5.6). Never used
//! by production code. None of them ever resolves a host name.

mod http_proxy;
mod socks5;
mod tls;

pub use http_proxy::{FakeHttpProxy, HttpProxyScript, RecordedHead};
pub use socks5::{FakeSocks5, RecordedSocks5, Socks5Script};
pub use tls::{SeenHandshake, TlsFixture};

use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Aborts the task it holds when dropped.
pub(crate) struct AbortOnDrop(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Echoes every byte back until the peer closes.
pub async fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::net::TcpStream;

    async fn roundtrip<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(stream: &mut S) {
        stream.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ping");
    }

    #[tokio::test]
    async fn the_fake_http_proxy_tunnels_checks_credentials_and_records() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            auth: Some(("u".into(), "p".into())),
            ..HttpProxyScript::default()
        })
        .await;
        // wrong credentials
        let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
        s.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 407 "), "{answer}");
        // dTpw = base64("u:p")
        let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
        s.write_all(
            format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\nProxy-Authorization: Basic dTpw\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        let (head, rest) = crate::transport::head::read_head(&mut s, 4096).await.unwrap();
        assert!(head.starts_with(b"HTTP/1.1 200 "), "{}", String::from_utf8_lossy(&head));
        assert!(rest.is_empty());
        roundtrip(&mut s).await;
        let heads = proxy.heads();
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[1].request_line, format!("CONNECT {echo} HTTP/1.1"));
        assert_eq!(heads[1].header("Proxy-Authorization"), Some("Basic dTpw"));
    }

    #[tokio::test]
    async fn the_fake_http_proxy_never_resolves_names() {
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
        s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        s.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 502 "), "{answer}");
    }

    #[tokio::test]
    async fn the_fake_socks5_server_connects_and_records() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            auth: Some(("u".into(), "p".into())),
            ..Socks5Script::default()
        })
        .await;
        let mut s = TcpStream::connect(server.addr()).await.unwrap();
        s.write_all(&[5, 2, 0, 2]).await.unwrap();
        let mut method = [0u8; 2];
        s.read_exact(&mut method).await.unwrap();
        assert_eq!(method, [5, 2]);
        s.write_all(&[1, 1, b'u', 1, b'p']).await.unwrap();
        let mut ok = [0u8; 2];
        s.read_exact(&mut ok).await.unwrap();
        assert_eq!(ok, [1, 0]);
        let SocketAddr::V4(v4) = echo else { panic!("loopback is v4") };
        let mut request = vec![5, 1, 0, 1];
        request.extend_from_slice(&v4.ip().octets());
        request.extend_from_slice(&v4.port().to_be_bytes());
        s.write_all(&request).await.unwrap();
        let mut reply = [0u8; 10];
        s.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[..2], [5, 0]);
        roundtrip(&mut s).await;
        let seen = server.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].methods, [0, 2]);
        assert_eq!(seen[0].credentials, Some(("u".into(), "p".into())));
        assert_eq!((seen[0].atyp, seen[0].host.as_str(), seen[0].port), (1, "127.0.0.1", v4.port()));
    }

    #[tokio::test]
    async fn the_tls_fixture_records_what_the_client_sent() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(fixture.roots())
        .with_no_client_auth();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
        let tcp = TcpStream::connect(addr).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
        let mut tls = connector.connect(name, tcp).await.unwrap();
        roundtrip(&mut tls).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while fixture.seen().is_empty() {
            assert!(tokio::time::Instant::now() < deadline, "handshake never recorded");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            fixture.seen(),
            [SeenHandshake { sni: Some("localhost".into()), alpn: None, client_cert: false }]
        );
        assert_eq!(fixture.leaf_fingerprint().len(), 32);
        let (cert, key) = fixture.issue_client("client");
        assert!(!cert.is_empty() && !key.is_empty());
    }
}
```

Run: `cargo test -p rurge-proto testing::`
Expected: 编译失败——三个子模块不存在。

- [ ] **Step 5: 实现 `testing::tls`**

创建 `crates/rurge-proto/src/testing/tls.rs`：

```rust
//! A throw-away CA, a server certificate and a TLS acceptor that records
//! what each client sent.

use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rurge_net::connector::BoxedStream;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use sha2::{Digest, Sha256};
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeenHandshake {
    pub sni: Option<String>,
    pub alpn: Option<String>,
    pub client_cert: bool,
}

pub struct TlsFixture {
    issuer: Issuer<'static, KeyPair>,
    ca: CertificateDer<'static>,
    leaf: CertificateDer<'static>,
    /// PKCS#8
    leaf_key: Vec<u8>,
    seen: Arc<Mutex<Vec<SeenHandshake>>>,
}

impl TlsFixture {
    /// A fresh CA and a server certificate for `names` (DNS names or IP literals).
    pub fn new(names: &[&str]) -> Arc<TlsFixture> {
        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_params = CertificateParams::new(Vec::<String>::new()).expect("ca params");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "rurge test CA");
        let ca = ca_params.self_signed(&ca_key).expect("ca cert").der().clone();
        let issuer = Issuer::new(ca_params, ca_key);
        let leaf_key = KeyPair::generate().expect("leaf key");
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let leaf = CertificateParams::new(names)
            .expect("leaf params")
            .signed_by(&leaf_key, &issuer)
            .expect("leaf cert")
            .der()
            .clone();
        Arc::new(TlsFixture {
            issuer,
            ca,
            leaf,
            leaf_key: leaf_key.serialize_der(),
            seen: Arc::default(),
        })
    }

    /// Trust anchors holding only the fixture's CA.
    pub fn roots(&self) -> Arc<RootCertStore> {
        let mut roots = RootCertStore::empty();
        roots.add(self.ca.clone()).expect("ca is a valid trust anchor");
        Arc::new(roots)
    }

    /// SHA-256 of the server certificate (DER).
    pub fn leaf_fingerprint(&self) -> [u8; 32] {
        Sha256::digest(self.leaf.as_ref()).into()
    }

    /// A client certificate signed by the fixture's CA: (certificate DER, PKCS#8 key DER).
    pub fn issue_client(&self, common_name: &str) -> (Vec<u8>, Vec<u8>) {
        let key = KeyPair::generate().expect("client key");
        let mut params = CertificateParams::new(Vec::<String>::new()).expect("client params");
        params.distinguished_name.push(DnType::CommonName, common_name);
        let cert = params.signed_by(&key, &self.issuer).expect("client cert");
        (cert.der().to_vec(), key.serialize_der())
    }

    /// With `require_client_cert`, only clients presenting a certificate
    /// signed by the fixture's CA complete the handshake.
    pub fn acceptor(&self, require_client_cert: bool) -> TlsAcceptor {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .expect("protocol versions");
        let builder = if require_client_cert {
            let verifier = WebPkiClientVerifier::builder_with_provider(self.roots(), provider)
                .build()
                .expect("client verifier");
            builder.with_client_cert_verifier(verifier)
        } else {
            builder.with_no_client_auth()
        };
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.leaf_key.clone()));
        let mut config = builder
            .with_single_cert(vec![self.leaf.clone()], key)
            .expect("server certificate");
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        TlsAcceptor::from(Arc::new(config))
    }

    /// Completes the server side of a handshake and records what the client sent.
    pub async fn accept(&self, acceptor: &TlsAcceptor, tcp: TcpStream) -> io::Result<BoxedStream> {
        let tls = acceptor.accept(tcp).await?;
        let (_, conn) = tls.get_ref();
        self.seen.lock().expect("seen").push(SeenHandshake {
            sni: conn.server_name().map(str::to_string),
            alpn: conn
                .alpn_protocol()
                .map(|p| String::from_utf8_lossy(p).into_owned()),
            client_cert: conn.peer_certificates().is_some(),
        });
        Ok(Box::new(tls))
    }

    pub fn seen(&self) -> Vec<SeenHandshake> {
        self.seen.lock().expect("seen").clone()
    }

    /// A TLS echo server on a loopback port; failed handshakes are dropped silently.
    pub async fn spawn_echo(self: &Arc<Self>, require_client_cert: bool) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let fixture = self.clone();
        let acceptor = self.acceptor(require_client_cert);
        tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (fixture, acceptor) = (fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(mut stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let mut buf = [0u8; 1024];
                    while let Ok(n) = stream.read(&mut buf).await {
                        if n == 0 || stream.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }
}
```

- [ ] **Step 6: 实现 `testing::http_proxy`**

创建 `crates/rurge-proto/src/testing/http_proxy.rs`：

```rust
//! A scriptable HTTP proxy: CONNECT tunnels and absolute-form requests.

use super::{AbortOnDrop, TlsFixture};
use crate::transport::head::read_head;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct HttpProxyScript {
    /// Require these Basic proxy credentials (user, password).
    pub auth: Option<(String, String)>,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<(u16, &'static str)>,
    /// Pad the CONNECT response head with a header of this many bytes.
    pub padding: usize,
    /// Wait this long before answering.
    pub delay: Duration,
    /// Bytes written right behind the response head of a CONNECT.
    pub trailing: Vec<u8>,
    /// Close the connection in the middle of the CONNECT response head.
    pub truncate: bool,
    /// Tunnel here whatever the client asked for (the fake never resolves names).
    pub connect_to: Option<SocketAddr>,
}

#[derive(Clone, Debug)]
pub struct RecordedHead {
    pub request_line: String,
    /// Names as the client wrote them.
    pub headers: Vec<(String, String)>,
}

impl RecordedHead {
    /// Case-insensitive lookup of the first header called `name`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeHttpProxy {
    addr: SocketAddr,
    heads: Arc<Mutex<Vec<RecordedHead>>>,
    _task: AbortOnDrop,
}

fn parse_head(head: &[u8]) -> RecordedHead {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    RecordedHead {
        request_line,
        headers,
    }
}

async fn respond(
    stream: &mut BoxedStream,
    code: u16,
    reason: &str,
    extra: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.shutdown().await
}

async fn serve(
    mut stream: BoxedStream,
    script: HttpProxyScript,
    heads: Arc<Mutex<Vec<RecordedHead>>>,
) -> io::Result<()> {
    let (head, rest) = read_head(&mut stream, 64 * 1024).await?;
    let recorded = parse_head(&head);
    heads.lock().expect("heads").push(recorded.clone());
    tokio::time::sleep(script.delay).await;
    if let Some((user, password)) = &script.auth {
        let expected = format!("Basic {}", STANDARD.encode(format!("{user}:{password}")));
        if recorded.header("Proxy-Authorization") != Some(expected.as_str()) {
            return respond(
                &mut stream,
                407,
                "Proxy Authentication Required",
                "Proxy-Authenticate: Basic realm=\"fake\"\r\n",
                b"",
            )
            .await;
        }
    }
    if let Some((code, reason)) = script.refuse {
        return respond(&mut stream, code, reason, "", b"").await;
    }
    let mut parts = recorded.request_line.split(' ');
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if method != "CONNECT" {
        // absolute-form request: answered here, tests only inspect what arrived
        return respond(&mut stream, 200, "OK", "", b"forwarded").await;
    }
    let Some(upstream_addr) = script.connect_to.or_else(|| target.parse().ok()) else {
        return respond(&mut stream, 502, "Bad Gateway", "", b"").await;
    };
    let Ok(mut upstream) = TcpStream::connect(upstream_addr).await else {
        return respond(&mut stream, 502, "Bad Gateway", "", b"").await;
    };
    let mut response = String::from("HTTP/1.1 200 Connection established\r\n");
    if script.padding > 0 {
        response.push_str(&format!("X-Padding: {}\r\n", "p".repeat(script.padding)));
    }
    response.push_str("\r\n");
    if script.truncate {
        stream
            .write_all(&response.as_bytes()[..response.len() / 2])
            .await?;
        return stream.shutdown().await;
    }
    stream.write_all(response.as_bytes()).await?;
    stream.write_all(&script.trailing).await?;
    upstream.write_all(&rest).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

impl FakeHttpProxy {
    pub async fn spawn(script: HttpProxyScript) -> FakeHttpProxy {
        FakeHttpProxy::start(script, None).await
    }

    /// The same proxy behind TLS (`https` policies).
    pub async fn spawn_tls(
        script: HttpProxyScript,
        fixture: Arc<TlsFixture>,
        require_client_cert: bool,
    ) -> FakeHttpProxy {
        FakeHttpProxy::start(script, Some((fixture, require_client_cert))).await
    }

    async fn start(script: HttpProxyScript, tls: Option<(Arc<TlsFixture>, bool)>) -> FakeHttpProxy {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let heads: Arc<Mutex<Vec<RecordedHead>>> = Arc::default();
        let log = heads.clone();
        let tls = tls.map(|(fixture, require)| {
            let acceptor = fixture.acceptor(require);
            (fixture, acceptor)
        });
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, log, tls) = (script.clone(), log.clone(), tls.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match &tls {
                        Some((fixture, acceptor)) => match fixture.accept(acceptor, tcp).await {
                            Ok(s) => s,
                            Err(_) => return,
                        },
                        None => Box::new(tcp),
                    };
                    let _ = serve(stream, script, log).await;
                });
            }
        });
        FakeHttpProxy {
            addr,
            heads,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every request head seen so far, in arrival order.
    pub fn heads(&self) -> Vec<RecordedHead> {
        self.heads.lock().expect("heads").clone()
    }
}
```

- [ ] **Step 7: 实现 `testing::socks5`**

创建 `crates/rurge-proto/src/testing/socks5.rs`：

```rust
//! A scriptable SOCKS5 server (RFC 1928 / 1929), CONNECT only.

use super::{AbortOnDrop, TlsFixture};
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct Socks5Script {
    /// Require these credentials (user, password).
    pub auth: Option<(String, String)>,
    /// Reply code for the CONNECT request (0 = succeeded).
    pub reply: u8,
    /// Connect here whatever the client asked for (the fake never resolves names).
    pub connect_to: Option<SocketAddr>,
    /// Wait this long before replying to the request.
    pub delay: Duration,
    /// Close right after the method selection.
    pub hang_up_after_greeting: bool,
    /// Select this method even if the client did not offer it.
    pub force_method: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedSocks5 {
    pub methods: Vec<u8>,
    pub credentials: Option<(String, String)>,
    pub atyp: u8,
    /// The address as text: an IP literal or the domain name.
    pub host: String,
    pub port: u16,
}

pub struct FakeSocks5 {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
    _task: AbortOnDrop,
}

async fn read_vec(stream: &mut BoxedStream, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf).await?;
    Ok(buf)
}

async fn reply(stream: &mut BoxedStream, code: u8) -> io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await
}

async fn serve(
    mut stream: BoxedStream,
    script: Socks5Script,
    requests: Arc<Mutex<Vec<RecordedSocks5>>>,
) -> io::Result<()> {
    let greeting = read_vec(&mut stream, 2).await?;
    let methods = read_vec(&mut stream, usize::from(greeting[1])).await?;
    let wanted = if script.auth.is_some() { 2 } else { 0 };
    let method = script
        .force_method
        .unwrap_or(if methods.contains(&wanted) { wanted } else { 0xff });
    stream.write_all(&[5, method]).await?;
    if method == 0xff || script.hang_up_after_greeting {
        return stream.shutdown().await;
    }
    let mut credentials = None;
    if method == 2 {
        let head = read_vec(&mut stream, 2).await?;
        let user = String::from_utf8_lossy(&read_vec(&mut stream, usize::from(head[1])).await?)
            .into_owned();
        let plen = read_vec(&mut stream, 1).await?[0];
        let password =
            String::from_utf8_lossy(&read_vec(&mut stream, usize::from(plen)).await?).into_owned();
        let ok = script.auth == Some((user.clone(), password.clone()));
        credentials = Some((user, password));
        stream.write_all(&[1, u8::from(!ok)]).await?;
        if !ok {
            return stream.shutdown().await;
        }
    }
    let request = read_vec(&mut stream, 4).await?;
    let atyp = request[3];
    let (host, literal) = match atyp {
        1 => {
            let b = read_vec(&mut stream, 4).await?;
            let ip = IpAddr::V4(Ipv4Addr::new(b[0], b[1], b[2], b[3]));
            (ip.to_string(), Some(ip))
        }
        4 => {
            let b: [u8; 16] = read_vec(&mut stream, 16)
                .await?
                .try_into()
                .expect("sixteen bytes");
            let ip = IpAddr::V6(Ipv6Addr::from(b));
            (ip.to_string(), Some(ip))
        }
        _ => {
            let len = read_vec(&mut stream, 1).await?[0];
            let name = read_vec(&mut stream, usize::from(len)).await?;
            (String::from_utf8_lossy(&name).into_owned(), None)
        }
    };
    let port_bytes = read_vec(&mut stream, 2).await?;
    let port = u16::from_be_bytes([port_bytes[0], port_bytes[1]]);
    requests.lock().expect("requests").push(RecordedSocks5 {
        methods,
        credentials,
        atyp,
        host,
        port,
    });
    tokio::time::sleep(script.delay).await;
    if script.reply != 0 {
        reply(&mut stream, script.reply).await?;
        return stream.shutdown().await;
    }
    let target = script
        .connect_to
        .or_else(|| literal.map(|ip| SocketAddr::new(ip, port)));
    let Some(target) = target else {
        // a domain name and nowhere to send it: host unreachable
        reply(&mut stream, 4).await?;
        return stream.shutdown().await;
    };
    let Ok(mut upstream) = TcpStream::connect(target).await else {
        reply(&mut stream, 5).await?;
        return stream.shutdown().await;
    };
    reply(&mut stream, 0).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

impl FakeSocks5 {
    pub async fn spawn(script: Socks5Script) -> FakeSocks5 {
        FakeSocks5::start(script, None).await
    }

    /// The same server behind TLS (`socks5-tls` policies).
    pub async fn spawn_tls(
        script: Socks5Script,
        fixture: Arc<TlsFixture>,
        require_client_cert: bool,
    ) -> FakeSocks5 {
        FakeSocks5::start(script, Some((fixture, require_client_cert))).await
    }

    async fn start(script: Socks5Script, tls: Option<(Arc<TlsFixture>, bool)>) -> FakeSocks5 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests: Arc<Mutex<Vec<RecordedSocks5>>> = Arc::default();
        let log = requests.clone();
        let tls = tls.map(|(fixture, require)| {
            let acceptor = fixture.acceptor(require);
            (fixture, acceptor)
        });
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let (script, log, tls) = (script.clone(), log.clone(), tls.clone());
                tokio::spawn(async move {
                    let stream: BoxedStream = match &tls {
                        Some((fixture, acceptor)) => match fixture.accept(acceptor, tcp).await {
                            Ok(s) => s,
                            Err(_) => return,
                        },
                        None => Box::new(tcp),
                    };
                    let _ = serve(stream, script, log).await;
                });
            }
        });
        FakeSocks5 {
            addr,
            requests,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every CONNECT request seen so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedSocks5> {
        self.requests.lock().expect("requests").clone()
    }
}
```

- [ ] **Step 8: 跑测试确认通过**

Run: `cargo test -p rurge-proto testing:: && cargo test -p rurge-proto transport::`
Expected: PASS（`testing` 4 个、`transport` 3 个）。再确认 feature 自身能编译：`cargo check -p rurge-proto --features testing`，Expected: 无警告。

- [ ] **Step 9: 门禁与提交**

```bash
git add crates/rurge-proto Cargo.lock
git commit -m "test(proto): 可编排的回环假上游（HTTP 代理、SOCKS5、TLS 夹具）与 transport::head / prefixed"
```

---

### Task 9: `rurge_net::tls::root_store` 与 TLS 层 `transport::tls`

**Files:**
- Create: `crates/rurge-net/src/tls.rs`
- Modify: `crates/rurge-net/src/lib.rs`
- Modify: `crates/rurge-net/src/http.rs`（`build_tls_config` 改用 `root_store()`）
- Create: `crates/rurge-proto/src/transport/tls.rs`
- Modify: `crates/rurge-proto/src/transport/mod.rs`

**Interfaces:**
- Consumes: `rurge_config::spec::{Sni, TlsOpts}`（Task 2）；`rurge_config::HostName`；`rurge_proto::BuildError`（Task 7）；`rurge_proto::testing::TlsFixture`（Task 8，仅测试）。
- Produces:
  - `rurge_net::tls::root_store() -> Arc<rustls::RootCertStore>`（系统根，空则 webpki 兜底；进程内只装载一次）
  - `rurge_proto::transport::tls::ClientIdentity { pub chain: Vec<CertificateDer<'static>>, pub key: PrivateKeyDer<'static> }`
  - `rurge_proto::transport::tls::TlsClient`：`TlsClient::build(opts: &TlsOpts, server: &HostName, default_alpn: &[&str], identity: Option<ClientIdentity>, roots: Arc<RootCertStore>) -> Result<TlsClient, BuildError>`，`TlsClient::wrap(&self, stream: BoxedStream) -> io::Result<BoxedStream>`
  - 校验优先级：指纹 > `skip-cert-verify` > 标准；三种模式都校验握手签名。

- [ ] **Step 1: `root_store`（先测试）**

创建 `crates/rurge-net/src/tls.rs`：

```rust
//! The trust anchors every rurge TLS client starts from.

use std::sync::{Arc, OnceLock};

/// The operating system's root certificates, or `webpki-roots` when none can
/// be loaded. Loaded once per process: reading the native store is slow.
pub fn root_store() -> Arc<rustls::RootCertStore> {
    static ROOTS: OnceLock<Arc<rustls::RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            let (added, _ignored) = roots.add_parsable_certificates(native.certs);
            if added == 0 {
                tracing::warn!(
                    errors = native.errors.len(),
                    "no native root certificates loaded; using webpki-roots"
                );
                roots
                    .roots
                    .extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            }
            Arc::new(roots)
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_store_is_never_empty_and_is_shared() {
        let a = root_store();
        assert!(!a.is_empty());
        assert!(Arc::ptr_eq(&a, &root_store()));
    }
}
```

`crates/rurge-net/src/lib.rs` 加 `pub mod tls;`。`crates/rurge-net/src/http.rs` 的 `build_tls_config` 里，`else` 分支整个换成：

```rust
        builder
            .with_root_certificates(crate::tls::root_store())
            .with_no_client_auth()
```

Run: `cargo test -p rurge-net`
Expected: PASS（含新用例；HTTPS 相关的既有用例不变）。

- [ ] **Step 2: 写失败的测试（`TlsClient`）**

`crates/rurge-proto/src/transport/mod.rs` 加 `pub mod tls;`。创建 `crates/rurge-proto/src/transport/tls.rs`，先只放测试：

```rust
//! TLS towards a proxy: the six TLS parameters of the compatibility matrix §4.4.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{SeenHandshake, TlsFixture};
    use rurge_config::spec::{Sni, TlsOpts};
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn empty_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    /// Handshakes with the fixture's echo server and proves the tunnel works.
    async fn talk(
        addr: SocketAddr,
        opts: &TlsOpts,
        host: &str,
        roots: Arc<RootCertStore>,
    ) -> io::Result<()> {
        let client = TlsClient::build(opts, &HostName::parse(host), &[], None, roots)
            .expect("the options build");
        let tcp = TcpStream::connect(addr).await?;
        let mut stream = client.wrap(Box::new(tcp)).await?;
        stream.write_all(b"ping").await?;
        let mut buf = [0u8; 4];
        stream.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"ping");
        Ok(())
    }

    async fn last_seen(fixture: &TlsFixture, count: usize) -> SeenHandshake {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while fixture.seen().len() < count {
            assert!(tokio::time::Instant::now() < deadline, "handshake never recorded");
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        fixture.seen()[count - 1].clone()
    }

    #[tokio::test]
    async fn standard_verification_uses_the_server_name() {
        let fixture = TlsFixture::new(&["localhost", "proxy.test"]);
        let addr = fixture.spawn_echo(false).await;
        talk(addr, &TlsOpts::default(), "localhost", fixture.roots()).await.unwrap();
        assert_eq!(last_seen(&fixture, 1).await.sni.as_deref(), Some("localhost"));
        // a name the certificate does not cover
        let err = talk(addr, &TlsOpts::default(), "wrong.test", fixture.roots()).await.unwrap_err();
        assert!(err.to_string().contains("NotValidForName"), "{err}");
        // an unknown CA
        assert!(talk(addr, &TlsOpts::default(), "localhost", empty_roots_with_one_other_ca()).await.is_err());
    }

    fn empty_roots_with_one_other_ca() -> Arc<RootCertStore> {
        TlsFixture::new(&["other.test"]).roots()
    }

    #[tokio::test]
    async fn verify_name_and_sni_are_independent() {
        let fixture = TlsFixture::new(&["localhost", "proxy.test"]);
        let addr = fixture.spawn_echo(false).await;
        // verify against another name than the one connected to
        let opts = TlsOpts { verify_name: Some("localhost".into()), ..TlsOpts::default() };
        talk(addr, &opts, "wrong.test", fixture.roots()).await.unwrap();
        assert_eq!(last_seen(&fixture, 1).await.sni.as_deref(), Some("wrong.test"));
        // a custom SNI is what gets sent, and what gets verified by default
        let opts = TlsOpts { sni: Sni::Name("proxy.test".into()), ..TlsOpts::default() };
        talk(addr, &opts, "wrong.test", fixture.roots()).await.unwrap();
        assert_eq!(last_seen(&fixture, 2).await.sni.as_deref(), Some("proxy.test"));
        // sni = off: nothing is sent, verification still uses the host name
        let opts = TlsOpts { sni: Sni::Off, ..TlsOpts::default() };
        talk(addr, &opts, "localhost", fixture.roots()).await.unwrap();
        assert_eq!(last_seen(&fixture, 3).await.sni, None);
    }

    #[tokio::test]
    async fn an_ip_literal_sends_no_sni() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let addr = fixture.spawn_echo(false).await;
        talk(addr, &TlsOpts::default(), "127.0.0.1", fixture.roots()).await.unwrap();
        assert_eq!(last_seen(&fixture, 1).await.sni, None);
    }

    #[tokio::test]
    async fn a_pinned_fingerprint_replaces_chain_validation() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let pinned = TlsOpts {
            fingerprint_sha256: Some(fixture.leaf_fingerprint()),
            // loses against the fingerprint
            skip_cert_verify: true,
            ..TlsOpts::default()
        };
        // no trust anchors and a name the certificate does not cover: still fine
        talk(addr, &pinned, "wrong.test", empty_roots()).await.unwrap();
        let wrong = TlsOpts { fingerprint_sha256: Some([7u8; 32]), skip_cert_verify: true, ..TlsOpts::default() };
        let err = talk(addr, &wrong, "localhost", fixture.roots()).await.unwrap_err();
        assert!(err.to_string().contains("server-cert-fingerprint-sha256"), "{err}");
    }

    #[tokio::test]
    async fn skip_cert_verify_accepts_anything() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let opts = TlsOpts { skip_cert_verify: true, ..TlsOpts::default() };
        talk(addr, &opts, "wrong.test", empty_roots()).await.unwrap();
    }

    #[tokio::test]
    async fn alpn_comes_from_the_options_or_the_protocol_default() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(false).await;
        let opts = TlsOpts { alpn: vec!["http/1.1".into()], ..TlsOpts::default() };
        talk(addr, &opts, "localhost", fixture.roots()).await.unwrap();
        assert_eq!(last_seen(&fixture, 1).await.alpn.as_deref(), Some("http/1.1"));
        let client = TlsClient::build(&TlsOpts::default(), &HostName::parse("localhost"), &["h2"], None, fixture.roots()).unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let _stream = client.wrap(Box::new(tcp)).await.unwrap();
        assert_eq!(last_seen(&fixture, 2).await.alpn.as_deref(), Some("h2"));
    }

    #[test]
    fn unusable_names_are_build_errors() {
        let opts = TlsOpts { sni: Sni::Name("not a name".into()), ..TlsOpts::default() };
        let err = TlsClient::build(&opts, &HostName::parse("localhost"), &[], None, empty_roots()).map(|_| ()).unwrap_err();
        assert_eq!(err.message, "`not a name` is not a valid TLS server name");
        // standard verification needs trust anchors
        let err = TlsClient::build(&TlsOpts::default(), &HostName::parse("localhost"), &[], None, empty_roots()).map(|_| ()).unwrap_err();
        assert!(err.message.starts_with("cannot set up certificate verification"), "{}", err.message);
    }
}
```

Run: `cargo test -p rurge-proto transport::tls`
Expected: 编译失败——`TlsClient` 未定义。

- [ ] **Step 3: 实现 `TlsClient`**

在 `crates/rurge-proto/src/transport/tls.rs` 的模块注释之后、测试模块之前加：

```rust
use crate::BuildError;
use rurge_config::HostName;
use rurge_config::spec::{Sni, TlsOpts};
use rurge_net::connector::BoxedStream;
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};
use std::io;
use std::sync::Arc;
use tokio_rustls::TlsConnector;

/// A client certificate chain (leaf first) and its private key.
pub struct ClientIdentity {
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
}

#[derive(Debug)]
enum Mode {
    Standard {
        inner: Arc<WebPkiServerVerifier>,
        /// `server-cert-verify-name`: verified instead of the connection's name.
        verify_name: Option<ServerName<'static>>,
    },
    /// SHA-256 of the leaf certificate; replaces chain validation (manual).
    Pinned([u8; 32]),
    Insecure,
}

#[derive(Debug)]
struct Verifier {
    mode: Mode,
    provider: Arc<CryptoProvider>,
}

fn same(a: &[u8], b: &[u8]) -> bool {
    // constant time: no early exit on the first differing byte
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        match &self.mode {
            Mode::Standard { inner, verify_name } => inner.verify_server_cert(
                end_entity,
                intermediates,
                verify_name.as_ref().unwrap_or(server_name),
                ocsp_response,
                now,
            ),
            Mode::Pinned(expected) => {
                if same(&Sha256::digest(end_entity.as_ref()), expected) {
                    Ok(ServerCertVerified::assertion())
                } else {
                    Err(rustls::Error::General(
                        "the server certificate does not match server-cert-fingerprint-sha256"
                            .into(),
                    ))
                }
            }
            Mode::Insecure => Ok(ServerCertVerified::assertion()),
        }
    }

    // Every mode still proves that the peer holds the certificate's key.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn dns_name(name: &str) -> Result<ServerName<'static>, BuildError> {
    ServerName::try_from(name.to_string())
        .map_err(|_| BuildError::new(format!("`{name}` is not a valid TLS server name")))
}

/// Everything that can be prepared ahead of a connection: built once per
/// outbound, used for every handshake.
pub struct TlsClient {
    connector: TlsConnector,
    name: ServerName<'static>,
}

impl TlsClient {
    /// `server` is the proxy's host. `default_alpn` applies when the policy
    /// sets none. `roots` is only consulted by standard verification.
    pub fn build(
        opts: &TlsOpts,
        server: &HostName,
        default_alpn: &[&str],
        identity: Option<ClientIdentity>,
        roots: Arc<RootCertStore>,
    ) -> Result<TlsClient, BuildError> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let name = match (&opts.sni, server) {
            (Sni::Name(custom), _) => dns_name(custom)?,
            (_, HostName::Domain(d)) => dns_name(d)?,
            // rustls sends no SNI for an IP address
            (_, HostName::Ip(ip)) => ServerName::IpAddress((*ip).into()),
        };
        let mode = if let Some(fingerprint) = opts.fingerprint_sha256 {
            Mode::Pinned(fingerprint)
        } else if opts.skip_cert_verify {
            Mode::Insecure
        } else {
            let verify_name = opts.verify_name.as_deref().map(dns_name).transpose()?;
            let inner = WebPkiServerVerifier::builder_with_provider(roots, provider.clone())
                .build()
                .map_err(|e| {
                    BuildError::new(format!("cannot set up certificate verification: {e}"))
                })?;
            Mode::Standard { inner, verify_name }
        };
        let builder = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| BuildError::new(format!("cannot set up TLS: {e}")))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Verifier { mode, provider }));
        let mut config = match identity {
            Some(identity) => builder
                .with_client_auth_cert(identity.chain, identity.key)
                .map_err(|e| {
                    BuildError::new(format!("the client certificate cannot be used: {e}"))
                })?,
            None => builder.with_no_client_auth(),
        };
        config.enable_sni = opts.sni != Sni::Off;
        config.alpn_protocols = if opts.alpn.is_empty() {
            default_alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
        } else {
            opts.alpn.iter().map(|p| p.as_bytes().to_vec()).collect()
        };
        Ok(TlsClient {
            connector: TlsConnector::from(Arc::new(config)),
            name,
        })
    }

    pub async fn wrap(&self, stream: BoxedStream) -> io::Result<BoxedStream> {
        let tls = self.connector.connect(self.name.clone(), stream).await?;
        Ok(Box::new(tls))
    }
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-proto transport::tls`
Expected: PASS（7 个）。`standard_verification_uses_the_server_name` 断言的 `NotValidForName` 是 rustls 0.23 对名字不匹配的错误文本；若实际文本不同，把断言改成实际文本里能区分"名字不匹配"的那一段，并在报告里写明。

- [ ] **Step 5: 门禁与提交**

```bash
git add crates/rurge-net crates/rurge-proto
git commit -m "feat(proto): TLS 层 TlsClient（标准 / 指纹 / 不校验三种模式、sni、verify-name、alpn）；根证书装载抽成 rurge_net::tls::root_store"
```

---

### Task 10: `decode_p12`——`[Keystore]` 的 p12 解码

**Files:**
- Modify: `Cargo.toml`（工作区：加 `p12-keystore = "0.2"`）
- Modify: `crates/rurge-proto/Cargo.toml`
- Create: `crates/rurge-proto/src/keystore.rs`
- Modify: `crates/rurge-proto/src/lib.rs`

**Interfaces:**
- Consumes: `rurge_config::{KeystoreItem, KeystoreType}`（`KeystoreItem { name, kind, base64, password: Option<String>, unknown, span }`）；`p12_keystore::{Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, MacAlgorithm, PrivateKeyChain}`（0.2.1：`KeyStore::from_pkcs12(&[u8], &str)`、`private_key_chain() -> Option<(&str, &PrivateKeyChain)>`、`PrivateKeyChain::{key() -> &[u8], chain() -> &[Certificate]}`、`Certificate::as_der() -> &[u8]`）；Task 9 的 `ClientIdentity` 与 `TlsClient`；Task 8 的 `TlsFixture`。
- Produces: `rurge_proto::keystore::decode_p12(item: &KeystoreItem) -> Result<ClientIdentity, BuildError>`。错误文本含条目名，**不含**密码与 Base64 内容。

- [ ] **Step 1: 依赖**

工作区 `Cargo.toml` 的 `[workspace.dependencies]` 末尾加 `p12-keystore = "0.2"`；`crates/rurge-proto/Cargo.toml` 的 `[dependencies]` 加 `p12-keystore.workspace = true`；`crates/rurge-proto/src/lib.rs` 加 `pub mod keystore;`。

Run: `cargo tree -p rurge-proto -e normal --prefix none | sort -u | grep -E -- "-(pre|rc|alpha|beta)" ; echo "exit=$?"`
Expected: 没有任何输出行（`grep` 退出码 1）——解析出来的依赖树里不能有预发布版本。若出现，停下来报告，不要继续。

- [ ] **Step 2: 写失败的测试**

创建 `crates/rurge-proto/src/keystore.rs`，先只放测试。两段 Base64 是用 OpenSSL 3.2.4 生成的**测试专用**证书与私钥（`openssl ecparam -name prime256v1 -genkey`、`openssl req -new -x509 -subj "/CN=rurge test client"`、`openssl pkcs12 -export [-legacy] -passout pass:rurge-test`），照抄，不要换行：

```rust
//! `[Keystore]` material decoded at build time.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TlsFixture;
    use crate::transport::tls::TlsClient;
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use p12_keystore::{
        Certificate, EncryptionAlgorithm, KeyStore, KeyStoreEntry, MacAlgorithm, PrivateKeyChain,
    };
    use rurge_config::spec::TlsOpts;
    use rurge_config::{HostName, KeystoreType, Span};
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    /// PBES2 / PBKDF2 / AES-256-CBC, MAC sha256 (the OpenSSL 3 default).
    const OPENSSL_MODERN: &str = "MIIEaQIBAzCCBB8GCSqGSIb3DQEHAaCCBBAEggQMMIIECDCCApoGCSqGSIb3DQEHBqCCAoswggKHAgEAMIICgAYJKoZIhvcNAQcBMF8GCSqGSIb3DQEFDTBSMDEGCSqGSIb3DQEFDDAkBBB6HUp0PGsWgLTI6W8R02dlAgIIADAMBggqhkiG9w0CCQUAMB0GCWCGSAFlAwQBKgQQM/JUD6j7ZhYhBzjwpZJDkoCCAhAPafxNG+tkEs1U7CV/HKK4SebsswJeAwQiAaZJwbsAwnz8/FX5Gr/m6RhUClFLm7sPnWMxm1AGYk2pR05hwGbyl4URBQviViYNXuJxdaQJmv0/eMGrSjvTXjAqAKFCrFA5Xl2e0VwUoTYWzMr2V4ejIeQTW4mz5biEQySUd0ll1p8cu4f822nkov9iIhFD/IbEjK+5X2QEAW4TvidOIe3xhE+vzCSA+959erv4A84po8C0DzVjP9Ph3W1X+EY6boclV7lAYp1VYyr67MC2D4a++XcGjMd1yhdvGfpfy23p4JABbJRYMe0O/oDLCF5pGcwjIk8g6+uFVqe+wMWCL8E2i+q9xJCPVmk2aWEMqxrWaukmLmxK5C6oLVMYStCuWdi/S6TLyDMEHD/4tZJTQvEyu653yZHzHjOXS6yR56j+0bc2h8V6VzhVttrMv0xCjYJUDNmf+2vWiJy1sUZkZc8uCw/LS1nUojCXCLBvaJQUd9dNwvv1MWxWD4gESNzSvs1dH8meaiAx0LC/wk9aqpocwFI+mzvsnyXd1l3ob2ecdqxUZByjsUvEuEBR1D1bWuX8HkM83J7WcVt2IOfXwYL81igHWoREOFADb1cPYpFM1ToWdLdnEddgfhy+I9rmye68+sgaQZVOkFwUiEGOPSzMHptolrMPKLMylZKMMkiF3EBY03aPBDAxCkl0ufnL/dcwggFmBgkqhkiG9w0BBwGgggFXBIIBUzCCAU8wggFLBgsqhkiG9w0BDAoBAqCB9zCB9DBfBgkqhkiG9w0BBQ0wUjAxBgkqhkiG9w0BBQwwJAQQY48lLjTXFIbuo58TlShk1gICCAAwDAYIKoZIhvcNAgkFADAdBglghkgBZQMEASoEEJaHl3v8mHNKgSGU4zkSBlIEgZAXK2FgrzL88deZuiabDTuH6vauWw/yH85wKB/1IrAw1goymBhvHQxAaKVbqV3Wm2zMlJkFwVhe1XSVRt5yI47pVgJ1BRh8Y0w6+hHf5SCcnq4es3a3JSS9tYGI2ENMixWQhtMl2IMqR48gsHtRJaGHJN/oCtvXOmYclN020DqW2TWQ85rR7NTztFPIS2a21+sxQjAbBgkqhkiG9w0BCRQxDh4MAGMAbABpAGUAbgB0MCMGCSqGSIb3DQEJFTEWBBRTsNY1tmgfOAeFrNhGIM2POMM6/jBBMDEwDQYJYIZIAWUDBAIBBQAEIGmDsVPr0Drmu9yRYgK1vDM7SW0HwC2j+CjsfNx4ZkXzBAir+S/IETZiQgICCAA=";
    /// pbeWithSHA1And40BitRC2-CBC certificate bag, pbeWithSHA1And3-KeyTripleDES-CBC key bag, MAC sha1 (`-legacy`).
    const OPENSSL_LEGACY: &str = "MIID0wIBAzCCA5kGCSqGSIb3DQEHAaCCA4oEggOGMIIDgjCCAlcGCSqGSIb3DQEHBqCCAkgwggJEAgEAMIICPQYJKoZIhvcNAQcBMBwGCiqGSIb3DQEMAQYwDgQIcuV1FKxH1JcCAggAgIICEGlkyJYANwRYwejjupnou1kGiu2SEPCNrsyPJLbMHyPoOc1plcCGVJNcRYUUi1HDgQlEEvMEWJGLfszKe8PRik01hLi6AGLdxwnrptqND/rPzpMRJd2HtNEs3OCDPqaDQapp4i31ZtldNbM2dzSqZQxBZrIMytLoJZZnBveOApV5uOTuHCUYn5hFUbERpZ+ua0vkeSa6yM3yC4UBCkwwN3TCqxjMO/f4qKawdy4/FnYB81P2jOjh0kJOcdRGRPUYiKQnoFzsWH29nus+2TJMYjZLFZwGAJpFyy5PMGQ+4u/FmrYPodHxvlnFem5Gfyt9T8oxEIIoGzul8ixHb1drGI14ZcN7lkqitJWDmqpBgoYeNrWb1Eyo3iruwFu2ITGRVk8r3EoxKMrjtwUHQhBGT/GYHVjBtCtZpjSBZZcWnV11nmyxScdPvlzb/0LCR8eJv6AQm399ZHL23GMslJrhr9yj+bCetpy0uq0P7Z2VYWQ4dfUuBvPE6aDPCc35kn+aRUkjgCh5lFObJQ5cd9aOssg9fAEPWZdaIx4C7wSssf/WkEVibw57Rc5KyZPIIXPKAv+Y+HniqsMt2ivYuo3JWhleW5y9GkQFm21MrjU1mo4suoXw9BRRxDcwyjeF/26DA16tDkLu8+v4qbdddkBnZLGMk9rgSGVEeF+MommhoW8lcvjvnj4/zgH8nWqfQtwpMjCCASMGCSqGSIb3DQEHAaCCARQEggEQMIIBDDCCAQgGCyqGSIb3DQEMCgECoIG0MIGxMBwGCiqGSIb3DQEMAQMwDgQIQvYVnv7KY8kCAggABIGQeCPLHIb2vKaFBaaneTHI8U8p/S9QQnF99LuPFy18zM2COf+D725SdUP46f+ItEkISnC2n8rWA4Cz2F1miYhQXdt9k/xaKibcM0w85GVgLLfwnoHl3bEmEX7uOZzVETKR+s/ND7xY3ubbCyImeT5Z3DI9fwa6tf95gTThSJvnmvgfy3X+skvKcvuUcPesQjC3MUIwGwYJKoZIhvcNAQkUMQ4eDABjAGwAaQBlAG4AdDAjBgkqhkiG9w0BCRUxFgQUU7DWNbZoHzgHhazYRiDNjzjDOv4wMTAhMAkGBSsOAwIaBQAEFH0qoFogC0j/D3eS5xuzYaKHhDyCBAhqWJ84rvZ7aQICCAA=";
    /// SHA-256 of the certificate (DER) inside both files.
    const OPENSSL_CERT_SHA256: &str = "308381cecb58ed0ed0920b78e04320ecd49b880c48ffbe025580b74a96c41f0b";

    fn item(base64: &str, password: Option<&str>) -> KeystoreItem {
        KeystoreItem {
            name: "cert1".into(),
            kind: KeystoreType::P12,
            base64: base64.into(),
            password: password.map(str::to_string),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn files_written_by_openssl_decode_with_either_encryption() {
        for fixture in [OPENSSL_MODERN, OPENSSL_LEGACY] {
            let identity = decode_p12(&item(fixture, Some("rurge-test"))).unwrap();
            assert_eq!(identity.chain.len(), 1);
            assert_eq!(hex(&Sha256::digest(identity.chain[0].as_ref())), OPENSSL_CERT_SHA256);
            // rustls can sign with the key
            rustls::crypto::ring::default_provider()
                .key_provider
                .load_private_key(identity.key)
                .expect("a usable PKCS#8 key");
        }
    }

    #[test]
    fn failures_name_the_item_and_never_the_secret() {
        let err = decode_p12(&item(OPENSSL_MODERN, Some("hunter2-wrong"))).map(|_| ()).unwrap_err();
        assert!(err.message.starts_with("keystore item `cert1` cannot be decoded"), "{}", err.message);
        assert!(!err.message.contains("hunter2-wrong"));
        let err = decode_p12(&item("@@@", Some("x"))).map(|_| ()).unwrap_err();
        assert_eq!(err.message, "keystore item `cert1`: `base64` is not valid Base64");
        let err = decode_p12(&item("AAAA", Some("x"))).map(|_| ()).unwrap_err();
        assert!(err.message.starts_with("keystore item `cert1` cannot be decoded"));
    }

    /// A p12 holding a client certificate signed by the fixture's CA.
    fn p12_for(fixture: &TlsFixture, algorithm: EncryptionAlgorithm, mac: MacAlgorithm) -> String {
        let (cert, key) = fixture.issue_client("rurge mtls client");
        let chain = PrivateKeyChain::new(key, [1u8, 2, 3, 4], [Certificate::from_der(&cert).unwrap()]);
        let mut store = KeyStore::new();
        store.add_entry("client", KeyStoreEntry::PrivateKeyChain(chain));
        let der = store
            .writer("pw")
            .encryption_algorithm(algorithm)
            .mac_algorithm(mac)
            .write()
            .unwrap();
        STANDARD.encode(der)
    }

    #[tokio::test]
    async fn a_decoded_identity_completes_mutual_tls() {
        let fixture = TlsFixture::new(&["localhost"]);
        let addr = fixture.spawn_echo(true).await;
        for (algorithm, mac) in [
            (EncryptionAlgorithm::PbeWithHmacSha256AndAes256, MacAlgorithm::HmacSha256),
            (EncryptionAlgorithm::PbeWithShaAnd3KeyTripleDesCbc, MacAlgorithm::HmacSha1),
        ] {
            let identity = decode_p12(&item(&p12_for(&fixture, algorithm, mac), Some("pw"))).unwrap();
            let client = TlsClient::build(
                &TlsOpts::default(),
                &HostName::parse("localhost"),
                &[],
                Some(identity),
                fixture.roots(),
            )
            .unwrap();
            let tcp = TcpStream::connect(addr).await.unwrap();
            let mut stream = client.wrap(Box::new(tcp)).await.unwrap();
            stream.write_all(b"ping").await.unwrap();
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await.unwrap();
            assert_eq!(&buf, b"ping");
        }
        assert!(fixture.seen().iter().all(|h| h.client_cert));
        // without a certificate the server refuses
        let bare = TlsClient::build(&TlsOpts::default(), &HostName::parse("localhost"), &[], None, fixture.roots()).unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let refused = async {
            let mut s = bare.wrap(Box::new(tcp)).await?;
            s.write_all(b"ping").await?;
            let mut buf = [0u8; 4];
            s.read_exact(&mut buf).await.map(|_| ())
        };
        assert!(refused.await.is_err());
    }
}
```

Run: `cargo test -p rurge-proto keystore::`
Expected: 编译失败——`decode_p12` 未定义。

- [ ] **Step 3: 实现**

在 `crates/rurge-proto/src/keystore.rs` 的模块注释之后、测试模块之前加：

```rust
use crate::BuildError;
use crate::transport::tls::ClientIdentity;
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use rurge_config::KeystoreItem;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// Decodes a `p12` keystore item into a client identity (leaf first).
/// The error names the item; it never repeats the password or the material.
pub fn decode_p12(item: &KeystoreItem) -> Result<ClientIdentity, BuildError> {
    let name = &item.name;
    let der = STANDARD
        .decode(&item.base64)
        .or_else(|_| STANDARD_NO_PAD.decode(&item.base64))
        .map_err(|_| BuildError::new(format!("keystore item `{name}`: `base64` is not valid Base64")))?;
    let store = p12_keystore::KeyStore::from_pkcs12(&der, item.password.as_deref().unwrap_or(""))
        .map_err(|e| {
            BuildError::new(format!(
                "keystore item `{name}` cannot be decoded (wrong password or unsupported PKCS#12): {e}"
            ))
        })?;
    let (_, chain) = store.private_key_chain().ok_or_else(|| {
        BuildError::new(format!("keystore item `{name}` holds no private key with a certificate"))
    })?;
    if chain.chain().is_empty() {
        return Err(BuildError::new(format!("keystore item `{name}` holds no certificate")));
    }
    Ok(ClientIdentity {
        chain: chain
            .chain()
            .iter()
            .map(|c| CertificateDer::from(c.as_der().to_vec()))
            .collect(),
        key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(chain.key().to_vec())),
    })
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test -p rurge-proto keystore::`
Expected: PASS（3 个）。若 `files_written_by_openssl_decode_with_either_encryption` 只在 `OPENSSL_LEGACY` 上失败，说明 `p12-keystore` 0.2 读不了 RC2-40 的证书包：保留现代样本的断言，把旧式样本那一轮改成只断言 `decode_p12` 返回 `Err` 且消息以 ``keystore item `cert1` cannot be decoded`` 开头，并在报告与计划末尾的「执行期修正记录」里写明，M1b 的文档任务据此登记到兼容性清单。

- [ ] **Step 5: 门禁与提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto
git commit -m "feat(proto): decode_p12：[Keystore] 的 p12 解码为客户端证书（p12-keystore 0.2）"
```

---

### Task 11: `http` / `https` 出站

**Files:**
- Modify: `Cargo.toml`（工作区：加 `getrandom = "0.3"`）
- Modify: `crates/rurge-proto/Cargo.toml`
- Modify: `crates/rurge-proto/src/build.rs`（加 `tls_client`）
- Create: `crates/rurge-proto/src/http.rs`
- Modify: `crates/rurge-proto/src/lib.rs`

**Interfaces:**
- Consumes: `rurge_config::spec::{HeaderPart, HeaderTemplate, HttpSpec, PolicySpec, ProtoSpec, TlsOpts}`；`rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target}`；Task 7 的 `HttpForward` `OutboundError` `BuildError`；Task 8 的 `read_head` `prefixed::boxed` 与假上游；Task 9 的 `TlsClient`；Task 10 的 `decode_p12`。
- Produces:
  - `rurge_proto::build::tls_client(opts: Option<&TlsOpts>, server: &HostName, default_alpn: &[&str], keystore: &[KeystoreItem], roots: Arc<RootCertStore>) -> Result<Option<TlsClient>, BuildError>`（在 `keystore` 里解 `client-cert`）
  - `rurge_proto::http::HttpOutbound`：`HttpOutbound::from_spec(spec: &PolicySpec, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<HttpOutbound, BuildError>`；实现 `Outbound`（`name()` 是策略名）与 `HttpForward`
  - 常量 `rurge_proto::http::MAX_HEAD: usize = 16 * 1024`
  - M1b 的工厂对 `ProtoSpec::Http` 只需调 `HttpOutbound::from_spec`。

- [ ] **Step 1: 依赖与模块**

工作区 `Cargo.toml` 的 `[workspace.dependencies]` 末尾加 `getrandom = "0.3"`。`crates/rurge-proto/Cargo.toml` 的 `[dependencies]` 加 `getrandom.workspace = true` 与 `tracing.workspace = true`。`crates/rurge-proto/src/lib.rs` 加 `pub mod http;`。

- [ ] **Step 2: 写失败的测试**

创建 `crates/rurge-proto/src/http.rs`，先只放测试：

```rust
//! `http` / `https` proxy outbound: CONNECT tunnels, and absolute-form
//! forwarding of plain HTTP requests (manual: Policies › HTTP and HTTP/2).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeHttpProxy, HttpProxyScript, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::{NameKind, SpecEnv, to_spec};
    use rurge_config::{KeystoreItem, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn spec(definition: &str) -> PolicySpec {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("Up", definition, &span).unwrap();
        let lookup = |_: &str| -> Option<NameKind> { None };
        let outcome = to_spec(&policy, &SpecEnv { keystore: &[], lookup: &lookup });
        outcome.spec.unwrap_or_else(|| panic!("{:?}", outcome.diagnostics))
    }

    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> HttpOutbound {
        let keystore: Vec<KeystoreItem> = Vec::new();
        HttpOutbound::from_spec(
            &spec(definition),
            &keystore,
            roots,
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn roundtrip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut buf = vec![0u8; payload.len()];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, payload);
    }

    #[tokio::test]
    async fn connect_tunnels_with_credentials() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            auth: Some(("user".into(), "pa:ss".into())),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}, user, pa:ss", proxy.addr().port()),
            no_roots(),
        );
        assert_eq!(out.name(), "Up");
        let mut stream = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"through the tunnel").await;
        let head = &proxy.heads()[0];
        assert_eq!(head.request_line, format!("CONNECT {echo} HTTP/1.1"));
        assert_eq!(head.header("Host"), Some(echo.to_string().as_str()));
        // base64("user:pa:ss")
        assert_eq!(head.header("Proxy-Authorization"), Some("Basic dXNlcjpwYTpzcw=="));
    }

    #[tokio::test]
    async fn the_target_name_is_sent_as_it_is_and_ipv6_is_bracketed() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            connect_to: Some(echo),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(&format!("http, 127.0.0.1, {}", proxy.addr().port()), no_roots());
        for (host, line) in [
            ("remote.example", "CONNECT remote.example:443 HTTP/1.1"),
            ("2001:db8::1", "CONNECT [2001:db8::1]:443 HTTP/1.1"),
        ] {
            let mut stream = out
                .connect_tcp(&Target::new(HostName::parse(host), 443), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"x").await;
            assert_eq!(proxy.heads().last().unwrap().request_line, line);
        }
        assert!(proxy.heads()[0].header("Proxy-Authorization").is_none());
    }

    #[tokio::test]
    async fn bytes_behind_the_response_head_belong_to_the_tunnel() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            trailing: b"early".to_vec(),
            padding: 3000,
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(&format!("http, 127.0.0.1, {}", proxy.addr().port()), no_roots());
        let mut stream = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        let mut early = [0u8; 5];
        stream.read_exact(&mut early).await.unwrap();
        assert_eq!(&early, b"early");
        roundtrip(&mut stream, b"after").await;
    }

    #[tokio::test]
    async fn refusals_become_proxy_errors() {
        let echo = echo_server().await;
        let cases: Vec<(HttpProxyScript, &str)> = vec![
            (
                HttpProxyScript { auth: Some(("u".into(), "p".into())), ..HttpProxyScript::default() },
                "http proxy answered 407 Proxy Authentication Required",
            ),
            (
                HttpProxyScript { refuse: Some((503, "Service Unavailable")), ..HttpProxyScript::default() },
                "http proxy answered 503 Service Unavailable",
            ),
            (
                HttpProxyScript { padding: 20_000, ..HttpProxyScript::default() },
                "http proxy sent a response header larger than 16384 bytes",
            ),
            (
                HttpProxyScript { truncate: true, ..HttpProxyScript::default() },
                "http proxy closed the connection during CONNECT",
            ),
        ];
        for (script, expected) in cases {
            let proxy = FakeHttpProxy::spawn(script).await;
            let out = outbound(&format!("http, 127.0.0.1, {}", proxy.addr().port()), no_roots());
            let err = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.map(|_| ()).unwrap_err();
            assert!(matches!(&err, OutboundError::Proxy(m) if m == expected), "{err}");
        }
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript {
            delay: Duration::from_secs(30),
            ..HttpProxyScript::default()
        })
        .await;
        let out = outbound(&format!("http, 127.0.0.1, {}", proxy.addr().port()), no_roots());
        let err = out
            .connect_tcp(&target(echo), &ConnectOpts { timeout: Duration::from_millis(200) })
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
    }

    #[tokio::test]
    async fn custom_headers_replace_and_random_strings_are_rendered_per_connection() {
        let echo = echo_server().await;
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let out = outbound(
            &format!(
                "http, 127.0.0.1, {}, headers=Host:edge.example;X-Pad:p<random-string(12)>q<random-string(2-6)>",
                proxy.addr().port()
            ),
            no_roots(),
        );
        for _ in 0..2 {
            out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        }
        let heads = proxy.heads();
        let hosts: Vec<&str> = heads[0].headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case("host")).map(|(_, v)| v.as_str()).collect();
        assert_eq!(hosts, ["edge.example"], "the configured Host replaces ours");
        let pads: Vec<&str> = heads.iter().map(|h| h.header("X-Pad").unwrap()).collect();
        for pad in &pads {
            let inner = pad.strip_prefix('p').unwrap();
            let (first, second) = inner.split_at(12);
            let second = second.strip_prefix('q').unwrap();
            assert!((2..=6).contains(&second.len()), "{pad}");
            assert!(
                first.chars().chain(second.chars()).all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{pad}"
            );
        }
        assert_ne!(pads[0], pads[1], "rendered anew for every connection");
    }

    #[tokio::test]
    async fn https_wraps_the_proxy_connection_in_tls() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["localhost"]);
        let proxy = FakeHttpProxy::spawn_tls(HttpProxyScript::default(), fixture.clone(), false).await;
        let out = outbound(&format!("https, localhost, {}", proxy.addr().port()), fixture.roots());
        let mut stream = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"inside tls").await;
        assert_eq!(fixture.seen()[0].sni.as_deref(), Some("localhost"));
        // an untrusted proxy certificate is a TLS error, not a proxy error
        let distrusting = outbound(&format!("https, localhost, {}", proxy.addr().port()), TlsFixture::new(&["x.test"]).roots());
        let err = distrusting.connect_tcp(&target(echo), &ConnectOpts::default()).await.map(|_| ()).unwrap_err();
        assert!(matches!(err, OutboundError::Tls(_)), "{err}");
    }

    #[tokio::test]
    async fn forward_mode_hands_out_the_connection_and_the_headers() {
        let proxy = FakeHttpProxy::spawn(HttpProxyScript::default()).await;
        let out = outbound(
            &format!("http, 127.0.0.1, {}, user, pass, headers=X-Via:rurge", proxy.addr().port()),
            no_roots(),
        );
        let forward = out.http_forward().expect("always-use-connect defaults to false");
        assert_eq!(
            forward.request_headers(),
            [
                ("Proxy-Authorization".to_string(), "Basic dXNlcjpwYXNz".to_string()),
                ("X-Via".to_string(), "rurge".to_string()),
            ]
        );
        let mut stream = forward.connect(&ConnectOpts::default()).await.unwrap();
        stream
            .write_all(b"GET http://origin.example/path HTTP/1.1\r\nHost: origin.example\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200 OK") && answer.ends_with("forwarded"), "{answer}");
        assert_eq!(proxy.heads()[0].request_line, "GET http://origin.example/path HTTP/1.1");

        let tunnel_only = outbound(
            &format!("http, 127.0.0.1, {}, always-use-connect=true", proxy.addr().port()),
            no_roots(),
        );
        assert!(tunnel_only.http_forward().is_none());
    }

    #[test]
    fn only_http_specs_build() {
        let direct = spec("direct");
        let keystore: Vec<KeystoreItem> = Vec::new();
        let err = HttpOutbound::from_spec(
            &direct,
            &keystore,
            no_roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "policy `Up` is not an http / https policy");
    }
}
```

Run: `cargo test -p rurge-proto http::`
Expected: 编译失败——`HttpOutbound` 未定义。

- [ ] **Step 3: 实现 `tls_client`**

在 `crates/rurge-proto/src/build.rs` 的 `impl std::error::Error for BuildError {}` 之后、测试模块之前加：

```rust
use crate::keystore::decode_p12;
use crate::transport::tls::TlsClient;
use rurge_config::spec::TlsOpts;
use rurge_config::{HostName, KeystoreItem};
use rustls::RootCertStore;
use std::sync::Arc;

/// The TLS layer of a policy, when it has one. `client-cert` is looked up in
/// `keystore` and decoded here, so a broken p12 surfaces at build time.
pub fn tls_client(
    opts: Option<&TlsOpts>,
    server: &HostName,
    default_alpn: &[&str],
    keystore: &[KeystoreItem],
    roots: Arc<RootCertStore>,
) -> Result<Option<TlsClient>, BuildError> {
    let Some(opts) = opts else {
        return Ok(None);
    };
    let identity = match &opts.client_cert {
        None => None,
        Some(name) => {
            let item = keystore
                .iter()
                .find(|k| &k.name == name)
                .ok_or_else(|| BuildError::new(format!("keystore item `{name}` does not exist")))?;
            Some(decode_p12(item)?)
        }
    };
    TlsClient::build(opts, server, default_alpn, identity, roots).map(Some)
}
```

（`use` 行挪到文件顶部，与 `use std::fmt;` 放在一起。）

- [ ] **Step 4: 实现 `HttpOutbound`**

在 `crates/rurge-proto/src/http.rs` 的模块注释之后、测试模块之前加：

```rust
use crate::build::tls_client;
use crate::transport::head::read_head;
use crate::transport::prefixed;
use crate::transport::tls::TlsClient;
use crate::{BuildError, HttpForward, Outbound, OutboundError};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rurge_config::spec::{HeaderPart, HeaderTemplate, PolicySpec, ProtoSpec};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

/// Largest CONNECT response head we accept.
pub const MAX_HEAD: usize = 16 * 1024;

/// 64 URL-safe symbols: a random byte maps onto them without bias.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub struct HttpOutbound {
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    tls: Option<TlsClient>,
    /// The `Proxy-Authorization` value, ready to send.
    authorization: Option<String>,
    headers: Vec<HeaderTemplate>,
    forward: bool,
}

fn random_string(min: usize, max: usize) -> String {
    let mut pick = [0u8; 4];
    let mut bytes = vec![0u8; max];
    if getrandom::fill(&mut pick).is_err() || getrandom::fill(&mut bytes).is_err() {
        // padding, not a secret: degrade instead of failing the connection
        tracing::warn!("no random source; <random-string> placeholders are not random");
    }
    let len = min + (u32::from_le_bytes(pick) as usize) % (max - min + 1);
    bytes[..len]
        .iter()
        .map(|b| char::from(ALPHABET[usize::from(b & 63)]))
        .collect()
}

fn render(templates: &[HeaderTemplate]) -> Vec<(String, String)> {
    templates
        .iter()
        .map(|t| {
            let value = t
                .value
                .iter()
                .map(|part| match part {
                    HeaderPart::Literal(text) => text.clone(),
                    HeaderPart::Random { min, max } => random_string(*min, *max),
                })
                .collect();
            (t.name.clone(), value)
        })
        .collect()
}

/// A configured header replaces one of ours with the same name, `Host` included (manual).
fn merge(base: &mut Vec<(String, String)>, custom: Vec<(String, String)>) {
    for (name, value) in custom {
        base.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&name));
        base.push((name, value));
    }
}

fn authority(target: &Target) -> String {
    match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => format!("[{v6}]:{}", target.port),
        host => format!("{host}:{}", target.port),
    }
}

fn check_status(head: &[u8]) -> Result<(), OutboundError> {
    let text = String::from_utf8_lossy(head);
    let line = text.lines().next().unwrap_or_default();
    let mut parts = line.splitn(3, ' ');
    let (version, code) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let reason = parts.next().unwrap_or("").trim();
    match code.parse::<u16>() {
        Ok(code) if version.starts_with("HTTP/1.") && (200..300).contains(&code) => Ok(()),
        Ok(code) if version.starts_with("HTTP/1.") => Err(OutboundError::Proxy(
            format!("http proxy answered {code} {reason}").trim_end().to_string(),
        )),
        _ => Err(OutboundError::Proxy(
            "http proxy sent a malformed response".to_string(),
        )),
    }
}

impl HttpOutbound {
    pub fn from_spec(
        spec: &PolicySpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<HttpOutbound, BuildError> {
        let (ProtoSpec::Http(http), Some(host), Some(port)) = (&spec.proto, &spec.server, spec.port)
        else {
            return Err(BuildError::new(format!(
                "policy `{}` is not an http / https policy",
                spec.name
            )));
        };
        let tls = tls_client(http.tls.as_ref(), host, &[], keystore, roots)?;
        let authorization = http.username.as_ref().map(|user| {
            let password = http.password.as_deref().unwrap_or("");
            format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
        });
        Ok(HttpOutbound {
            name: spec.name.clone(),
            server: Target::new(host.clone(), port),
            connector,
            tls,
            authorization,
            headers: http.headers.clone(),
            forward: !http.always_use_connect,
        })
    }

    /// TCP to the proxy, then TLS for `https`.
    async fn dial(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let stream = self.connector.connect(&self.server, opts).await?;
        match &self.tls {
            Some(tls) => tls
                .wrap(stream)
                .await
                .map_err(|e| OutboundError::Tls(e.to_string())),
            None => Ok(stream),
        }
    }

    fn connect_request(&self, target: &Target) -> Vec<u8> {
        let authority = authority(target);
        let mut headers = vec![("Host".to_string(), authority.clone())];
        if let Some(value) = &self.authorization {
            headers.push(("Proxy-Authorization".to_string(), value.clone()));
        }
        merge(&mut headers, render(&self.headers));
        let mut request = format!("CONNECT {authority} HTTP/1.1\r\n");
        for (name, value) in headers {
            request.push_str(&name);
            request.push_str(": ");
            request.push_str(&value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        request.into_bytes()
    }

    async fn tunnel(&self, target: &Target, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let mut stream = self.dial(opts).await?;
        stream.write_all(&self.connect_request(target)).await?;
        let (head, rest) = read_head(&mut stream, MAX_HEAD).await.map_err(|e| match e.kind() {
            io::ErrorKind::InvalidData => OutboundError::Proxy(format!(
                "http proxy sent a response header larger than {MAX_HEAD} bytes"
            )),
            io::ErrorKind::UnexpectedEof => {
                OutboundError::Proxy("http proxy closed the connection during CONNECT".to_string())
            }
            _ => OutboundError::from(e),
        })?;
        check_status(&head)?;
        Ok(prefixed::boxed(rest, stream))
    }
}

impl Outbound for HttpOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection, TLS and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn http_forward(&self) -> Option<&dyn HttpForward> {
        self.forward.then_some(self as &dyn HttpForward)
    }
}

impl HttpForward for HttpOutbound {
    fn connect<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.dial(opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn request_headers(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        if let Some(value) = &self.authorization {
            headers.push(("Proxy-Authorization".to_string(), value.clone()));
        }
        merge(&mut headers, render(&self.headers));
        headers
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test -p rurge-proto http::`
Expected: PASS（9 个）。

- [ ] **Step 6: 门禁与提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto
git commit -m "feat(proto): http / https 出站：CONNECT 隧道、headers 与随机串占位、明文 HTTP 的转发模式"
```

---

### Task 12: `socks5` / `socks5-tls` 出站

**Files:**
- Create: `crates/rurge-proto/src/socks5.rs`
- Modify: `crates/rurge-proto/src/lib.rs`

**Interfaces:**
- Consumes: `rurge_config::spec::{PolicySpec, ProtoSpec, Socks5Spec}`；Task 11 的 `build::tls_client`；Task 8 的 `FakeSocks5` `Socks5Script` `TlsFixture` `echo_server`。
- Produces: `rurge_proto::socks5::Socks5Outbound`：`Socks5Outbound::from_spec(spec: &PolicySpec, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<Socks5Outbound, BuildError>`；实现 `Outbound`。域名目标用 ATYP = 3（远程解析）。

- [ ] **Step 1: 写失败的测试**

`crates/rurge-proto/src/lib.rs` 加 `pub mod socks5;`。创建 `crates/rurge-proto/src/socks5.rs`，先只放测试：

```rust
//! `socks5` / `socks5-tls` proxy outbound (RFC 1928, RFC 1929), CONNECT only.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeSocks5, Socks5Script, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::{NameKind, SpecEnv, to_spec};
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn outbound(definition: &str, roots: Arc<RootCertStore>) -> Socks5Outbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("Up", definition, &span).unwrap();
        let lookup = |_: &str| -> Option<NameKind> { None };
        let outcome = to_spec(&policy, &SpecEnv { keystore: &[], lookup: &lookup });
        let spec = outcome.spec.unwrap_or_else(|| panic!("{:?}", outcome.diagnostics));
        let keystore: Vec<KeystoreItem> = Vec::new();
        Socks5Outbound::from_spec(
            &spec,
            &keystore,
            roots,
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn no_roots() -> Arc<RootCertStore> {
        Arc::new(RootCertStore::empty())
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn roundtrip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut buf = vec![0u8; payload.len()];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(buf, payload);
    }

    #[tokio::test]
    async fn connects_without_and_with_credentials() {
        let echo = echo_server().await;
        let open = FakeSocks5::spawn(Socks5Script::default()).await;
        let out = outbound(&format!("socks5, 127.0.0.1, {}", open.addr().port()), no_roots());
        assert_eq!(out.name(), "Up");
        let mut stream = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"no auth").await;
        assert_eq!(open.requests()[0].methods, [0], "only `no authentication` is offered");

        let guarded = FakeSocks5::spawn(Socks5Script {
            auth: Some(("user".into(), "pass".into())),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(&format!("socks5, 127.0.0.1, {}, user, pass", guarded.addr().port()), no_roots());
        let mut stream = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"with auth").await;
        let seen = &guarded.requests()[0];
        assert_eq!(seen.methods, [0, 2]);
        assert_eq!(seen.credentials, Some(("user".into(), "pass".into())));
        assert_eq!((seen.atyp, seen.port), (1, echo.port()));
    }

    #[tokio::test]
    async fn names_are_resolved_by_the_proxy_and_ipv6_is_its_own_type() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            connect_to: Some(echo),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(&format!("socks5, 127.0.0.1, {}", server.addr().port()), no_roots());
        for host in ["remote.example", "2001:db8::1"] {
            let mut stream = out
                .connect_tcp(&Target::new(HostName::parse(host), 443), &ConnectOpts::default())
                .await
                .unwrap();
            roundtrip(&mut stream, b"x").await;
        }
        let seen = server.requests();
        assert_eq!((seen[0].atyp, seen[0].host.as_str(), seen[0].port), (3, "remote.example", 443));
        assert_eq!((seen[1].atyp, seen[1].host.as_str()), (4, "2001:db8::1"));
    }

    #[tokio::test]
    async fn refusals_become_proxy_errors() {
        let echo = echo_server().await;
        let cases: Vec<(Socks5Script, &str, &str)> = vec![
            (
                Socks5Script { auth: Some(("u".into(), "p".into())), ..Socks5Script::default() },
                "socks5, 127.0.0.1, {port}, u, wrong",
                "socks5: authentication failed",
            ),
            (
                Socks5Script { auth: Some(("u".into(), "p".into())), ..Socks5Script::default() },
                "socks5, 127.0.0.1, {port}",
                "socks5: the proxy accepts none of the offered authentication methods",
            ),
            (
                Socks5Script { force_method: Some(2), ..Socks5Script::default() },
                "socks5, 127.0.0.1, {port}",
                "socks5: the proxy selected authentication method 2, which was not offered",
            ),
            (
                Socks5Script { reply: 5, ..Socks5Script::default() },
                "socks5, 127.0.0.1, {port}",
                "socks5: connection refused",
            ),
            (
                Socks5Script { reply: 9, ..Socks5Script::default() },
                "socks5, 127.0.0.1, {port}",
                "socks5: reply code 9",
            ),
            (
                Socks5Script { hang_up_after_greeting: true, ..Socks5Script::default() },
                "socks5, 127.0.0.1, {port}",
                "socks5: the proxy closed the connection during the handshake",
            ),
        ];
        for (script, definition, expected) in cases {
            let server = FakeSocks5::spawn(script).await;
            let out = outbound(&definition.replace("{port}", &server.addr().port().to_string()), no_roots());
            let err = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.map(|_| ()).unwrap_err();
            assert!(matches!(&err, OutboundError::Proxy(m) if m == expected), "{definition}: {err}");
        }
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out_and_long_names_are_refused_locally() {
        let echo = echo_server().await;
        let server = FakeSocks5::spawn(Socks5Script {
            delay: Duration::from_secs(30),
            ..Socks5Script::default()
        })
        .await;
        let out = outbound(&format!("socks5, 127.0.0.1, {}", server.addr().port()), no_roots());
        let err = out
            .connect_tcp(&target(echo), &ConnectOpts { timeout: Duration::from_millis(200) })
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        let long = Target::new(HostName::Domain("a".repeat(256)), 80);
        let err = out.connect_tcp(&long, &ConnectOpts::default()).await.map(|_| ()).unwrap_err();
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "socks5: the host name is longer than 255 bytes"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn socks5_tls_wraps_the_session_in_tls() {
        let echo = echo_server().await;
        let fixture = TlsFixture::new(&["localhost"]);
        let server = FakeSocks5::spawn_tls(Socks5Script::default(), fixture.clone(), false).await;
        let out = outbound(&format!("socks5-tls, localhost, {}", server.addr().port()), fixture.roots());
        let mut stream = out.connect_tcp(&target(echo), &ConnectOpts::default()).await.unwrap();
        roundtrip(&mut stream, b"inside tls").await;
        assert_eq!(fixture.seen()[0].sni.as_deref(), Some("localhost"));
    }
}
```

Run: `cargo test -p rurge-proto socks5::`
Expected: 编译失败——`Socks5Outbound` 未定义。

- [ ] **Step 2: 实现**

在 `crates/rurge-proto/src/socks5.rs` 的模块注释之后、测试模块之前加：

```rust
use crate::build::tls_client;
use crate::transport::tls::TlsClient;
use crate::{BuildError, Outbound, OutboundError};
use rurge_config::spec::{PolicySpec, ProtoSpec};
use rurge_config::{HostName, KeystoreItem};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const VERSION: u8 = 5;
const NO_AUTH: u8 = 0;
const USER_PASS: u8 = 2;
const NO_ACCEPTABLE: u8 = 0xff;

pub struct Socks5Outbound {
    name: String,
    server: Target,
    connector: Arc<dyn Connector>,
    tls: Option<TlsClient>,
    credentials: Option<(String, String)>,
}

fn proxy(message: impl Into<String>) -> OutboundError {
    OutboundError::Proxy(format!("socks5: {}", message.into()))
}

/// A connection the proxy closes mid-handshake is its refusal, not an I/O
/// fault of ours. Depending on timing and platform the close shows up as an
/// EOF, a reset or a broken pipe.
fn handshake_io(e: io::Error) -> OutboundError {
    match e.kind() {
        io::ErrorKind::UnexpectedEof
        | io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => proxy("the proxy closed the connection during the handshake"),
        _ => OutboundError::from(e),
    }
}

fn reply_text(code: u8) -> String {
    match code {
        1 => "general failure".to_string(),
        2 => "connection not allowed by the ruleset".to_string(),
        3 => "network unreachable".to_string(),
        4 => "host unreachable".to_string(),
        5 => "connection refused".to_string(),
        6 => "TTL expired".to_string(),
        7 => "command not supported".to_string(),
        8 => "address type not supported".to_string(),
        other => format!("reply code {other}"),
    }
}

fn connect_request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, 1, 0];
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            request.push(1);
            request.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            request.push(4);
            request.extend_from_slice(&v6.octets());
        }
        // the proxy resolves the name (remote resolution)
        HostName::Domain(name) => {
            let len = u8::try_from(name.len())
                .map_err(|_| proxy("the host name is longer than 255 bytes"))?;
            request.push(3);
            request.push(len);
            request.extend_from_slice(name.as_bytes());
        }
    }
    request.extend_from_slice(&target.port.to_be_bytes());
    Ok(request)
}

impl Socks5Outbound {
    pub fn from_spec(
        spec: &PolicySpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<Socks5Outbound, BuildError> {
        let (ProtoSpec::Socks5(socks), Some(host), Some(port)) =
            (&spec.proto, &spec.server, spec.port)
        else {
            return Err(BuildError::new(format!(
                "policy `{}` is not a socks5 / socks5-tls policy",
                spec.name
            )));
        };
        let tls = tls_client(socks.tls.as_ref(), host, &[], keystore, roots)?;
        let credentials = socks
            .username
            .clone()
            .map(|user| (user, socks.password.clone().unwrap_or_default()));
        Ok(Socks5Outbound {
            name: spec.name.clone(),
            server: Target::new(host.clone(), port),
            connector,
            tls,
            credentials,
        })
    }

    async fn handshake(&self, target: &Target, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        // checked first: no connection is opened for a request we cannot send
        let request = connect_request(target)?;
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(tls) = &self.tls {
            stream = tls
                .wrap(stream)
                .await
                .map_err(|e| OutboundError::Tls(e.to_string()))?;
        }
        let offered: &[u8] = if self.credentials.is_some() {
            &[NO_AUTH, USER_PASS]
        } else {
            &[NO_AUTH]
        };
        let mut greeting = vec![VERSION, offered.len() as u8];
        greeting.extend_from_slice(offered);
        stream.write_all(&greeting).await.map_err(handshake_io)?;
        let mut selected = [0u8; 2];
        stream.read_exact(&mut selected).await.map_err(handshake_io)?;
        match selected[1] {
            NO_ACCEPTABLE => {
                return Err(proxy("the proxy accepts none of the offered authentication methods"));
            }
            method if !offered.contains(&method) => {
                return Err(proxy(format!(
                    "the proxy selected authentication method {method}, which was not offered"
                )));
            }
            USER_PASS => {
                let (user, password) = self.credentials.as_ref().expect("offered only with credentials");
                // lengths were checked when the profile was loaded (<= 255)
                let mut auth = vec![1, user.len() as u8];
                auth.extend_from_slice(user.as_bytes());
                auth.push(password.len() as u8);
                auth.extend_from_slice(password.as_bytes());
                stream.write_all(&auth).await.map_err(handshake_io)?;
                let mut status = [0u8; 2];
                stream.read_exact(&mut status).await.map_err(handshake_io)?;
                if status[1] != 0 {
                    return Err(proxy("authentication failed"));
                }
            }
            _ => {}
        }
        stream.write_all(&request).await.map_err(handshake_io)?;
        let mut reply = [0u8; 4];
        stream.read_exact(&mut reply).await.map_err(handshake_io)?;
        if reply[1] != 0 {
            return Err(proxy(reply_text(reply[1])));
        }
        // skip the bound address
        let remaining = match reply[3] {
            1 => 4 + 2,
            4 => 16 + 2,
            3 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len).await.map_err(handshake_io)?;
                usize::from(len[0]) + 2
            }
            other => return Err(proxy(format!("unknown address type {other} in the reply"))),
        };
        let mut bound = vec![0u8; remaining];
        stream.read_exact(&mut bound).await.map_err(handshake_io)?;
        Ok(stream)
    }
}

impl Outbound for Socks5Outbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            match tokio::time::timeout(opts.timeout, self.handshake(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}
```

- [ ] **Step 3: 跑测试确认通过**

Run: `cargo test -p rurge-proto socks5::`
Expected: PASS（5 个）。

- [ ] **Step 4: 门禁与提交**

```bash
git add crates/rurge-proto
git commit -m "feat(proto): socks5 / socks5-tls 出站：方法协商、用户名密码、域名远程解析、应答码文本"
```

---

### Task 13: 文档与计划收尾

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/plans/2026-09-19-phase2-m1a-outbound-library-plan.md`（本文件末尾两张表）

**Interfaces:**
- Consumes: 前 12 个任务的执行记录（SDD 账本或各任务报告）。
- Produces: 无代码接口。

- [ ] **Step 1: `CLAUDE.md`**

「当前状态」一段末尾那句"阶段 2（出站协议与策略组）的总设计文档与 M1 设计文档已写好，尚未开始实现。"换成：

```
阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；这些还没有接进引擎（四种协议在运行期仍是 `W0007` + REJECT），装配属于 M1b。
```

「先读这些文档」里 M1 设计文档那一条之后加一条：

```
- `docs/superpowers/plans/2026-09-19-phase2-m1a-outbound-library-plan.md`：M1a 实施计划（13 个任务）。开头「计划期决定」表（P1–P10）记录写计划时核对源码得出的结论（`socket2` 无 TFO 封装、`p12-keystore` 用 0.2 等）；末尾「执行期修正记录」与「延后事项」同 M2a。
```

「常用命令」里 `cargo test -p rurge-api` 那一行之后加：

```bash
cargo test -p rurge-proto                       # 出站协议库：TLS 层、p12、http / socks5 出站对回环假上游（rurge_proto::testing）
```

- [ ] **Step 2: 填写本计划末尾的两张表**

按执行记录如实填写「执行期修正记录」（每一处与计划文字不同的实现：是什么、为什么、在哪个提交）与「延后事项」（评审里标为 minor 而未修的、以及计划里预先列出的几条的最终状态）。没有偏差就写"无"，不要留空表。

- [ ] **Step 3: 门禁与提交**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-09-19-phase2-m1a-outbound-library-plan.md
git commit -m "docs: M1a 配置与出站库：CLAUDE.md 状态与计划收尾"
```

---

## 执行期修正记录

| 任务 | 偏差 | 原因 | 提交 |
| ---- | ---- | ---- | ---- |
| 1 | `pub mod spec;` 放在 `pub mod span;` 之后（按字母序），不是计划写的位置 | 保持模块列表有序；纯排版 | 5376fe1 |
| 2 | 计划代码经 rustfmt 重排；`pub use spec::…` 的顺序由 rustfmt 决定 | 纯排版 | 56fbc08 |
| 3 | 语料库快照的真实文件名是 `corpus__corpus__kitchen-sink.snap`（计划写的是 `corpus__kitchen-sink.snap`）；快照变化恰好是预期的两条 `W0029`（`tfo`、`udp-relay`） | 计划记错了 insta 的文件名 | 3f2f859 |
| 5 | 测试沿用模块里已有的 `Fixed` 解析器，没有照计划再声明一个 | 计划的测试块会造成重复定义 | 3dd7f91 |
| 8 | `pub mod` 按字母序；rustfmt 重排 | 纯排版 | dd270c2 |
| 9 | 证书名不匹配的断言检查 `not valid for name`，不是计划写的 `NotValidForName` | rustls 0.23.43 的 Display 文本是 "invalid peer certificate: certificate not valid for name …"，`NotValidForName` 只是 Debug 的变体名；该文本在 rustls 里只出现一处，仍能把名称不匹配与其它失败分开 | 3f6d7ea |
| 10 | 测试模块里去掉重复的 `use base64::Engine`；Step 4 预留的"旧式 p12 读不了"分支没有用上 | 前者是重复导入；后者因为 `p12-keystore` 0.2.1 两份 OpenSSL 样本（现代 PBES2 与 `-legacy` 的 RC2-40 + 3DES）都能解码 | a6c9ab1 |
| 11 | **`HttpOutbound` 在拨号之前校验目标主机名**：域名的每个字节必须落在 `0x21..=0x7e` 且不为空（IP 字面量总是合法），否则返回 `OutboundError::Proxy("the target host name is not valid for an HTTP proxy request")`，错误文本不回显主机名；新增测试证明这种目标不会让假上游收到任何请求 | 任务评审的 Critical（出在计划自己的代码里）：计划的 `authority()` / `connect_request()` 把目标主机名原样写进 CONNECT 请求行与 `Host` 头，而 SOCKS5 入站只检查域名是合法 UTF-8，本机客户端可以用带 CR/LF 的主机名让 rurge 向用户的上游代理多发一条带 `Proxy-Authorization` 的请求（请求走私）。裁定：出站不信任调用方，在本任务内修 | 3b0885f |
| 11 | 顺带补了三处测试：`build::tls_client` 的三个分支（无 TLS、Keystore 条目不存在、p12 解不开）与 `check_status` 的"响应格式错误"路径 | 评审的 Minor；修复轮里顺手补上（M1b 的干构建依赖 `tls_client` 的报错） | 3b0885f |
| 12 | `Socks5Outbound::from_spec` 自己再校验一次用户名与密码各不超过 255 字节，超长返回 `BuildError`（文本只含策略名，不含凭据）；握手里的 `as u8` 保留，注释改为"长度已在 `from_spec` 校验"；新增测试 `credentials_longer_than_255_bytes_are_refused_at_build_time` | 派发前的控制者裁定（Task 11 的教训：出站不信任调用方）。计划代码只靠配置加载时的 255 字节上限，而 `PolicySpec` 的字段是 pub | b177aeb |
| 12 | 测试里把 `spec()` 从 `outbound()` 拆出来 | 与 `http.rs` 的写法一致，并让上一行的测试能拿到 `PolicySpec` 来改 | b177aeb |

## 延后事项

标为"整分支终审时分诊"的条目，其最终去向由终审后的修复提交更新到本表。

| 事项 | 去向 |
| ---- | ---- |
| `tfo` 三平台不生效（P1） | 不变：M1b 文档任务登记到兼容性清单 4.3；`socket2` 提供安全封装后再评估 |
| `dns-follow-interface` | 不变：M5（M1 设计 11 节 C2） |
| Linux / macOS 的 `bind_interface`、`set_tclass_v6` 分支本机无法编译 | 不变：首次推送后由 CI 证明；Task 6 的评审已逐个对照 `socket2` 0.6.5 / `if-addrs` 0.15.0 的源码核对了名字、签名与 cfg 条件。可选的本地检查：`rustup target add x86_64-unknown-linux-gnu` 后 `cargo check --target x86_64-unknown-linux-gnu -p rurge-platform`（会给本机装一个 target，由用户决定） |
| 旧式（RC2-40 + 3DES）p12 若 `p12-keystore` 0.2 读不了（Task 10 Step 4 的分支） | **已结**：0.2.1 两种都能解码（Task 10），无需登记差异 |
| 出站按指纹跨代复用、`RegistryCell`、工厂、引擎接线、能力表翻转、API、互操作夹具 | 不变：M1b |
| **根因未修**：SOCKS5 入站只检查域名是合法 UTF-8，`HostName::parse` 只修剪两端，内部的控制字符会一路传到出站（M1a 只在 `http` 出站侧拦截） | M1b 计划的具名条目（M1b 本来就要改 `rurge-inbound`）：入站侧拒绝含控制字符 / 空白的主机名 |
| `HttpOutbound::request_headers()` 每次调用都重新渲染 `<random-string>` 占位 | M1b：转发模式的调用方每条连接只调用一次（写进 M1b 对应任务的接口说明） |
| 生产环境的 DIRECT 仍用 `NoopSocketHook`，`interface` / `tos` 在运行期不生效 | M1b：注入 bin 侧的 `PlatformSockets` 适配器（M1 设计 6.7） |
| 未与真实 Surge 核对的两处语义：自定义 `sni` 同时成为证书校验名（除非给了 `server-cert-verify-name`）；`https` 上游握手不带 ALPN | M1b 文档任务在兼容性清单里标注"未核对" |
| `race` 的 3 秒分支被 `queue.is_empty()` 挡住：首选族有 13 个以上地址时，另一族要等队列排空才加入，而不是准时在 3 秒加入（修法：`queue.extend(more)` 并去掉那半个条件） | 整分支终审时分诊 |
| 没有测试钉住握手签名校验：把 `verify_tls12_signature` / `verify_tls13_signature` 的函数体换成断言，现有七个 TLS 测试仍全绿（便宜的补法：`Verifier { Insecure }` + 伪造的 `DigitallySignedStruct` 必须 `Err`） | 整分支终审时分诊 |
| `server-cert-verify-name` 只在标准校验分支解析；与指纹 / `skip-cert-verify` 同时出现时，格式错误的值不会被报告 | 整分支终审时分诊；不处理则留到 M2（TLS 族） |
| DNS 应答为空时报 "no usable address … (ip-version)"（文本误导，错误种类同为 NotFound）；超时文本里 IPv6 字面量没有方括号（`connect to ::1:80 timed out`） | M1b（那里会再动连接器的报错文本） |
| TLS / shadow-tls 参数写在 `reject*` 别名上（以及 shadow-tls 参数写在 `direct` 上）时落到通用的 `W0001`，而不是 `W0028` | 不处理：两者结果都是"忽略并告警" |
| `[Keystore]` 里空的 `base64` 值算合法 Base64（不报 `E0021`） | M1b：干构建会在解码时报 `E0022` |
| `decode_p12` 的"没有私钥" / "证书链为空"分支、无填充的 Base64 没有测试；"叶子在前"只用单证书链验证过 | 有真实样本时补（M2 的双向 TLS 互操作夹具） |
| `http` 出站的小项：16 KiB 头部上限是近似值（`read_head` 每读 512 字节检查一次）；"没有随机源"的 WARN 可能每个占位、每条连接各打一次；`HeaderTemplate` 的 min ≤ max 只靠约定（字段是 pub） | 不处理 / 出现实际问题时再收紧 |
| `rurge_proto::testing` 的两个假上游各有一份 accept 循环（约 20 行重复） | 第三个假上游（M2）出现时抽公共循环（预检裁定） |
| `echo_server` 与 `TlsFixture::spawn_echo` 只返回 `SocketAddr`，测试无法提前停掉它们（每个 `#[tokio::test]` 的运行时结束时会取消） | 不处理 |
| 两个已有的时序测试各偶发失败过一次、重跑通过，均与改动无关：`rurge-dns` 的 `fanout::tests::empty_answer_rules`（100 ms 时序断言）、bin 的 `run::watch_reloads_rules_on_change`（文件监视时序） | 再出现一次就单独修（像 M4a 对 observe 测试那样） |
| `socks5` 出站里 `expect("offered only with credentials")` 之所以不可达，只靠 match 的分支顺序（"未提供的方法"守卫分支排在 `USER_PASS` 分支之前），代码里没有说明——面向网络的解析器里不该留这种 panic 形状（修法：`match (selected[1], &self.credentials)`，或至少加注释） | 整分支终审时分诊 |
| `socks5` 应答里 ATYP = 3（长度来自线路）与未知 ATYP 两个分支没有任何测试执行到：`FakeSocks5` 恒定回 ATYP = 1，需要给 `testing/socks5.rs` 加一个 `reply_atyp` 旋钮 | 整分支终审时分诊 |
| `socks5` 的超长域名用例只断言了错误文本，没有证明"没有拨号"（结构上成立：`connect_request` 是拿不到 connector 的自由函数；用计数的 `Connector` 桩可以钉死） | 整分支终审时分诊 |
| `socks5` 的三个版本字节（方法选择、认证应答、CONNECT 应答）不校验：上游不是 SOCKS5 服务器时报的是语义错位的方法 / 应答码错误，而不是"不是 SOCKS5 服务器"；空域名会按 ATYP 3 / LEN 0 发出（长度前缀格式，无害）；只有密码没有用户名时密码被静默丢弃（与 `http` 出站同一约定） | 不处理 / 诊断质量，出现实际问题时再改 |
| `http` 出站的 CONNECT 写用的是裸 `?`，没有 `socks5` 出站那层 EOF / reset 归一；两个出站的测试辅助函数（`no_roots` `target` `roundtrip` `spec`，约 35 行）近乎相同 | 第三个出站（M2）到位时对齐，并把测试辅助函数挪进 `crate::testing` |
