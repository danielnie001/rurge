# 阶段 2 / M2a「Trojan 优先」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `[Proxy]` 里的 `trojan` 策略（可带 `ws`）经真实的 TLS / WebSocket 传输转发 TCP，并顺带落地 M1b 留给 M2 的承接事项。

**Architecture:** `rurge-config::spec` 增加 `WsOpts` / `TrojanSpec` 与它们的读取函数；`rurge-proto` 增加传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流适配器（`tokio-tungstenite`）、惰性请求头 `LazyHead` 与 `TrojanOutbound`；`ProtoSpec::Trojan`、`to_spec` 的接线与 `EngineFactory` 的分支在同一个任务里一起落地，使 trojan 策略在一个提交里从"未实现 → REJECT"翻成真实出站；随后翻转 bin 的能力表、补互操作与文档。

**Tech Stack:** Rust 1.89 / edition 2024、tokio、rustls 0.23（ring）、tokio-rustls、`tokio-tungstenite` 0.28（新）、`futures-util`（新的直接依赖，已在 lock 里）、`sha2`（SHA224）、`http` 1、`bytes` 1。

**Spec:** `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`（M2 设计，第 1.4 节的 M2a 行、第 4–6、8–13 节）；上位文档 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`。

## Global Constraints

- MSRV **1.89**，edition 2024；`unsafe_code = "forbid"`（全工作区，`rurge-platform` 除外且本计划不碰它）。
- 每个任务结束跑门禁（本机 msvc 缺 rustfmt，用 gnu 工具链的同版本二进制）：
  `RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace --no-fail-fast`
  测试二进制在没有任何失败用例的情况下异常退出（本机已知的偶发基建抖动）→ 立刻重跑一次并保留两次输出。
- 测试只用回环地址 + 端口 0 + 有界等待；不用固定 sleep 当同步手段；**不碰公网**。
- 测试与任何命令**不得修改本机的系统代理 / 注册表 / 网络设置，不得注册真实服务或计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`，不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**（sing-box 没有装，互操作用例本机跳过；不 `rustup target add`、不 `cargo install`）。`cargo` 为新依赖下载 crate 源码属于正常构建。
- 互操作夹具渲染的 sing-box 配置绝不出现 `set_system_proxy` / `tun` / `auto_route`，只监听 `127.0.0.1`，所有目标是回环 IP 字面量。
- 凭据（口令、其 SHA224、`ws-headers` 的值、`ws-path`）及其派生物、对端 / 客户端给的原始文本，永不出现在错误文本、诊断、日志、API 输出里；对端文本一律经 `rurge_proto::outbound::untrusted_text`。
- rurge 专有的运行时选项只经命令行参数与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）；与 Surge 的任何行为差异登记进 `docs/surge-compatibility-matrix.md`。
- 平台代码只在 `rurge-platform`；`rurge-engine` / `rurge-api` / `rurge-policy` 不依赖 `rurge-platform`；`rurge-policy` 不依赖任何协议实现。
- 文档与提交标题用中文；代码、注释、日志、CLI 文本用英文。提交信息以这两行结尾：
  `Co-Authored-By: <实现者的模型名> <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`
- 不 push、不 merge、不改写历史。
- 核对第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`。

---

## 计划期决定（写计划时核对源码得出；与设计文字有出入处以此为准，Task 8 同步订正设计）

| 编号 | 事项 | 结论 |
| ---- | ---- | ---- |
| P1 | 设计 V1：`tokio-tungstenite` 的版本 | **0.28.0**（cargo 的 MSRV 感知解析在 1.89 下选它；0.30 需要更新的 Rust）。`default-features = false, features = ["handshake"]`：不带它的 TLS 与 `connect` 特性。以工作区现有 `Cargo.lock` 为起点，它只新增 **3** 个条目：`tokio-tungstenite`、`tungstenite`、`utf-8`（`rand 0.9` `sha1` `data-encoding` `httparse` `http` `thiserror` `futures-util` 都已在 lock 里）。实施时新增条目多于这 3 个 → 停下来报告 |
| P2 | 设计 V1：自带 `http::Request` 的要求 | tungstenite 0.28 的 `ClientHandshake::start` 要求 URI 的 scheme 是 `ws` / `wss`（否则 `Url(UnsupportedUrlScheme)`），并要求请求里 `Host` `Connection` `Upgrade` `Sec-WebSocket-Version` `Sec-WebSocket-Key` 五个头**各恰好一个**；请求行只写 `path_and_query`。所以：URI 写成 `ws://<Host 值><ws-path>`；`ws-headers` 里的 `Host` 成为 `Host`；`Connection` / `Upgrade` / `Sec-WebSocket-*` 属于握手自己，出现在 `ws-headers` 里时 `W0012` 并忽略（否则拨号期才失败） |
| P3 | tungstenite 的错误文本 | `Error::Utf8` 等变体的 Display 会原样引用头的**值**。规则：**永不转发它的 Display 文本**，按变体映射成固定文本；`Error::Http` 只取状态码 |
| P4 | trojan 的 `password` 是否接受位置参数 | **不接受**（手册只写 `password=<password>`）。位置值按"多余的位置参数"报 `W0001`（不回显）。原因：`redact_profile` 只对 `http` `https` `socks5` `socks5-tls` 抹位置凭据，接受位置口令会在 `profiles/current` 里留一个脱敏漏洞。设计 4.2 的"`password` 接受位置参数"据此订正 |
| P5 | `Debug` | spec 类型沿用 M1 的约定派生 `Debug`（测试断言要用；日志与诊断从不打印 spec）；**持有凭据派生物的出站对象不实现 `Debug`**（`HttpOutbound` 就没有）。设计 4.1 的那句据此订正 |
| P6 | 任务的先后 | `ProtoSpec` 在 `EngineFactory::build` 里被穷举匹配，而注册表以"有没有 spec"决定走工厂还是 REJECT。所以 `ProtoSpec::Trojan`、`to_spec` 的接线、工厂分支必须**同一个提交**落地（Task 4）：之前的任务只加公开的读取函数与出站类型，trojan 策略的行为不变（`W0007` + REJECT）；Task 4 一步翻成真实出站。Task 5 才翻 bin 的能力表——两个相邻提交之间 `W0007` 仍会出现而策略已可用，与 M1b 的 Task 5 / Task 10 同样的过渡态，可接受 |
| P7 | WebSocket 的上限（设计 5.2 留给计划定） | 入站帧与消息 ≤ **1 MiB**（`WebSocketConfig::max_frame_size` / `max_message_size`）；出站每帧 ≤ **64 KiB**；读缓冲用库的默认值 |
| P8 | 端到端用例里的 TLS 信任 | 引擎测试夹具经 `Runtime::build` → `EngineFactory::new`（系统根证书），注入不了测试 CA。所以端到端用例在策略行上写 `server-cert-fingerprint-sha256=<假服务端叶证书的指纹>`；`rurge-proto` 自己的单元测试与互操作用例继续用 `TlsFixture::roots()` |
| P9 | SOCKS 风格的地址编码 | trojan 与 socks5 用同一种 `ATYP ADDR PORT`：抽成 `rurge_proto::addr::socks_addr`，`socks5.rs` 改用它（错误文本一字不变，现有用例钉着） |
| P10 | 设计 V2：sing-box 1.14.1 的写法 | trojan 入站：`{"type":"trojan","listen":"127.0.0.1","listen_port":N,"users":[{"name":"u","password":"p"}],"tls":{"enabled":true,"certificate_path":…,"key_path":…}}`；WebSocket：`"transport":{"type":"ws","path":"/x"}`，不需要构建标签（只有 gRPC / QUIC 需要） |
| P11 | `Stack` 的字段 | M2a 的 `Stack` 没有 `shadow_tls` 字段（M2c 加）——不为还不存在的层留空位 |
| P12 | WebSocket 的缺省 `Host` | 取策略的服务器主机（域名经 `hostname::to_ascii`；IPv6 加方括号）；端口不是该层的缺省端口（有 TLS 443，无 TLS 80）时带 `:port`。未与真实 Surge 核对，登记进清单 |
| P13 | `server-cert-verify-name` 的配置期校验规则 | 与 rustls 的 `ServerName::try_from` 同口径：IP 字面量，或 DNS 名（总长 ≤ 253、每段 1–63 个 `[A-Za-z0-9_-]`、不以 `-` 开头或结尾、末段不全是数字，允许一个结尾的点）。proto 侧的 `dns_name` 仍是最后一道（`E0022`） |
| P14 | 惰性请求头的触发规则（设计 6.1 写的是"应用先读就立刻单独发出"） | **订正**：引擎的转发循环在隧道建立的那一刻就开始轮询"读"，远早于客户端的第一段字节到达；按设计原文，请求头几乎总是单独发出，合并的意图落空。改为：读在请求头未发出时先等一个宽限期 `HEAD_GRACE` = **100 ms**（参考实现 sing-box 的同类机制是 300 ms），期间若有写，头与首段负载一起发出，并**唤醒挂在定时器上的读**（宽限期不拖慢响应）；宽限期内一直没有写（SSH / SMTP 这类服务端先说话的协议）才单独发出请求头，代价是这类协议的首个字节晚 100 ms。写计划时已在临时工程里验证四种情形 |

## 承接事项（M2 设计第 9 节）

| # | 事项 | 任务 |
| - | ---- | ---- |
| 1 | `server-cert-verify-name`：配置期校验；与指纹 / `skip-cert-verify` 同时出现时 `W0012`；`TlsClient::build` 在选择校验模式之前解析 | Task 1 |
| 2 | 两个缺失用例：链式会话进行中重载；保存的选择指向已不存在的成员 | Task 6 |
| 3 | `interface` / `allow-other-interface` / `tos` / `ip-version` × `underlying-proxy` → `W0028` | Task 1 |
| 4 | "防环回退对 `[Host]` 偏保守"前提不成立，关闭；订正 M1b 计划「延后事项」表 | Task 8 |
| 5 | `publish_registry` 的 `assert!` | M2b（不在本计划） |

## File Structure

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/ws.rs`（新） | `WsOpts`、`read_ws` | 1 |
| `crates/rurge-config/src/spec/trojan.rs`（新） | `TrojanSpec`、`read_trojan` | 1 |
| `crates/rurge-config/src/spec/{mod,http,tls}.rs` | 导出；`is_token` / `is_field_text` 改 `pub(crate)`；verify-name；`W0028` 组合告警；（Task 4）`ProtoSpec::Trojan` 与 `to_spec` 接线 | 1、4 |
| `crates/rurge-config/src/redact.rs` | `ws-headers`、`ws-path` 进 `SECRET_PARAMS` | 1 |
| `crates/rurge-proto/src/transport/ws.rs`（新） | `WsClient`、`WsByteStream` | 2 |
| `crates/rurge-proto/src/transport/stack.rs`（新） | `Stack::open` | 2 |
| `crates/rurge-proto/src/testing/ws.rs`（新） | 回环 WebSocket 接入（记录握手） | 2 |
| `crates/rurge-proto/src/addr.rs`（新） | `socks_addr` | 3 |
| `crates/rurge-proto/src/transport/lazy_head.rs`（新） | `LazyHead` | 3 |
| `crates/rurge-proto/src/trojan.rs`（新） | `TrojanOutbound` | 3 |
| `crates/rurge-proto/src/testing/trojan.rs`（新） | `FakeTrojan` | 3 |
| `crates/rurge-proto/src/transport/tls.rs` | verify-name 提前解析 | 1 |
| `crates/rurge-engine/src/outbounds.rs` | 工厂分支、`tls_of` | 4 |
| `crates/rurge-engine/tests/outbounds.rs`、`tests/pipeline.rs` | 端到端用例 | 4、6 |
| `crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs` | 能力表翻转 | 5 |
| `tests/interop/src/lib.rs`、`tests/interop/tests/sing_box.rs`、`tests/interop/README.md` | trojan 入站与 ws 传输 | 7 |
| `docs/**`、`README.md`、`CLAUDE.md` | 文档 | 8 |

---

### Task 1: 配置层——`WsOpts` / `TrojanSpec` 的读取函数、脱敏、承接事项 1 与 3

本任务**不**改 `ProtoSpec`、**不**改 `to_spec` 对 `PolicyKind::Trojan` 的处理：trojan 策略的行为保持不变（没有 spec → `W0007` + REJECT）。P6 说明了原因。

**Files:**
- Create: `crates/rurge-config/src/spec/ws.rs`
- Create: `crates/rurge-config/src/spec/trojan.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（模块声明与导出；`to_spec` 末尾的 `W0028` 组合告警）
- Modify: `crates/rurge-config/src/spec/http.rs`（`is_token`、`is_field_text` 改成 `pub(crate)`）
- Modify: `crates/rurge-config/src/spec/tls.rs`（`server-cert-verify-name`）
- Modify: `crates/rurge-config/src/redact.rs`（`SECRET_PARAMS`）
- Modify: `crates/rurge-proto/src/transport/tls.rs`（`TlsClient::build` 提前解析 verify-name）
- Test: 各文件自己的 `#[cfg(test)] mod tests`；`crates/rurge-config/tests/policy_spec.rs`

**Interfaces:**
- Consumes: `ParamReader`（`str` / `bool` / `has` / `touch` / `error` / `warn` / `invalid` / `has_errors`）、`tls::read_tls(r, keystore) -> TlsOpts`、`codes::{E_INVALID_POLICY_PARAM, W_INVALID_VALUE, W_PARAM_NOT_APPLICABLE}`。
- Produces（后续任务依赖的确切签名）:
  - `rurge_config::spec::WsOpts { pub path: String, pub headers: Vec<(String, String)> }`
  - `rurge_config::spec::ws::read_ws(r: &mut ParamReader<'_>) -> Option<WsOpts>`
  - `rurge_config::spec::TrojanSpec { pub tls: TlsOpts, pub password: String, pub ws: Option<WsOpts> }`
  - `rurge_config::spec::trojan::read_trojan(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TrojanSpec`（报过错之后返回值无意义，调用方看 `r.has_errors()`）

- [ ] **Step 1: 写失败的用例——`spec/ws.rs`**

新建 `crates/rurge-config/src/spec/ws.rs`，先只放类型、一个返回 `None` 的桩和用例：

```rust
//! WebSocket transport parameters (`ws`, `ws-path`, `ws-headers`; manual:
//! Policies › VMess, Policies › Trojan).

use super::http::{is_field_text, is_token};
use super::reader::ParamReader;
use crate::diagnostic::codes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsOpts {
    /// Starts with `/`; may carry a query.
    pub path: String,
    /// Extra handshake headers in the order written. A `Host` entry replaces
    /// the default `Host`.
    pub headers: Vec<(String, String)>,
}

/// `None` without `ws=true`.
pub fn read_ws(_r: &mut ParamReader<'_>) -> Option<WsOpts> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (Option<WsOpts>, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        // what the caller has read by then
        r.touch("password");
        let ws = read_ws(&mut r);
        (ws, r.finish())
    }

    #[test]
    fn defaults_and_the_manuals_spelling() {
        let (ws, diags) = read("trojan, h.test, 443, password=p, ws=true");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            ws,
            Some(WsOpts {
                path: "/".into(),
                headers: Vec::new()
            })
        );
        let (ws, diags) = read(
            "trojan, h.test, 443, password=p, ws=true, ws-path=/ray?ed=1, ws-headers=Host:edge.example|X-Key: v 1 ",
        );
        assert!(diags.is_empty(), "{diags:?}");
        let ws = ws.unwrap();
        assert_eq!(ws.path, "/ray?ed=1");
        assert_eq!(
            ws.headers,
            [
                ("Host".to_string(), "edge.example".to_string()),
                ("X-Key".to_string(), "v 1".to_string())
            ]
        );
    }

    #[test]
    fn without_ws_the_other_two_are_not_applicable() {
        let (ws, diags) = read("trojan, h.test, 443, password=p, ws-path=/x, ws-headers=A:b");
        assert_eq!(ws, None);
        let messages: Vec<(&str, &str)> = diags
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `ws-path` has no effect without `ws=true`; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: `ws-headers` has no effect without `ws=true`; ignored"
                ),
            ]
        );
        let (ws, diags) = read("trojan, h.test, 443, password=p, ws=false");
        assert!(ws.is_none() && diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_bad_path_is_an_error_that_never_quotes_it() {
        for bad in ["ws-path=secret", "ws-path=/a b", "ws-path=/é", "ws-path="] {
            let (_, diags) = read(&format!("trojan, h.test, 443, password=p, ws=true, {bad}"));
            assert_eq!(diags.len(), 1, "{bad}: {diags:?}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
            assert_eq!(
                diags[0].message,
                "policy `P`: invalid `ws-path` (expected an ASCII path that starts with `/` and holds no space or control character)"
            );
        }
    }

    #[test]
    fn bad_headers_are_errors_and_managed_ones_are_dropped() {
        let (_, diags) = read("trojan, h.test, 443, password=p, ws=true, ws-headers=nocolon");
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid `ws-headers`: entry #1 has no `:`"
        );
        let (_, diags) = read("trojan, h.test, 443, password=p, ws=true, ws-headers=A:b|bad name:v");
        assert_eq!(
            diags[0].message,
            "policy `P`: invalid `ws-headers`: entry #2 has an invalid header name"
        );
        let (_, diags) =
            read("trojan, h.test, 443, password=p, ws=true, ws-headers=X-A:line\u{7}feed");
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid `ws-headers`: header `X-A` holds a control character"
            )
        );
        let (ws, diags) = read(
            "trojan, h.test, 443, password=p, ws=true, ws-headers=Upgrade:h2c|X-A:1|Sec-WebSocket-Protocol:x|connection:close",
        );
        assert_eq!(ws.unwrap().headers, [("X-A".to_string(), "1".to_string())]);
        let dropped: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert!(diags.iter().all(|d| d.code == codes::W_INVALID_VALUE));
        assert_eq!(
            dropped,
            [
                "policy `P`: `ws-headers`: `Upgrade` is written by the WebSocket handshake itself; ignored",
                "policy `P`: `ws-headers`: `Sec-WebSocket-Protocol` is written by the WebSocket handshake itself; ignored",
                "policy `P`: `ws-headers`: `connection` is written by the WebSocket handshake itself; ignored",
            ]
        );
    }
}
```

在 `crates/rurge-config/src/spec/http.rs` 里把两个函数的可见性改成 `pub(crate)`（函数体不变）：

```rust
pub(crate) fn is_token(name: &str) -> bool {
```
```rust
pub(crate) fn is_field_text(text: &str) -> bool {
```

在 `crates/rurge-config/src/spec/mod.rs` 顶部的模块声明里加入 `pub mod ws;`（按字母序放好），并在导出处加入 `pub use ws::WsOpts;`。`trojan` 模块在 Step 4 才声明。

- [ ] **Step 2: 跑用例，确认失败**

Run: `cargo test -p rurge-config spec::ws`
Expected: FAIL——`defaults_and_the_manuals_spelling` 断言 `Some(..)` 得到 `None`；其余用例因为没有诊断而失败。

- [ ] **Step 3: 实现 `read_ws`**

把 `ws.rs` 里的桩换成：

```rust
const WS_KEYS: [&str; 2] = ["ws-path", "ws-headers"];

/// Headers the WebSocket handshake writes itself (see `transport::ws`).
fn is_managed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "connection" || lower == "upgrade" || lower.starts_with("sec-websocket-")
}

fn valid_path(path: &str) -> bool {
    path.starts_with('/') && path.bytes().all(|b| b.is_ascii_graphic())
}

/// `None` without `ws=true`; the other two parameters are then `W0028`.
/// Neither the path nor a header value is ever quoted in a diagnostic: both
/// are routinely used as shared secrets.
pub fn read_ws(r: &mut ParamReader<'_>) -> Option<WsOpts> {
    if !r.bool("ws").unwrap_or(false) {
        for key in WS_KEYS {
            if r.has(key) {
                r.touch(key);
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect without `ws=true`; ignored"),
                );
            }
        }
        return None;
    }
    let mut path = "/".to_string();
    if let Some(v) = r.str("ws-path") {
        let v = v.trim();
        if valid_path(v) {
            path = v.to_string();
        } else {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                "invalid `ws-path` (expected an ASCII path that starts with `/` and holds no space or control character)"
                    .to_string(),
            );
        }
    }
    let mut headers = Vec::new();
    if let Some(list) = r.str("ws-headers") {
        let items = list.split('|').map(str::trim).filter(|i| !i.is_empty());
        for (n, item) in items.enumerate() {
            let Some((name, value)) = item.split_once(':') else {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("invalid `ws-headers`: entry #{} has no `:`", n + 1),
                );
                continue;
            };
            let (name, value) = (name.trim(), value.trim());
            if !is_token(name) {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "invalid `ws-headers`: entry #{} has an invalid header name",
                        n + 1
                    ),
                );
            } else if !is_field_text(value) {
                r.error(
                    codes::E_INVALID_POLICY_PARAM,
                    format!("invalid `ws-headers`: header `{name}` holds a control character"),
                );
            } else if is_managed(name) {
                r.warn(
                    codes::W_INVALID_VALUE,
                    format!(
                        "`ws-headers`: `{name}` is written by the WebSocket handshake itself; ignored"
                    ),
                );
            } else {
                headers.push((name.to_string(), value.to_string()));
            }
        }
    }
    Some(WsOpts { path, headers })
}
```

Run: `cargo test -p rurge-config spec::ws`
Expected: PASS（4 个用例）。

- [ ] **Step 4: `spec/trojan.rs`——先写用例再实现**

新建 `crates/rurge-config/src/spec/trojan.rs`，并在 `spec/mod.rs` 里加 `pub mod trojan;` 与 `pub use trojan::TrojanSpec;`：

```rust
//! `trojan` policy parameters (manual: Policies › Trojan).

use super::reader::ParamReader;
use super::tls::{TlsOpts, read_tls};
use super::ws::{WsOpts, read_ws};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrojanSpec {
    /// Trojan always runs over TLS.
    pub tls: TlsOpts,
    pub password: String,
    pub ws: Option<WsOpts>,
}

/// Everything `trojan`-specific on the line. After an error was reported the
/// returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The password is named-only (`password=`), as the manual writes it: a
/// positional value stays unread and is reported as an extra positional
/// value, never quoted.
pub fn read_trojan(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TrojanSpec {
    let tls = read_tls(r, keystore);
    let password = r.str("password").unwrap_or_default().to_string();
    if password.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            "`password` is required".to_string(),
        );
    }
    let ws = read_ws(r);
    TrojanSpec { tls, password, ws }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::Sni;
    use std::path::Path;
    use std::sync::Arc;

    fn read(def: &str) -> (TrojanSpec, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let spec = read_trojan(&mut r, &[]);
        let failed = r.has_errors();
        (spec, failed, r.finish())
    }

    #[test]
    fn the_manuals_example_and_the_tls_parameters() {
        let (spec, failed, diags) = read("trojan, 192.0.2.15, 443, password=pwd, sni=example.com");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.password, "pwd");
        assert_eq!(spec.tls.sni, Sni::Name("example.com".into()));
        assert!(spec.ws.is_none());
        let (spec, failed, _) = read("trojan, h.test, 443, password=p, ws=true, ws-path=/t");
        assert!(!failed);
        assert_eq!(spec.ws.unwrap().path, "/t");
    }

    #[test]
    fn a_missing_password_is_an_error_and_a_positional_one_is_not_read() {
        let (_, failed, diags) = read("trojan, h.test, 443");
        assert!(failed);
        assert_eq!(
            (diags[0].code, diags[0].message.as_str()),
            (
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: `password` is required"
            )
        );
        let (_, failed, diags) = read("trojan, h.test, 443, hunter2");
        assert!(failed);
        let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "policy `P`: `password` is required",
                "policy `P`: unexpected positional value #1 ignored"
            ]
        );
        assert!(messages.iter().all(|m| !m.contains("hunter2")));
    }
}
```

Run: `cargo test -p rurge-config spec::trojan`
Expected: PASS（2 个用例）。（本步没有单独的 RED：`read_trojan` 只是把三个已有读取函数拼起来，失败路径由第二个用例钉住。）

- [ ] **Step 5: 承接事项 1——`server-cert-verify-name` 的配置期校验（先写用例）**

在 `crates/rurge-config/src/spec/tls.rs` 的测试模块末尾加：

```rust
    #[test]
    fn verify_name_is_checked_at_load_time() {
        for good in ["real.example.com", "_srv.example.", "192.0.2.7", "2001:db8::1", "a-b.c"] {
            let (tls, diags) = read(&format!("https, h, 443, server-cert-verify-name={good}"));
            assert!(diags.is_empty(), "{good}: {diags:?}");
            assert_eq!(tls.verify_name.as_deref(), Some(good));
        }
        for bad in ["bücher.example", "a b", "-a.example", "a-.example", "a..b", "example.123", "x".repeat(64).as_str()] {
            let (tls, diags) = read(&format!("https, h, 443, server-cert-verify-name={bad}"));
            assert_eq!(tls.verify_name, None, "{bad}");
            assert_eq!(diags.len(), 1, "{bad}: {diags:?}");
            assert_eq!(diags[0].code, codes::E_INVALID_POLICY_PARAM);
        }
    }

    #[test]
    fn verify_name_without_chain_validation_is_ignored_with_a_warning() {
        let fp = "ab".repeat(32);
        for extra in [
            format!("server-cert-fingerprint-sha256={fp}"),
            "skip-cert-verify=true".to_string(),
        ] {
            let (_, diags) = read(&format!(
                "https, h, 443, server-cert-verify-name=real.example.com, {extra}"
            ));
            assert_eq!(diags.len(), 1, "{extra}: {diags:?}");
            assert_eq!(
                (diags[0].code, diags[0].message.as_str()),
                (
                    codes::W_INVALID_VALUE,
                    "policy `P`: `server-cert-verify-name` is ignored because the certificate chain is not validated (`server-cert-fingerprint-sha256` / `skip-cert-verify`)"
                )
            );
        }
    }
```

两个用例用的 `read(def) -> (TlsOpts, Vec<crate::Diagnostic>)` 是该测试模块里现成的辅助函数（`https` 定义行 → `read_tls`）。

Run: `cargo test -p rurge-config spec::tls::tests::verify_name`
Expected: FAIL——坏名字现在被原样接受（`verify_name` 是 `Some`）；第二个用例没有任何诊断。

- [ ] **Step 6: 实现 verify-name 的校验与告警**

`crates/rurge-config/src/spec/tls.rs`，在 `parse_fingerprint` 后面加：

```rust
/// What `rustls` accepts as a server name: an IP literal, or a DNS name of
/// at most 253 bytes whose labels are 1-63 of `[A-Za-z0-9_-]`, do not start
/// or end with `-`, and whose last label is not all digits. One trailing dot
/// is fine. An IDN must be written in its `xn--` form.
fn is_server_name(name: &str) -> bool {
    if name.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = name.split('.').collect();
    let label_ok = |l: &&str| {
        (1..=63).contains(&l.len())
            && l.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    labels.iter().all(label_ok)
        && labels
            .last()
            .is_some_and(|l| !l.bytes().all(|b| b.is_ascii_digit()))
}
```

把 `read_tls` 里读 `server-cert-verify-name` 的那段换成：

```rust
    let mut verify_name = None;
    if let Some(v) = r.str("server-cert-verify-name") {
        let v = v.trim();
        if is_server_name(v) {
            verify_name = Some(v.to_string());
        } else {
            r.invalid(
                "server-cert-verify-name",
                v,
                "a host name (an IDN in its xn-- form) or an IP address",
            );
        }
    }
```

在已有的 `if skip_cert_verify && fingerprint_sha256.is_some() { … }` 之后加：

```rust
    if verify_name.is_some() && (fingerprint_sha256.is_some() || skip_cert_verify) {
        r.warn(
            codes::W_INVALID_VALUE,
            "`server-cert-verify-name` is ignored because the certificate chain is not validated (`server-cert-fingerprint-sha256` / `skip-cert-verify`)"
                .to_string(),
        );
    }
```

注意：`192.0.2.7` 走的是 IP 分支，所以"末段不全是数字"不会误伤它；`example.123` 不是 IP，被拒绝。

`crates/rurge-proto/src/transport/tls.rs` 的 `TlsClient::build`：把 verify-name 的解析挪到选择模式之前，让任何模式下的坏名字都是构建错误（调用方不经 `rurge-config` 也成立）。把

```rust
        let mode = if let Some(fingerprint) = opts.fingerprint_sha256 {
```

之前加一行，并把标准分支里的同一行删掉：

```rust
        // parsed whatever the mode: a name that is not one is a build error
        let verify_name = opts.verify_name.as_deref().map(dns_name).transpose()?;
```

同文件的 `mod tests` 里加一个用例：

```rust
    #[test]
    fn a_bad_verify_name_is_a_build_error_in_every_mode() {
        for (skip, fingerprint) in [(false, None), (true, None), (false, Some([7u8; 32]))] {
            let opts = TlsOpts {
                skip_cert_verify: skip,
                fingerprint_sha256: fingerprint,
                verify_name: Some("not a name".into()),
                ..TlsOpts::default()
            };
            let err = TlsClient::build(
                &opts,
                &HostName::parse("proxy.example"),
                &[],
                None,
                // standard verification cannot even be set up without a root
                TlsFixture::new(&["proxy.example"]).roots(),
            )
            .err()
            .expect("the name is refused");
            assert_eq!(err.message, "`not a name` is not a valid TLS server name");
        }
    }
```

（放进该文件现有的 `mod tests`：`TlsFixture`、`TlsOpts`、`HostName` 都已在作用域里。标准模式要有非空的根证书库才走得到名字检查之后，所以用夹具自己的根。）

Run: `cargo test -p rurge-config spec::tls && cargo test -p rurge-proto transport::tls`
Expected: PASS。

- [ ] **Step 7: 承接事项 3——socket 选项 × `underlying-proxy` 的 `W0028`（先写用例）**

`crates/rurge-config/src/spec/mod.rs` 测试模块里加：

```rust
    #[test]
    fn socket_options_under_a_chain_are_reported() {
        let o = outcome(
            "P",
            "http, h, 80, underlying-proxy=Entry, interface=eth0, allow-other-interface=true, tos=16, ip-version=v4-only",
        );
        assert!(o.spec.is_some(), "a warning, not an error");
        let messages: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                (codes::W_PARAM_NOT_APPLICABLE, "policy `P`: `interface` has no effect on a policy with `underlying-proxy`; ignored"),
                (codes::W_PARAM_NOT_APPLICABLE, "policy `P`: `allow-other-interface` has no effect on a policy with `underlying-proxy`; ignored"),
                (codes::W_PARAM_NOT_APPLICABLE, "policy `P`: `tos` has no effect on a policy with `underlying-proxy`; ignored"),
                (codes::W_PARAM_NOT_APPLICABLE, "policy `P`: `ip-version` has no effect on a policy with `underlying-proxy`; ignored"),
            ]
        );
        // `underlying-proxy=DIRECT` is no chain: nothing to report
        let o = outcome("P", "http, h, 80, underlying-proxy=DIRECT, interface=eth0");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        let o = outcome("P", "http, h, 80, interface=eth0, tos=16");
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
    }
```

Run: `cargo test -p rurge-config spec::tests::socket_options_under_a_chain_are_reported`
Expected: FAIL（没有任何诊断）。

实现：`to_spec` 里，在调用 `check_underlying(&mut r, &mut common, env);` 的那个 `if` 块**之后**（`let failed = r.has_errors();` 之前）加：

```rust
    // socket options belong to the hop that opens the socket (matrix 4.3)
    if common.underlying_proxy.is_some() {
        for key in ["interface", "allow-other-interface", "tos", "ip-version"] {
            if r.has(key) {
                r.warn(
                    codes::W_PARAM_NOT_APPLICABLE,
                    format!("`{key}` has no effect on a policy with `underlying-proxy`; ignored"),
                );
            }
        }
    }
```

Run: 同上。Expected: PASS。

- [ ] **Step 8: 脱敏——`ws-headers` 与 `ws-path`（先写用例）**

`crates/rurge-config/src/redact.rs`：在 `a_definition_is_redacted_like_its_profile_line` 的定义列表里加一项，并把 secret 名单扩一下：

```rust
            "trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=Host:edge.test|X-Key:k3y",
```
```rust
            for secret in ["s3cret", "hunter2", "alice", "bob", "tok3n", "pw0rd", "s3cretpath", "k3y", "edge.test"] {
```

并在该用例末尾加一条精确断言：

```rust
        assert_eq!(
            redact_definition(
                "trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=Host:edge.test|X-Key:k3y"
            ),
            "trojan, t.test, 443, password=***, ws=true, ws-path=***, ws-headers=***"
        );
```

Run: `cargo test -p rurge-config redact`
Expected: FAIL——`ws-path` / `ws-headers` 的值原样出现。

实现：`SECRET_PARAMS` 由 9 项变 11 项，并改它上面的文档注释：

```rust
/// Inline `name=value` parameters redacted wherever they appear in a value.
/// `username` also covers harmless SSH user names; `headers` and `ws-headers`
/// are blanked whole (header names included), and so is `ws-path`, which
/// nodes behind a CDN routinely use as a shared secret. Over-redacting is the
/// safe side for an endpoint whose purpose is safe output.
const SECRET_PARAMS: [&str; 11] = [
    "password",
    "psk",
    "private-key",
    "pre-shared-key",
    "base64",
    "token",
    "uuid",
    "username",
    "headers",
    "ws-headers",
    "ws-path",
];
```

`redact_param` 要求参数名前面是行首、逗号或空白，所以 `headers` 不会匹配到 `ws-headers=` 里面去；上面那条精确断言（输出恰好是 `ws-headers=***`）把这一点钉住。

Run: `cargo test -p rurge-config redact`
Expected: PASS。再跑 `cargo test -p rurge-engine --lib views`（`lineHash` 取脱敏后的定义，不应受影响）。

- [ ] **Step 9: 集成用例——trojan 策略的行为没有变**

`crates/rurge-config/tests/policy_spec.rs` 末尾加：

```rust
#[test]
fn a_trojan_policy_has_no_spec_until_the_outbound_is_wired_in() {
    // M2a plan P6: the readers exist, `to_spec` does not use them yet, so
    // the registry keeps treating the policy as "not implemented"
    let loaded = load("T = trojan, t.example, 443, password=p, ws=true", "");
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    assert!(loaded.config.spec("T").is_none());
}
```

（Task 4 会把这个用例改成相反的断言。）

- [ ] **Step 10: 门禁与语料库快照**

跑完整门禁。`tests/corpus/valid/kitchen-sink.conf` 第 61 行有一条 trojan 策略；本任务不该改变任何快照。若 `corpus` 用例因为新的 `W0028` / `W0012` 诊断而失败，用 `cargo insta test -p rurge-config --review` 审阅：**只**接受本任务引入的这三类诊断，其它差异一律停下来报告。

- [ ] **Step 11: 提交**

```bash
git add crates/rurge-config crates/rurge-proto/src/transport/tls.rs
git commit -m "feat(config): WsOpts / TrojanSpec 的读取函数；ws-headers 与 ws-path 脱敏；server-cert-verify-name 配置期校验；socket 选项 × underlying-proxy 的 W0028"
```

（提交信息末尾加 Global Constraints 里的两行署名；后面每个任务的提交同理，不再重复。）


---

### Task 2: `transport::ws`（WebSocket 字节流）、`transport::Stack`（传输阶梯）与 `testing::ws`

下面 `WsByteStream` 的读写逻辑与 `WsClient::wrap` 的请求构造，写计划时已在一次性临时工程里对 `tokio-tungstenite 0.28.0` 编译并跑通（200 KB 往返、跨帧重组、Ping、Close = EOF、403）。P2 的两条要求（`ws://` URI、五个握手头各一个）就是那次验证里撞出来的。

**Files:**
- Modify: `Cargo.toml`（工作区依赖）、`crates/rurge-proto/Cargo.toml`
- Create: `crates/rurge-proto/src/transport/ws.rs`
- Create: `crates/rurge-proto/src/transport/stack.rs`
- Create: `crates/rurge-proto/src/testing/ws.rs`
- Modify: `crates/rurge-proto/src/transport/mod.rs`、`crates/rurge-proto/src/testing/mod.rs`

**Interfaces:**
- Consumes: `rurge_config::spec::WsOpts`（Task 1）；`crate::http::wire_host(&Target) -> Option<String>`；`TlsClient::wrap(&self, BoxedStream) -> io::Result<BoxedStream>`；`OutboundError::{Proxy, Io, tls()}`；`BuildError::new`；`testing::{TlsFixture, AbortOnDrop}`。
- Produces:
  - `rurge_proto::transport::ws::WsClient::new(opts: &WsOpts, server: &Target, tls: bool) -> Result<WsClient, BuildError>`
  - `WsClient::wrap(&self, stream: BoxedStream) -> Result<BoxedStream, OutboundError>`
  - `rurge_proto::transport::ws::WsByteStream`（`pub(crate) fn new(inner: WebSocketStream<BoxedStream>) -> WsByteStream`，服务端测试夹具也用它）
  - `rurge_proto::transport::stack::Stack::new(connector: Arc<dyn Connector>, server: Target, tls: Option<TlsClient>, ws: Option<WsClient>) -> Stack`、`Stack::open(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError>`、`Stack::server(&self) -> &Target`
  - `rurge_proto::testing::{FakeWs, WsScript, RecordedWs}`；`rurge_proto::testing::ws::accept_bytes(stream: BoxedStream, seen: &Mutex<Vec<RecordedWs>>) -> io::Result<BoxedStream>`（Task 3 的 `FakeTrojan` 用）

- [ ] **Step 1: 依赖**

`Cargo.toml`（工作区 `[workspace.dependencies]`）加两行：

```toml
tokio-tungstenite = { version = "0.28", default-features = false, features = ["handshake"] }
futures-util = { version = "0.3", default-features = false, features = ["sink"] }
```

`crates/rurge-proto/Cargo.toml` 的 `[dependencies]` 加：

```toml
tokio-tungstenite.workspace = true
futures-util.workspace = true
http.workspace = true
bytes.workspace = true
```

Run: `cargo check -p rurge-proto && git diff --stat Cargo.lock`
Expected: 编译通过；`Cargo.lock` 新增的 `[[package]]` 恰好是 `tokio-tungstenite`、`tungstenite`、`utf-8` 三个（P1）。用 `git diff Cargo.lock | grep '^+name = '` 核对；多出别的包 → 停下来报告。

- [ ] **Step 2: `testing::ws`——回环 WebSocket 接入**

新建 `crates/rurge-proto/src/testing/ws.rs`：

```rust
//! A scriptable WebSocket acceptor: records each handshake, then echoes
//! bytes (or misbehaves as scripted).

use super::{AbortOnDrop, TlsFixture};
use crate::transport::ws::WsByteStream;
use bytes::Bytes;
use futures_util::SinkExt;
use rurge_net::connector::BoxedStream;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

#[derive(Clone, Debug, Default)]
pub struct WsScript {
    /// Refuse the handshake with this HTTP status.
    pub refuse: Option<u16>,
    /// After the handshake, send one text frame.
    pub send_text: bool,
    /// After the handshake, send one binary frame of this many bytes.
    pub send_frame: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct RecordedWs {
    /// Path and query as the client wrote them.
    pub path: String,
    /// Names in lower case (as the `http` crate stores them).
    pub headers: Vec<(String, String)>,
}

impl RecordedWs {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeWs {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<RecordedWs>>>,
    _task: AbortOnDrop,
}

async fn accept_socket(
    stream: BoxedStream,
    seen: &Mutex<Vec<RecordedWs>>,
    refuse: Option<u16>,
) -> io::Result<tokio_tungstenite::WebSocketStream<BoxedStream>> {
    let callback = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let headers = req
            .headers()
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();
        let path = req
            .uri()
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_default();
        seen.lock().expect("seen").push(RecordedWs { path, headers });
        match refuse {
            None => Ok(resp),
            Some(code) => {
                let mut no = ErrorResponse::new(Some("refused by the script".to_string()));
                *no.status_mut() = http::StatusCode::from_u16(code).expect("a status code");
                Err(no)
            }
        }
    };
    tokio_tungstenite::accept_hdr_async(stream, callback)
        .await
        .map_err(|e| io::Error::other(e.to_string()))
}

/// The server side of a WebSocket handshake on `stream`, as a byte stream.
/// `FakeTrojan` puts its protocol on top of this.
pub async fn accept_bytes(
    stream: BoxedStream,
    seen: &Mutex<Vec<RecordedWs>>,
) -> io::Result<BoxedStream> {
    Ok(Box::new(WsByteStream::new(
        accept_socket(stream, seen, None).await?,
    )))
}

async fn serve(
    stream: BoxedStream,
    script: WsScript,
    seen: Arc<Mutex<Vec<RecordedWs>>>,
) -> io::Result<()> {
    let mut socket = accept_socket(stream, &seen, script.refuse).await?;
    if script.send_text {
        let _ = socket.send(Message::text("not binary")).await;
    }
    if let Some(len) = script.send_frame {
        let _ = socket.send(Message::Binary(Bytes::from(vec![7u8; len]))).await;
    }
    let mut bytes = WsByteStream::new(socket);
    let mut buf = [0u8; 1024];
    loop {
        let n = bytes.read(&mut buf).await?;
        if n == 0 {
            return bytes.shutdown().await;
        }
        bytes.write_all(&buf[..n]).await?;
        bytes.flush().await?;
    }
}

impl FakeWs {
    pub async fn spawn(script: WsScript) -> FakeWs {
        FakeWs::start(script, None).await
    }

    /// The same acceptor behind TLS.
    pub async fn spawn_tls(script: WsScript, fixture: Arc<TlsFixture>) -> FakeWs {
        FakeWs::start(script, Some(fixture)).await
    }

    async fn start(script: WsScript, tls: Option<Arc<TlsFixture>>) -> FakeWs {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let seen: Arc<Mutex<Vec<RecordedWs>>> = Arc::default();
        let log = seen.clone();
        let tls = tls.map(|fixture| {
            let acceptor = fixture.acceptor(false);
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
        FakeWs {
            addr,
            seen,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every handshake seen so far, in arrival order.
    pub fn seen(&self) -> Vec<RecordedWs> {
        self.seen.lock().expect("seen").clone()
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`：加 `pub mod ws;`（`accept_bytes` 要从外面按路径可达）与

```rust
pub use ws::{FakeWs, RecordedWs, WsScript};
```

- [ ] **Step 3: `transport::ws`——先写用例**

新建 `crates/rurge-proto/src/transport/ws.rs`，先放类型与**桩**（`wrap` 直接把流原样返回、`new` 不校验），再放完整的测试模块；`transport/mod.rs` 加 `pub mod ws;`（以及 Step 5 的 `pub mod stack;`，现在先只加 `ws`）。

测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeWs, WsScript};
    use rurge_config::HostName;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    fn opts(path: &str, headers: &[(&str, &str)]) -> WsOpts {
        WsOpts {
            path: path.to_string(),
            headers: headers
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn server(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn open(fake: &FakeWs, client: &WsClient) -> Result<BoxedStream, OutboundError> {
        let tcp = TcpStream::connect(fake.addr()).await.unwrap();
        client.wrap(Box::new(tcp)).await
    }

    #[tokio::test]
    async fn bytes_cross_in_both_directions_whatever_the_slicing() {
        let fake = FakeWs::spawn(WsScript::default()).await;
        let client = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
        // (payload length, bytes per write, bytes per read): one-byte frames,
        // odd sizes, and writes larger than one outgoing frame; the echo
        // comes back in frames of at most 1 KiB
        for (len, write_chunk, read_chunk) in [
            (4_000usize, 1usize, 3usize),
            (50_000, 1_000, 777),
            (200_000, 70_000, 4_096),
        ] {
            let stream = open(&fake, &client).await.unwrap();
            let payload: Vec<u8> = (0..len as u32).map(|i| (i % 251) as u8).collect();
            let (mut rd, mut wr) = tokio::io::split(stream);
            let to_send = payload.clone();
            let writer = tokio::spawn(async move {
                for chunk in to_send.chunks(write_chunk) {
                    wr.write_all(chunk).await.unwrap();
                }
                wr.flush().await.unwrap();
                wr
            });
            let mut back = vec![0u8; payload.len()];
            let mut got = 0;
            while got < back.len() {
                let end = (got + read_chunk).min(back.len());
                let n = rd.read(&mut back[got..end]).await.unwrap();
                assert!(n > 0, "EOF after {got} of {len} bytes");
                got += n;
            }
            assert_eq!(back, payload, "{len} / {write_chunk} / {read_chunk}");
            let mut stream = rd.unsplit(writer.await.unwrap());
            stream.shutdown().await.unwrap();
            let mut rest = Vec::new();
            stream.read_to_end(&mut rest).await.unwrap();
            assert!(rest.is_empty(), "the peer's Close is an EOF");
        }
    }

    #[tokio::test]
    async fn the_request_carries_the_path_the_host_and_the_extra_headers() {
        let fake = FakeWs::spawn(WsScript::default()).await;
        let custom = WsClient::new(
            &opts("/ray?ed=1", &[("Host", "edge.example"), ("X-Key", "v 1")]),
            &server(fake.addr()),
            false,
        )
        .unwrap();
        drop(open(&fake, &custom).await.unwrap());
        let default = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
        drop(open(&fake, &default).await.unwrap());
        let seen = fake.seen();
        assert_eq!(seen[0].path, "/ray?ed=1");
        assert_eq!(seen[0].header("host"), Some("edge.example"));
        assert_eq!(seen[0].header("x-key"), Some("v 1"));
        assert_eq!(seen[0].header("upgrade"), Some("websocket"));
        assert_eq!(seen[1].path, "/");
        // not the layer's default port (80 without TLS): the port is written
        assert_eq!(
            seen[1].header("host"),
            Some(format!("127.0.0.1:{}", fake.addr().port()).as_str())
        );
    }

    #[test]
    fn the_default_host_follows_the_servers_name_and_the_layers_port() {
        let host = |name: &str, port: u16, tls: bool| {
            WsClient::new(
                &opts("/", &[]),
                &Target::new(HostName::parse(name), port),
                tls,
            )
            .unwrap()
            .host
        };
        assert_eq!(host("edge.example", 443, true), "edge.example");
        assert_eq!(host("edge.example", 80, false), "edge.example");
        assert_eq!(host("edge.example", 8443, true), "edge.example:8443");
        assert_eq!(host("bücher.example", 443, true), "xn--bcher-kva.example");
        assert_eq!(host("2001:db8::1", 443, true), "[2001:db8::1]");
        assert_eq!(host("2001:db8::1", 8080, true), "[2001:db8::1]:8080");
    }

    #[test]
    fn what_cannot_be_sent_is_a_build_error_that_quotes_nothing() {
        let target = Target::new(HostName::parse("edge.example"), 443);
        for (o, expected) in [
            (
                opts("/ok", &[("Host", "bad host")]),
                "`ws-path` and the `Host` header do not form a valid request URI",
            ),
            (
                opts("/ok", &[("X-A", "line\nbreak")]),
                "`ws-headers` entry #1 cannot be sent",
            ),
            (
                opts("/ok", &[("X-A", "1"), ("Upgrade", "h2c")]),
                "`ws-headers` entry #2 is a header of the WebSocket handshake itself",
            ),
        ] {
            let err = WsClient::new(&o, &target, true).err().expect("refused");
            assert_eq!(err.message, expected);
        }
        let err = WsClient::new(
            &opts("/", &[]),
            &Target::new(HostName::Domain("a@b.test".into()), 443),
            true,
        )
        .err()
        .expect("refused");
        assert_eq!(
            err.message,
            "the server's host name cannot be written into a WebSocket request"
        );
    }

    #[tokio::test]
    async fn a_refused_handshake_names_the_status_code_only() {
        let fake = FakeWs::spawn(WsScript {
            refuse: Some(403),
            ..WsScript::default()
        })
        .await;
        let client = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
        let err = open(&fake, &client).await.err().expect("refused");
        assert!(
            matches!(&err, OutboundError::Proxy(m) if m == "ws: handshake failed: HTTP 403"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_text_frame_and_an_oversized_frame_end_the_stream_with_an_error() {
        for (script, expected) in [
            (
                WsScript {
                    send_text: true,
                    ..WsScript::default()
                },
                "ws: the server sent a text frame",
            ),
            (
                WsScript {
                    send_frame: Some(MAX_INCOMING + 1),
                    ..WsScript::default()
                },
                "ws: the server sent a frame larger than the limit",
            ),
        ] {
            let fake = FakeWs::spawn(script).await;
            let client = WsClient::new(&opts("/", &[]), &server(fake.addr()), false).unwrap();
            let mut stream = open(&fake, &client).await.unwrap();
            let mut buf = [0u8; 16];
            let err = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut buf))
                .await
                .expect("the frame arrives")
                .err()
                .expect("an error, not data");
            assert_eq!(err.to_string(), expected);
        }
    }
}
```

Run: `cargo test -p rurge-proto transport::ws`
Expected: FAIL（桩不做握手：假服务端把收到的原始字节当成坏的 HTTP 请求关掉连接，往返用例读到 EOF；其余用例的断言各自不成立）。

- [ ] **Step 4: 实现 `transport::ws`**

`crates/rurge-proto/src/transport/ws.rs` 的非测试部分：

```rust
//! WebSocket as a byte stream (`ws=true`): a client handshake on top of any
//! stream, then binary frames in both directions.

use crate::{BuildError, OutboundError};
use bytes::{Buf, Bytes};
use futures_util::{Sink, Stream};
use http::header::{HeaderName, HeaderValue};
use rurge_config::spec::WsOpts;
use rurge_net::connector::{BoxedStream, Target};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::client::generate_key;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// Largest frame and largest message accepted from the peer.
pub const MAX_INCOMING: usize = 1 << 20;
/// Largest payload put into one outgoing frame.
const MAX_OUTGOING: usize = 64 * 1024;

/// tungstenite's own error texts can quote header values, so none of them is
/// ever passed on: every variant maps onto a fixed text.
fn ws_io(e: WsError) -> io::Error {
    match e {
        WsError::Io(e) => e,
        WsError::ConnectionClosed | WsError::AlreadyClosed => {
            io::Error::new(io::ErrorKind::BrokenPipe, "ws: the connection is closed")
        }
        WsError::Capacity(_) => io::Error::new(
            io::ErrorKind::InvalidData,
            "ws: the server sent a frame larger than the limit",
        ),
        _ => io::Error::new(io::ErrorKind::InvalidData, "ws: protocol error"),
    }
}

/// Bytes over an established WebSocket, for either role.
pub struct WsByteStream {
    inner: WebSocketStream<BoxedStream>,
    /// What is left of the binary message being read.
    pending: Bytes,
    eof: bool,
}

impl WsByteStream {
    pub(crate) fn new(inner: WebSocketStream<BoxedStream>) -> WsByteStream {
        WsByteStream {
            inner,
            pending: Bytes::new(),
            eof: false,
        }
    }
}

impl AsyncRead for WsByteStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(buf.remaining());
                buf.put_slice(&self.pending[..n]);
                self.pending.advance(n);
                return Poll::Ready(Ok(()));
            }
            if self.eof {
                return Poll::Ready(Ok(()));
            }
            match ready!(Pin::new(&mut self.inner).poll_next(cx)) {
                Some(Ok(Message::Binary(data))) => self.pending = data,
                // the library answers a ping on its own; nothing here is payload
                Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {}
                Some(Ok(Message::Text(_))) => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "ws: the server sent a text frame",
                    )));
                }
                Some(Ok(Message::Close(_))) | None => self.eof = true,
                Some(Err(WsError::ConnectionClosed | WsError::AlreadyClosed)) => self.eof = true,
                Some(Err(e)) => return Poll::Ready(Err(ws_io(e))),
            }
        }
    }
}

impl AsyncWrite for WsByteStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        ready!(Pin::new(&mut self.inner).poll_ready(cx)).map_err(ws_io)?;
        let n = data.len().min(MAX_OUTGOING);
        Pin::new(&mut self.inner)
            .start_send(Message::Binary(Bytes::copy_from_slice(&data[..n])))
            .map_err(ws_io)?;
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx).map_err(ws_io)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // sends Close once and flushes; a peer that is already gone is fine
        match ready!(Pin::new(&mut self.inner).poll_close(cx)) {
            Ok(()) | Err(WsError::ConnectionClosed | WsError::AlreadyClosed) => {
                Poll::Ready(Ok(()))
            }
            Err(e) => Poll::Ready(Err(ws_io(e))),
        }
    }
}

/// Headers the handshake writes itself; a caller-supplied one would make
/// tungstenite refuse the request as a duplicate.
fn is_managed(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "connection" || lower == "upgrade" || lower.starts_with("sec-websocket-")
}

/// Everything that can be prepared ahead of a connection.
pub struct WsClient {
    /// `ws://<host><path>`: tungstenite insists on the scheme; only the path
    /// and query reach the wire.
    uri: http::Uri,
    host: String,
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl WsClient {
    /// `server` is the proxy's own address; `tls` tells which default port
    /// the layer below has (443 with TLS, 80 without). No error text quotes a
    /// path, a header value or a host: all three are used as shared secrets.
    pub fn new(opts: &WsOpts, server: &Target, tls: bool) -> Result<WsClient, BuildError> {
        let mut host = None;
        let mut headers = Vec::new();
        for (n, (name, value)) in opts.headers.iter().enumerate() {
            if name.eq_ignore_ascii_case("host") {
                host = Some(value.clone());
                continue;
            }
            if is_managed(name) {
                return Err(BuildError::new(format!(
                    "`ws-headers` entry #{} is a header of the WebSocket handshake itself",
                    n + 1
                )));
            }
            let (Ok(name), Ok(value)) = (
                HeaderName::try_from(name.as_str()),
                HeaderValue::from_str(value),
            ) else {
                return Err(BuildError::new(format!(
                    "`ws-headers` entry #{} cannot be sent",
                    n + 1
                )));
            };
            headers.push((name, value));
        }
        let host = match host {
            Some(host) => host,
            None => {
                let name = crate::http::wire_host(server).ok_or_else(|| {
                    BuildError::new(
                        "the server's host name cannot be written into a WebSocket request",
                    )
                })?;
                let default_port = if tls { 443 } else { 80 };
                if server.port == default_port {
                    name
                } else {
                    format!("{name}:{}", server.port)
                }
            }
        };
        let uri = format!("ws://{host}{}", opts.path)
            .parse::<http::Uri>()
            .map_err(|_| {
                BuildError::new("`ws-path` and the `Host` header do not form a valid request URI")
            })?;
        Ok(WsClient { uri, host, headers })
    }

    pub async fn wrap(&self, stream: BoxedStream) -> Result<BoxedStream, OutboundError> {
        let mut request = http::Request::builder()
            .method("GET")
            .uri(self.uri.clone())
            .header("Host", self.host.as_str())
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header("Sec-WebSocket-Key", generate_key());
        for (name, value) in &self.headers {
            request = request.header(name.clone(), value.clone());
        }
        let request = request
            .body(())
            .map_err(|_| OutboundError::Proxy("ws: the request cannot be built".to_string()))?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_INCOMING))
            .max_frame_size(Some(MAX_INCOMING));
        match tokio_tungstenite::client_async_with_config(request, stream, Some(config)).await {
            Ok((socket, _response)) => Ok(Box::new(WsByteStream::new(socket))),
            Err(WsError::Http(response)) => Err(OutboundError::Proxy(format!(
                "ws: handshake failed: HTTP {}",
                response.status().as_u16()
            ))),
            Err(WsError::Io(e)) => Err(OutboundError::from(e)),
            // never the library's text (see `ws_io`)
            Err(_) => Err(OutboundError::Proxy("ws: handshake failed".to_string())),
        }
    }
}
```

`host` 是私有字段；`the_default_host_follows_…` 用例在同一个模块里，可以直接读它。

Run: `cargo test -p rurge-proto transport::ws`
Expected: PASS（6 个用例）。超大帧用例若报的不是 `Capacity` 变体（文本对不上），打印实际的变体，把它并进 `ws_io` 的那一条映射里，并在报告里写明。

- [ ] **Step 5: `transport::Stack`——先写用例再实现**

新建 `crates/rurge-proto/src/transport/stack.rs`；`transport/mod.rs` 加 `pub mod stack;`，并加导出 `pub use stack::Stack;`：

```rust
//! The fixed ladder between a connector and a protocol's own handshake
//! (phase 2 design 5.4): connect → tls → ws. Shadow TLS joins in M2c.

use crate::OutboundError;
use crate::transport::tls::TlsClient;
use crate::transport::ws::WsClient;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use std::sync::Arc;

pub struct Stack {
    connector: Arc<dyn Connector>,
    server: Target,
    tls: Option<TlsClient>,
    ws: Option<WsClient>,
}

impl Stack {
    pub fn new(
        connector: Arc<dyn Connector>,
        server: Target,
        tls: Option<TlsClient>,
        ws: Option<WsClient>,
    ) -> Stack {
        Stack {
            connector,
            server,
            tls,
            ws,
        }
    }

    pub fn server(&self) -> &Target {
        &self.server
    }

    /// No timeout of its own: the caller wraps the ladder and its own
    /// handshake into one budget.
    pub async fn open(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        let mut stream = self.connector.connect(&self.server, opts).await?;
        if let Some(tls) = &self.tls {
            stream = tls.wrap(stream).await.map_err(OutboundError::tls)?;
        }
        if let Some(ws) = &self.ws {
            stream = ws.wrap(stream).await?;
        }
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeWs, TlsFixture, WsScript};
    use rurge_config::HostName;
    use rurge_config::spec::{TlsOpts, WsOpts};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn tls_then_websocket_and_no_alpn_unless_asked_for() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeWs::spawn_tls(WsScript::default(), fixture.clone()).await;
        let server = Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port());
        let tls = TlsClient::build(
            &TlsOpts::default(),
            &server.host,
            &[],
            None,
            fixture.roots(),
        )
        .unwrap();
        let ws = WsClient::new(
            &WsOpts {
                path: "/tunnel".into(),
                headers: Vec::new(),
            },
            &server,
            true,
        )
        .unwrap();
        let stack = Stack::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            server,
            Some(tls),
            Some(ws),
        );
        let mut stream = stack.open(&ConnectOpts::default()).await.unwrap();
        stream.write_all(b"through both layers").await.unwrap();
        stream.flush().await.unwrap();
        let mut buf = [0u8; 19];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"through both layers");
        assert_eq!(fake.seen()[0].path, "/tunnel");
        // the fixture offers h2 first: an ALPN of ours would have picked it
        assert_eq!(fixture.seen()[0].alpn, None);
    }

    #[tokio::test]
    async fn each_layer_says_which_one_failed() {
        // a plain WebSocket server behind a TLS client: the TLS layer fails
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeWs::spawn(WsScript::default()).await;
        let server = Target::new(HostName::Ip(fake.addr().ip()), fake.addr().port());
        let tls = TlsClient::build(
            &TlsOpts::default(),
            &server.host,
            &[],
            None,
            fixture.roots(),
        )
        .unwrap();
        let stack = Stack::new(
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
            server,
            Some(tls),
            None,
        );
        let err = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stack.open(&ConnectOpts::default()),
        )
        .await
        .expect("the peer answers or closes")
        .err()
        .expect("TLS against a plain server fails");
        assert!(matches!(err, OutboundError::Tls(_)), "{err}");
    }
}
```

（`Stack` 没有需要单独 RED 的逻辑——它是三次调用的固定顺序；用例钉的是"顺序对、ALPN 缺省为空、哪一层失败说得清"。）

Run: `cargo test -p rurge-proto transport::stack`
Expected: PASS。

- [ ] **Step 6: 门禁，提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto
git commit -m "feat(proto): WebSocket 字节流（tokio-tungstenite）、传输阶梯 transport::Stack 与回环假 WebSocket 服务端"
```

---

### Task 3: `addr::socks_addr`、`LazyHead`、`TrojanOutbound` 与 `FakeTrojan`

`LazyHead` 的读写逻辑写计划时已在临时工程里编译并跑通（先写 → 头与首段负载同一次写出；读先挂起、写随后到 → 仍然合并，且写会唤醒挂起的读；一直不写 → 宽限期后头单独发出；先关 → 头仍然发出）。见 P14。

**Files:**
- Create: `crates/rurge-proto/src/addr.rs`
- Create: `crates/rurge-proto/src/transport/lazy_head.rs`
- Create: `crates/rurge-proto/src/trojan.rs`
- Create: `crates/rurge-proto/src/testing/trojan.rs`
- Modify: `crates/rurge-proto/src/lib.rs`、`crates/rurge-proto/src/socks5.rs`、`crates/rurge-proto/src/transport/mod.rs`、`crates/rurge-proto/src/testing/mod.rs`

**Interfaces:**
- Consumes: `Stack`、`WsClient`（Task 2）；`rurge_config::spec::trojan::read_trojan`、`TrojanSpec`（Task 1）；`crate::build::tls_client(opts: Option<&TlsOpts>, server: &HostName, default_alpn: &[&str], keystore: &[KeystoreItem], roots: Arc<RootCertStore>) -> Result<Option<TlsClient>, BuildError>`；`crate::hostname::to_ascii`；`testing::ws::accept_bytes`。
- Produces:
  - `rurge_proto::trojan::TrojanOutbound::new(name: &str, server: Target, spec: &TrojanSpec, keystore: &[KeystoreItem], roots: Arc<RootCertStore>, connector: Arc<dyn Connector>) -> Result<TrojanOutbound, BuildError>`（实现 `Outbound`）
  - `rurge_proto::testing::{FakeTrojan, TrojanScript, RecordedTrojan}`
  - `rurge_proto::transport::lazy_head::{LazyHead, HEAD_GRACE}`：`LazyHead::new(inner: BoxedStream, head: Vec<u8>) -> LazyHead`、`LazyHead::with_grace(inner, head, grace: Duration) -> LazyHead`

- [ ] **Step 1: `addr::socks_addr`，并让 socks5 改用它**

新建 `crates/rurge-proto/src/addr.rs`：

```rust
//! `ATYP ADDR PORT` as SOCKS5 writes it (RFC 1928 §5). Trojan uses the same
//! encoding.

use rurge_config::HostName;
use rurge_net::connector::Target;
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddrError {
    /// The name is not one a proxy request may carry (`hostname::to_ascii`).
    Unsendable,
    /// Longer than the one-byte length allows.
    TooLong,
}

/// A domain goes out by name (remote resolution), an IDN as its A-labels.
pub(crate) fn socks_addr(target: &Target) -> Result<Vec<u8>, AddrError> {
    let mut out = Vec::new();
    match &target.host {
        HostName::Ip(IpAddr::V4(v4)) => {
            out.push(1);
            out.extend_from_slice(&v4.octets());
        }
        HostName::Ip(IpAddr::V6(v6)) => {
            out.push(4);
            out.extend_from_slice(&v6.octets());
        }
        HostName::Domain(name) => {
            let name = crate::hostname::to_ascii(name).ok_or(AddrError::Unsendable)?;
            let len = u8::try_from(name.len()).map_err(|_| AddrError::TooLong)?;
            out.push(3);
            out.push(len);
            out.extend_from_slice(name.as_bytes());
        }
    }
    out.extend_from_slice(&target.port.to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_address_types() {
        let t = |host: &str| Target::new(HostName::parse(host), 0x1f90);
        assert_eq!(socks_addr(&t("10.1.2.3")).unwrap(), [1, 10, 1, 2, 3, 0x1f, 0x90]);
        let v6 = socks_addr(&t("2001:db8::1")).unwrap();
        assert_eq!((v6[0], v6.len()), (4, 1 + 16 + 2));
        let mut expected = vec![3, 21];
        expected.extend_from_slice(b"xn--bcher-kva.example");
        expected.extend_from_slice(&[0x1f, 0x90]);
        assert_eq!(socks_addr(&t("bücher.example")).unwrap(), expected);
        assert_eq!(
            socks_addr(&Target::new(HostName::Domain("a@b.test".into()), 1)),
            Err(AddrError::Unsendable)
        );
        assert_eq!(
            socks_addr(&Target::new(HostName::Domain("a".repeat(256)), 1)),
            Err(AddrError::TooLong)
        );
    }
}
```

`crates/rurge-proto/src/lib.rs` 加 `mod addr;`。

`crates/rurge-proto/src/socks5.rs` 的 `connect_request` 换成（错误文本一字不变；该文件里因此不再用到的 `use`——例如 `std::net::IpAddr`、`HostName`——若编译器报未使用就删掉，仍被别处用到就留着）：

```rust
fn connect_request(target: &Target) -> Result<Vec<u8>, OutboundError> {
    let mut request = vec![VERSION, 1, 0];
    request.extend(crate::addr::socks_addr(target).map_err(|e| match e {
        // the proxy resolves the name (remote resolution); an IDN goes out as A-labels
        crate::addr::AddrError::Unsendable => {
            proxy("the host name cannot be sent to a SOCKS5 proxy")
        }
        crate::addr::AddrError::TooLong => proxy("the host name is longer than 255 bytes"),
    })?);
    Ok(request)
}
```

Run: `cargo test -p rurge-proto addr && cargo test -p rurge-proto socks5`
Expected: PASS（socks5 的全部现有用例不变）。

- [ ] **Step 2: `LazyHead`——先写用例**

新建 `crates/rurge-proto/src/transport/lazy_head.rs`，先放类型与**桩**：`HEAD_GRACE` 常量、`new` / `with_grace` 存字段，`AsyncRead` / `AsyncWrite` 全部直接转给 `inner`（即永远不发请求头）；`transport/mod.rs` 加 `pub mod lazy_head;`。用例：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    /// Accepts one connection; returns (what the first read got, the rest).
    /// Says `banner` right after the first read.
    async fn peer() -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<(Vec<u8>, Vec<u8>)>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut first = vec![0u8; 256];
            let n = tcp.read(&mut first).await.unwrap();
            first.truncate(n);
            // the client may already be gone
            let _ = tcp.write_all(b"banner").await;
            let mut rest = Vec::new();
            let _ = tcp.read_to_end(&mut rest).await;
            (first, rest)
        });
        (addr, task)
    }

    #[tokio::test]
    async fn the_head_leaves_with_the_first_payload() {
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut lazy = LazyHead::new(Box::new(tcp), b"HEAD|".to_vec());
        lazy.write_all(b"payload").await.unwrap();
        let mut banner = [0u8; 6];
        lazy.read_exact(&mut banner).await.unwrap();
        lazy.write_all(b"+more").await.unwrap();
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, rest) = task.await.unwrap();
        assert_eq!(first, b"HEAD|payload", "one write, so one segment");
        assert_eq!(rest, b"+more");
    }

    #[tokio::test]
    async fn a_read_that_is_already_pending_does_not_send_the_head_alone() {
        // what a relay does: the read half is polled first, the payload follows
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        // a grace far longer than the test: only the writer can release the reader
        let lazy = LazyHead::with_grace(Box::new(tcp), b"HEAD|".to_vec(), Duration::from_secs(30));
        let (mut rd, mut wr) = tokio::io::split(lazy);
        let reader = tokio::spawn(async move {
            let mut banner = [0u8; 6];
            rd.read_exact(&mut banner).await.unwrap();
            (rd, banner)
        });
        // lets the reader park itself first; whichever half runs first, the
        // result below must be the same
        tokio::time::sleep(Duration::from_millis(20)).await;
        wr.write_all(b"payload").await.unwrap();
        let (rd, banner) = tokio::time::timeout(Duration::from_secs(5), reader)
            .await
            .expect("the write released the parked reader, not the 30 s timer")
            .unwrap();
        assert_eq!(&banner, b"banner");
        let mut lazy = rd.unsplit(wr);
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, _) = task.await.unwrap();
        assert_eq!(first, b"HEAD|payload", "coalesced although a read was pending");
    }

    #[tokio::test]
    async fn an_application_that_stays_silent_gets_the_head_out_after_the_grace() {
        // a server-speaks-first protocol (SSH, SMTP): without this the two
        // sides would wait for each other forever
        let (addr, task) = peer().await;
        let tcp = TcpStream::connect(addr).await.unwrap();
        let grace = Duration::from_millis(50);
        let mut lazy = LazyHead::with_grace(Box::new(tcp), b"HEAD|".to_vec(), grace);
        let started = std::time::Instant::now();
        let mut banner = [0u8; 6];
        tokio::time::timeout(Duration::from_secs(5), lazy.read_exact(&mut banner))
            .await
            .expect("the head went out, so the peer answered")
            .unwrap();
        assert_eq!(&banner, b"banner");
        assert!(started.elapsed() >= grace, "the grace was honoured");
        lazy.write_all(b"later").await.unwrap();
        lazy.shutdown().await.unwrap();
        drop(lazy);
        let (first, rest) = task.await.unwrap();
        assert_eq!(
            (first.as_slice(), rest.as_slice()),
            (&b"HEAD|"[..], &b"later"[..])
        );
    }

    #[tokio::test]
    async fn a_shutdown_before_anything_else_still_sends_the_head() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut all = Vec::new();
            tcp.read_to_end(&mut all).await.unwrap();
            all
        });
        let tcp = TcpStream::connect(addr).await.unwrap();
        let mut lazy = LazyHead::new(Box::new(tcp), b"HEAD|".to_vec());
        lazy.shutdown().await.unwrap();
        drop(lazy);
        assert_eq!(server.await.unwrap(), b"HEAD|");
    }
}
```

第二个用例里的 20 ms 不是同步手段：无论读写两半谁先被轮询，结果都必须是"头与首段负载一次写出"；它只是让"读先挂起"这一支更容易被走到。

Run: `cargo test -p rurge-proto transport::lazy_head`
Expected: FAIL（桩从不发请求头：第一、二个用例得到 `payload`，第三个在 5 秒上界处超时，第四个得到空）。

- [ ] **Step 3: 实现 `LazyHead`**

下面这份实现写计划时已在临时工程里编译并跑通上面四种情形（P14）：

```rust
//! A protocol's request head held back until the first payload write, so
//! both leave in one write: a lone head-sized first record is a known
//! traffic signature.
//!
//! A relay polls the read side the moment the tunnel exists, long before the
//! client's first bytes arrive, so a read must not send the head at once.
//! It waits `HEAD_GRACE` for a write instead; only an application that stays
//! silent that long (a server-speaks-first protocol: SSH, SMTP) gets the head
//! sent on its own. The write that sends the head wakes a reader parked on
//! the timer, so the grace never delays a response.

use rurge_net::connector::BoxedStream;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, Waker, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::time::Sleep;

/// How long a read waits for the first payload before the head goes out alone.
pub const HEAD_GRACE: Duration = Duration::from_millis(100);

pub struct LazyHead {
    inner: BoxedStream,
    head: Option<Vec<u8>>,
    written: usize,
    coalesced: usize,
    grace: Duration,
    /// Armed by the first read that finds the head unsent.
    timer: Option<Pin<Box<Sleep>>>,
    /// A reader parked on the timer; woken as soon as a write sends the head.
    reader: Option<Waker>,
}

impl LazyHead {
    pub fn new(inner: BoxedStream, head: Vec<u8>) -> LazyHead {
        LazyHead::with_grace(inner, head, HEAD_GRACE)
    }

    pub fn with_grace(inner: BoxedStream, head: Vec<u8>, grace: Duration) -> LazyHead {
        LazyHead {
            inner,
            head: Some(head),
            written: 0,
            coalesced: 0,
            grace,
            timer: None,
            reader: None,
        }
    }

    fn poll_head(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while let Some(head) = &self.head {
            if self.written == head.len() {
                self.head = None;
                self.timer = None;
                if let Some(reader) = self.reader.take() {
                    reader.wake();
                }
                break;
            }
            let n = ready!(Pin::new(&mut self.inner).poll_write(cx, &head[self.written..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.written += n;
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for LazyHead {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            if self.written == 0 {
                // nothing sent yet: give the first payload a moment to arrive
                let grace = self.grace;
                let timer = self
                    .timer
                    .get_or_insert_with(|| Box::pin(tokio::time::sleep(grace)));
                if timer.as_mut().poll(cx).is_pending() {
                    self.reader = Some(cx.waker().clone());
                    return Poll::Pending;
                }
            }
            ready!(self.poll_head(cx))?;
            ready!(Pin::new(&mut self.inner).poll_flush(cx))?;
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for LazyHead {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.head.is_none() {
            return Pin::new(&mut self.inner).poll_write(cx, data);
        }
        if self.written == 0 && self.coalesced == 0 {
            if let Some(head) = &mut self.head {
                head.extend_from_slice(data);
            }
            self.coalesced = data.len();
        }
        ready!(self.poll_head(cx))?;
        let n = self.coalesced.min(data.len());
        self.coalesced = 0;
        if n == 0 {
            return Pin::new(&mut self.inner).poll_write(cx, data);
        }
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            ready!(self.poll_head(cx))?;
        }
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.head.is_some() {
            ready!(self.poll_head(cx))?;
        }
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
```

Run: `cargo test -p rurge-proto transport::lazy_head`
Expected: PASS（4 个用例）。

- [ ] **Step 4: `FakeTrojan`**

新建 `crates/rurge-proto/src/testing/trojan.rs`：

```rust
//! A scriptable Trojan server: TLS, optionally a WebSocket below the
//! protocol, the request head, then a relay. It never resolves a name.

use super::ws::{RecordedWs, accept_bytes};
use super::{AbortOnDrop, TlsFixture};
use rurge_net::connector::BoxedStream;
use sha2::{Digest, Sha224};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct TrojanScript {
    pub password: String,
    /// Expect a WebSocket handshake between TLS and the request head.
    pub ws: bool,
    /// Relay here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedTrojan {
    pub command: u8,
    pub atyp: u8,
    /// An IP literal, or the name exactly as it was on the wire.
    pub host: String,
    pub port: u16,
    /// Payload that arrived in the same read as the end of the head.
    pub early: Vec<u8>,
}

pub struct FakeTrojan {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedTrojan>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    connections: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

/// `Some((request, bytes consumed))` once `buf` holds a whole head after the
/// 58-byte `hash CRLF` prefix.
fn parse_request(buf: &[u8]) -> Option<(RecordedTrojan, usize)> {
    let (command, atyp) = (*buf.first()?, *buf.get(1)?);
    let (host, used) = match atyp {
        1 => {
            let b: [u8; 4] = buf.get(2..6)?.try_into().ok()?;
            (IpAddr::V4(Ipv4Addr::from(b)).to_string(), 6)
        }
        4 => {
            let b: [u8; 16] = buf.get(2..18)?.try_into().ok()?;
            (IpAddr::V6(Ipv6Addr::from(b)).to_string(), 18)
        }
        _ => {
            let len = usize::from(*buf.get(2)?);
            let name = buf.get(3..3 + len)?;
            (String::from_utf8_lossy(name).into_owned(), 3 + len)
        }
    };
    let port = u16::from_be_bytes(buf.get(used..used + 2)?.try_into().ok()?);
    // the closing CRLF
    buf.get(used + 2..used + 4)?;
    Some((
        RecordedTrojan {
            command,
            atyp,
            host,
            port,
            early: Vec::new(),
        },
        used + 4,
    ))
}

struct Shared {
    script: TrojanScript,
    requests: Arc<Mutex<Vec<RecordedTrojan>>>,
    ws_seen: Arc<Mutex<Vec<RecordedWs>>>,
    rejected: Arc<AtomicUsize>,
}

async fn serve(mut stream: BoxedStream, shared: Arc<Shared>) -> io::Result<()> {
    if shared.script.ws {
        stream = accept_bytes(stream, &shared.ws_seen).await?;
    }
    let expected: String = Sha224::digest(shared.script.password.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let (mut request, consumed) = loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() < 58 {
            continue;
        }
        if &buf[..56] != expected.as_bytes() || &buf[56..58] != b"\r\n" {
            // what a real server's fallback site would say
            shared.rejected.fetch_add(1, Ordering::SeqCst);
            stream
                .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .await?;
            return stream.shutdown().await;
        }
        if let Some((request, used)) = parse_request(&buf[58..]) {
            break (request, 58 + used);
        }
    };
    request.early = buf[consumed..].to_vec();
    shared.requests.lock().expect("requests").push(request.clone());
    let upstream_addr = match shared.script.connect_to {
        Some(addr) => addr,
        None => match request.host.parse::<IpAddr>() {
            Ok(ip) => SocketAddr::new(ip, request.port),
            // never resolves: a name without `connect_to` is a dead end
            Err(_) => return stream.shutdown().await,
        },
    };
    let mut upstream = TcpStream::connect(upstream_addr).await?;
    upstream.write_all(&request.early).await?;
    let _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream).await;
    Ok(())
}

impl FakeTrojan {
    pub async fn spawn(script: TrojanScript, fixture: Arc<TlsFixture>) -> FakeTrojan {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requests: Arc<Mutex<Vec<RecordedTrojan>>> = Arc::default();
        let ws_seen: Arc<Mutex<Vec<RecordedWs>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            script,
            requests: requests.clone(),
            ws_seen: ws_seen.clone(),
            rejected: rejected.clone(),
        });
        let acceptor = fixture.acceptor(false);
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                count.fetch_add(1, Ordering::SeqCst);
                let (shared, fixture, acceptor) =
                    (shared.clone(), fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    let _ = serve(stream, shared).await;
                });
            }
        });
        FakeTrojan {
            addr,
            requests,
            ws_seen,
            connections,
            rejected,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn requests(&self) -> Vec<RecordedTrojan> {
        self.requests.lock().expect("requests").clone()
    }

    pub fn ws_seen(&self) -> Vec<RecordedWs> {
        self.ws_seen.lock().expect("ws").clone()
    }

    /// TCP connections accepted so far (before TLS).
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Connections answered like a web server because the hash was wrong.
    pub fn rejected(&self) -> usize {
        self.rejected.load(Ordering::SeqCst)
    }
}
```

`testing/mod.rs`：加 `mod trojan;` 与 `pub use trojan::{FakeTrojan, RecordedTrojan, TrojanScript};`。

- [ ] **Step 5: `TrojanOutbound`——先写用例**

新建 `crates/rurge-proto/src/trojan.rs`，先放类型与**桩**（`new` 照 Step 6 写完整——它没有可"桩"的逻辑；`connect_tcp` 的桩只做 `self.stack.open(opts)`，不发请求头），`lib.rs` 加 `pub mod trojan;`。测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeTrojan, SeenHandshake, TlsFixture, TrojanScript, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::trojan::read_trojan;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// The outbound for `definition` (a `trojan, host, port, ...` line).
    fn outbound(definition: &str, fixture: &Arc<TlsFixture>) -> TrojanOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("T", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_trojan(&mut r, &[]);
        assert!(!r.has_errors(), "{:?}", r.finish());
        TrojanOutbound::new(
            "T",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn fake(password: &str, ws: bool, connect_to: Option<SocketAddr>) -> (Arc<TlsFixture>, FakeTrojan) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeTrojan::spawn(
            TrojanScript {
                password: password.to_string(),
                ws,
                connect_to,
            },
            fixture.clone(),
        )
        .await;
        (fixture, fake)
    }

    #[test]
    fn the_wire_form_of_a_password_is_the_known_answer() {
        assert_eq!(
            std::str::from_utf8(&wire_hash("password")).unwrap(),
            "d63dc919e201d7bc4c825630d2cf25fdc93d4b2f0d46706d29038d01"
        );
    }

    #[tokio::test]
    async fn the_head_rides_with_the_first_payload() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        assert_eq!(out.name(), "T");
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"first payload").await.unwrap();
        let mut buf = [0u8; 13];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"first payload");
        let seen = fake.requests();
        assert_eq!(
            (seen[0].command, seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (1, 1, "127.0.0.1", echo.port())
        );
        assert_eq!(seen[0].early, b"first payload", "one TLS record, not two");
        assert!(out.http_forward().is_none());
    }

    #[tokio::test]
    async fn a_target_that_speaks_first_is_reached_without_a_write() {
        // accepts, greets, then echoes
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let greeter = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let _ = tcp.write_all(b"220 hi\r\n").await;
                    let mut buf = [0u8; 64];
                    while let Ok(n) = tcp.read(&mut buf).await {
                        if n == 0 || tcp.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        let (fixture, fake) = fake("pw", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(greeter), &ConnectOpts::default())
            .await
            .unwrap();
        let mut banner = [0u8; 8];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut banner))
            .await
            .expect("the head went out although nothing was written")
            .unwrap();
        assert_eq!(&banner, b"220 hi\r\n");
        assert!(fake.requests()[0].early.is_empty());
    }

    #[tokio::test]
    async fn names_go_out_as_a_labels_and_an_unsendable_name_never_dials() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", false, Some(echo)).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=pw", fake.addr().port()),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(
                &Target::new(HostName::Domain("bücher.example".into()), 443),
                &ConnectOpts::default(),
            )
            .await
            .unwrap();
        stream.write_all(b"x").await.unwrap();
        let mut one = [0u8; 1];
        stream.read_exact(&mut one).await.unwrap();
        let seen = fake.requests();
        assert_eq!(
            (seen[0].atyp, seen[0].host.as_str(), seen[0].port),
            (3, "xn--bcher-kva.example", 443)
        );
        let before = fake.connections();
        for (name, expected) in [
            ("a@b.test".to_string(), "trojan: the host name cannot be sent to the server"),
            ("a".repeat(256), "trojan: the host name is longer than 255 bytes"),
        ] {
            let err = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name), 443),
                    &ConnectOpts::default(),
                )
                .await
                .err()
                .expect("refused");
            assert!(matches!(&err, OutboundError::Proxy(m) if m == expected), "{err}");
        }
        assert_eq!(fake.connections(), before, "nothing was dialled");
    }

    #[tokio::test]
    async fn a_wrong_password_cannot_be_told_at_connect_time() {
        // the protocol has no reply: the server treats us like a stray web
        // client, and that only shows once the relay starts
        let echo = echo_server().await;
        let (fixture, fake) = fake("right", false, None).await;
        let out = outbound(
            &format!("trojan, 127.0.0.1, {}, password=wrong", fake.addr().port()),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .expect("connecting succeeds");
        stream.write_all(b"hello").await.unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut answer))
            .await
            .expect("the server closes")
            .unwrap();
        assert!(answer.starts_with(b"HTTP/1.1 400"), "{}", String::from_utf8_lossy(&answer));
        assert_eq!((fake.rejected(), fake.requests().len()), (1, 0));
    }

    #[tokio::test]
    async fn over_websocket() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", true, None).await;
        let out = outbound(
            &format!(
                "trojan, 127.0.0.1, {}, password=pw, ws=true, ws-path=/t, ws-headers=Host:edge.test|X-K:v",
                fake.addr().port()
            ),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"inside a frame").await.unwrap();
        stream.flush().await.unwrap();
        let mut buf = [0u8; 14];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"inside a frame");
        let ws = fake.ws_seen();
        assert_eq!(ws[0].path, "/t");
        assert_eq!((ws[0].header("host"), ws[0].header("x-k")), (Some("edge.test"), Some("v")));
        assert_eq!(fake.requests()[0].early, b"inside a frame");
    }

    #[tokio::test]
    async fn the_tls_parameters_apply_and_there_is_no_alpn_by_default() {
        let echo = echo_server().await;
        let (fixture, fake) = fake("pw", false, None).await;
        let port = fake.addr().port();
        for (extra, expected) in [
            ("", SeenHandshake { sni: None, alpn: None, client_cert: false }),
            (
                ", sni=front.example, server-cert-verify-name=127.0.0.1, alpn=http/1.1",
                SeenHandshake {
                    sni: Some("front.example".into()),
                    alpn: Some("http/1.1".into()),
                    client_cert: false,
                },
            ),
        ] {
            let out = outbound(&format!("trojan, 127.0.0.1, {port}, password=pw{extra}"), &fixture);
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            stream.write_all(b"x").await.unwrap();
            let mut one = [0u8; 1];
            stream.read_exact(&mut one).await.unwrap();
            assert_eq!(fixture.seen().last(), Some(&expected), "{extra}");
        }
    }

    #[tokio::test]
    async fn one_budget_covers_the_whole_ladder() {
        // accepts TCP and then says nothing: the TLS handshake never ends
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let silent = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((tcp, _)) = listener.accept().await {
                held.push(tcp);
            }
        });
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let out = outbound(&format!("trojan, 127.0.0.1, {}, password=pw", silent.port()), &fixture);
        let started = std::time::Instant::now();
        let err = out
            .connect_tcp(
                &target(silent),
                &ConnectOpts {
                    timeout: Duration::from_millis(300),
                },
            )
            .await
            .err()
            .expect("times out");
        assert!(matches!(err, OutboundError::Timeout), "{err}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
```

Run: `cargo test -p rurge-proto trojan::`
Expected: FAIL——桩不发请求头：`the_head_rides_with_the_first_payload` 里假服务端把 `first payload` 当成口令哈希，回 400；`wire_hash` 尚不存在时先让它返回全零（桩），已知答案用例随之失败。

- [ ] **Step 6: 实现 `TrojanOutbound`**

`crates/rurge-proto/src/trojan.rs` 的非测试部分：

```rust
//! `trojan` outbound (manual: Policies › Trojan): TLS (optionally a
//! WebSocket), then `hex(SHA224(password)) CRLF CMD ATYP ADDR PORT CRLF` and
//! the payload. The server never answers the head: a wrong password only
//! shows once the relay starts, as whatever the server's fallback site says.

use crate::addr::{AddrError, socks_addr};
use crate::build::tls_client;
use crate::transport::Stack;
use crate::transport::lazy_head::LazyHead;
use crate::transport::ws::WsClient;
use crate::{BuildError, Outbound, OutboundError};
use rurge_config::KeystoreItem;
use rurge_config::spec::TrojanSpec;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use sha2::{Digest, Sha224};
use std::sync::Arc;

const CONNECT: u8 = 1;

/// No `Debug`: the hash is as good as the password.
pub struct TrojanOutbound {
    name: String,
    stack: Stack,
    /// `hex(SHA224(password))`, what the wire carries; the password itself is not kept.
    hash: [u8; 56],
}

fn wire_hash(password: &str) -> [u8; 56] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha224::digest(password.as_bytes());
    let mut out = [0u8; 56];
    for (i, byte) in digest.iter().enumerate() {
        out[2 * i] = HEX[usize::from(byte >> 4)];
        out[2 * i + 1] = HEX[usize::from(byte & 15)];
    }
    out
}

impl TrojanOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &TrojanSpec,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TrojanOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and the
        // dry build both prefix it
        if spec.password.is_empty() {
            return Err(BuildError::new("`password` is empty"));
        }
        // no ALPN unless the policy asks for one: a WebSocket below must not
        // be negotiated into h2 (M2 design 4.5)
        let tls = tls_client(Some(&spec.tls), &server.host, &[], keystore, roots)?;
        let ws = spec
            .ws
            .as_ref()
            .map(|ws| WsClient::new(ws, &server, true))
            .transpose()?;
        Ok(TrojanOutbound {
            name: name.to_string(),
            stack: Stack::new(connector, server, tls, ws),
            hash: wire_hash(&spec.password),
        })
    }

    fn head(&self, target: &Target) -> Result<Vec<u8>, OutboundError> {
        let addr = socks_addr(target).map_err(|e| {
            OutboundError::Proxy(
                match e {
                    AddrError::Unsendable => "trojan: the host name cannot be sent to the server",
                    AddrError::TooLong => "trojan: the host name is longer than 255 bytes",
                }
                .to_string(),
            )
        })?;
        let mut head = Vec::with_capacity(56 + 2 + 1 + addr.len() + 2);
        head.extend_from_slice(&self.hash);
        head.extend_from_slice(b"\r\n");
        head.push(CONNECT);
        head.extend_from_slice(&addr);
        head.extend_from_slice(b"\r\n");
        Ok(head)
    }
}

impl Outbound for TrojanOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // never dial for a target whose name cannot be sent
            let head = self.head(target)?;
            // one budget for the connection, TLS and the WebSocket handshake
            let stream = match tokio::time::timeout(opts.timeout, self.stack.open(opts)).await {
                Ok(result) => result?,
                Err(_) => return Err(OutboundError::Timeout),
            };
            Ok(Box::new(LazyHead::new(stream, head)) as BoxedStream)
        })
    }
}
```

`tls_client` 对 `Some(opts)` 总是返回 `Ok(Some(_))`，所以 `Stack` 拿到的 `tls` 一定是 `Some`。构建错误的文本不带策略名：`rurge-policy` 的 `build_one`（`policy `<name>`: …`）与干构建（`policy `<name>` cannot be built: …`）各自统一加前缀，出站自己再加就会重复。

Run: `cargo test -p rurge-proto trojan::`
Expected: PASS（8 个用例）。

- [ ] **Step 7: 门禁，提交**

```bash
git add crates/rurge-proto
git commit -m "feat(proto): trojan 出站（惰性请求头、SOCKS 风格地址的公共编码）与回环假 Trojan 服务端"
```


---

### Task 4: 换轨——`ProtoSpec::Trojan`、`to_spec` 接线、`EngineFactory` 分支与端到端用例

这个提交之前，trojan 策略没有 spec，注册表把它当"未实现"（REJECT）；这个提交之后，它是一个真实出站。三处改动必须同一个提交落地（P6）：`ProtoSpec` 加变体会让 `EngineFactory::build` 的穷举匹配编译不过，而 spec 一旦存在注册表就会去调工厂。

**Files:**
- Modify: `crates/rurge-config/src/spec/mod.rs`（`ProtoSpec::Trojan`、`to_spec` 的分支、`check_underlying` 的适用条件）
- Modify: `crates/rurge-config/tests/policy_spec.rs`（把 Task 1 的"没有 spec"用例翻过来）
- Modify: `crates/rurge-engine/src/outbounds.rs`（工厂分支、`tls_of`、单元用例）
- Modify: `crates/rurge-engine/tests/outbounds.rs`（三个端到端用例）
- Modify: `crates/rurge-engine/tests/pipeline.rs`（防环用例）

**Interfaces:**
- Consumes: `read_trojan`（Task 1）；`TrojanOutbound::new`（Task 3）；`rurge_proto::testing::{FakeTrojan, TrojanScript, TlsFixture}`；`crates/rurge-engine/tests/outbounds.rs` 里现成的 `harness(Profile{..})`、`connect_via_http`、`get`、`plain_get`、`origin_addr`、`wait_until`；`tests/pipeline.rs` 里现成的 `engine_from_profile(dir, profile)` 与 `internal_sessions(&engine)`。
- Produces: `rurge_config::spec::ProtoSpec::Trojan(TrojanSpec)`；trojan 策略经 `EngineFactory` 构建成 `TrojanOutbound`。

- [ ] **Step 1: 配置层——先改用例**

`crates/rurge-config/tests/policy_spec.rs`：把 Task 1 加的 `a_trojan_policy_has_no_spec_until_the_outbound_is_wired_in` **整个替换**为：

```rust
#[test]
fn a_trojan_policy_is_typed() {
    let loaded = load(
        "T = trojan, t.example, 443, password=p, ws=true, ws-path=/w, sni=front.example, underlying-proxy=E\nE = socks5, e.example, 1080\nBad = trojan, b.example, 443",
        "",
    );
    let errors: Vec<String> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.message.clone())
        .collect();
    assert_eq!(errors, ["policy `Bad`: `password` is required"]);
    let spec = loaded.config.spec("T").expect("typed");
    let ProtoSpec::Trojan(trojan) = &spec.proto else {
        panic!("{:?}", spec.proto)
    };
    assert_eq!(trojan.password, "p");
    assert_eq!(trojan.ws.as_ref().unwrap().path, "/w");
    assert_eq!(spec.common.underlying_proxy.as_deref(), Some("E"));
    assert!(loaded.config.spec("Bad").is_none(), "an error drops the spec");
}

#[test]
fn a_trojan_policy_takes_part_in_the_chain_checks() {
    let loaded = load(
        "A = trojan, a.example, 443, password=p, underlying-proxy=B\nB = trojan, b.example, 443, password=p, underlying-proxy=A",
        "",
    );
    assert!(codes_of(&loaded, Severity::Error).contains(&codes::E_UNDERLYING_PROXY_CYCLE));
    let loaded = load("A = trojan, a.example, 443, password=p, underlying-proxy=Ghost", "");
    assert_eq!(codes_of(&loaded, Severity::Error), [codes::E_UNKNOWN_POLICY_REF]);
}
```

Run: `cargo test -p rurge-config --test policy_spec a_trojan`
Expected: FAIL（`spec("T")` 是 `None`；`Ghost` 没有被报告）。

- [ ] **Step 2: 配置层——`ProtoSpec::Trojan` 与 `to_spec`**

`crates/rurge-config/src/spec/mod.rs`：

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtoSpec {
    Direct,
    Reject(Builtin),
    Http(HttpSpec),
    Socks5(Socks5Spec),
    Trojan(TrojanSpec),
}
```

`to_spec` 的 `match policy.kind` 里，在 `PolicyKind::Socks5 | PolicyKind::Socks5Tls => { … }` 之后、`_ => return SpecOutcome::default(),` 之前加：

```rust
        PolicyKind::Trojan => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            tls::note_shadow_tls(&mut r, &mut notes);
            let trojan = trojan::read_trojan(&mut r, env.keystore);
            (common, ProtoSpec::Trojan(trojan))
        }
```

把 `check_underlying` 的适用条件从"列举代理协议"改成"不是别名"（以后每加一种协议不必再改这里）：

```rust
    if !matches!(proto, ProtoSpec::Direct | ProtoSpec::Reject(_)) {
        check_underlying(&mut r, &mut common, env);
    }
```

（原来是 `if matches!(proto, ProtoSpec::Http(_) | ProtoSpec::Socks5(_))`。`Direct` 分支里 `read_common` 已经把 `underlying_proxy` 清成 `None`，所以这一改对别名没有可见变化。）

此时 `cargo check --workspace` 会在 `rurge-engine` 报 `ProtoSpec::Trojan` 未覆盖——Step 3 修。

- [ ] **Step 3: 引擎工厂——先写用例再加分支**

`crates/rurge-engine/src/outbounds.rs` 的测试模块：把 `every_m1_protocol_builds` 的 profile 里加一行 `T = trojan, proxy.test, 443, password=pw, ws=true, ws-path=/x`（紧接 `ST` 那一行之后），列表里加 `("T", "T")`；并新增一个用例：

```rust
    #[test]
    fn a_trojan_policy_that_cannot_be_built_is_a_load_error() {
        let cfg = config(
            "[Proxy]\nT = trojan, proxy.test, 443, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("policy `T` cannot be built: keystore item `cert1`"),
            "{}",
            messages[0]
        );
        assert!(!messages[0].contains("hunter2") && !messages[0].contains("s3same0pen"));
    }
```

（最后一条断言防的是将来有人把口令拼进构建错误；这条错误文本是关于 keystore 的，今天本来就不含口令。）

Run: `cargo test -p rurge-engine --lib outbounds`
Expected: 编译失败（`ProtoSpec::Trojan(_)` not covered）——这就是本步的 RED。

实现：`use` 里加 `use rurge_proto::trojan::TrojanOutbound;` 与 `use rurge_net::connector::Target;`（若 `Target` 已在作用域就不重复）。`tls_of` 加一个分支：

```rust
        ProtoSpec::Trojan(trojan) => Some(&trojan.tls),
```

`build` 的 `match &spec.proto` 加一个分支：

```rust
            ProtoSpec::Trojan(trojan) => {
                let (Some(host), Some(port)) = (&spec.server, spec.port) else {
                    return Err(BuildError::new("a trojan policy needs a server and a port"));
                };
                Arc::new(TrojanOutbound::new(
                    &spec.name,
                    Target::new(host.clone(), port),
                    trojan,
                    &self.keystore,
                    self.roots.clone(),
                    connector,
                )?)
            }
```

（`skip-cert-verify` 的 WARN 走 `skips_verification` → `tls_of`，自动覆盖 trojan。）

Run: `cargo test -p rurge-config && cargo test -p rurge-engine --lib outbounds`
Expected: PASS。

- [ ] **Step 4: 端到端——经 trojan 上游的 CONNECT、明文请求与链**

`crates/rurge-engine/tests/outbounds.rs`：`use` 行加 `FakeTrojan, TlsFixture, TrojanScript`（与现有的 `rurge_proto::testing::{…}` 合并），再加一个辅助函数与三个用例：

```rust
/// A loopback Trojan server relaying to `to`, and the policy parameters that
/// make rurge trust it: the harness cannot inject a test CA (the runtime
/// builds its factory with the system roots), so the leaf is pinned.
async fn trojan_upstream(ws: bool, to: SocketAddr) -> (FakeTrojan, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let pin: String = fixture
        .leaf_fingerprint()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let fake = FakeTrojan::spawn(
        TrojanScript {
            password: "s3same".into(),
            ws,
            connect_to: Some(to),
        },
        fixture,
    )
    .await;
    (fake, format!("password=s3same, server-cert-fingerprint-sha256={pin}"))
}

#[tokio::test]
async fn a_connect_leaves_through_a_trojan_upstream_with_the_name_unresolved() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!("T = trojan, 127.0.0.1, {}, {params}", upstream.addr().port()),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    let response = get(&mut tunnel, "target.test", "/hello").await;
    assert!(response.ends_with("hi there"), "{response}");
    let seen = upstream.requests();
    assert_eq!(
        (seen[0].command, seen[0].atyp, seen[0].host.as_str(), seen[0].port),
        (1, 3, "target.test", 8080),
        "the server resolves the name: rurge never looked it up"
    );
    // whether the head rode with the first payload depends on timing here
    // (`HEAD_GRACE`); `LazyHead`'s own tests pin that deterministically
    assert!(h.dns.queries().is_empty());
    drop(tunnel);
    let log = h.engine.request_log();
    wait_until("the session to finish", || !log.recent(10).is_empty()).await;
    let record = &log.recent(10)[0];
    assert_eq!(record.policy, ["T"]);
    assert!(record.error.is_none(), "{:?}", record.error);
}

#[tokio::test]
async fn a_plain_request_is_tunnelled_through_trojan_over_websocket() {
    // only an HTTP proxy takes a plain request in absolute form; everything
    // else gets a tunnel
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (upstream, params) = trojan_upstream(true, origin_addr(&origin)).await;
    let h = harness(Profile {
        proxies: &format!(
            "T = trojan, 127.0.0.1, {}, {params}, ws=true, ws-path=/tunnel, ws-headers=Host:edge.test",
            upstream.addr().port()
        ),
        rules: "DOMAIN,target.test,T",
        ..Profile::default()
    })
    .await;
    let response = plain_get(h.http(), "http://target.test:8080/hello", "target.test:8080").await;
    assert!(response.ends_with("hi there"), "{response}");
    let ws = upstream.ws_seen();
    assert_eq!((ws[0].path.as_str(), ws[0].header("host")), ("/tunnel", Some("edge.test")));
    assert_eq!(upstream.requests()[0].host, "target.test");
    assert_eq!(origin.hits("/hello"), 1);
}

#[tokio::test]
async fn a_trojan_exit_is_reached_through_a_socks5_entry_by_name() {
    let origin = TestServer::spawn().await;
    origin.set("/hello", "hi there");
    let (exit, params) = trojan_upstream(false, origin_addr(&origin)).await;
    let entry = FakeSocks5::spawn(Socks5Script {
        connect_to: Some(exit.addr()),
        ..Socks5Script::default()
    })
    .await;
    let h = harness(Profile {
        proxies: &format!(
            "Entry = socks5, 127.0.0.1, {}\nExit = trojan, exit.example, 443, {params}, underlying-proxy=Entry",
            entry.addr().port()
        ),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:8080").await;
    assert!(get(&mut tunnel, "target.test", "/hello").await.ends_with("hi there"));
    // the entry is asked for the exit's server by name; the exit for the target by name
    let first = &entry.requests()[0];
    assert_eq!((first.atyp, first.host.as_str(), first.port), (3, "exit.example", 443));
    assert_eq!(exit.requests()[0].host, "target.test");
    assert!(h.dns.queries().is_empty(), "nothing on this path is resolved locally");
}
```

`plain_get(proxy: SocketAddr, url: &str, host: &str) -> String`、`origin_addr(origin: &TestServer) -> SocketAddr` 与 `TestServer::hits(path)` 都是该文件 / `rurge_net::testing` 里现成的。

**RED 的取法**：`trojan_upstream` 与第一个用例在 Step 2 之前就写好并跑一次——那时 trojan 策略还没有 spec，注册表把它当 REJECT，`connect_via_http` 拿不到 `200`，用例失败；把这次输出放进报告。Step 2–3 之后三个用例一起转绿。

Run: `cargo test -p rurge-engine --test outbounds trojan`
Expected: PASS（三个用例）。

- [ ] **Step 5: 端到端——以域名配置的 trojan 命中 DNS 会话时的防环**

能力一旦变活，就回头核对设计承诺过的守卫（M1b 的 Critical 就漏在这一步）。`crates/rurge-engine/tests/pipeline.rs`，紧挨 `a_dns_session_bypasses_a_proxy_configured_by_host_name`：

```rust
/// The anti-loop guard is about where the socket is opened, not about the
/// protocol: a host-named trojan server is bypassed exactly like an http one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dns_session_bypasses_a_trojan_proxy_configured_by_host_name() {
    let dns = MockDns::spawn().await;
    dns.set("target.test", &["127.0.0.1"], &[], 60);
    dns.set("proxy.test", &["127.0.0.1"], &[], 60);
    let dir = tempfile::tempdir().unwrap();
    let profile = format!(
        "[General]\nhttp-listen = 127.0.0.1:0\nsocks5-listen = 127.0.0.1:0\n\
encrypted-dns-follow-outbound-mode = true\nencrypted-dns-server = tcp://127.0.0.1:{}\nipv6 = false\n\
[Proxy]\nUp = trojan, proxy.test, 443, password=pw\n[Proxy Group]\n[Rule]\nPROTOCOL,DNS,Up\nFINAL,DIRECT\n",
        dns.addr().port()
    );
    let engine = engine_from_profile(dir.path(), &profile).await;
    let res = tokio::time::timeout(
        Duration::from_secs(5),
        engine
            .runtime()
            .stack
            .resolver
            .lookup("target.test", rurge_dns::resolver::LookupOpts::default()),
    )
    .await
    .expect("the lookup must not wait for the proxy's own name to be resolved");
    assert!(res.is_ok(), "resolution through the pipeline: {res:?}");
    let internal = internal_sessions(&engine);
    assert!(
        internal.iter().any(|r| {
            r.error.as_deref()
                == Some("dns-follow: proxy configured by host name bypassed to avoid a resolution loop")
                && r.policy.first().map(String::as_str) == Some("Up")
        }),
        "{internal:?}"
    );
}
```

Run: `cargo test -p rurge-engine --test pipeline a_dns_session`
Expected: PASS（新用例连同原有的几个防环用例）。

- [ ] **Step 6: 语料库快照**

`tests/corpus/valid/kitchen-sink.conf` 第 61 行的 trojan 策略现在会得到 spec。跑 `cargo test -p rurge-config --test corpus`：若快照有差异，用 `cargo insta test -p rurge-config --review` 审阅——**只**接受与这条策略的类型化直接相关的变化（例如它带的某个参数现在产生 `W0029`）；其它差异停下来报告。同样跑 `cargo test -p rurge-rules`（`corpus_engine`）与 `cargo test -p rurge`（CLI 会对语料库跑 `check`：trojan 行现在要经过干构建，应当构建成功）。

- [ ] **Step 7: 门禁，提交**

```bash
git add crates/rurge-config crates/rurge-engine
git commit -m "feat(config,engine): ProtoSpec::Trojan 接入 to_spec 与 EngineFactory；经 trojan 上游的端到端用例（CONNECT、WebSocket、链、DNS 防环）"
```

---

### Task 5: bin——能力表翻转 `trojan` 与 CLI 用例

**Files:**
- Modify: `crates/rurge/src/capabilities.rs`
- Modify: `crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 4 之后 trojan 策略可构建。
- Produces: `capabilities::current().policy_kinds` 含 `PolicyKind::Trojan`；`rurge check` 对 trojan 策略不再报 `W0007`。

- [ ] **Step 1: 写失败的用例**

`crates/rurge/tests/cli.rs`，在 `check_knows_the_m1_protocols_and_runs_the_dry_build` 之后加：

```rust
const TROJAN: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=/w\nOld = ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=x\n[Rule]\nFINAL,DIRECT\n";
const TROJAN_BAD_PATH: &str = "[General]\n[Proxy]\nT = trojan, proxy.test, 443, password=s3same, ws=true, ws-path=s3cretpath\n[Rule]\nFINAL,DIRECT\n";

#[test]
fn check_knows_trojan() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "trojan.conf", TROJAN))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `ss` is still a later milestone; `trojan` is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(out.contains("`ss`") && !out.contains("`trojan`"), "{out}");

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", TROJAN_BAD_PATH))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains("bad.conf:3"))
        .stdout(predicate::str::contains("s3cretpath").not())
        .stdout(predicate::str::contains("s3same").not());
}
```

Run: `cargo test -p rurge --test cli check_knows_trojan`
Expected: FAIL（`W0007` 出现两次，且提到 `` `trojan` ``）。

- [ ] **Step 2: 翻转**

`crates/rurge/src/capabilities.rs`：文件头注释改成

```rust
//! What this build of rurge actually implements: the built-in alias
//! policies, the HTTP / SOCKS5 proxy family (phase 2 M1), `trojan`
//! (phase 2 M2a) and `select` groups.
```

`policy_kinds` 的集合里加一行 `PolicyKind::Trojan,`（放在 `PolicyKind::Socks5Tls,` 之后）。

Run: `cargo test -p rurge --test cli check_knows`
Expected: PASS（新用例与原来的 `check_knows_the_m1_protocols_and_runs_the_dry_build`）。

- [ ] **Step 3: 能力翻转时的守卫核对（写进报告，不改代码）**

能力表翻转是让死路径变活的时刻。逐条核对并在报告里各写一句"在哪、由哪个用例钉住"：

1. DNS 跟随出站模式的防环回退对 trojan 生效——Task 4 Step 5 的用例。
2. `skip-cert-verify` 的 WARN 对 trojan 生效——`EngineFactory::build` 经 `tls_of`；在 `crates/rurge-engine/src/outbounds.rs` 的测试模块里加一个断言用例：

```rust
    #[test]
    fn a_trojan_policy_that_skips_verification_is_noticed() {
        let cfg = config(
            "[Proxy]\nT = trojan, proxy.test, 443, password=pw, skip-cert-verify=true\n[Rule]\nFINAL,DIRECT\n",
        );
        assert!(skips_verification(cfg.spec("T").unwrap()));
    }
```

3. `GET /v1/policies/detail` 与 `lineHash` 对 trojan 策略不泄露口令、`ws-path`、`ws-headers`：`policy_detail` 就是 `redact_definition(definition)`，Task 1 Step 8 的精确断言已钉住；`lineHash` 在 `crates/rurge-engine/src/views.rs` 的 `line_hash_hides_credentials_but_changes_with_everything_else` 里再加一段：

```rust
        // trojan: the password, the WebSocket path and its headers are all secrets
        let trojan = config("A = trojan, t.test, 443, password=pw0rd, ws=true, ws-path=/s3cretpath, ws-headers=X-Key:k3y");
        let same_but_secrets = config("A = trojan, t.test, 443, password=other, ws=true, ws-path=/elsewhere, ws-headers=X-Key:zzz");
        assert_eq!(
            member_view(&trojan, "A").line_hash,
            member_view(&same_but_secrets, "A").line_hash,
            "secret-only differences must not change the hash"
        );
        let without_ws = config("A = trojan, t.test, 443, password=pw0rd");
        assert_ne!(member_view(&trojan, "A").line_hash, member_view(&without_ws, "A").line_hash);
```

4. 明文 HTTP 经 trojan 走隧道而不是绝对 URI 转发——Task 4 Step 4 的第二个用例。
5. `use-local-host-item-for-proxy`（目标主机名在 `[Host]` 里钉了 IP 时把 IP 交给代理）对 trojan 生效：`Engine::dial` 的判断只看 `TerminalKind::Proxy`，与协议无关；不另加用例，在报告里指出代码位置即可。

- [ ] **Step 4: 门禁，提交**

```bash
git add crates/rurge crates/rurge-engine
git commit -m "feat(cli): 能力表加入 trojan；check 的 CLI 用例；能力翻转时的守卫核对用例"
```

---

### Task 6: 承接事项 2——两个缺失的端到端用例

两个用例钉住 M1b 已有的行为，预期**一上来就绿**（M1b 终审裁定留给 M2 的用例，不是新功能）。任何一个红了 → 不要改用例去迁就，停下来报告：那是一个真实缺陷。

**Files:**
- Modify: `crates/rurge-engine/tests/outbounds.rs`

**Interfaces:**
- Consumes: 该文件现成的 `harness`、`Profile::text(dns)`、`runtime(dir, profile, shared)`、`connect_via_http`；`Engine::swap_runtime(next) -> bool`、`Engine::shared()`、`EngineShared.selections`（`SelectionTable::{get, set}`）；`rurge_proto::testing::echo_server`。

- [ ] **Step 1: 链式会话进行中重载**

```rust
/// Sends `payload` through `tunnel` and expects it back (the far end echoes).
async fn echo_through(tunnel: &mut TcpStream, payload: &[u8]) {
    tunnel.write_all(payload).await.unwrap();
    let mut back = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(5), tunnel.read_exact(&mut back))
        .await
        .expect("the echo comes back")
        .unwrap();
    assert_eq!(back, payload);
}

#[tokio::test]
async fn a_reload_leaves_a_chained_session_alone_and_moves_the_next_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let exit = FakeHttpProxy::spawn(HttpProxyScript {
        connect_to: Some(echo),
        ..HttpProxyScript::default()
    })
    .await;
    let entry = |to: SocketAddr| Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let entry_a = FakeSocks5::spawn(entry(exit.addr())).await;
    let entry_b = FakeSocks5::spawn(entry(exit.addr())).await;
    let proxies = |under: &str| {
        format!(
            "EntryA = socks5, 127.0.0.1, {}\nEntryB = socks5, 127.0.0.1, {}\nExit = http, exit.example, 8080, underlying-proxy={under}",
            entry_a.addr().port(),
            entry_b.addr().port()
        )
    };
    let h = harness(Profile {
        proxies: &proxies("EntryA"),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    })
    .await;
    let mut first = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut first, b"before the reload").await;

    // the next generation enters the chain somewhere else
    let next = Profile {
        proxies: &proxies("EntryB"),
        rules: "DOMAIN,target.test,Exit",
        ..Profile::default()
    }
    .text(h.dns.addr());
    let next = runtime(h.dir.path(), &next, h.engine.shared()).await;
    h.engine.swap_runtime(next);

    // the session in flight keeps the outbound — and the chain — it was dialled with
    echo_through(&mut first, b"after the reload").await;
    let mut second = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut second, b"a new session").await;
    assert_eq!(
        (entry_a.requests().len(), entry_b.requests().len()),
        (1, 1),
        "the old session stayed on EntryA; the new one went through EntryB"
    );
}
```

- [ ] **Step 2: 保存的选择指向已不存在的成员**

```rust
#[tokio::test]
async fn a_selection_whose_member_is_gone_falls_back_to_the_first_member() {
    let echo = rurge_proto::testing::echo_server().await;
    let upstream = |to: SocketAddr| Socks5Script {
        connect_to: Some(to),
        ..Socks5Script::default()
    };
    let a = FakeSocks5::spawn(upstream(echo)).await;
    let b = FakeSocks5::spawn(upstream(echo)).await;
    let c = FakeSocks5::spawn(upstream(echo)).await;
    let line = |name: &str, fake: &FakeSocks5| {
        format!("{name} = socks5, 127.0.0.1, {}", fake.addr().port())
    };
    let h = harness(Profile {
        proxies: &format!("{}\n{}", line("A", &a), line("B", &b)),
        groups: "Pick = select, A, B",
        rules: "DOMAIN,target.test,Pick",
        ..Profile::default()
    })
    .await;
    h.engine.shared().selections.set("Pick", "B");
    let mut t = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut t, b"via B").await;
    assert_eq!((a.requests().len(), b.requests().len()), (0, 1));

    // B leaves the profile; the saved selection now names nobody
    let next = Profile {
        proxies: &format!("{}\n{}", line("A", &a), line("C", &c)),
        groups: "Pick = select, A, C",
        rules: "DOMAIN,target.test,Pick",
        ..Profile::default()
    }
    .text(h.dns.addr());
    let next = runtime(h.dir.path(), &next, h.engine.shared()).await;
    h.engine.swap_runtime(next);
    let mut t = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut t, b"via the first member").await;
    assert_eq!(
        (a.requests().len(), c.requests().len()),
        (1, 0),
        "a stale selection means the first member"
    );
    // the table is not rewritten behind the user's back: if B comes back, so does the choice
    assert_eq!(h.engine.shared().selections.get("Pick").as_deref(), Some("B"));
    assert_eq!(h.engine.runtime().policies.current_member("Pick").as_deref(), Some("A"));
}
```

Run: `cargo test -p rurge-engine --test outbounds a_reload_leaves a_selection_whose`
Expected: PASS（两个用例）。`harness` 把 `target.test` 解析成 `127.0.0.1`，但这两条路径走的是远程解析，不依赖它。

- [ ] **Step 3: 提交**

```bash
git add crates/rurge-engine/tests/outbounds.rs
git commit -m "test(engine): 链式会话进行中重载；保存的选择指向已不存在的成员（M1b 终审留下的两个用例）"
```

---

### Task 7: 互操作——sing-box 的 trojan 入站（含 WebSocket 传输）

本机没有 sing-box：用例会打印跳过原因。**不要下载或安装它**。本任务能在本机验证的只有：夹具的单元用例、`cargo check` / clippy、以及"配置绝不碰本机"的安全守卫。把改动读两遍。

**Files:**
- Modify: `tests/interop/src/lib.rs`
- Modify: `tests/interop/tests/sing_box.rs`
- Modify: `tests/interop/README.md`

**Interfaces:**
- Consumes: `rurge_interop::{Inbound, InboundKind, TlsFiles, SingBox, render, sing_box_or_skip}`；`sing_box.rs` 里现成的 `outbound(profile, name, fixture)`、`roundtrip(out, echo)`、`target(addr)`；`TlsFixture::{leaf_pem, leaf_key_pem, roots}`。
- Produces: `InboundKind::Trojan`；`Inbound.ws_path: Option<String>`。

- [ ] **Step 1: 夹具的单元用例（先写）**

`tests/interop/src/lib.rs` 的测试模块：`every_kind()` 里三个现有 `Inbound` 字面量各补一个字段 `ws_path: None,`，并在末尾加第四项：

```rust
            (
                Inbound {
                    kind: InboundKind::Trojan,
                    users: vec![("u".into(), "pw".into())],
                    tls: Some(TlsFiles {
                        certificate: "leaf.pem".into(),
                        key: "leaf.key".into(),
                        client_ca: None,
                    }),
                    ws_path: Some("/ws".into()),
                },
                1004,
            ),
```

`inbounds_are_rendered_as_sing_box_spells_them` 末尾加：

```rust
        let trojan = &config["inbounds"][3];
        assert_eq!(trojan["type"], "trojan");
        // trojan users are `name` + `password`, not `username`
        assert_eq!(trojan["users"], json!([{ "name": "u", "password": "pw" }]));
        assert_eq!(trojan["tls"]["enabled"], true);
        assert_eq!(trojan["transport"], json!({ "type": "ws", "path": "/ws" }));
        assert!(config["inbounds"][0].get("transport").is_none());
```

（`the_configuration_never_touches_the_machine` 不用改：它对整份渲染结果做子串检查，新入站自动在内——注意 `"tun"` 这个禁用子串：`"transport"` 不含它。）

Run: `cargo test -p rurge-interop --lib`
Expected: 编译失败（`InboundKind::Trojan`、`ws_path` 不存在）。

- [ ] **Step 2: 渲染**

`InboundKind` 加 `Trojan`；`Inbound` 加字段：

```rust
    /// A V2Ray WebSocket transport on this path (trojan only).
    pub ws_path: Option<String>,
```

`render` 里：类型名的 `match` 加 `InboundKind::Trojan => "trojan",`；用户的渲染按类型分开——

```rust
            if !inbound.users.is_empty() {
                v["users"] = inbound
                    .users
                    .iter()
                    .map(|(u, p)| match inbound.kind {
                        // sing-box's trojan users are `name` + `password`
                        InboundKind::Trojan => json!({ "name": u, "password": p }),
                        _ => json!({ "username": u, "password": p }),
                    })
                    .collect();
            }
```

TLS 的断言放宽为"http 或 trojan"：

```rust
                assert!(
                    matches!(inbound.kind, InboundKind::Http | InboundKind::Trojan),
                    "only sing-box's http and trojan inbounds are given tls here"
                );
```

并在 TLS 之后加：

```rust
            if let Some(path) = &inbound.ws_path {
                assert_eq!(inbound.kind, InboundKind::Trojan, "ws is rendered for trojan only");
                v["transport"] = json!({ "type": "ws", "path": path });
            }
```

`Inbound` 的文档注释里那句 "Only the `http` inbound of sing-box speaks TLS." 改成 "sing-box's `http` and `trojan` inbounds speak TLS; `socks` and `mixed` do not."。

`tests/interop/tests/sing_box.rs` 里所有构造 `Inbound { … }` 的地方（`plain(..)` 辅助函数与各用例里的字面量）补 `ws_path: None,`。

Run: `cargo test -p rurge-interop --lib && cargo check -p rurge-interop --all-targets`
Expected: PASS。

- [ ] **Step 3: 互操作用例**

`tests/interop/tests/sing_box.rs` 末尾加（现有的 https 用例用一个内联闭包把夹具的 PEM 写进临时目录；这里要用两次，所以提成一个函数）：

```rust
/// The fixture's leaf certificate and key as PEM files in `dir`.
fn leaf_files(fixture: &TlsFixture, dir: &Path) -> TlsFiles {
    let write = |name: &str, text: String| {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    };
    TlsFiles {
        certificate: write("leaf.pem", fixture.leaf_pem()),
        key: write("leaf.key", fixture.leaf_key_pem()),
        client_ca: None,
    }
}

fn trojan_inbound(tls: TlsFiles, ws_path: Option<&str>) -> Inbound {
    Inbound {
        kind: InboundKind::Trojan,
        users: vec![("u".into(), "s3same".into())],
        tls: Some(tls),
        ws_path: ws_path.map(str::to_string),
    }
}

#[tokio::test]
async fn trojan_with_and_without_websocket() {
    let Some(bin) = sing_box_or_skip("trojan_with_and_without_websocket") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![
            trojan_inbound(leaf_files(&fixture, dir.path()), None),
            trojan_inbound(leaf_files(&fixture, dir.path()), Some("/ws")),
        ],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nPlain = trojan, 127.0.0.1, {}, password=s3same\nWs = trojan, 127.0.0.1, {}, password=s3same, ws=true, ws-path=/ws\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0),
        sb.port(1)
    );
    roundtrip(&outbound(&profile, "Plain", Some(&fixture)), echo).await;
    roundtrip(&outbound(&profile, "Ws", Some(&fixture)), echo).await;
}

#[tokio::test]
async fn a_wrong_trojan_password_is_not_relayed() {
    let Some(bin) = sing_box_or_skip("a_wrong_trojan_password_is_not_relayed") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let sb = SingBox::spawn(
        &bin,
        dir.path(),
        vec![trojan_inbound(leaf_files(&fixture, dir.path()), None)],
    );
    let echo = echo_server().await;
    let profile = format!(
        "[Proxy]\nWrong = trojan, 127.0.0.1, {}, password=nope\n[Rule]\nFINAL,DIRECT\n",
        sb.port(0)
    );
    // the protocol has no reply, so connecting succeeds; nothing comes back
    let mut stream = outbound(&profile, "Wrong", Some(&fixture))
        .connect_tcp(&target(echo), &ConnectOpts::default())
        .await
        .expect("connecting succeeds");
    stream.write_all(b"interop").await.unwrap();
    let mut buf = [0u8; 7];
    let got = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.read_exact(&mut buf),
    )
    .await
    .expect("sing-box closes the connection (it has no fallback configured)");
    assert!(got.is_err() || &buf != b"interop", "the payload must not be echoed");
}
```

两个 trojan 入站共用同一对 PEM 文件（`leaf_files` 两次写同样的内容到同样的路径）。

Run: `cargo test -p rurge-interop`
Expected: 本机——新用例各打印一行 `skipping …`，其余通过；`cargo clippy -p rurge-interop --all-targets -- -D warnings` 零警告。

- [ ] **Step 4: README 与提交**

`tests/interop/README.md`：在列举已覆盖协议的地方加上 `trojan`（TLS；WebSocket 传输），并注明 trojan 的"密码错误"用例断言的是"负载不被回显"，因为协议本身没有应答。CI 不需要改动（同一个 sing-box 二进制）。

```bash
git add tests/interop
git commit -m "test(interop): sing-box 的 trojan 入站（含 WebSocket 传输）与用例"
```

---

### Task 8: 文档

**Files:**
- Modify: `docs/surge-compatibility-matrix.md`、`README.md`、`CLAUDE.md`、`docs/api/phase1.md`、`docs/api/phase2.md`
- Create: `docs/acceptance/phase2-manual.md`
- Modify: `docs/superpowers/specs/2026-09-20-phase2-m2-tls-family-design.md`（计划期决定带来的订正）
- Modify: `docs/superpowers/plans/2026-09-19-phase2-m1b-assembly-control-plane-plan.md`（「延后事项」表的一行）
- Modify: 本计划文件末尾的两张表

- [ ] **Step 1: 兼容性清单**（每一处都先 `Grep` 定位现有行，只改那一行的相应单元格）

| 位置 | 改动 |
| ---- | ---- |
| 4.2 `trojan` 行 | 备注写：`M2a（阶段 2）已实现（TCP）：TLS 必有，可叠加 WebSocket；请求头与首段负载合并成一次写出，客户端 100 ms 内不发数据时（服务端先说话的协议）请求头单独发出、这类协议的首字节因此晚 100 ms；密码错误在连接期无法识别（协议没有应答，服务端把连接交给它的回落站点）；默认不带 ALPN（未与真实 Surge 核对）；口令只接受 password=（手册的写法），位置参数不读；UDP 属 M5` |
| 4.2 `W0007` 那一行（"策略指向 rurge 尚未实现的协议类型"） | 列表追加：`M2a 已移除 trojan` |
| 4.3 `interface` `allow-other-interface` `tos` | 现有的"有 underlying-proxy 时无效…"后面追加：`；M2a 起加载时报 W0028` |
| 4.3 `ip-version` | 现有的"有 underlying-proxy 时无效"后面追加：`（M2a 起加载时报 W0028）` |
| 4.4 `server-cert-verify-name` | 追加：`；M2a 起配置期校验（IP 字面量或 ASCII 主机名，IDN 要写成 xn-- 形式，否则 E0018）；与 server-cert-fingerprint-sha256 或 skip-cert-verify 同时出现时不起作用并报 W0012` |
| 4.6 `trojan` 行、`vmess` 行里的 `ws` `ws-path` `ws-headers` | trojan 行备注追加：`ws-path 必须是以 / 开头的 ASCII 路径（无空白与控制字符）；ws-headers 的名字必须是 HTTP token、值只允许 HTAB 一种控制字符；Connection / Upgrade / Sec-WebSocket-* 由握手自己写，出现时 W0012 并忽略；Host 缺省取服务器主机名（端口非 443 时带端口；未与真实 Surge 核对）；不支持 early data；入站帧上限 1 MiB`。vmess 行不动（M2b） |
| 10.4 `GET /v1/profiles/current` | 脱敏名单追加 `ws-headers` `ws-path` |
| 所有出站的主机名规则 | 4.2 `http` / `socks5` 两行里"目标主机名…只允许 ASCII 字母、数字、`-`、`.`、`_`"的说明同样适用于 trojan：在 trojan 行备注里加一句 `目标主机名的字母表规则同 http / socks5` |

- [ ] **Step 2: API 文档**

`docs/api/phase1.md`（`profiles/current` 的脱敏规则）与 `docs/api/phase2.md`（`policies/detail` 与 `lineHash`）：脱敏名单加入 `ws-headers=` 与 `ws-path=`（整值抹掉）。`docs/api/phase2.md` 的会话日志错误文本一节（没有就在 `policies/detail` 之后新建一小节"会话日志里的出站错误文本"）登记：`trojan: the host name cannot be sent to the server`、`trojan: the host name is longer than 255 bytes`、`ws: handshake failed: HTTP <code>`、`ws: handshake failed`、`ws: protocol error`、`ws: the server sent a text frame`、`ws: the server sent a frame larger than the limit`、`ws: the connection is closed`。

- [ ] **Step 3: 手工验收清单**

新建 `docs/acceptance/phase2-manual.md`：

```markdown
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
```

- [ ] **Step 4: M2 设计的订正**（计划期决定带来的，逐条对应）

| 设计位置 | 订正 |
| -------- | ---- |
| 4.1 末句"凭据字段的 `Debug` 输出一律抹掉（与 `HttpSpec` 的做法一致）" | 改为：`spec 类型沿用 M1 的约定派生 Debug（测试断言要用；日志与诊断从不打印 spec）；持有凭据派生物的出站对象不实现 Debug`（P5） |
| 4.2 第一行"`password` 接受位置参数（沿用 `read_credentials` 的"命名优先于位置"）" | 改为：`trojan 的 password 只接受命名写法（手册如此）；位置值不读，按多余的位置参数报 W0001——redact_profile 只对 http / socks5 系抹位置凭据，接受位置口令会留下脱敏漏洞`（P4）。anytls 的同一句留给 M2b 的计划核对 |
| 4.2 `ws-headers` 一行 | 追加：`Connection / Upgrade / Sec-WebSocket-* 由握手自己写，出现时 W0012 并忽略`（P2） |
| 6.1 "若应用先读：第一次 `poll_read` 之前先把请求头单独刷出" | 改为 P14 的规则：读在请求头未发出时先等 `HEAD_GRACE`（100 ms）；期间有写 → 头与首段负载一起发出并唤醒挂起的读；一直没有写 → 头单独发出。原因：转发循环从隧道建立起就在轮询读 |
| 5.2 "帧与消息的大小上限取有界值（写计划时定具体数字；量级为 1 MiB）" | 改为：`入站帧与消息 ≤ 1 MiB，出站每帧 ≤ 64 KiB`（P7）；并补一句 `tungstenite 要求请求 URI 带 ws:// scheme、五个握手头各恰好一个；它的错误文本会引用头的值，所以一律按变体映射成固定文本`（P2 / P3） |
| 第 15 节 V1、V2 | 两行末尾各加 `（已核对：M2a 计划 P1–P3、P7 / P10）` |
| 新增第 17 节「M2a 实施期的订正」 | 表头 `编号 | 设计原文 | 订正`；先放上面五条；实施中发现的新出入由各任务追加 |

- [ ] **Step 5: M1b 计划「延后事项」表**

`docs/superpowers/plans/2026-09-19-phase2-m1b-assembly-control-plane-plan.md` 末尾「延后事项」表里那一行（`Grep` "偏保守"）：去向改为

`关闭（M2 设计第 9 节第 4 条）：前提不成立——解析器把代理服务器主机名排除在 [Host] 之外（rurge-dns 的 proxy_hostnames，清单 6.3，有用例钉着），use-local-host-item-for-proxy 管的是目标主机名；以域名配置的代理永远需要一次真实查询，旁路是精确的`

- [ ] **Step 6: README 与 CLAUDE.md**

`README.md`（中英两处）：特性表里 Trojan 标为已支持（TCP；WebSocket 传输），状态一节写明"阶段 2 / M2a 已完成"，路线图相应更新；措辞与现有的 M1 条目保持同一风格。`CLAUDE.md`：
- 「当前状态」段末那句"M2（TLS 族）的设计文档已写好……尚未开始实现"改为：`M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——rurge-config::spec 的 WsOpts / TrojanSpec、rurge-proto 的传输阶梯 transport::Stack（connect → tls → ws）、WebSocket 字节流（tokio-tungstenite）、惰性请求头 LazyHead、trojan 出站与 rurge_proto::testing 的 FakeWs / FakeTrojan、能力表翻转 trojan、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）与 M2c（Shadow TLS）尚未开始。`
- 「先读这些文档」加入本计划文件（一句话说明：8 个任务；开头「计划期决定」P1–P14；末尾两张表）与 `docs/acceptance/phase2-manual.md`。
- 「常用命令」的 `cargo test -p rurge-proto` 一行的说明末尾加上 `、trojan 出站与 WebSocket 层`。

- [ ] **Step 7: 本计划末尾的两张表**

把各任务报告里的"偏差"逐条登记进「执行期修正记录」（任务、改了什么、原因、提交）；把评审中裁定延后的条目登记进「延后事项」（事项、去向）。两张表由控制者在派发本任务时给出素材（`task-8-inputs.md`），不要自己编。

- [ ] **Step 8: 提交**

```bash
git add docs README.md CLAUDE.md
git commit -m "docs: 阶段 2 / M2a（Trojan 优先）：兼容性清单、API 参考、手工验收清单、设计订正、README 与 CLAUDE.md"
```

---

## 验收对照（M2 设计第 12 节里属于 M2a 的部分）

| 设计的验收项 | 由谁证明 |
| ------------ | -------- |
| `trojan`（± ws）对回环假服务端的成功与失败路径 | Task 3 的 8 个单元用例；Task 2 的 WebSocket 用例（设计第 11 节说的"任意切片方式喂入，结果一致"落成一张切片表：单字节帧、奇数尺寸、大于一帧的写——对真实套接字做 proptest 得不偿失） |
| 对 sing-box 转发通过（trojan、trojan + ws） | Task 7（首次推送后的 CI；本机跳过） |
| 六个 TLS 参数在 trojan 上有用例 | Task 3 `the_tls_parameters_apply…`（`sni`、`server-cert-verify-name`、`alpn`）；Task 4 的端到端用例（指纹）；`skip-cert-verify` 与 `client-cert` 沿用 M1 的 TLS 层用例，Task 4 / 5 钉住它们对 trojan 的接线（构建错误、WARN） |
| trojan 能作链的出口 | Task 4 `a_trojan_exit_is_reached_through_a_socks5_entry_by_name` |
| 承接事项 1–4 | Task 1（1、3）、Task 6（2）、Task 8（4） |
| 凭据不外泄 | Task 1（诊断与脱敏）、Task 2 / 3（错误文本）、Task 5（CLI 与 `policies/detail`） |
| 需要真实节点的项目进手工验收清单 | Task 8 Step 3 |

trojan 作链的**入口**（别的策略以 trojan 为 `underlying-proxy`）不另加用例：`ChainConnector` 只依赖 `Outbound::connect_tcp`，M1b 的三跳链用例已覆盖该机制；若评审认为需要，加在 Task 4。

## 执行期修正记录

| 任务 | 改动 | 原因 | 提交 |
| ---- | ---- | ---- | ---- |
| 1 | 既有用例 `spec::tls::tests::all_six_parameters` 的诊断断言改为"恰好一条 `W0012`" | 该用例同时写了 `server-cert-verify-name` 与指纹，承接事项 1 新增的"verify-name 在不校验证书链时无效"告警正好命中它；字段断言未动 | bdfc38c |
| 2 | `WsByteStream::poll_write` 改为**写穿**：新增 `queued` 字段，`start_send` 之后立刻 flush，flush 挂起则本次写返回 `Pending`、重试时补报字节数 | 计划给的实现只把帧放进 tungstenite 的 128 KiB 写缓冲，而引擎的转发循环（read → write_all）与 http / socks5 出站都从不 flush——ws 隧道一接入就会卡死。写计划时的验证程序自己调了 flush，所以没暴露。评审（opus）对照库源码发现 | 72d3ec6 |
| 2 | 随修复轮带上：两个往返用例加有界等待并去掉显式 flush、假服务端回显循环去掉 flush、出站 64 KiB 上限用例、`testing/ws.rs` 的握手错误改固定文本、写在关闭之后映射为 `BrokenPipe`、`MAX_INCOMING` 改私有、两处解释性注释 | 评审的 Minor 与 ⚠️（P7 的出站上限原本没有任何用例观察得到） | 72d3ec6 |
| 2 | clippy `err_expect`：`.err().expect(…)` → `.expect_err(…)`；rustfmt 重排 | 门禁驱动的机械修正 | 5302465 |
| 3 | `LazyHead` 的状态机重写：新增 `started`（请求头一旦开始发送就不再增长）；`coalesced` 成为"这些字节已随请求头发出"的唯一依据；`poll_read` 在请求头未发出期间的每一次 `Pending` 都保存 waker | 计划给的 `poll_write` 只看 `head` 不看 `coalesced`：写方把负载并进请求头后得到 `Pending`，同一任务里的读方把请求头（连同负载）发完，写方重试时再写一遍——负载被静默重复发送（trojan + WebSocket 时可达，因为写穿的 `WsByteStream` 会返回 `Pending`）。评审（opus）发现；修正版由控制者先在临时工程里对新旧两版实现各跑一遍交错场景后再交给实现者 | 28e42d0 |
| 3 | 交错回归用例改用可编排的内层流（`Take(n)` / `Park`） | 控制者第一次给的回归用例在旧实现上并不变红（无操作 waker 下 `sleep(ZERO)` 首次轮询是 `Pending`，旧的读在 `written == 0` 时走定时器分支）——实现者核对时发现；新用例让请求头先发出 2 字节再挂起，旧实现线上读到 `HEAD|payloadpayload+more` | afef3b4 |
| 3 | 随修复轮带上：trojan 用例里四处 `read_exact` 加 5 秒上界、假服务端的 ATYP 解析 `_ =>` 改 `3 =>`、空口令构建错误的用例、超时用例补下界断言 | 评审的 Minor（实现者自己的 RED 运行就表现为挂起而不是失败） | 28e42d0 |
| 3 | Step 5 的 RED 预期不成立：桩实现下异步用例是挂起而不是"假服务端回 400" | 桩不发请求头，负载不足 58 字节，双方都在等读；RED 运行改用超时界定 | 075ed3d |
| 4 | `W0028`（socket 选项 × `underlying-proxy`）的判断块挪进 `if !matches!(proto, Direct \| Reject(_))` 里，并补断言 | Task 1 评审的 Minor：`reject*` 别名带 `underlying-proxy` 时（`read_common` 对 `Applies::Reject` 不清该字段）会得到一条措辞有误导的 `W0028` | c99388e |
| 4 | 计划 Step 6 的前提"CLI 会对语料库跑 `check`"不成立 | 仓库里没有这样的用例；trojan 的干构建由工厂的正反两个单元用例与 Task 5 的 CLI 用例覆盖 | — |
| 5 | 用例 `every_m1_protocol_builds` 改名 `every_implemented_protocol_builds` | Task 4 评审的 Minor：它现在也构建 M2 的协议 | a65ea44 |
| 6 | 提交标题在计划给的基础上多了一句 | Task 4 评审的两条 Minor（三个 trojan 用例里的下标改 `first().expect(..)`、DNS 断言补失败信息并把 `HEAD_GRACE` 注释挪到位）按控制者的指示随同一提交落地 | ea609bd |
| 6 | 两个用例没有 RED | 开工前裁定：它们钉的是 M1b 已交付的行为（M1b 终审留给 M2 的用例），一上来就应当通过；任何一个变红都是真实缺陷，要停下来报告 | ea609bd |
| 7 | rustfmt 把一处单行 `assert!` 拆成多行 | 门禁驱动的机械修正 | 0da035f |

## 延后事项

| 事项 | 去向 |
| ---- | ---- |
| `WsByteStream.queued` 与 `LazyHead.coalesced` 都假设"挂起的写会用同一段缓冲重试"；调用方**放弃**一次挂起的写、再写一段不同的缓冲时，会被告知一个不属于它的字节数（`queued` 的情形下 `n > len` 还会让 `write_all` panic）。工作区里没有这样的调用方（`write_all` 与转发循环都用同一段缓冲重试，取消之后不再复用写端） | 整分支终审时分诊（候选加固：`min(queued, data.len())`，`poll_shutdown` 里清零） |
| `a_write_reaches_the_peer_without_an_explicit_flush` 只给读加了上界；写穿之后，回归会卡在没有上界的 `write_all` 上 | 整分支终审时分诊 |
| `ws_io` 的 `Capacity` 文本说的是"frame"，该变体也覆盖超大的重组消息与 `TooManyHeaders` | 保持现状（文本已登记进 API 文档） |
| `is_managed` 在 `rurge-config` 与 `rurge-proto` 各有一份 | 保持现状（开工前裁定：出站不信任调用方，四行） |
| `the_head_does_not_grow_under_an_inner_write_that_is_pending` 在旧实现上也通过（不变式靠构造保证）；`lazy_head.rs` 测试模块里的 `use` 位置与重复导入 | 整分支终审时分诊（外观） |
| `LazyHead` 的 `WriteZero` 错误不是粘性的（之后的轮询会再试内层写） | 整分支终审时分诊 |
| `TrojanSpec` 派生 `Debug`、口令是明文字段（P5：spec 类型沿用 M1 的约定；生产代码里没有任何地方格式化 spec） | 整分支终审时分诊；M2b 加 vmess / anytls 的 spec 时一并考虑给凭据字段包一层不打印的类型 |
| "需要 server 与 port"的前置检查：trojan 的在引擎工厂里，http / socks5 的在各自的 `from_spec` 里 | M2b（下一个协议落地时收进 proto 或抽公共函数） |
| Task 4 新增的 pipeline 用例没有像相邻用例那样断言 mock DNS 确实被查询过 | 整分支终审时分诊 |
| 互操作用例共用的 `roundtrip()` 读回显没有上界（握手本身受 `ConnectOpts` 约束；M1b 的三个用例同样如此） | 整分支终审时分诊（要修就修在辅助函数里） |
| sing-box 的 trojan 互操作（真实握手、WebSocket 升级、密码错误时的表现）本机无法运行 | 首次推送后的 CI 证明（`RURGE_INTEROP_REQUIRED=1`）；本机不安装 sing-box |
