# 阶段 1 / M2b「DNS 客户端」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 实现 `rurge-dns` crate（UDP / TCP / DoT / DoH 上游、手册语义的并发查询与重试、缓存、`[Host]` 映射链、系统 hosts、特殊主机名）、`rurge-platform::dns`，以及离线开发命令 `rurge dns lookup`，并让 `rurge rule match` 改用它解析。

**Architecture:** 上游以「线格式字节」为接口（`Upstream::query(&[u8]) -> Vec<u8>`），DNS 报文编解码集中在 `message.rs`（hickory-proto 只做编解码）；`fanout.rs` 用 `JoinSet` 实现「全上游并发、1 秒重发、5 次失败、首个有效应答获胜、空应答规则、A / AAAA 并行与部分结果」；`Resolver` 组合上游集合选择、引导（URL 型上游的主机名只由传统上游解析，经 `BootstrapConnector` 预解析后再走注入的 `Connector`）、LRU + 乐观刷新缓存、`[Host]` 链与 hosts 文件、特殊主机名，并实现 `rurge_rules::engine::LazyResolver` 与 `rurge_net::connector::Resolve`。

**Tech Stack:** Rust stable / edition 2024、tokio（UdpSocket / TcpStream / JoinSet / time）、hickory-proto 0.26（编解码）、tokio-rustls 0.26 + rustls 0.23（DoT）、`rurge-net` 的 `HttpClient::send`（DoH）、lru 0.18、resolv-conf / ipconfig / if-addrs（仅 `rurge-platform`）、rcgen（测试）。

**Spec:** `docs/superpowers/specs/2026-09-04-phase1-m2-rules-dns-design.md`（第 7、8（`dns`）、10.2、11、13 ～ 16 节）；需求编号 FR-DNS-01 ～ 06、FR-DNS-04（阶段 1 部分）、FR-DNS-11（部分）、NFR-01（DNS 缓存命中 < 1 ms）。

## Global Constraints

- 工具链：`rust-toolchain.toml` 固定 `channel = "stable"`；workspace `edition = "2024"`，`rust-version = "1.88"`（执行期修正：hickory-proto 0.26 要求 MSRV 1.88，Task 1 起由 1.85 提升；1.88 已稳定的语法可用）；lints `unsafe_code = "forbid"`，clippy `all = warn`。
- 质量门：每个任务结束时 `cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --workspace` 三者必须通过。本机 msvc 工具链缺 rustfmt 时用 `RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check`。
- crate 与依赖方向固定：`rurge (bin) → rurge-dns → rurge-rules → rurge-net → rurge-config`；`rurge-platform` 不依赖任何内部 crate，只被 bin 依赖；平台特定代码只出现在 `rurge-platform`（AR-02），`rurge-dns` 通过 `SystemDns` trait 取系统 DNS 信息。
- 手册语义（设计 §7，binding）：向全部选定上游并发发送；1 s 无应答重发；5 次后失败；首个有效应答获胜；只有全部上游明确空应答（或部分空应答其余超时）才报 `EmptyAnswer`；`ipv6 = true` 且本机有 IPv6 时 A / AAAA 并行，重发定时器触发时只有一种到达则以部分结果完成；连续 5 次「A 有应答而 AAAA 超时」抑制 AAAA 直到 flush / 网络变化；缓存按最小 TTL、LRU 默认 2000、过期条目立即返回并后台刷新、负缓存 30 s；配置了任意 `tcp://` / `encrypted-dns-server` 时 UDP 上游只用于引导；URL 型上游的主机名只由传统上游解析（`[Host]` 的 `server:` URL 同样）；`ipv6 = false` 丢弃 IPv6 地址的服务器；`[Host]` 按配置顺序首个命中，代理服务器主机名永不匹配 `[Host]`，别名最多 8 跳；hosts 文件追加在 `[Host]` 之后；`.local` 与单标签名交系统解析；尾点剥离并禁用搜索域；`localhost` / `*.localhost` 直接回环。
- 测试不访问公网：全部上游都是进程内模拟服务器（127.0.0.1）；`rurge dns lookup` 的 CLI 测试用 `--server` 指向模拟上游。
- 公共接口签名以本计划各任务的 **Interfaces** 块为准；设计文档第 15 节列出的差异在实现所在任务里同步登记到 `docs/surge-compatibility-matrix.md`（Task 14 统一核对）。
- 诊断代码一经定义不得改号；本计划新增 `W0026`（不支持的上游 scheme）与 `W0027`（`[Host]` 的 `script:` 被跳过）。运行时告警用 `tracing::warn!`，同类告警只发一次。
- rurge 专有运行时选项只走命令行参数与环境变量（本计划新增 `--dns-cache-size` / `RURGE_DNS_CACHE_SIZE`）。
- 语言：文档与提交信息中文；代码标识符、注释、日志、CLI 输出英文。
- 提交：每个任务一次提交，在分支 `m2b-dns` 上进行，不推送、不合并；提交信息末尾带两行尾注 `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` 与 `Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW`。
- 手册基线：Surge 官方手册 2026-09 版；实现语义有疑问时以设计文档与手册为准，不凭记忆。

## 文件结构

```
Cargo.toml                                    workspace：新增 hickory-proto / lru / resolv-conf / ipconfig / if-addrs
crates/rurge-net/src/http.rs                  暴露 tls_client_config(skip_verify) 供 DoT 复用
crates/rurge-platform/
  Cargo.toml                                  新增 resolv-conf（unix）/ ipconfig（windows）/ if-addrs
  src/lib.rs                                  pub mod dns;
  src/dns.rs                                  servers / search_domains / hosts_path / has_ipv6
crates/rurge-dns/
  Cargo.toml
  src/lib.rs                                  模块声明与再导出
  src/message.rs                              hickory 编解码：Question / Qtype / build_query / parse_response / build_response（测试用）/ wire_id / wire_truncated
  src/upstream/mod.rs                         Upstream trait / UpstreamError / UpstreamSpec / UpstreamRef
  src/upstream/tcp.rs                         长度前缀帧 exchange_framed；TcpUpstream（tcp:// 与 tls://，经 Connector 持久连接）
  src/upstream/udp.rs                         UdpUpstream（每上游一个 socket、ID 匹配、TC → TCP 重发一次）
  src/upstream/doh.rs                         DohUpstream（HttpClient::send POST application/dns-message）
  src/bootstrap.rs                            Bootstrap（传统上游解析 URL 主机名，最短 60 s 缓存）与 BootstrapConnector
  src/fanout.rs                               resolve_name：并发 / 重发 / 首个有效应答 / 空应答 / 部分结果
  src/cache.rs                                DnsCache：LRU + TTL + 乐观刷新 + 负缓存 + 快照
  src/hosts.rs                                HostMap（[Host] 链 + 集合键 + hosts 文件）与 parse_hosts_file
  src/system.rs                               SystemDns trait、NoSystemDns、StaticSystemDns
  src/resolver.rs                             Resolver / ResolverConfig / ResolverDeps / LookupOpts / DnsResult / DnsError / Source
  src/testing.rs                              进程内模拟上游：MockDns（UDP + TCP，或 UDP + DoT；feature = "testing" 或 cfg(test)）；DoH 用 rurge-net 的 TestServer
  tests/resolver.rs                           端到端集成测试（配置 → Resolver → 模拟上游）
  benches/dns.rs                              缓存命中基准
crates/rurge/
  src/main.rs                                 新增 Dns 子命令
  src/cli/runtime.rs                          Stack 增加 resolver；PlatformSystemDns 适配器；--dns-cache-size
  src/cli/rule.rs                             rule match 默认改用 rurge-dns Resolver
  src/cli/dns.rs                              dns lookup / dns cache
  tests/cli.rs                                新增 dns lookup 测试
```

依赖方向：`rurge` → `rurge-dns`、`rurge-rules`、`rurge-net`、`rurge-config`、`rurge-platform`；`rurge-dns` → `rurge-rules`、`rurge-net`、`rurge-config`。模块内依赖：`resolver` → 其余全部；`fanout` → `upstream`、`message`；`bootstrap` → `upstream::udp`、`fanout`、`message`；`hosts` → `rurge_rules::set`；`upstream::*` → `message`（只用 `wire_id` / `wire_truncated`）。

---

### Task 1: 分支、依赖、`rurge-dns` 骨架、诊断码与 `tls_client_config`

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/rurge-config/src/diagnostic.rs`
- Modify: `crates/rurge-net/src/http.rs`
- Modify: `crates/rurge-net/Cargo.toml`（无改动时跳过）
- Modify: `crates/rurge-platform/Cargo.toml`
- Create: `crates/rurge-dns/Cargo.toml`、`crates/rurge-dns/src/lib.rs`、`crates/rurge-dns/benches/dns.rs`（占位）

**Interfaces:**
- Consumes: 无。
- Produces:
  - `codes::W_DNS_UPSTREAM_UNSUPPORTED = "W0026"`、`codes::W_HOST_SCRIPT_SKIPPED = "W0027"`。
  - `rurge_net::http::tls_client_config(skip_verify: bool) -> Result<Arc<rustls::ClientConfig>, HttpError>`（公开；与客户端内部同一套根证书 / ALPN / 跳过校验逻辑）。
  - 空 crate `rurge-dns`（`testing` feature、dev-dependencies 含 tokio `test-util`）。

- [ ] **Step 1: 建分支并提交计划**

```bash
git checkout -b m2b-dns
git add docs/superpowers/plans/2026-09-04-phase1-m2b-dns-plan.md
git commit -F - <<'EOF'
docs: M2b DNS 实施计划

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

- [ ] **Step 2: workspace 依赖**

`Cargo.toml` 的 `[workspace.dependencies]` 追加：

```toml
rurge-dns = { path = "crates/rurge-dns" }
hickory-proto = "0.26"
lru = "0.18"
resolv-conf = "0.7"
ipconfig = "0.3"
if-addrs = "0.15"
```

（`hickory-proto` 默认 feature `std` 足够，只用编解码；`ipconfig` 在非 Windows 上编译为空 crate，无需按平台限定依赖。）

- [ ] **Step 3: 诊断码**

`crates/rurge-config/src/diagnostic.rs` 的 `codes`，在 `W_SET_NESTING` 之后追加：

```rust
    /// A DNS upstream the current version cannot use (h3:// / quic://, or unparsable).
    pub const W_DNS_UPSTREAM_UNSUPPORTED: &str = "W0026";
    /// A `[Host]` entry with a `script:` value was skipped (scripts arrive in phase 5).
    pub const W_HOST_SCRIPT_SKIPPED: &str = "W0027";
```

- [ ] **Step 4: 公开 TLS 客户端配置**

`crates/rurge-net/src/http.rs`：把 `pub(crate) fn build_tls_config(skip_verify: bool) -> Result<rustls::ClientConfig, HttpError>` 保留为内部实现，并新增公开包装：

```rust
/// Shared TLS client configuration (native roots with webpki fallback, ALPN
/// h2 + http/1.1, optional verification bypass) for other transports (DoT).
pub fn tls_client_config(skip_verify: bool) -> Result<Arc<rustls::ClientConfig>, HttpError> {
    build_tls_config(skip_verify).map(Arc::new)
}
```

并在 `HttpClient::new` 中改用 `tls_client_config(cfg.skip_cert_verification)?`（原来是 `Arc::new(build_tls_config(...)?)`）。加一个测试：

```rust
    #[test]
    fn tls_client_config_advertises_h2_and_http1() {
        let cfg = tls_client_config(false).unwrap();
        assert_eq!(cfg.alpn_protocols, vec![b"h2".to_vec(), b"http/1.1".to_vec()]);
        assert!(tls_client_config(true).is_ok());
    }
```

- [ ] **Step 5: `rurge-platform` 依赖**

`crates/rurge-platform/Cargo.toml` 的 `[dependencies]` 追加：

```toml
resolv-conf.workspace = true
ipconfig.workspace = true
if-addrs.workspace = true
```

- [ ] **Step 6: `rurge-dns` 骨架**

`crates/rurge-dns/Cargo.toml`：

```toml
[package]
name = "rurge-dns"
description = "Surge-compatible DNS client for rurge"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
rurge-config.workspace = true
rurge-net.workspace = true
rurge-rules.workspace = true
tokio.workspace = true
hickory-proto.workspace = true
lru.workspace = true
arc-swap.workspace = true
url.workspace = true
http.workspace = true
http-body-util.workspace = true
bytes.workspace = true
rustls.workspace = true
tokio-rustls.workspace = true
tracing.workspace = true
rcgen = { workspace = true, optional = true }

[features]
# In-process mock DNS servers (`rurge_dns::testing`); enabled by dependants' dev-dependencies.
testing = ["dep:rcgen"]

[dev-dependencies]
# Self dev-dependency: turns `testing` on for this crate's own integration tests and benches.
rurge-dns = { path = ".", features = ["testing"] }
rurge-net = { workspace = true, features = ["testing"] }
tokio = { workspace = true, features = ["test-util"] }
rcgen.workspace = true
tempfile.workspace = true
criterion.workspace = true

[[bench]]
name = "dns"
harness = false

[lints]
workspace = true
```

`crates/rurge-dns/src/lib.rs`：

```rust
//! Surge-compatible DNS client (M2 design §7): upstream transports, the
//! concurrent query engine, cache, `[Host]` mapping chain and the resolver
//! that every other crate resolves through.
```

`crates/rurge-dns/benches/dns.rs`（占位，Task 14 填充）：

```rust
use criterion::{Criterion, criterion_group, criterion_main};

fn placeholder(_c: &mut Criterion) {}

criterion_group!(benches, placeholder);
criterion_main!(benches);
```

- [ ] **Step 7: 编译与质量门**

```bash
cargo build --workspace
cargo test -p rurge-net tls_client_config
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
```

预期：新 crate 编译通过；已有 191 个测试通过 + 1 个新测试；`Cargo.lock` 更新。

- [ ] **Step 8: 提交**

```bash
git add Cargo.toml Cargo.lock crates/rurge-config crates/rurge-net crates/rurge-platform crates/rurge-dns
git commit -F - <<'EOF'
chore: M2b 骨架：rurge-dns crate、依赖、诊断码 W0026 / W0027、公开 tls_client_config

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 2: `rurge-platform::dns`

**Files:**
- Modify: `crates/rurge-platform/src/lib.rs`（`pub mod dns;`）
- Create: `crates/rurge-platform/src/dns.rs`

**Interfaces:**
- Consumes: `resolv_conf::{Config, ScopedIp}`（`Config::parse(&str)`、`config.nameservers: Vec<ScopedIp>`、`config.get_search() -> Option<&Vec<String>>`、`config.get_domain() -> Option<&String>`、`IpAddr::from(&ScopedIp)`）；Windows：`ipconfig::{get_adapters, OperStatus, IfType}`、`ipconfig::computer::{get_search_list, get_domain}`；`if_addrs::{get_if_addrs, IfAddr}`。
- Produces（`rurge_platform::dns::*`）：`servers() -> Vec<SocketAddr>`（端口 53，去重保序）；`search_domains() -> Vec<String>`（小写、去尾点、去重）；`hosts_path() -> PathBuf`；`has_ipv6() -> bool`；`parse_resolv_conf(text: &str) -> (Vec<IpAddr>, Vec<String>)`（跨平台可测）；`is_global_v6(ip: &Ipv6Addr) -> bool`。全部只读、永不失败（出错时返回空值并 `tracing::debug!`）。

- [ ] **Step 1: 写 `dns.rs`（含测试）**

```rust
//! System resolver configuration (design §8): DNS servers, search domains,
//! the hosts file path and IPv6 availability. Read-only; on any error the
//! functions return empty values and log at debug level.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

pub fn servers() -> Vec<SocketAddr> {
    let mut out: Vec<SocketAddr> = Vec::new();
    for ip in platform::servers() {
        let sa = SocketAddr::new(ip, 53);
        if !out.contains(&sa) {
            out.push(sa);
        }
    }
    out
}

pub fn search_domains() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in platform::search_domains() {
        let d = d.trim().trim_matches('.').to_ascii_lowercase();
        if !d.is_empty() && !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

pub fn hosts_path() -> PathBuf {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    } else {
        PathBuf::from("/etc/hosts")
    }
}

/// True when some non-loopback interface carries a global IPv6 address.
pub fn has_ipv6() -> bool {
    match if_addrs::get_if_addrs() {
        Ok(list) => list.iter().any(|i| {
            !i.is_loopback()
                && match &i.addr {
                    if_addrs::IfAddr::V6(v6) => is_global_v6(&v6.ip),
                    if_addrs::IfAddr::V4(_) => false,
                }
        }),
        Err(e) => {
            tracing::debug!(error = %e, "cannot enumerate interfaces");
            false
        }
    }
}

/// Not loopback, link-local (fe80::/10), unique-local (fc00::/7), multicast or unspecified.
pub fn is_global_v6(ip: &Ipv6Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_unicast_link_local()
        && !ip.is_unique_local()
        && !ip.is_multicast()
        && !ip.is_unspecified()
}

/// `resolv.conf` text → (nameservers, search domains). Usable on every platform (tests).
pub fn parse_resolv_conf(text: &str) -> (Vec<IpAddr>, Vec<String>) {
    match resolv_conf::Config::parse(text) {
        Ok(cfg) => {
            let servers = cfg.nameservers.iter().map(IpAddr::from).collect();
            let mut search: Vec<String> = cfg.get_search().cloned().unwrap_or_default();
            if search.is_empty() {
                if let Some(d) = cfg.get_domain() {
                    search.push(d.clone());
                }
            }
            (servers, search)
        }
        Err(e) => {
            tracing::debug!(error = %e, "cannot parse resolv.conf");
            (Vec::new(), Vec::new())
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::net::IpAddr;

    pub fn servers() -> Vec<IpAddr> {
        match ipconfig::get_adapters() {
            Ok(adapters) => adapters
                .iter()
                .filter(|a| a.oper_status() == ipconfig::OperStatus::IfOperStatusUp)
                .filter(|a| a.if_type() != ipconfig::IfType::SoftwareLoopback)
                .flat_map(|a| a.dns_servers().iter().copied())
                .collect(),
            Err(e) => {
                tracing::debug!(error = %e, "cannot read adapters");
                Vec::new()
            }
        }
    }

    pub fn search_domains() -> Vec<String> {
        let mut out = ipconfig::computer::get_search_list().unwrap_or_default();
        if let Ok(Some(domain)) = ipconfig::computer::get_domain() {
            out.push(domain);
        }
        out
    }
}

#[cfg(not(windows))]
mod platform {
    use std::net::IpAddr;

    fn read() -> (Vec<IpAddr>, Vec<String>) {
        match std::fs::read_to_string("/etc/resolv.conf") {
            Ok(text) => super::parse_resolv_conf(&text),
            Err(e) => {
                tracing::debug!(error = %e, "cannot read /etc/resolv.conf");
                (Vec::new(), Vec::new())
            }
        }
    }

    pub fn servers() -> Vec<IpAddr> {
        read().0
    }

    pub fn search_domains() -> Vec<String> {
        read().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolv_conf_servers_and_search() {
        let (servers, search) = parse_resolv_conf(
            "# comment\nnameserver 192.0.2.1\nnameserver 2001:db8::1\nnameserver fe80::1%eth0\nsearch Example.COM. lan\noptions ndots:2\n",
        );
        assert_eq!(servers.len(), 3);
        assert_eq!(servers[0], "192.0.2.1".parse::<IpAddr>().unwrap());
        assert_eq!(servers[1], "2001:db8::1".parse::<IpAddr>().unwrap());
        assert_eq!(search, vec!["Example.COM.".to_string(), "lan".to_string()]);
        let (_, only_domain) = parse_resolv_conf("nameserver 1.1.1.1\ndomain home.arpa\n");
        assert_eq!(only_domain, vec!["home.arpa".to_string()]);
        assert_eq!(parse_resolv_conf("").0.len(), 0);
    }

    #[test]
    fn global_v6_classification() {
        let g = |s: &str| is_global_v6(&s.parse::<Ipv6Addr>().unwrap());
        assert!(g("2001:db8::1"));
        assert!(!g("::1"));
        assert!(!g("fe80::1"));
        assert!(!g("fd00::1"));
        assert!(!g("ff02::1"));
        assert!(!g("::"));
    }

    #[test]
    fn hosts_path_is_platform_specific() {
        let p = hosts_path();
        assert!(p.ends_with("hosts"));
        if cfg!(windows) {
            assert!(p.to_string_lossy().to_ascii_lowercase().contains("drivers"));
        } else {
            assert_eq!(p, PathBuf::from("/etc/hosts"));
        }
    }

    #[test]
    fn live_functions_never_panic() {
        let s = servers();
        assert!(s.iter().all(|sa| sa.port() == 53));
        let _ = search_domains();
        let _ = has_ipv6();
    }
}
```

`lib.rs`：追加 `pub mod dns;`。

> `IfAddr::V6(Ifv6Addr { ip, .. })` 的 `ip` 是公开字段；`Interface::is_loopback()` 存在。若 `ipconfig::IfType` 未实现 `PartialEq`（0.3.4 已实现），用 `matches!` 改写比较。`search_domains` 返回值的大小写与尾点在 `search_domains()` 中统一，`parse_resolv_conf` 保持原文（测试断言原文）。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-platform dns
```

预期：4 个测试通过。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-platform
git commit -F - <<'EOF'
feat(platform): 系统 DNS 服务器、搜索域、hosts 路径与 IPv6 探测

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 3: 报文编解码 `message.rs`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/message.rs`

**Interfaces:**
- Consumes: hickory-proto 0.26.2 —— `Message::new(id, MessageType, OpCode)`、公开字段 `message.metadata`（`recursion_desired`、`recursion_available`、`response_code`、`truncation`、`message_type`）与经 `Deref` 可读的 `message.id` / `message.response_code` / `message.truncation` / `message.message_type`；公开字段 `message.queries: Vec<Query>`、`message.answers: Vec<Record>`；`Message::add_query`、`set_edns`、`to_vec()`、`Message::from_vec(&[u8])`；`Query::query(Name, RecordType)`；`Record { name, dns_class, ttl, data: RData }`（公开字段）与 `Record::from_rdata(name, ttl, rdata)`；`RData::{A(A(Ipv4Addr)), AAAA(AAAA(Ipv6Addr))}`；`Name::from_ascii("example.com.")`；`Edns::new().set_max_payload(DEFAULT_MAX_PAYLOAD_LEN /* 1232 */)`；`ResponseCode::{NoError, NXDomain, ServFail, Refused}`。
- Produces（`rurge_dns::message::*`）：
  - `Qtype::{A, Aaaa}`（`Copy, Hash`）、`Qtype::as_str(self) -> &'static str`（`A` / `AAAA`）。
  - `Question { name: String /* 小写、无尾点 */, qtype: Qtype }`（`Clone, PartialEq, Eq, Hash`）。
  - `Rcode::{NoError, NxDomain, ServFail, Refused, Other(u8)}`（`Copy, Display`：`NOERROR` / `NXDOMAIN` / `SERVFAIL` / `REFUSED` / `RCODE<n>`）。
  - `Answer { id: u16, rcode: Rcode, truncated: bool, question: Option<Question>, v4: Vec<(Ipv4Addr, u32)>, v6: Vec<(Ipv6Addr, u32)> }`；`Answer::is_valid_for(&self, q: &Question) -> bool`；`Answer::is_empty_for(&self, q: &Question) -> bool`。
  - `CodecError(pub String)`（`Display`）。
  - `random_id() -> u16`；`build_query(id: u16, q: &Question) -> Result<Vec<u8>, CodecError>`（RD=1，EDNS 1232）；`parse_query(bytes: &[u8]) -> Result<(u16, Question), CodecError>`；`parse_response(bytes: &[u8]) -> Result<Answer, CodecError>`；`build_response(id: u16, q: &Question, rcode: Rcode, records: &[(IpAddr, u32)], truncated: bool) -> Result<Vec<u8>, CodecError>`（供模拟服务器与测试）；`wire_id(bytes: &[u8]) -> Option<u16>`（≥ 12 字节头）；`wire_truncated(bytes: &[u8]) -> bool`。

- [ ] **Step 1: 写 `message.rs`（含测试）**

```rust
//! Wire-format encoding and decoding on top of hickory-proto (design §7.2):
//! only what a stub resolver needs — A / AAAA questions and answers, EDNS
//! payload size, response codes and the TC bit. Transports never look inside
//! messages beyond `wire_id` / `wire_truncated`.

use hickory_proto::op::{DEFAULT_MAX_PAYLOAD_LEN, Edns, Message, MessageType, OpCode, Query, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Qtype {
    A,
    Aaaa,
}

impl Qtype {
    pub fn as_str(self) -> &'static str {
        match self {
            Qtype::A => "A",
            Qtype::Aaaa => "AAAA",
        }
    }

    fn record_type(self) -> RecordType {
        match self {
            Qtype::A => RecordType::A,
            Qtype::Aaaa => RecordType::AAAA,
        }
    }

    fn from_record_type(rt: RecordType) -> Option<Qtype> {
        match rt {
            RecordType::A => Some(Qtype::A),
            RecordType::AAAA => Some(Qtype::Aaaa),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Question {
    pub name: String,
    pub qtype: Qtype,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rcode {
    NoError,
    NxDomain,
    ServFail,
    Refused,
    Other(u8),
}

impl Rcode {
    fn from_hickory(rc: ResponseCode) -> Rcode {
        match rc {
            ResponseCode::NoError => Rcode::NoError,
            ResponseCode::NXDomain => Rcode::NxDomain,
            ResponseCode::ServFail => Rcode::ServFail,
            ResponseCode::Refused => Rcode::Refused,
            other => Rcode::Other(other.low()),
        }
    }

    fn to_hickory(self) -> ResponseCode {
        match self {
            Rcode::NoError => ResponseCode::NoError,
            Rcode::NxDomain => ResponseCode::NXDomain,
            Rcode::ServFail => ResponseCode::ServFail,
            Rcode::Refused => ResponseCode::Refused,
            Rcode::Other(n) => ResponseCode::from_low(n),
        }
    }
}

impl fmt::Display for Rcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rcode::NoError => f.write_str("NOERROR"),
            Rcode::NxDomain => f.write_str("NXDOMAIN"),
            Rcode::ServFail => f.write_str("SERVFAIL"),
            Rcode::Refused => f.write_str("REFUSED"),
            Rcode::Other(n) => write!(f, "RCODE{n}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub id: u16,
    pub rcode: Rcode,
    pub truncated: bool,
    pub question: Option<Question>,
    pub v4: Vec<(Ipv4Addr, u32)>,
    pub v6: Vec<(Ipv6Addr, u32)>,
}

impl Answer {
    fn has_records(&self, qtype: Qtype) -> bool {
        match qtype {
            Qtype::A => !self.v4.is_empty(),
            Qtype::Aaaa => !self.v6.is_empty(),
        }
    }

    /// NOERROR, the question we asked, and at least one record of that type.
    pub fn is_valid_for(&self, q: &Question) -> bool {
        self.rcode == Rcode::NoError && self.question.as_ref() == Some(q) && self.has_records(q.qtype)
    }

    /// NOERROR or NXDOMAIN without records of the asked type.
    pub fn is_empty_for(&self, q: &Question) -> bool {
        matches!(self.rcode, Rcode::NoError | Rcode::NxDomain)
            && self.question.as_ref().is_none_or(|x| x == q)
            && !self.has_records(q.qtype)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

fn codec<E: fmt::Display>(e: E) -> CodecError {
    CodecError(e.to_string())
}

/// Unpredictable message IDs without a `rand` dependency: a per-process
/// random hasher over a counter and the clock.
pub fn random_id() -> u16 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(n);
    h.write_u64(nanos);
    (h.finish() & 0xffff) as u16
}

fn fqdn(name: &str) -> Result<Name, CodecError> {
    let mut s = name.trim().trim_end_matches('.').to_ascii_lowercase();
    if s.is_empty() {
        return Err(CodecError("empty name".to_string()));
    }
    s.push('.');
    Name::from_ascii(&s).map_err(codec)
}

fn question_of(q: &Query) -> Option<Question> {
    let qtype = Qtype::from_record_type(q.query_type())?;
    let name = q.name().to_ascii().trim_end_matches('.').to_ascii_lowercase();
    Some(Question { name, qtype })
}

pub fn build_query(id: u16, q: &Question) -> Result<Vec<u8>, CodecError> {
    let mut m = Message::new(id, MessageType::Query, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(fqdn(&q.name)?, q.qtype.record_type()));
    let mut edns = Edns::new();
    edns.set_max_payload(DEFAULT_MAX_PAYLOAD_LEN);
    m.set_edns(edns);
    m.to_vec().map_err(codec)
}

pub fn parse_query(bytes: &[u8]) -> Result<(u16, Question), CodecError> {
    let m = Message::from_vec(bytes).map_err(codec)?;
    let q = m.queries.first().ok_or_else(|| CodecError("no question".to_string()))?;
    let question = question_of(q).ok_or_else(|| CodecError(format!("unsupported query type {}", q.query_type())))?;
    Ok((m.id, question))
}

pub fn parse_response(bytes: &[u8]) -> Result<Answer, CodecError> {
    let m = Message::from_vec(bytes).map_err(codec)?;
    if m.message_type != MessageType::Response {
        return Err(CodecError("not a response".to_string()));
    }
    let question = m.queries.first().and_then(question_of);
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for r in &m.answers {
        match &r.data {
            RData::A(a) => v4.push((a.0, r.ttl)),
            RData::AAAA(a) => v6.push((a.0, r.ttl)),
            _ => {}
        }
    }
    Ok(Answer {
        id: m.id,
        rcode: Rcode::from_hickory(m.response_code),
        truncated: m.truncation,
        question,
        v4,
        v6,
    })
}

/// A response for mock servers and tests. `truncated` sets the TC bit and,
/// like a real server, sends no answer records.
pub fn build_response(id: u16, q: &Question, rcode: Rcode, records: &[(IpAddr, u32)], truncated: bool) -> Result<Vec<u8>, CodecError> {
    let mut m = Message::new(id, MessageType::Response, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.metadata.recursion_available = true;
    m.metadata.response_code = rcode.to_hickory();
    m.metadata.truncation = truncated;
    let name = fqdn(&q.name)?;
    m.add_query(Query::query(name.clone(), q.qtype.record_type()));
    if !truncated {
        for (ip, ttl) in records {
            let data = match ip {
                IpAddr::V4(a) => RData::A(A(*a)),
                IpAddr::V6(a) => RData::AAAA(AAAA(*a)),
            };
            m.answers.push(Record::from_rdata(name.clone(), *ttl, data));
        }
    }
    m.to_vec().map_err(codec)
}

pub fn wire_id(bytes: &[u8]) -> Option<u16> {
    (bytes.len() >= 12).then(|| u16::from_be_bytes([bytes[0], bytes[1]]))
}

pub fn wire_truncated(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && bytes[2] & 0x02 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(name: &str, qtype: Qtype) -> Question {
        Question {
            name: name.to_string(),
            qtype,
        }
    }

    #[test]
    fn query_round_trip_normalizes_the_name() {
        let wire = build_query(0x1234, &q("WWW.Example.COM.", Qtype::A)).unwrap();
        assert_eq!(wire_id(&wire), Some(0x1234));
        assert!(!wire_truncated(&wire));
        let (id, question) = parse_query(&wire).unwrap();
        assert_eq!(id, 0x1234);
        assert_eq!(question, q("www.example.com", Qtype::A));
        let (_, aaaa) = parse_query(&build_query(1, &q("x.test", Qtype::Aaaa)).unwrap()).unwrap();
        assert_eq!(aaaa.qtype, Qtype::Aaaa);
        assert!(build_query(1, &q("", Qtype::A)).is_err());
        assert!(parse_query(b"nope").is_err());
    }

    #[test]
    fn response_round_trip_with_records_and_ttls() {
        let question = q("a.test", Qtype::A);
        let wire = build_response(
            7,
            &question,
            Rcode::NoError,
            &[("10.0.0.1".parse().unwrap(), 300), ("10.0.0.2".parse().unwrap(), 60), ("fd00::1".parse().unwrap(), 30)],
            false,
        )
        .unwrap();
        let a = parse_response(&wire).unwrap();
        assert_eq!(a.id, 7);
        assert_eq!(a.rcode, Rcode::NoError);
        assert!(!a.truncated);
        assert_eq!(a.question, Some(question.clone()));
        assert_eq!(a.v4, vec![("10.0.0.1".parse().unwrap(), 300), ("10.0.0.2".parse().unwrap(), 60)]);
        assert_eq!(a.v6, vec![("fd00::1".parse().unwrap(), 30)]);
        assert!(a.is_valid_for(&question));
        assert!(!a.is_valid_for(&q("a.test", Qtype::Aaaa)) || !a.v6.is_empty());
        assert!(!a.is_empty_for(&question));
        // a query is not a response
        assert!(parse_response(&build_query(1, &question).unwrap()).is_err());
    }

    #[test]
    fn empty_and_error_answers() {
        let question = q("nx.test", Qtype::A);
        let nx = parse_response(&build_response(1, &question, Rcode::NxDomain, &[], false).unwrap()).unwrap();
        assert_eq!(nx.rcode, Rcode::NxDomain);
        assert!(nx.is_empty_for(&question) && !nx.is_valid_for(&question));
        let noerror_empty = parse_response(&build_response(1, &question, Rcode::NoError, &[], false).unwrap()).unwrap();
        assert!(noerror_empty.is_empty_for(&question));
        let servfail = parse_response(&build_response(1, &question, Rcode::ServFail, &[], false).unwrap()).unwrap();
        assert!(!servfail.is_empty_for(&question) && !servfail.is_valid_for(&question));
        assert_eq!(servfail.rcode.to_string(), "SERVFAIL");
        assert_eq!(Rcode::Other(9).to_string(), "RCODE9");
        // wrong question type is neither valid nor empty for the asked type
        let other = parse_response(&build_response(1, &q("nx.test", Qtype::Aaaa), Rcode::NoError, &[], false).unwrap()).unwrap();
        assert!(!other.is_valid_for(&question) && !other.is_empty_for(&question));
    }

    #[test]
    fn truncated_responses_set_tc_and_carry_no_records() {
        let question = q("big.test", Qtype::A);
        let wire = build_response(3, &question, Rcode::NoError, &[("10.0.0.1".parse().unwrap(), 1)], true).unwrap();
        assert!(wire_truncated(&wire));
        let a = parse_response(&wire).unwrap();
        assert!(a.truncated && a.v4.is_empty());
    }

    #[test]
    fn random_ids_vary() {
        let ids: std::collections::HashSet<u16> = (0..64).map(|_| random_id()).collect();
        assert!(ids.len() > 8, "{ids:?}");
    }
}
```

`lib.rs`：`pub mod message;`。

> hickory-proto 0.26 备选写法（按编译结果取舍，报告中说明）：`Query` 若是公开字段而非方法，用 `q.name` / `q.query_type`；`ResponseCode::low()` / `from_low()` 若不存在，用 `u16::from(rc) as u8` 与 `ResponseCode::from(u16)`；`Record::from_rdata` 若不存在，用结构体字面量 `Record { name, dns_class: DNSClass::IN, ttl, data }`（导入 `hickory_proto::rr::DNSClass`）；`m.answers.push` 亦可换成 `m.add_answer(record)`。`Option::is_none_or` 需要 Rust ≥ 1.82（满足）。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns message
```

预期：5 个测试通过。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): DNS 报文编解码（hickory-proto：查询 / 应答 / EDNS / TC）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 4: 进程内模拟上游 `testing::MockDns`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/testing.rs`

**Interfaces:**
- Consumes: `crate::message::{Qtype, Question, Rcode, build_response, parse_query}`、`tokio::net::{UdpSocket, TcpListener}`、`tokio_rustls::TlsAcceptor` + `rcgen`（TLS 变体）、`rurge_net::http::tls_client_config`（测试）。
- Produces（`rurge_dns::testing::*`，`#[cfg(any(test, feature = "testing"))]`）：
  - `MockDns::spawn() -> MockDns`（async；UDP + TCP 同一端口）；`MockDns::spawn_tls() -> MockDns`（async；UDP + TLS-over-TCP，自签证书 `localhost` / `127.0.0.1`）；`addr(&self) -> SocketAddr`。
  - 行为设置（全部 `&self`，即时生效）：`set(&self, name: &str, v4: &[&str], v6: &[&str], ttl: u32)`；`set_empty(&self, name)`（NOERROR 无记录）；`set_rcode(&self, name, Rcode)`；`set_delay(&self, Duration)`（全部应答延迟）；`set_drop_all(&self, bool)`；`set_drop_first(&self, n: usize)`（丢弃接下来 n 个查询）；`set_drop_qtype(&self, Qtype, bool)`；`set_truncate_udp(&self, bool)`（UDP 应答只带 TC，TCP 应答完整）。未设置的名字 → NXDOMAIN。
  - 观测：`queries(&self) -> Vec<(Question, String /* "udp" | "tcp" */)>`；`query_count(&self, name: &str, qtype: Qtype) -> usize`。
  - 名字统一小写、无尾点（调用方给的 `name` 也做同样归一化）。

- [ ] **Step 1: 写 `testing.rs`（含自测）**

```rust
//! In-process mock DNS servers for tests: one UDP socket and one TCP (or
//! TLS) listener on the same port, with per-name answers and programmable
//! misbehaviour (delay, drops, truncation, error codes).

use crate::message::{Qtype, Question, Rcode, build_response, parse_query};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;

#[derive(Clone, Debug)]
enum Behaviour {
    Records { v4: Vec<IpAddr>, v6: Vec<IpAddr>, ttl: u32 },
    Empty,
    Rcode(Rcode),
}

#[derive(Default)]
struct State {
    names: HashMap<String, Behaviour>,
    delay: Duration,
    drop_all: bool,
    drop_first: usize,
    drop_qtype: HashSet<Qtype>,
    truncate_udp: bool,
    queries: Vec<(Question, String)>,
}

pub struct MockDns {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    _shutdown: oneshot::Sender<()>,
}

fn norm(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Decide the reply for one query: `None` = drop.
fn respond(state: &Arc<Mutex<State>>, wire: &[u8], transport: &str) -> Option<(Duration, Vec<u8>)> {
    let (id, q) = parse_query(wire).ok()?;
    let mut st = state.lock().expect("mock state");
    st.queries.push((q.clone(), transport.to_string()));
    if st.drop_all || st.drop_qtype.contains(&q.qtype) {
        return None;
    }
    if st.drop_first > 0 {
        st.drop_first -= 1;
        return None;
    }
    let truncate = st.truncate_udp && transport == "udp";
    let (rcode, records): (Rcode, Vec<(IpAddr, u32)>) = match st.names.get(&q.name) {
        Some(Behaviour::Records { v4, v6, ttl }) => {
            let list = match q.qtype {
                Qtype::A => v4,
                Qtype::Aaaa => v6,
            };
            (Rcode::NoError, list.iter().map(|ip| (*ip, *ttl)).collect())
        }
        Some(Behaviour::Empty) => (Rcode::NoError, Vec::new()),
        Some(Behaviour::Rcode(rc)) => (*rc, Vec::new()),
        None => (Rcode::NxDomain, Vec::new()),
    };
    let delay = st.delay;
    drop(st);
    let bytes = build_response(id, &q, rcode, &records, truncate).ok()?;
    Some((delay, bytes))
}

impl MockDns {
    pub async fn spawn() -> MockDns {
        Self::start(false).await
    }

    pub async fn spawn_tls() -> MockDns {
        Self::start(true).await
    }

    async fn start(tls: bool) -> MockDns {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind tcp");
        let addr = listener.local_addr().expect("addr");
        let udp = Arc::new(UdpSocket::bind(addr).await.expect("bind udp on the same port"));
        let state = Arc::new(Mutex::new(State::default()));
        let acceptor = if tls { Some(tls_acceptor()) } else { None };
        let (tx, mut rx) = oneshot::channel::<()>();

        let udp_state = state.clone();
        let udp_socket = udp.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = udp_socket.recv_from(&mut buf).await else { return };
                if let Some((delay, reply)) = respond(&udp_state, &buf[..n], "udp") {
                    let sock = udp_socket.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = sock.send_to(&reply, peer).await;
                    });
                }
            }
        });

        let tcp_state = state.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut rx => return,
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { return };
                        let st = tcp_state.clone();
                        let acceptor = acceptor.clone();
                        tokio::spawn(async move {
                            match acceptor {
                                Some(a) => {
                                    if let Ok(tls) = a.accept(stream).await {
                                        serve_framed(tls, st, "tcp").await;
                                    }
                                }
                                None => serve_framed(stream, st, "tcp").await,
                            }
                        });
                    }
                }
            }
        });
        MockDns {
            addr,
            state,
            _shutdown: tx,
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn set(&self, name: &str, v4: &[&str], v6: &[&str], ttl: u32) {
        let v4 = v4.iter().map(|s| s.parse().expect("ipv4")).collect();
        let v6 = v6.iter().map(|s| s.parse().expect("ipv6")).collect();
        self.state.lock().expect("mock state").names.insert(norm(name), Behaviour::Records { v4, v6, ttl });
    }

    pub fn set_empty(&self, name: &str) {
        self.state.lock().expect("mock state").names.insert(norm(name), Behaviour::Empty);
    }

    pub fn set_rcode(&self, name: &str, rcode: Rcode) {
        self.state.lock().expect("mock state").names.insert(norm(name), Behaviour::Rcode(rcode));
    }

    pub fn set_delay(&self, delay: Duration) {
        self.state.lock().expect("mock state").delay = delay;
    }

    pub fn set_drop_all(&self, drop: bool) {
        self.state.lock().expect("mock state").drop_all = drop;
    }

    pub fn set_drop_first(&self, n: usize) {
        self.state.lock().expect("mock state").drop_first = n;
    }

    pub fn set_drop_qtype(&self, qtype: Qtype, drop: bool) {
        let mut st = self.state.lock().expect("mock state");
        if drop {
            st.drop_qtype.insert(qtype);
        } else {
            st.drop_qtype.remove(&qtype);
        }
    }

    pub fn set_truncate_udp(&self, truncate: bool) {
        self.state.lock().expect("mock state").truncate_udp = truncate;
    }

    pub fn queries(&self) -> Vec<(Question, String)> {
        self.state.lock().expect("mock state").queries.clone()
    }

    pub fn query_count(&self, name: &str, qtype: Qtype) -> usize {
        let name = norm(name);
        self.state
            .lock()
            .expect("mock state")
            .queries
            .iter()
            .filter(|(q, _)| q.name == name && q.qtype == qtype)
            .count()
    }
}

async fn serve_framed<S: AsyncReadExt + AsyncWriteExt + Unpin>(mut stream: S, state: Arc<Mutex<State>>, transport: &str) {
    loop {
        let mut len = [0u8; 2];
        if stream.read_exact(&mut len).await.is_err() {
            return;
        }
        let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
        if stream.read_exact(&mut msg).await.is_err() {
            return;
        }
        let Some((delay, reply)) = respond(&state, &msg, transport) else { continue };
        tokio::time::sleep(delay).await;
        let mut out = (reply.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(&reply);
        if stream.write_all(&out).await.is_err() {
            return;
        }
    }
}

fn tls_acceptor() -> TlsAcceptor {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).expect("self-signed cert");
    let cert_der = cert.der().clone();
    let key_der: rustls::pki_types::PrivateKeyDer<'static> = signing_key.into();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der)
        .expect("server config");
    TlsAcceptor::from(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{build_query, parse_response};
    use tokio::net::TcpStream;

    fn q(name: &str, qtype: Qtype) -> Question {
        Question {
            name: name.to_string(),
            qtype,
        }
    }

    async fn udp_ask(addr: SocketAddr, wire: &[u8]) -> Option<Vec<u8>> {
        let s = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        s.send_to(wire, addr).await.unwrap();
        let mut buf = vec![0u8; 4096];
        match tokio::time::timeout(Duration::from_millis(500), s.recv_from(&mut buf)).await {
            Ok(Ok((n, _))) => Some(buf[..n].to_vec()),
            _ => None,
        }
    }

    async fn tcp_ask<S: AsyncReadExt + AsyncWriteExt + Unpin>(stream: &mut S, wire: &[u8]) -> Vec<u8> {
        let mut out = (wire.len() as u16).to_be_bytes().to_vec();
        out.extend_from_slice(wire);
        stream.write_all(&out).await.unwrap();
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).await.unwrap();
        let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
        stream.read_exact(&mut msg).await.unwrap();
        msg
    }

    #[tokio::test]
    async fn answers_over_udp_and_tcp_and_records_queries() {
        let m = MockDns::spawn().await;
        m.set("A.test.", &["10.0.0.1"], &["fd00::1"], 42);
        let wire = build_query(5, &q("a.test", Qtype::A)).unwrap();
        let a = parse_response(&udp_ask(m.addr(), &wire).await.unwrap()).unwrap();
        assert_eq!(a.id, 5);
        assert_eq!(a.v4, vec![("10.0.0.1".parse().unwrap(), 42)]);
        let mut tcp = TcpStream::connect(m.addr()).await.unwrap();
        let aaaa = parse_response(&tcp_ask(&mut tcp, &build_query(6, &q("a.test", Qtype::Aaaa)).unwrap()).await).unwrap();
        assert_eq!(aaaa.v6, vec![("fd00::1".parse().unwrap(), 42)]);
        let nx = parse_response(&tcp_ask(&mut tcp, &build_query(7, &q("nope.test", Qtype::A)).unwrap()).await).unwrap();
        assert_eq!(nx.rcode, Rcode::NxDomain);
        assert_eq!(m.query_count("a.test", Qtype::A), 1);
        assert_eq!(m.query_count("a.test", Qtype::Aaaa), 1);
        assert_eq!(m.queries().len(), 3);
        assert_eq!(m.queries()[1].1, "tcp");
    }

    #[tokio::test]
    async fn misbehaviours() {
        let m = MockDns::spawn().await;
        m.set("a.test", &["10.0.0.1"], &[], 1);
        m.set_empty("empty.test");
        m.set_rcode("fail.test", Rcode::ServFail);
        let wire = build_query(1, &q("a.test", Qtype::A)).unwrap();
        m.set_drop_first(1);
        assert!(udp_ask(m.addr(), &wire).await.is_none());
        assert!(udp_ask(m.addr(), &wire).await.is_some());
        m.set_drop_qtype(Qtype::A, true);
        assert!(udp_ask(m.addr(), &wire).await.is_none());
        m.set_drop_qtype(Qtype::A, false);
        m.set_truncate_udp(true);
        let tc = parse_response(&udp_ask(m.addr(), &wire).await.unwrap()).unwrap();
        assert!(tc.truncated && tc.v4.is_empty());
        let mut tcp = TcpStream::connect(m.addr()).await.unwrap();
        let full = parse_response(&tcp_ask(&mut tcp, &wire).await).unwrap();
        assert!(!full.truncated && full.v4.len() == 1);
        m.set_truncate_udp(false);
        let e = parse_response(&udp_ask(m.addr(), &build_query(2, &q("empty.test", Qtype::A)).unwrap()).await.unwrap()).unwrap();
        assert!(e.rcode == Rcode::NoError && e.v4.is_empty());
        let f = parse_response(&udp_ask(m.addr(), &build_query(3, &q("fail.test", Qtype::A)).unwrap()).await.unwrap()).unwrap();
        assert_eq!(f.rcode, Rcode::ServFail);
        m.set_delay(Duration::from_millis(200));
        let started = std::time::Instant::now();
        assert!(udp_ask(m.addr(), &wire).await.is_some());
        assert!(started.elapsed() >= Duration::from_millis(200));
        m.set_drop_all(true);
        assert!(udp_ask(m.addr(), &wire).await.is_none());
    }

    #[tokio::test]
    async fn tls_variant_serves_dot() {
        let m = MockDns::spawn_tls().await;
        m.set("dot.test", &["10.0.0.9"], &[], 9);
        let config = rurge_net::http::tls_client_config(true).unwrap();
        let tcp = TcpStream::connect(m.addr()).await.unwrap();
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1".to_string()).unwrap();
        let mut tls = tokio_rustls::TlsConnector::from(config).connect(name, tcp).await.unwrap();
        let a = parse_response(&tcp_ask(&mut tls, &build_query(8, &q("dot.test", Qtype::A)).unwrap()).await).unwrap();
        assert_eq!(a.v4, vec![("10.0.0.9".parse().unwrap(), 9)]);
        // UDP still works on the same port
        let u = parse_response(&udp_ask(m.addr(), &build_query(9, &q("dot.test", Qtype::A)).unwrap()).await.unwrap()).unwrap();
        assert_eq!(u.id, 9);
    }
}
```

`lib.rs`：`#[cfg(any(test, feature = "testing"))] pub mod testing;`。

> `ServerName::try_from(String)` 对 IP 字面量返回 `IpAddress` 变体；`tls_client_config(true)` 跳过证书校验，因此自签证书可用。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns testing
cargo check -p rurge-dns --features testing
```

预期：3 个测试通过；feature 构建通过。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
test(dns): 进程内模拟 DNS 上游（UDP / TCP / DoT，可编程延迟、丢包、截断与错误码）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 5: 上游抽象 `Upstream` / `UpstreamSpec` 与 TCP / DoT 传输

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/upstream/mod.rs`
- Create: `crates/rurge-dns/src/upstream/tcp.rs`

**Interfaces:**
- Consumes: `rurge_config::general::{DnsServer, EncryptedDns, EncryptedDnsScheme}`、`rurge_config::host::DnsUpstream`、`rurge_config::HostName`、`rurge_net::BoxFuture`、`rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target}`、`rurge_net::http::tls_client_config(skip_verify: bool) -> Result<Arc<rustls::ClientConfig>, HttpError>`（Task 1）、`tokio_rustls::TlsConnector`、`rustls::pki_types::ServerName`、`url::Url`。
- Produces（`rurge_dns::upstream::*`）：
  - `UpstreamError::{Timeout, Io(String), Tls(String), Http(String), Bootstrap(String), BadResponse(String)}`（`Clone, PartialEq, Display`）。
  - `trait Upstream: Send + Sync { fn name(&self) -> &str; fn query<'a>(&'a self, wire: &'a [u8], deadline: tokio::time::Instant) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>>; }`；`type UpstreamRef = Arc<dyn Upstream>`。
  - `UpstreamSpec::{Udp(SocketAddr), Tcp { host: String, port: u16 }, Tls { host: String, port: u16 }, Https(Url)}`（`Clone, PartialEq, Eq, Hash, Debug`）；`UpstreamSpec::parse(&str) -> Result<UpstreamSpec, String>`；`from_dns_server(&DnsServer) -> Option<UpstreamSpec>`（`System` → `None`）；`from_encrypted(&EncryptedDns) -> Result<UpstreamSpec, String>`（`h3://` / `quic://` → `Err`）；`from_dns_upstream(&DnsUpstream) -> Result<UpstreamSpec, String>`；`is_traditional(&self) -> bool`（只有 `Udp`）；`name(&self) -> String`（`udp://1.1.1.1:53`、`tcp://h:53`、`tls://h:853`、URL 原文）；`host(&self) -> Option<&str>`；`needs_bootstrap(&self) -> bool`（主机名不是 IP 字面量）。
  - `rurge_dns::upstream::tcp::{MAX_MESSAGE, exchange_framed, TcpUpstream}`：`exchange_framed<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, wire: &[u8], deadline: Instant) -> Result<Vec<u8>, UpstreamError>`（async）；`TcpUpstream::plain(host: &str, port: u16, connector: Arc<dyn Connector>) -> TcpUpstream`；`TcpUpstream::tls(host: &str, port: u16, connector: Arc<dyn Connector>, config: Arc<rustls::ClientConfig>) -> TcpUpstream`；实现 `Upstream`（持久连接、出错后丢弃连接下次重连）。
  - 子模块声明：`pub mod tcp; pub mod udp; pub mod doh;`（`udp` / `doh` 由 Task 6 / 7 创建；本任务先只声明 `tcp`，后续任务各自追加）。

- [ ] **Step 1: 写 `upstream/mod.rs`（含 `UpstreamSpec` 测试）**

```rust
//! Upstream transports (design §7.2). Every transport exchanges wire-format
//! DNS messages; message encoding lives in `crate::message`.

pub mod tcp;

use rurge_config::general::{DnsServer, EncryptedDns, EncryptedDnsScheme};
use rurge_config::host::DnsUpstream;
use rurge_net::BoxFuture;
use std::fmt;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use tokio::time::Instant;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpstreamError {
    Timeout,
    Io(String),
    Tls(String),
    Http(String),
    Bootstrap(String),
    BadResponse(String),
}

impl fmt::Display for UpstreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpstreamError::Timeout => f.write_str("timeout"),
            UpstreamError::Io(e) => write!(f, "io: {e}"),
            UpstreamError::Tls(e) => write!(f, "tls: {e}"),
            UpstreamError::Http(e) => write!(f, "http: {e}"),
            UpstreamError::Bootstrap(e) => write!(f, "bootstrap: {e}"),
            UpstreamError::BadResponse(e) => write!(f, "bad response: {e}"),
        }
    }
}

/// One DNS server reachable through one transport.
pub trait Upstream: Send + Sync {
    /// Display name, e.g. `udp://1.1.1.1:53`.
    fn name(&self) -> &str;
    /// Sends one wire-format query and returns one wire-format response.
    fn query<'a>(&'a self, wire: &'a [u8], deadline: Instant) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>>;
}

pub type UpstreamRef = Arc<dyn Upstream>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum UpstreamSpec {
    Udp(SocketAddr),
    Tcp { host: String, port: u16 },
    Tls { host: String, port: u16 },
    Https(Url),
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &s[prefix.len()..])
}

/// `host`, `host:port`, `[v6]`, `[v6]:port`; anything after `/` is ignored.
fn host_port(rest: &str, default_port: u16) -> Result<(String, u16), String> {
    let rest = rest.split('/').next().unwrap_or("").trim();
    if rest.is_empty() {
        return Err("missing host".to_string());
    }
    let (host, port) = if let Some(r) = rest.strip_prefix('[') {
        let (h, tail) = r.split_once(']').ok_or_else(|| "unterminated IPv6 literal".to_string())?;
        (h.to_string(), tail.strip_prefix(':'))
    } else if rest.matches(':').count() == 1 {
        let (h, p) = rest.split_once(':').expect("one colon");
        (h.to_string(), Some(p))
    } else {
        (rest.to_string(), None)
    };
    let port = match port {
        Some(p) => p.parse::<u16>().map_err(|_| format!("invalid port `{p}`"))?,
        None => default_port,
    };
    Ok((host.trim_end_matches('.').to_ascii_lowercase(), port))
}

impl UpstreamSpec {
    pub fn parse(s: &str) -> Result<UpstreamSpec, String> {
        let s = s.trim();
        let lower = s.to_ascii_lowercase();
        if lower.starts_with("https://") {
            let url = Url::parse(s).map_err(|e| format!("`{s}`: {e}"))?;
            if url.host_str().is_none() {
                return Err(format!("`{s}`: missing host"));
            }
            return Ok(UpstreamSpec::Https(url));
        }
        if lower.starts_with("h3://") || lower.starts_with("quic://") {
            return Err(format!("`{s}`: h3:// and quic:// upstreams are not supported until phase 2"));
        }
        if let Some(rest) = strip_prefix_ci(s, "tls://") {
            let (host, port) = host_port(rest, 853).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Tls { host, port });
        }
        if let Some(rest) = strip_prefix_ci(s, "tcp://") {
            let (host, port) = host_port(rest, 53).map_err(|e| format!("`{s}`: {e}"))?;
            return Ok(UpstreamSpec::Tcp { host, port });
        }
        if lower == "system" {
            return Err("`system` is not an upstream; the resolver expands it".to_string());
        }
        if let Ok(addr) = s.parse::<SocketAddr>() {
            return Ok(UpstreamSpec::Udp(addr));
        }
        let bare = s.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(UpstreamSpec::Udp(SocketAddr::new(ip, 53)));
        }
        Err(format!("`{s}`: expected ip[:port], tcp://, tls:// or https://"))
    }

    pub fn from_dns_server(s: &DnsServer) -> Option<UpstreamSpec> {
        match s {
            DnsServer::System => None,
            DnsServer::Udp(addr) => Some(UpstreamSpec::Udp(*addr)),
        }
    }

    pub fn from_encrypted(e: &EncryptedDns) -> Result<UpstreamSpec, String> {
        match e.scheme {
            EncryptedDnsScheme::H3 | EncryptedDnsScheme::Quic => {
                Err(format!("`{}`: h3:// and quic:// upstreams are not supported until phase 2", e.url))
            }
            _ => UpstreamSpec::parse(&e.url),
        }
    }

    pub fn from_dns_upstream(u: &DnsUpstream) -> Result<UpstreamSpec, String> {
        match u {
            DnsUpstream::Udp(addr) => Ok(UpstreamSpec::Udp(*addr)),
            DnsUpstream::Encrypted(e) => UpstreamSpec::from_encrypted(e),
        }
    }

    /// Plain UDP servers are the "traditional" upstreams used for bootstrap.
    pub fn is_traditional(&self) -> bool {
        matches!(self, UpstreamSpec::Udp(_))
    }

    pub fn name(&self) -> String {
        match self {
            UpstreamSpec::Udp(addr) => format!("udp://{addr}"),
            UpstreamSpec::Tcp { host, port } => format!("tcp://{host}:{port}"),
            UpstreamSpec::Tls { host, port } => format!("tls://{host}:{port}"),
            UpstreamSpec::Https(url) => url.as_str().to_string(),
        }
    }

    pub fn host(&self) -> Option<&str> {
        match self {
            UpstreamSpec::Udp(_) => None,
            UpstreamSpec::Tcp { host, .. } | UpstreamSpec::Tls { host, .. } => Some(host),
            UpstreamSpec::Https(url) => url.host_str(),
        }
    }

    /// URL-type upstreams whose host is a name (not an IP literal).
    pub fn needs_bootstrap(&self) -> bool {
        self.host()
            .map(|h| h.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>().is_err())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_form() {
        assert_eq!(UpstreamSpec::parse("1.1.1.1").unwrap(), UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap()));
        assert_eq!(UpstreamSpec::parse("192.0.2.53:5353").unwrap(), UpstreamSpec::Udp("192.0.2.53:5353".parse().unwrap()));
        assert_eq!(UpstreamSpec::parse("::1").unwrap(), UpstreamSpec::Udp("[::1]:53".parse().unwrap()));
        assert_eq!(UpstreamSpec::parse("[2001:db8::1]:5353").unwrap(), UpstreamSpec::Udp("[2001:db8::1]:5353".parse().unwrap()));
        assert_eq!(UpstreamSpec::parse("tcp://dns.Example.com").unwrap(), UpstreamSpec::Tcp { host: "dns.example.com".into(), port: 53 });
        assert_eq!(UpstreamSpec::parse("TLS://dns.example.com:8853/").unwrap(), UpstreamSpec::Tls { host: "dns.example.com".into(), port: 8853 });
        assert_eq!(UpstreamSpec::parse("tls://[::1]:853").unwrap(), UpstreamSpec::Tls { host: "::1".into(), port: 853 });
        assert!(matches!(UpstreamSpec::parse("https://dns.example.com/dns-query").unwrap(), UpstreamSpec::Https(_)));
        assert!(UpstreamSpec::parse("h3://dns.example.com/dns-query").unwrap_err().contains("phase 2"));
        assert!(UpstreamSpec::parse("quic://dns.example.com").unwrap_err().contains("phase 2"));
        assert!(UpstreamSpec::parse("system").is_err());
        assert!(UpstreamSpec::parse("dns.example.com").is_err(), "hostnames are not allowed as plain servers");
        assert!(UpstreamSpec::parse("tcp://").is_err());
        assert!(UpstreamSpec::parse("tls://host:99999").is_err());
    }

    #[test]
    fn names_hosts_and_bootstrap_need() {
        let udp = UpstreamSpec::parse("1.1.1.1").unwrap();
        assert_eq!(udp.name(), "udp://1.1.1.1:53");
        assert!(udp.is_traditional() && udp.host().is_none() && !udp.needs_bootstrap());
        let tls = UpstreamSpec::parse("tls://dns.example.com").unwrap();
        assert_eq!(tls.name(), "tls://dns.example.com:853");
        assert_eq!(tls.host(), Some("dns.example.com"));
        assert!(tls.needs_bootstrap() && !tls.is_traditional());
        let tcp_ip = UpstreamSpec::parse("tcp://9.9.9.9:53").unwrap();
        assert!(!tcp_ip.needs_bootstrap());
        let doh = UpstreamSpec::parse("https://1.1.1.1/dns-query").unwrap();
        assert_eq!(doh.name(), "https://1.1.1.1/dns-query");
        assert!(!doh.needs_bootstrap());
        assert!(UpstreamSpec::parse("https://dns.example.com/dns-query").unwrap().needs_bootstrap());
    }

    #[test]
    fn conversions_from_config_types() {
        assert_eq!(UpstreamSpec::from_dns_server(&DnsServer::System), None);
        assert_eq!(
            UpstreamSpec::from_dns_server(&DnsServer::Udp("8.8.8.8:53".parse().unwrap())),
            Some(UpstreamSpec::Udp("8.8.8.8:53".parse().unwrap()))
        );
        let e = EncryptedDns::parse("https://dns.example.com/dns-query").unwrap();
        assert!(matches!(UpstreamSpec::from_encrypted(&e).unwrap(), UpstreamSpec::Https(_)));
        let h3 = EncryptedDns::parse("h3://dns.example.com/dns-query").unwrap();
        assert!(UpstreamSpec::from_encrypted(&h3).is_err());
        assert_eq!(
            UpstreamSpec::from_dns_upstream(&DnsUpstream::Udp("1.1.1.1:53".parse().unwrap())).unwrap(),
            UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap())
        );
    }
}
```

> `EncryptedDns::parse` 在 M1 中存在（识别五种 scheme）；若签名不同以 `crates/rurge-config/src/general.rs` 为准调整测试。

- [ ] **Step 2: 写 `upstream/tcp.rs`（含测试）**

```rust
//! DNS over TCP and over TLS (design §7.2): one persistent connection per
//! upstream, RFC 1035 §4.2.2 two-byte length framing, reconnect after an error.

use super::{Upstream, UpstreamError};
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_rustls::TlsConnector;

pub const MAX_MESSAGE: usize = 65_535;

/// Writes one length-prefixed message and reads one length-prefixed reply.
pub async fn exchange_framed<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    wire: &[u8],
    deadline: Instant,
) -> Result<Vec<u8>, UpstreamError> {
    if wire.len() > MAX_MESSAGE {
        return Err(UpstreamError::BadResponse("query larger than 65535 bytes".to_string()));
    }
    let mut buf = Vec::with_capacity(wire.len() + 2);
    buf.extend_from_slice(&(wire.len() as u16).to_be_bytes());
    buf.extend_from_slice(wire);
    let io = async {
        stream.write_all(&buf).await?;
        let mut len = [0u8; 2];
        stream.read_exact(&mut len).await?;
        let n = usize::from(u16::from_be_bytes(len));
        let mut resp = vec![0u8; n];
        stream.read_exact(&mut resp).await?;
        Ok::<Vec<u8>, std::io::Error>(resp)
    };
    match tokio::time::timeout_at(deadline, io).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => Err(UpstreamError::Io(e.to_string())),
        Err(_) => Err(UpstreamError::Timeout),
    }
}

pub struct TcpUpstream {
    name: String,
    host: String,
    port: u16,
    tls: Option<Arc<ClientConfig>>,
    connector: Arc<dyn Connector>,
    conn: Mutex<Option<BoxedStream>>,
}

impl TcpUpstream {
    pub fn plain(host: &str, port: u16, connector: Arc<dyn Connector>) -> TcpUpstream {
        TcpUpstream {
            name: format!("tcp://{host}:{port}"),
            host: host.to_string(),
            port,
            tls: None,
            connector,
            conn: Mutex::new(None),
        }
    }

    pub fn tls(host: &str, port: u16, connector: Arc<dyn Connector>, config: Arc<ClientConfig>) -> TcpUpstream {
        TcpUpstream {
            name: format!("tls://{host}:{port}"),
            host: host.to_string(),
            port,
            tls: Some(config),
            connector,
            conn: Mutex::new(None),
        }
    }

    async fn connect(&self, deadline: Instant) -> Result<BoxedStream, UpstreamError> {
        let timeout = deadline.saturating_duration_since(Instant::now());
        let target = Target::new(HostName::parse(&self.host), self.port);
        let opts = ConnectOpts { timeout, prefer_v6: false };
        let stream = self
            .connector
            .connect(&target, &opts)
            .await
            .map_err(|e| UpstreamError::Io(e.to_string()))?;
        let Some(config) = &self.tls else {
            return Ok(stream);
        };
        let name = ServerName::try_from(self.host.clone()).map_err(|e| UpstreamError::Tls(e.to_string()))?;
        let handshake = TlsConnector::from(config.clone()).connect(name, stream);
        match tokio::time::timeout_at(deadline, handshake).await {
            Ok(Ok(tls)) => Ok(Box::new(tls) as BoxedStream),
            Ok(Err(e)) => Err(UpstreamError::Tls(e.to_string())),
            Err(_) => Err(UpstreamError::Timeout),
        }
    }
}

impl Upstream for TcpUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(&'a self, wire: &'a [u8], deadline: Instant) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            // One exchange at a time per connection; concurrency comes from
            // querying several upstreams at once.
            let mut guard = self.conn.lock().await;
            if guard.is_none() {
                *guard = Some(self.connect(deadline).await?);
            }
            let stream = guard.as_mut().expect("connection present");
            match exchange_framed(stream, wire, deadline).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    *guard = None;
                    Err(e)
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// Framed echo server: replies with the request bytes, `replies` times per connection.
    async fn framed_echo(replies: usize) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    for _ in 0..replies {
                        let mut len = [0u8; 2];
                        if s.read_exact(&mut len).await.is_err() {
                            return;
                        }
                        let n = usize::from(u16::from_be_bytes(len));
                        let mut msg = vec![0u8; n];
                        if s.read_exact(&mut msg).await.is_err() {
                            return;
                        }
                        let mut out = len.to_vec();
                        out.extend_from_slice(&msg);
                        if s.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        addr
    }

    fn connector() -> Arc<dyn Connector> {
        Arc::new(DirectConnector::new(Arc::new(SystemResolve)))
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn framing_round_trip_over_duplex() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        let server = tokio::spawn(async move {
            let mut len = [0u8; 2];
            b.read_exact(&mut len).await.unwrap();
            let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
            b.read_exact(&mut msg).await.unwrap();
            assert_eq!(msg, b"hello");
            b.write_all(&[0, 3, b'a', b'b', b'c']).await.unwrap();
        });
        let resp = exchange_framed(&mut a, b"hello", deadline(1000)).await.unwrap();
        assert_eq!(resp, b"abc");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn plain_tcp_reuses_the_connection_and_reconnects_after_close() {
        let addr = framed_echo(2).await;
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        assert_eq!(up.name(), format!("tcp://127.0.0.1:{}", addr.port()));
        assert_eq!(up.query(b"\x12\x34one", deadline(1000)).await.unwrap(), b"\x12\x34one");
        assert_eq!(up.query(b"\x12\x34two", deadline(1000)).await.unwrap(), b"\x12\x34two");
        // The server closes after two replies: the third query fails once, then reconnects.
        let third = up.query(b"\x12\x34three", deadline(1000)).await;
        assert!(third.is_err() || third.as_deref() == Ok(b"\x12\x34three"));
        assert_eq!(up.query(b"\x12\x34four", deadline(1000)).await.unwrap(), b"\x12\x34four");
    }

    #[tokio::test]
    async fn timeout_when_the_server_never_answers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_s, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let up = TcpUpstream::plain("127.0.0.1", addr.port(), connector());
        assert_eq!(up.query(b"\x00\x01x", deadline(200)).await, Err(UpstreamError::Timeout));
    }

    #[tokio::test]
    async fn dot_over_a_self_signed_server() {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
        let cert_der = cert.der().clone();
        let key_der: rustls::pki_types::PrivateKeyDer<'static> = signing_key.into();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der)
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (tcp, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let mut tls = acceptor.accept(tcp).await.unwrap();
                    let mut len = [0u8; 2];
                    tls.read_exact(&mut len).await.unwrap();
                    let mut msg = vec![0u8; usize::from(u16::from_be_bytes(len))];
                    tls.read_exact(&mut msg).await.unwrap();
                    let mut out = len.to_vec();
                    out.extend_from_slice(&msg);
                    tls.write_all(&out).await.unwrap();
                });
            }
        });
        let config = rurge_net::http::tls_client_config(true).unwrap();
        let up = TcpUpstream::tls("127.0.0.1", addr.port(), connector(), config);
        assert_eq!(up.name(), format!("tls://127.0.0.1:{}", addr.port()));
        assert_eq!(up.query(b"\xab\xcdsecure", deadline(2000)).await.unwrap(), b"\xab\xcdsecure");
        let strict = rurge_net::http::tls_client_config(false).unwrap();
        let strict_up = TcpUpstream::tls("127.0.0.1", addr.port(), connector(), strict);
        assert!(matches!(strict_up.query(b"\x00\x02x", deadline(2000)).await, Err(UpstreamError::Tls(_))));
    }
}
```

`lib.rs`：`pub mod upstream;` 并再导出 `pub use upstream::{Upstream, UpstreamError, UpstreamRef, UpstreamSpec};`。

> 测试依赖 `rcgen`、`rustls`、`tokio-rustls`（Task 1 已加入 `rurge-dns` 的依赖 / dev-dependencies）。`rustls::crypto::ring::default_provider` 需要 rustls 的 `ring` feature（workspace 已启用）。

- [ ] **Step 3: 运行**

```bash
cargo test -p rurge-dns upstream
```

预期：3 个 `UpstreamSpec` 测试 + 4 个 TCP / DoT 测试通过。

- [ ] **Step 4: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): Upstream 抽象与 UpstreamSpec 解析；TCP / DoT 持久连接传输

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 6: UDP 上游 `UdpUpstream`

**Files:**
- Modify: `crates/rurge-dns/src/upstream/mod.rs`（追加 `pub mod udp;`）
- Create: `crates/rurge-dns/src/upstream/udp.rs`

**Interfaces:**
- Consumes: `super::{Upstream, UpstreamError}`、`super::tcp::exchange_framed`、`crate::message::{wire_id, wire_truncated}`（Task 3：`wire_id(&[u8]) -> Option<u16>` 要求至少 12 字节头；`wire_truncated(&[u8]) -> bool` 读 TC 位）、`tokio::net::{UdpSocket, TcpStream}`、`tokio::sync::{OnceCell, oneshot}`。
- Produces（`rurge_dns::upstream::udp::*`）：`UDP_BUFFER: usize = 4096`；`UdpUpstream::new(addr: SocketAddr) -> UdpUpstream`；实现 `Upstream`（名字 `udp://<addr>`）；并发查询安全：一个后台接收循环按报文 ID 把应答分发给等待者；TC 应答改用 TCP 向同一地址重发一次；`Drop` 终止接收循环。

- [ ] **Step 1: 写 `upstream/udp.rs`（含测试）**

```rust
//! DNS over UDP (design §7.2): one connected socket per upstream, a receive
//! loop that routes answers to waiters by message ID (so several queries can
//! be in flight at once), and one retry over TCP when the answer is truncated.

use super::tcp::exchange_framed;
use super::{Upstream, UpstreamError};
use crate::message::{wire_id, wire_truncated};
use rurge_net::BoxFuture;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpStream, UdpSocket};
use tokio::sync::{OnceCell, oneshot};
use tokio::task::JoinHandle;
use tokio::time::Instant;

pub const UDP_BUFFER: usize = 4096;

struct Shared {
    socket: UdpSocket,
    pending: Mutex<HashMap<u16, oneshot::Sender<Vec<u8>>>>,
}

pub struct UdpUpstream {
    name: String,
    addr: SocketAddr,
    state: OnceCell<(Arc<Shared>, JoinHandle<()>)>,
}

fn io_err(e: std::io::Error) -> UpstreamError {
    UpstreamError::Io(e.to_string())
}

impl UdpUpstream {
    pub fn new(addr: SocketAddr) -> UdpUpstream {
        UdpUpstream {
            name: format!("udp://{addr}"),
            addr,
            state: OnceCell::new(),
        }
    }

    async fn shared(&self) -> Result<Arc<Shared>, UpstreamError> {
        let (shared, _) = self
            .state
            .get_or_try_init(|| async {
                let bind: SocketAddr = if self.addr.is_ipv4() {
                    "0.0.0.0:0".parse().expect("valid")
                } else {
                    "[::]:0".parse().expect("valid")
                };
                let socket = UdpSocket::bind(bind).await.map_err(io_err)?;
                socket.connect(self.addr).await.map_err(io_err)?;
                let shared = Arc::new(Shared {
                    socket,
                    pending: Mutex::new(HashMap::new()),
                });
                let receiver = Arc::clone(&shared);
                let task = tokio::spawn(async move {
                    let mut buf = vec![0u8; UDP_BUFFER];
                    loop {
                        let n = match receiver.socket.recv(&mut buf).await {
                            Ok(n) => n,
                            Err(_) => continue,
                        };
                        let Some(id) = wire_id(&buf[..n]) else { continue };
                        let waiter = receiver.pending.lock().expect("udp pending lock").remove(&id);
                        if let Some(tx) = waiter {
                            let _ = tx.send(buf[..n].to_vec());
                        }
                    }
                });
                Ok::<(Arc<Shared>, JoinHandle<()>), UpstreamError>((shared, task))
            })
            .await?;
        Ok(Arc::clone(shared))
    }
}

impl Drop for UdpUpstream {
    fn drop(&mut self) {
        if let Some((_, task)) = self.state.get() {
            task.abort();
        }
    }
}

impl Upstream for UdpUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(&'a self, wire: &'a [u8], deadline: Instant) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            let id = wire_id(wire).ok_or_else(|| UpstreamError::BadResponse("query shorter than a DNS header".to_string()))?;
            let shared = self.shared().await?;
            let (tx, rx) = oneshot::channel();
            shared.pending.lock().expect("udp pending lock").insert(id, tx);
            if let Err(e) = shared.socket.send(wire).await {
                shared.pending.lock().expect("udp pending lock").remove(&id);
                return Err(io_err(e));
            }
            let resp = match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(bytes)) => bytes,
                Ok(Err(_)) => return Err(UpstreamError::Io("receiver dropped".to_string())),
                Err(_) => {
                    shared.pending.lock().expect("udp pending lock").remove(&id);
                    return Err(UpstreamError::Timeout);
                }
            };
            if !wire_truncated(&resp) {
                return Ok(resp);
            }
            // Truncated: repeat the same query once over TCP to the same server.
            let mut tcp = match tokio::time::timeout_at(deadline, TcpStream::connect(self.addr)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => return Err(io_err(e)),
                Err(_) => return Err(UpstreamError::Timeout),
            };
            exchange_framed(&mut tcp, wire, deadline).await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// A 12-byte header with the given id and flags, followed by `payload`.
    fn msg(id: u16, flags: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v[0..2].copy_from_slice(&id.to_be_bytes());
        v[2] = flags;
        v.extend_from_slice(payload);
        v
    }

    /// Raw UDP server whose behaviour is a function of the request bytes:
    /// `None` = drop; `Some((delay, reply))` = answer after `delay`.
    async fn raw_udp(behaviour: impl Fn(&[u8]) -> Option<(Duration, Vec<u8>)> + Send + Sync + 'static) -> SocketAddr {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = socket.local_addr().unwrap();
        let behaviour = Arc::new(behaviour);
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else { return };
                if let Some((delay, reply)) = behaviour(&buf[..n]) {
                    let socket = Arc::clone(&socket);
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = socket.send_to(&reply, peer).await;
                    });
                }
            }
        });
        addr
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn echoes_and_matches_ids_under_concurrency() {
        let addr = raw_udp(|req| {
            // Answer the first id slowly, everything else at once, echoing the request.
            let delay = if req[0..2] == [0, 1] { Duration::from_millis(150) } else { Duration::ZERO };
            Some((delay, req.to_vec()))
        })
        .await;
        let up = Arc::new(UdpUpstream::new(addr));
        assert_eq!(up.name(), format!("udp://{addr}"));
        let a = {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(1, 0, b"slow"), deadline(2000)).await })
        };
        let b = {
            let up = Arc::clone(&up);
            tokio::spawn(async move { up.query(&msg(2, 0, b"fast"), deadline(2000)).await })
        };
        assert_eq!(b.await.unwrap().unwrap(), msg(2, 0, b"fast"));
        assert_eq!(a.await.unwrap().unwrap(), msg(1, 0, b"slow"));
    }

    #[tokio::test]
    async fn ignores_answers_with_a_foreign_id() {
        let addr = raw_udp(|req| {
            let mut wrong = req.to_vec();
            wrong[0] ^= 0xff;
            // First a wrong-id reply, then the real one 20 ms later.
            Some((Duration::ZERO, wrong))
        })
        .await;
        // The behaviour closure can only send one reply; send the real one from a second server task.
        let up = UdpUpstream::new(addr);
        let err = up.query(&msg(7, 0, b"x"), deadline(200)).await;
        assert_eq!(err, Err(UpstreamError::Timeout), "a foreign id must not satisfy the waiter");
    }

    #[tokio::test]
    async fn drops_time_out() {
        let addr = raw_udp(|_| None).await;
        let up = UdpUpstream::new(addr);
        assert_eq!(up.query(&msg(3, 0, b"x"), deadline(150)).await, Err(UpstreamError::Timeout));
    }

    #[tokio::test]
    async fn truncated_answer_is_retried_over_tcp() {
        // TCP echo bound first so the UDP server can share the port number.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    s.read_exact(&mut len).await.unwrap();
                    let mut m = vec![0u8; usize::from(u16::from_be_bytes(len))];
                    s.read_exact(&mut m).await.unwrap();
                    let mut full = m.clone();
                    full.extend_from_slice(b"-full");
                    let mut out = (full.len() as u16).to_be_bytes().to_vec();
                    out.extend_from_slice(&full);
                    s.write_all(&out).await.unwrap();
                });
            }
        });
        let socket = UdpSocket::bind(("127.0.0.1", port)).await.unwrap();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            loop {
                let Ok((n, peer)) = socket.recv_from(&mut buf).await else { return };
                let mut reply = buf[..n].to_vec();
                reply[2] |= 0x02; // TC
                let _ = socket.send_to(&reply, peer).await;
            }
        });
        let up = UdpUpstream::new(SocketAddr::from(([127, 0, 0, 1], port)));
        let resp = up.query(&msg(9, 0, b"big"), deadline(2000)).await.unwrap();
        assert_eq!(resp, [msg(9, 0, b"big"), b"-full".to_vec()].concat());
    }

    #[tokio::test]
    async fn rejects_queries_without_a_header() {
        let up = UdpUpstream::new("127.0.0.1:9".parse().unwrap());
        assert!(matches!(up.query(b"short", deadline(100)).await, Err(UpstreamError::BadResponse(_))));
    }
}
```

`upstream/mod.rs` 追加 `pub mod udp;`。

> 说明：`ignores_answers_with_a_foreign_id` 只验证「错误 ID 的应答不会满足等待者」（随后超时）；正确 ID 迟到的路径由第一个测试覆盖。同一时刻两个查询若随机到相同 ID，后者会覆盖前者的等待者（前者超时后由 fanout 重发），概率 1/65536，登记为已知限制。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns udp
```

预期：5 个测试通过。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): UDP 上游（按 ID 分发的接收循环、TC 后 TCP 重发）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 7: DoH 上游 `DohUpstream`

**Files:**
- Modify: `crates/rurge-dns/src/upstream/mod.rs`（追加 `pub mod doh;`）
- Create: `crates/rurge-dns/src/upstream/doh.rs`

**Interfaces:**
- Consumes: `super::{Upstream, UpstreamError}`、`rurge_net::http::HttpClient::send(&self, Request<Full<Bytes>>, timeout: Duration) -> Result<http::Response<Incoming>, HttpError>`、`http::{Request, header::{ACCEPT, CONTENT_TYPE}}`、`http_body_util::{BodyExt, Full, Limited}`、`bytes::Bytes`、`rurge_net::testing::TestServer`（测试：`spawn()`、`spawn_tls()`、`set`、`set_header`、`set_status`、`requests()`、`url()`）。
- Produces（`rurge_dns::upstream::doh::*`）：`DNS_MESSAGE: &str = "application/dns-message"`；`MAX_RESPONSE: usize = 65_535`；`DohUpstream::new(url: Url, http: Arc<HttpClient>) -> DohUpstream`；实现 `Upstream`（名字 = URL 原文；RFC 8484 POST；发送时报文 ID 置 0，返回时恢复调用方 ID；非 200 → `Http`，非 `application/dns-message` → `BadResponse`）。

- [ ] **Step 1: 写 `upstream/doh.rs`（含测试）**

```rust
//! DNS over HTTPS (design §7.2): RFC 8484 POST bodies of type
//! `application/dns-message` through the shared HTTP client (HTTP/2 when the
//! server negotiates it). The message ID is sent as 0 (RFC 8484 §4.1) and the
//! caller's ID is restored on the reply.

use super::{Upstream, UpstreamError};
use bytes::Bytes;
use http::Request;
use http::header::{ACCEPT, CONTENT_TYPE};
use http_body_util::{BodyExt, Full, Limited};
use rurge_net::BoxFuture;
use rurge_net::http::HttpClient;
use std::sync::Arc;
use tokio::time::Instant;
use url::Url;

pub const DNS_MESSAGE: &str = "application/dns-message";
pub const MAX_RESPONSE: usize = 65_535;

pub struct DohUpstream {
    name: String,
    url: Url,
    http: Arc<HttpClient>,
}

impl DohUpstream {
    pub fn new(url: Url, http: Arc<HttpClient>) -> DohUpstream {
        DohUpstream {
            name: url.as_str().to_string(),
            url,
            http,
        }
    }
}

impl Upstream for DohUpstream {
    fn name(&self) -> &str {
        &self.name
    }

    fn query<'a>(&'a self, wire: &'a [u8], deadline: Instant) -> BoxFuture<'a, Result<Vec<u8>, UpstreamError>> {
        Box::pin(async move {
            if wire.len() < 12 {
                return Err(UpstreamError::BadResponse("query shorter than a DNS header".to_string()));
            }
            let id = [wire[0], wire[1]];
            let mut body = wire.to_vec();
            body[0] = 0;
            body[1] = 0;
            let req = Request::post(self.url.as_str())
                .header(CONTENT_TYPE, DNS_MESSAGE)
                .header(ACCEPT, DNS_MESSAGE)
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| UpstreamError::Http(e.to_string()))?;
            let timeout = deadline.saturating_duration_since(Instant::now());
            let resp = self
                .http
                .send(req, timeout)
                .await
                .map_err(|e| UpstreamError::Http(e.to_string()))?;
            let status = resp.status();
            if status != http::StatusCode::OK {
                return Err(UpstreamError::Http(format!("status {status}")));
            }
            let content_type = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_ascii_lowercase();
            if !content_type.starts_with(DNS_MESSAGE) {
                return Err(UpstreamError::BadResponse(format!("content-type `{content_type}`")));
            }
            let collected = match tokio::time::timeout_at(deadline, Limited::new(resp.into_body(), MAX_RESPONSE).collect()).await {
                Ok(Ok(c)) => c,
                Ok(Err(e)) => return Err(UpstreamError::Http(e.to_string())),
                Err(_) => return Err(UpstreamError::Timeout),
            };
            let mut bytes = collected.to_bytes().to_vec();
            if bytes.len() < 12 {
                return Err(UpstreamError::BadResponse("response shorter than a DNS header".to_string()));
            }
            bytes[0] = id[0];
            bytes[1] = id[1];
            Ok(bytes)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::http::HttpClientConfig;
    use rurge_net::testing::TestServer;
    use std::time::Duration;

    fn client(skip_verify: bool) -> Arc<HttpClient> {
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        Arc::new(
            HttpClient::new(
                connector,
                HttpClientConfig {
                    skip_cert_verification: skip_verify,
                    ..HttpClientConfig::default()
                },
            )
            .unwrap(),
        )
    }

    fn header_with_id(id: u16) -> Vec<u8> {
        let mut v = vec![0u8; 12];
        v[0..2].copy_from_slice(&id.to_be_bytes());
        v.extend_from_slice(b"payload");
        v
    }

    fn deadline(ms: u64) -> Instant {
        Instant::now() + Duration::from_millis(ms)
    }

    #[tokio::test]
    async fn posts_dns_message_and_restores_the_id() {
        let server = TestServer::spawn().await;
        let mut canned = vec![0u8; 12];
        canned[2] = 0x81; // QR + RD
        canned.extend_from_slice(b"answer");
        server.set("/dns-query", canned.clone());
        server.set_header("/dns-query", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/dns-query"), client(false));
        assert_eq!(up.name(), server.url("/dns-query").as_str());
        let resp = up.query(&header_with_id(0x1234), deadline(2000)).await.unwrap();
        assert_eq!(&resp[0..2], &[0x12, 0x34]);
        assert_eq!(&resp[2..], &canned[2..]);
        let req = &server.requests()[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.header("content-type"), Some("application/dns-message"));
        assert_eq!(req.header("accept"), Some("application/dns-message"));
    }

    #[tokio::test]
    async fn https_with_http2() {
        let server = TestServer::spawn_tls().await;
        server.set("/dns-query", vec![0u8; 12]);
        server.set_header("/dns-query", "content-type", "application/dns-message");
        let up = DohUpstream::new(server.url("/dns-query"), client(true));
        let resp = up.query(&header_with_id(1), deadline(3000)).await.unwrap();
        assert_eq!(&resp[0..2], &[0, 1]);
        assert_eq!(server.requests()[0].version, "HTTP/2.0");
    }

    #[tokio::test]
    async fn bad_status_and_content_type_are_errors() {
        let server = TestServer::spawn().await;
        server.set("/html", vec![0u8; 12]);
        server.set_header("/html", "content-type", "text/html");
        let up = DohUpstream::new(server.url("/html"), client(false));
        assert!(matches!(up.query(&header_with_id(2), deadline(2000)).await, Err(UpstreamError::BadResponse(_))));
        server.set("/fail", vec![0u8; 12]);
        server.set_status("/fail", 500);
        let up = DohUpstream::new(server.url("/fail"), client(false));
        assert!(matches!(up.query(&header_with_id(3), deadline(2000)).await, Err(UpstreamError::Http(_))));
        let up = DohUpstream::new(server.url("/missing"), client(false));
        assert!(matches!(up.query(&header_with_id(4), deadline(2000)).await, Err(UpstreamError::Http(_))));
    }

    #[tokio::test]
    async fn slow_server_times_out() {
        let server = TestServer::spawn().await;
        server.set("/slow", vec![0u8; 12]);
        server.set_header("/slow", "content-type", "application/dns-message");
        server.set_delay("/slow", Duration::from_secs(3));
        let up = DohUpstream::new(server.url("/slow"), client(false));
        assert!(matches!(up.query(&header_with_id(5), deadline(200)).await, Err(UpstreamError::Http(_) | UpstreamError::Timeout)));
    }
}
```

`upstream/mod.rs` 追加 `pub mod doh;`。

> `HttpClient::send` 的超时只覆盖到响应头到达；正文用 `deadline` 再限一次。`TestServer` 对未设置的路径返回 404 → `Http`。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns doh
```

预期：4 个测试通过（HTTPS 用例需 `rurge-net` 的 `testing` feature，Task 1 已在 dev-dependencies 启用）。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): DoH 上游（RFC 8484 POST，经内部 HTTP 客户端）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 8: 并发查询与重试 `fanout`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/fanout.rs`

**Interfaces:**
- Consumes: `crate::message::{Answer, Qtype, Question, Rcode, CodecError, build_query, parse_response, random_id}`（Task 3）、`crate::upstream::{UpstreamError, UpstreamRef}`、`crate::testing::MockDns`（Task 4，测试）、`tokio::task::JoinSet`、`tokio::time::{Instant, sleep_until}`。
  - `Answer` 提供 `is_valid_for(&Question) -> bool`（NOERROR、问题段匹配、含该类型记录）与 `is_empty_for(&Question) -> bool`（NOERROR / NXDOMAIN 且无该类型记录）；`Answer.rcode: Rcode`（`Display`）。
- Produces（`rurge_dns::fanout::*`）：
  - `FanoutOpts { resend: Duration, attempts: u32 }`（`Default`：1 s、5）。
  - `Answers { v4: Vec<(Ipv4Addr, u32)>, v6: Vec<(Ipv6Addr, u32)>, upstream: String, partial: bool, aaaa_timed_out: bool }`（`Default`）。
  - `FanoutError::{EmptyAnswer, Timeout, AllFailed(Vec<(String, String)>)}`（`Display`）。
  - `resolve_name(upstreams: &[UpstreamRef], name: &str, want_v6: bool, opts: &FanoutOpts) -> Result<Answers, FanoutError>`（async）。
  - 追踪事件（供 `rurge dns lookup --trace` 显示）：`tracing::debug!(target: "rurge_dns::fanout", ...)`，消息为 `send`（字段 `upstream`、`qtype`、`round`）、`answer`（`upstream`、`qtype`、`rcode`、`records`、`elapsed_ms`）、`failed`（`upstream`、`qtype`、`error`、`elapsed_ms`）。
  - 语义（binding）：每轮向全部上游并发发送（A；`want_v6` 时同时发 AAAA），每 `resend` 一轮，共 `attempts` 轮，总截止 = `resend × attempts`；首个有效应答获胜；某问题只有全部上游都明确空应答才判 Empty；在重发定时器触发时若一种记录已到而另一种未到，以部分结果完成（`partial = true`；A 已到而 AAAA 未到时 `aaaa_timed_out = true`）；截止时：有有效应答 → 返回（部分）；否则若某问题有过空应答且其余只是超时 → `EmptyAnswer`；否则有失败记录 → `AllFailed`，否则 `Timeout`。

- [ ] **Step 1: 写 `fanout.rs`**

```rust
//! Concurrent query engine (design §7.3, manual `dns/overview.html`): every
//! selected upstream is asked at once, the query is re-sent every `resend`
//! until `attempts` rounds went out, the first valid answer wins, and "empty"
//! is reported only when every upstream said so (or some said so and the
//! rest never answered).

use crate::message::{Answer, Qtype, Question, build_query, parse_response, random_id};
use crate::upstream::{UpstreamError, UpstreamRef};
use std::collections::HashSet;
use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

#[derive(Clone, Debug)]
pub struct FanoutOpts {
    pub resend: Duration,
    pub attempts: u32,
}

impl Default for FanoutOpts {
    fn default() -> Self {
        FanoutOpts {
            resend: Duration::from_secs(1),
            attempts: 5,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    pub v4: Vec<(Ipv4Addr, u32)>,
    pub v6: Vec<(Ipv6Addr, u32)>,
    /// Upstream whose answer won (the A answer's, else the AAAA answer's).
    pub upstream: String,
    /// One family is missing because its answer had not arrived at a resend tick.
    pub partial: bool,
    /// A arrived but AAAA never did — feeds AAAA suppression.
    pub aaaa_timed_out: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FanoutError {
    EmptyAnswer,
    Timeout,
    AllFailed(Vec<(String, String)>),
}

impl fmt::Display for FanoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FanoutError::EmptyAnswer => f.write_str("empty answer"),
            FanoutError::Timeout => f.write_str("timeout"),
            FanoutError::AllFailed(list) => {
                if list.is_empty() {
                    return f.write_str("no upstream");
                }
                let parts: Vec<String> = list.iter().map(|(u, e)| format!("{u}: {e}")).collect();
                write!(f, "all upstreams failed ({})", parts.join("; "))
            }
        }
    }
}

struct Outcome {
    upstream: String,
    qtype: Qtype,
    result: Result<Answer, UpstreamError>,
}

enum State {
    Pending,
    Valid(Answer, String),
    Empty,
}

struct Track {
    question: Question,
    state: State,
    empties: HashSet<String>,
    failures: Vec<(String, String)>,
}

impl Track {
    fn decided(&self) -> bool {
        !matches!(self.state, State::Pending)
    }

    fn apply(&mut self, upstream: &str, result: Result<Answer, UpstreamError>, total_upstreams: usize) {
        if self.decided() {
            return;
        }
        match result {
            Ok(answer) if answer.is_valid_for(&self.question) => {
                self.state = State::Valid(answer, upstream.to_string());
            }
            Ok(answer) if answer.is_empty_for(&self.question) => {
                self.empties.insert(upstream.to_string());
                if self.empties.len() >= total_upstreams {
                    self.state = State::Empty;
                }
            }
            Ok(answer) => self.failures.push((upstream.to_string(), format!("rcode {}", answer.rcode))),
            Err(e) => self.failures.push((upstream.to_string(), e.to_string())),
        }
    }

    /// At the deadline: "some upstreams answered empty and the rest never answered" counts as empty.
    fn settle_empty(&mut self) {
        if !self.decided() && !self.empties.is_empty() && self.failures.is_empty() {
            self.state = State::Empty;
        }
    }
}

async fn one_query(upstream: UpstreamRef, question: Question, deadline: Instant) -> Outcome {
    let qtype = question.qtype;
    let name = upstream.name().to_string();
    let started = Instant::now();
    let result = async {
        let id = random_id();
        let wire = build_query(id, &question).map_err(|e| UpstreamError::BadResponse(e.to_string()))?;
        let bytes = upstream.query(&wire, deadline).await?;
        let answer = parse_response(&bytes).map_err(|e| UpstreamError::BadResponse(e.to_string()))?;
        if answer.id != id {
            return Err(UpstreamError::BadResponse(format!("id mismatch: sent {id}, got {}", answer.id)));
        }
        Ok(answer)
    }
    .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &result {
        Ok(a) => tracing::debug!(target: "rurge_dns::fanout", upstream = %name, qtype = qtype.as_str(), rcode = %a.rcode, records = a.v4.len() + a.v6.len(), elapsed_ms, "answer"),
        Err(e) => tracing::debug!(target: "rurge_dns::fanout", upstream = %name, qtype = qtype.as_str(), error = %e, elapsed_ms, "failed"),
    }
    Outcome {
        upstream: name,
        qtype,
        result,
    }
}

pub async fn resolve_name(
    upstreams: &[UpstreamRef],
    name: &str,
    want_v6: bool,
    opts: &FanoutOpts,
) -> Result<Answers, FanoutError> {
    if upstreams.is_empty() {
        return Err(FanoutError::AllFailed(Vec::new()));
    }
    let start = Instant::now();
    let attempts = opts.attempts.max(1);
    let deadline = start + opts.resend * attempts;
    let mut tracks: Vec<Track> = [Qtype::A, Qtype::Aaaa]
        .into_iter()
        .filter(|q| *q == Qtype::A || want_v6)
        .map(|qtype| Track {
            question: Question {
                name: name.to_string(),
                qtype,
            },
            state: State::Pending,
            empties: HashSet::new(),
            failures: Vec::new(),
        })
        .collect();
    let mut set: JoinSet<Outcome> = JoinSet::new();
    let mut round = 0u32;
    let mut next_send = start;
    let mut partial = false;
    loop {
        if round < attempts && Instant::now() >= next_send {
            for up in upstreams {
                for t in tracks.iter().filter(|t| !t.decided()) {
                    tracing::debug!(target: "rurge_dns::fanout", upstream = %up.name(), qtype = t.question.qtype.as_str(), round, "send");
                    set.spawn(one_query(up.clone(), t.question.clone(), deadline));
                }
            }
            round += 1;
            next_send = start + opts.resend * round;
        }
        if tracks.iter().all(Track::decided) {
            break;
        }
        let tick = if round < attempts { next_send } else { deadline };
        tokio::select! {
            joined = set.join_next(), if !set.is_empty() => {
                if let Some(Ok(outcome)) = joined {
                    if let Some(t) = tracks.iter_mut().find(|t| t.question.qtype == outcome.qtype) {
                        t.apply(&outcome.upstream, outcome.result, upstreams.len());
                    }
                }
            }
            _ = sleep_until(tick) => {
                if round >= attempts {
                    break;
                }
                let any_valid = tracks.iter().any(|t| matches!(t.state, State::Valid(..)));
                let any_pending = tracks.iter().any(|t| !t.decided());
                if any_valid && any_pending {
                    partial = true;
                    break;
                }
            }
        }
    }
    set.abort_all();
    for t in &mut tracks {
        t.settle_empty();
    }
    let mut answers = Answers {
        partial,
        ..Answers::default()
    };
    let mut any_valid = false;
    let mut any_empty = false;
    let mut failures = Vec::new();
    for t in &mut tracks {
        match std::mem::replace(&mut t.state, State::Pending) {
            State::Valid(answer, upstream) => {
                any_valid = true;
                if answers.upstream.is_empty() || t.question.qtype == Qtype::A {
                    answers.upstream = upstream;
                }
                match t.question.qtype {
                    Qtype::A => answers.v4 = answer.v4,
                    Qtype::Aaaa => answers.v6 = answer.v6,
                }
            }
            State::Empty => any_empty = true,
            State::Pending => {
                if t.question.qtype == Qtype::Aaaa {
                    answers.aaaa_timed_out = true;
                }
                answers.partial = true;
                failures.append(&mut t.failures);
            }
        }
    }
    if any_valid {
        return Ok(answers);
    }
    if any_empty {
        return Err(FanoutError::EmptyAnswer);
    }
    if !failures.is_empty() {
        failures.sort();
        failures.dedup();
        return Err(FanoutError::AllFailed(failures));
    }
    Err(FanoutError::Timeout)
}
```

`lib.rs`：`pub mod fanout;`。

> `aaaa_timed_out` 只在 A 有效而 AAAA 仍 Pending 时为真（Empty 的 AAAA 不算超时）。`answers.partial` 在「有效 + 另一种 Pending」时为真；`want_v6 = false` 时永远为假。

- [ ] **Step 2: 写测试**

在 `fanout.rs` 末尾追加（`MockDns` 来自 Task 4）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Rcode;
    use crate::testing::MockDns;
    use crate::upstream::udp::UdpUpstream;
    use std::sync::Arc;

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 4,
        }
    }

    fn up(server: &MockDns) -> UpstreamRef {
        Arc::new(UdpUpstream::new(server.addr()))
    }

    #[tokio::test]
    async fn single_upstream_answers() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1", "10.0.0.2"], &[], 300);
        let ans = resolve_name(&[up(&s)], "a.test", false, &fast()).await.unwrap();
        assert_eq!(ans.v4, vec![("10.0.0.1".parse().unwrap(), 300), ("10.0.0.2".parse().unwrap(), 300)]);
        assert!(ans.v6.is_empty() && !ans.partial && !ans.aaaa_timed_out);
        assert_eq!(ans.upstream, format!("udp://{}", s.addr()));
        assert_eq!(s.query_count("a.test", Qtype::A), 1);
        assert_eq!(s.query_count("a.test", Qtype::Aaaa), 0, "AAAA not asked when want_v6 is false");
    }

    #[tokio::test]
    async fn first_valid_answer_wins() {
        let slow = MockDns::spawn().await;
        slow.set("a.test", &["10.0.0.9"], &[], 60);
        slow.set_delay(Duration::from_millis(300));
        let fast_srv = MockDns::spawn().await;
        fast_srv.set("a.test", &["10.0.0.1"], &[], 60);
        let started = Instant::now();
        let ans = resolve_name(&[up(&slow), up(&fast_srv)], "a.test", false, &fast()).await.unwrap();
        assert_eq!(ans.v4[0].0, "10.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(ans.upstream, format!("udp://{}", fast_srv.addr()));
        assert!(started.elapsed() < Duration::from_millis(250));
    }

    #[tokio::test]
    async fn resends_after_the_timer_and_recovers() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_first(1);
        let started = Instant::now();
        let ans = resolve_name(&[up(&s)], "a.test", false, &fast()).await.unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert!(started.elapsed() >= Duration::from_millis(100), "answer came from the second round");
        assert_eq!(s.query_count("a.test", Qtype::A), 2);
    }

    #[tokio::test]
    async fn all_dropped_is_a_timeout_after_every_attempt() {
        let s = MockDns::spawn().await;
        s.set("a.test", &["10.0.0.1"], &[], 60);
        s.set_drop_all(true);
        let started = Instant::now();
        let err = resolve_name(&[up(&s)], "a.test", false, &fast()).await.unwrap_err();
        assert_eq!(err, FanoutError::Timeout);
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert_eq!(s.query_count("a.test", Qtype::A), 4);
    }

    #[tokio::test]
    async fn empty_answer_rules() {
        // all empty → EmptyAnswer immediately
        let e1 = MockDns::spawn().await;
        e1.set_empty("nx.test");
        let e2 = MockDns::spawn().await;
        e2.set_empty("nx.test");
        let started = Instant::now();
        assert_eq!(resolve_name(&[up(&e1), up(&e2)], "nx.test", false, &fast()).await, Err(FanoutError::EmptyAnswer));
        assert!(started.elapsed() < Duration::from_millis(100));
        // one empty + one dropping → EmptyAnswer at the deadline
        let d = MockDns::spawn().await;
        d.set_drop_all(true);
        assert_eq!(resolve_name(&[up(&e1), up(&d)], "nx.test", false, &fast()).await, Err(FanoutError::EmptyAnswer));
        // one empty + one valid → valid
        let v = MockDns::spawn().await;
        v.set("nx.test", &["10.0.0.5"], &[], 60);
        let ans = resolve_name(&[up(&e1), up(&v)], "nx.test", false, &fast()).await.unwrap();
        assert_eq!(ans.upstream, format!("udp://{}", v.addr()));
        // unknown names answer NXDOMAIN, which is also "empty"
        assert_eq!(resolve_name(&[up(&v)], "unknown.test", false, &fast()).await, Err(FanoutError::EmptyAnswer));
    }

    #[tokio::test]
    async fn server_failures_are_reported() {
        let s = MockDns::spawn().await;
        s.set_rcode("bad.test", Rcode::ServFail);
        match resolve_name(&[up(&s)], "bad.test", false, &fast()).await {
            Err(FanoutError::AllFailed(list)) => {
                assert_eq!(list.len(), 1);
                assert!(list[0].1.contains("SERVFAIL") || list[0].1.contains("rcode"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(resolve_name(&[], "a.test", false, &fast()).await, Err(FanoutError::AllFailed(Vec::new())));
    }

    #[tokio::test]
    async fn a_and_aaaa_in_parallel_with_partial_results() {
        let s = MockDns::spawn().await;
        s.set("dual.test", &["10.0.0.1"], &["fd00::1"], 60);
        let ans = resolve_name(&[up(&s)], "dual.test", true, &fast()).await.unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert_eq!(ans.v6.len(), 1);
        assert!(!ans.partial && !ans.aaaa_timed_out);
        assert_eq!(s.query_count("dual.test", Qtype::Aaaa), 1);

        s.set_drop_qtype(Qtype::Aaaa, true);
        let started = Instant::now();
        let ans = resolve_name(&[up(&s)], "dual.test", true, &fast()).await.unwrap();
        assert_eq!(ans.v4.len(), 1);
        assert!(ans.v6.is_empty());
        assert!(ans.partial && ans.aaaa_timed_out, "A answered, AAAA missing at the resend tick");
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(350), "{elapsed:?}");

        s.set_drop_qtype(Qtype::Aaaa, false);
        s.set_drop_qtype(Qtype::A, true);
        let ans = resolve_name(&[up(&s)], "dual.test", true, &fast()).await.unwrap();
        assert!(ans.v4.is_empty() && ans.v6.len() == 1);
        assert!(ans.partial && !ans.aaaa_timed_out);

        // AAAA empty (no record) is not "timed out"
        s.set_drop_qtype(Qtype::A, false);
        s.set("v4only.test", &["10.0.0.2"], &[], 60);
        let ans = resolve_name(&[up(&s)], "v4only.test", true, &fast()).await.unwrap();
        assert!(!ans.partial && !ans.aaaa_timed_out && ans.v6.is_empty());
    }
}
```

- [ ] **Step 3: 运行**

```bash
cargo test -p rurge-dns fanout
```

预期：7 个测试通过（含时序断言，总耗时约 3 s）。

- [ ] **Step 4: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): 并发查询引擎：全上游并发、定时重发、首个有效应答、空应答规则、A / AAAA 部分结果

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 9: 引导解析 `Bootstrap` 与 `BootstrapConnector`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/bootstrap.rs`

**Interfaces:**
- Consumes: `crate::cache::{CachedAddrs, CacheHit, DnsCache}`、`crate::fanout::{FanoutOpts, resolve_name}`、`crate::upstream::{UpstreamError, UpstreamRef}`、`rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target, interleave}`、`rurge_config::HostName`、`arc_swap::ArcSwap`。
- Produces（`rurge_dns::bootstrap::*`）：
  - `BOOTSTRAP_MIN_TTL: Duration = 60 s`。
  - `Bootstrap::new(upstreams: Vec<UpstreamRef>, want_v6: bool, opts: FanoutOpts) -> Arc<Bootstrap>`；`set_upstreams(&self, Vec<UpstreamRef>)`；`upstream_names(&self) -> Vec<String>`；`resolve(&self, host: &str) -> Result<Vec<IpAddr>, UpstreamError>`（async；IP 字面量直返；缓存命中直返；无传统上游 → `UpstreamError::Bootstrap`）；`flush(&self)`。
  - `BootstrapConnector::new(inner: Arc<dyn Connector>, bootstrap: Arc<Bootstrap>) -> BootstrapConnector`；实现 `Connector`：域名目标先经 `Bootstrap::resolve`，再按 `interleave` 顺序用内层连接器逐个 IP 连接（保留原端口）；IP 目标直接透传。

- [ ] **Step 1: 写 `bootstrap.rs`（含测试）**

```rust
//! Bootstrap (design §7.2): the hostnames inside URL-type upstreams
//! (`tcp://` / `tls://` / `https://`) are resolved only through traditional
//! upstreams — the plain UDP servers, or the system's — with a small cache
//! (minimum TTL 60 s). `BootstrapConnector` wraps the injected `Connector` so
//! DoT / DoH connections dial the pre-resolved address while TLS still sees
//! the hostname.

use crate::cache::{CacheHit, CachedAddrs, DnsCache};
use crate::fanout::{FanoutOpts, resolve_name};
use crate::upstream::{UpstreamError, UpstreamRef};
use arc_swap::ArcSwap;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target, interleave};
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

pub const BOOTSTRAP_MIN_TTL: Duration = Duration::from_secs(60);

pub struct Bootstrap {
    upstreams: ArcSwap<Vec<UpstreamRef>>,
    cache: DnsCache,
    want_v6: bool,
    opts: FanoutOpts,
}

impl Bootstrap {
    pub fn new(upstreams: Vec<UpstreamRef>, want_v6: bool, opts: FanoutOpts) -> Arc<Bootstrap> {
        Arc::new(Bootstrap {
            upstreams: ArcSwap::from_pointee(upstreams),
            cache: DnsCache::new(64),
            want_v6,
            opts,
        })
    }

    pub fn set_upstreams(&self, upstreams: Vec<UpstreamRef>) {
        self.upstreams.store(Arc::new(upstreams));
    }

    pub fn upstream_names(&self) -> Vec<String> {
        self.upstreams.load().iter().map(|u| u.name().to_string()).collect()
    }

    pub fn flush(&self) {
        self.cache.flush();
    }

    pub async fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, UpstreamError> {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(vec![ip]);
        }
        let name = bare.trim_end_matches('.').to_ascii_lowercase();
        if let Some(CacheHit::Fresh(a)) = self.cache.get(&name) {
            return Ok(to_list(&a.v4, &a.v6));
        }
        let upstreams = self.upstreams.load();
        if upstreams.is_empty() {
            return Err(UpstreamError::Bootstrap(format!("no traditional upstream to resolve `{host}`")));
        }
        let answers = resolve_name(&upstreams, &name, self.want_v6, &self.opts)
            .await
            .map_err(|e| UpstreamError::Bootstrap(format!("{host}: {e}")))?;
        let min_ttl = answers
            .v4
            .iter()
            .map(|(_, t)| *t)
            .chain(answers.v6.iter().map(|(_, t)| *t))
            .min()
            .unwrap_or(0);
        let ttl = Duration::from_secs(u64::from(min_ttl)).max(BOOTSTRAP_MIN_TTL);
        let v4: Vec<_> = answers.v4.iter().map(|(ip, _)| *ip).collect();
        let v6: Vec<_> = answers.v6.iter().map(|(ip, _)| *ip).collect();
        self.cache.put(
            &name,
            CachedAddrs {
                v4: v4.clone(),
                v6: v6.clone(),
                ttl,
                source: answers.upstream,
            },
        );
        Ok(to_list(&v4, &v6))
    }
}

fn to_list(v4: &[std::net::Ipv4Addr], v6: &[std::net::Ipv6Addr]) -> Vec<IpAddr> {
    v4.iter()
        .map(|a| IpAddr::V4(*a))
        .chain(v6.iter().map(|a| IpAddr::V6(*a)))
        .collect()
}

pub struct BootstrapConnector {
    inner: Arc<dyn Connector>,
    bootstrap: Arc<Bootstrap>,
}

impl BootstrapConnector {
    pub fn new(inner: Arc<dyn Connector>, bootstrap: Arc<Bootstrap>) -> BootstrapConnector {
        BootstrapConnector { inner, bootstrap }
    }
}

impl Connector for BootstrapConnector {
    fn connect<'a>(&'a self, target: &'a Target, opts: &'a ConnectOpts) -> BoxFuture<'a, io::Result<BoxedStream>> {
        Box::pin(async move {
            let host = match &target.host {
                HostName::Ip(_) => return self.inner.connect(target, opts).await,
                HostName::Domain(d) => d.clone(),
            };
            let ips = self.bootstrap.resolve(&host).await.map_err(io::Error::other)?;
            let mut last = io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}"));
            for ip in interleave(ips, opts.prefer_v6) {
                let t = Target::new(HostName::Ip(ip), target.port);
                match self.inner.connect(&t, opts).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => last = e,
                }
            }
            Err(last)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Qtype;
    use crate::testing::MockDns;
    use crate::upstream::udp::UdpUpstream;
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 3,
        }
    }

    #[tokio::test]
    async fn resolves_through_traditional_upstreams_and_caches() {
        let s = MockDns::spawn().await;
        s.set("dns.example", &["127.0.0.1"], &[], 5);
        let b = Bootstrap::new(vec![Arc::new(UdpUpstream::new(s.addr()))], false, fast());
        assert_eq!(b.resolve("dns.example").await.unwrap(), vec!["127.0.0.1".parse::<IpAddr>().unwrap()]);
        assert_eq!(b.resolve("DNS.example.").await.unwrap().len(), 1);
        assert_eq!(s.query_count("dns.example", Qtype::A), 1, "second lookup served from the bootstrap cache");
        assert_eq!(b.resolve("10.0.0.1").await.unwrap(), vec!["10.0.0.1".parse::<IpAddr>().unwrap()]);
        assert_eq!(b.resolve("[::1]").await.unwrap(), vec!["::1".parse::<IpAddr>().unwrap()]);
        assert_eq!(b.upstream_names(), vec![format!("udp://{}", s.addr())]);
        b.flush();
        b.resolve("dns.example").await.unwrap();
        assert_eq!(s.query_count("dns.example", Qtype::A), 2);
    }

    #[tokio::test]
    async fn errors_without_upstreams_or_answers() {
        let b = Bootstrap::new(Vec::new(), false, fast());
        assert!(matches!(b.resolve("dns.example").await, Err(UpstreamError::Bootstrap(_))));
        let s = MockDns::spawn().await;
        s.set_empty("dns.example");
        let b = Bootstrap::new(vec![Arc::new(UdpUpstream::new(s.addr()))], false, fast());
        let err = b.resolve("dns.example").await.unwrap_err();
        assert!(matches!(err, UpstreamError::Bootstrap(ref m) if m.contains("empty")), "{err}");
    }

    #[tokio::test]
    async fn connector_dials_the_resolved_address() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = s.write_all(b"ok").await;
                });
            }
        });
        let dns = MockDns::spawn().await;
        dns.set("dot.example", &["127.0.0.1"], &[], 60);
        let bootstrap = Bootstrap::new(vec![Arc::new(UdpUpstream::new(dns.addr()))], false, fast());
        let connector = BootstrapConnector::new(Arc::new(DirectConnector::new(Arc::new(SystemResolve))), bootstrap);
        let mut stream = connector
            .connect(&Target::new(HostName::parse("dot.example"), port), &ConnectOpts::default())
            .await
            .unwrap();
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"ok");
        // IP targets bypass bootstrap entirely
        let mut direct = connector
            .connect(&Target::new(HostName::parse("127.0.0.1"), port), &ConnectOpts::default())
            .await
            .unwrap();
        direct.read_exact(&mut buf).await.unwrap();
        assert_eq!(dns.query_count("dot.example", Qtype::A), 1);
        let err = connector
            .connect(&Target::new(HostName::parse("nx.example"), port), &ConnectOpts::default())
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("nx.example"));
    }
}
```

`lib.rs`：`pub mod bootstrap;`。

> `rurge_net::connector::interleave` 在 M2a 中是 `pub fn`；`io::Error::other` 需要 Rust ≥ 1.74（满足）。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns bootstrap
```

预期：3 个测试通过。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): 引导解析器与 BootstrapConnector（URL 型上游主机名只经传统上游解析）

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 10: DNS 缓存 `DnsCache`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/cache.rs`

**Interfaces:**
- Consumes: `lru::LruCache`（0.18：`new(NonZeroUsize)`、`get(&mut self, &K)`、`put`、`pop`、`iter`、`len`、`clear`）、`tokio::time::Instant`（可在 `start_paused` 测试中推进）。
- Produces（`rurge_dns::cache::*`）：
  - `DEFAULT_CAPACITY: usize = 2000`、`NEGATIVE_TTL: Duration = 30 s`、`REFRESH_RETRY_INTERVAL: Duration = 60 s`。
  - `CachedAddrs { v4: Vec<Ipv4Addr>, v6: Vec<Ipv6Addr>, ttl: Duration, source: String }`（`Clone, PartialEq`）。
  - `CacheHit::{Fresh(CachedAddrs), Stale(CachedAddrs), Negative}`。
  - `CacheEntry { name, v4, v6, expires_in: Option<Duration>, stale: bool, negative: bool, source: String }`（快照行）。
  - `DnsCache::new(capacity: usize) -> DnsCache`；`get(&self, name: &str) -> Option<CacheHit>`；`put(&self, name: &str, addrs: CachedAddrs)`（`ttl == 0` 时不存并移除旧条目）；`put_negative(&self, name: &str)`；`begin_refresh(&self, name: &str) -> bool`（可以刷新时置位并返回 true；已在刷新中或距上次尝试不足 60 s 返回 false）；`end_refresh(&self, name: &str)`；`flush(&self)`；`len(&self) -> usize`；`is_empty`；`snapshot(&self) -> Vec<CacheEntry>`（按名字排序）。
  - 键统一为小写域名（调用方负责）；所有方法 `&self`（内部 `Mutex`），可跨任务共享。

- [ ] **Step 1: 写 `cache.rs`（含测试）**

```rust
//! DNS cache (design §7.4): LRU keyed by lowercase name, TTL = the answer's
//! smallest record TTL, optimistic refresh (an expired entry is still served
//! while one background refresh runs), negative caching for empty answers.

use lru::LruCache;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::Instant;

pub const DEFAULT_CAPACITY: usize = 2000;
pub const NEGATIVE_TTL: Duration = Duration::from_secs(30);
pub const REFRESH_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedAddrs {
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    /// TTL the answer carried (smallest record TTL).
    pub ttl: Duration,
    /// Upstream name that answered, for `cache_snapshot`.
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheHit {
    Fresh(CachedAddrs),
    /// Expired: serve it and refresh in the background (`begin_refresh`).
    Stale(CachedAddrs),
    /// A fresh negative entry: answer "empty" without asking upstream.
    Negative,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    pub name: String,
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    pub expires_in: Option<Duration>,
    pub stale: bool,
    pub negative: bool,
    pub source: String,
}

#[derive(Clone, Debug)]
struct Entry {
    /// `None` = negative entry.
    addrs: Option<CachedAddrs>,
    expires_at: Instant,
    refreshing: bool,
    last_refresh: Option<Instant>,
}

pub struct DnsCache {
    inner: Mutex<LruCache<String, Entry>>,
}

impl DnsCache {
    pub fn new(capacity: usize) -> DnsCache {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("capacity >= 1");
        DnsCache {
            inner: Mutex::new(LruCache::new(cap)),
        }
    }

    pub fn get(&self, name: &str) -> Option<CacheHit> {
        let now = Instant::now();
        let mut cache = self.inner.lock().expect("dns cache lock");
        let decision = match cache.get(name) {
            None => return None,
            Some(e) => match &e.addrs {
                None if now < e.expires_at => Some(CacheHit::Negative),
                None => None,
                Some(a) if now < e.expires_at => Some(CacheHit::Fresh(a.clone())),
                Some(a) => Some(CacheHit::Stale(a.clone())),
            },
        };
        if decision.is_none() {
            // An expired negative entry is a plain miss.
            cache.pop(name);
        }
        decision
    }

    pub fn put(&self, name: &str, addrs: CachedAddrs) {
        let mut cache = self.inner.lock().expect("dns cache lock");
        if addrs.ttl.is_zero() {
            cache.pop(name);
            return;
        }
        let expires_at = Instant::now() + addrs.ttl;
        cache.put(
            name.to_string(),
            Entry {
                addrs: Some(addrs),
                expires_at,
                refreshing: false,
                last_refresh: None,
            },
        );
    }

    pub fn put_negative(&self, name: &str) {
        let mut cache = self.inner.lock().expect("dns cache lock");
        cache.put(
            name.to_string(),
            Entry {
                addrs: None,
                expires_at: Instant::now() + NEGATIVE_TTL,
                refreshing: false,
                last_refresh: None,
            },
        );
    }

    /// Claims the single background refresh slot of a stale entry.
    pub fn begin_refresh(&self, name: &str) -> bool {
        let now = Instant::now();
        let mut cache = self.inner.lock().expect("dns cache lock");
        let Some(e) = cache.get_mut(name) else {
            return false;
        };
        if e.refreshing {
            return false;
        }
        if let Some(last) = e.last_refresh {
            if now.duration_since(last) < REFRESH_RETRY_INTERVAL {
                return false;
            }
        }
        e.refreshing = true;
        e.last_refresh = Some(now);
        true
    }

    /// Releases the refresh slot without replacing the entry (a successful
    /// refresh calls `put`, which replaces it).
    pub fn end_refresh(&self, name: &str) {
        let mut cache = self.inner.lock().expect("dns cache lock");
        if let Some(e) = cache.get_mut(name) {
            e.refreshing = false;
        }
    }

    pub fn flush(&self) {
        self.inner.lock().expect("dns cache lock").clear();
    }

    pub fn len(&self) -> usize {
        self.inner.lock().expect("dns cache lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn snapshot(&self) -> Vec<CacheEntry> {
        let now = Instant::now();
        let cache = self.inner.lock().expect("dns cache lock");
        let mut out: Vec<CacheEntry> = cache
            .iter()
            .map(|(name, e)| {
                let expires_in = if now < e.expires_at {
                    Some(e.expires_at - now)
                } else {
                    None
                };
                match &e.addrs {
                    Some(a) => CacheEntry {
                        name: name.clone(),
                        v4: a.v4.clone(),
                        v6: a.v6.clone(),
                        expires_in,
                        stale: expires_in.is_none(),
                        negative: false,
                        source: a.source.clone(),
                    },
                    None => CacheEntry {
                        name: name.clone(),
                        v4: Vec::new(),
                        v6: Vec::new(),
                        expires_in,
                        stale: expires_in.is_none(),
                        negative: true,
                        source: "negative".to_string(),
                    },
                }
            })
            .collect();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::advance;

    fn addrs(ttl_secs: u64) -> CachedAddrs {
        CachedAddrs {
            v4: vec!["10.0.0.1".parse().unwrap()],
            v6: vec![],
            ttl: Duration::from_secs(ttl_secs),
            source: "udp://1.1.1.1:53".to_string(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fresh_then_stale() {
        let c = DnsCache::new(10);
        c.put("a.com", addrs(10));
        assert_eq!(c.get("a.com"), Some(CacheHit::Fresh(addrs(10))));
        advance(Duration::from_secs(11)).await;
        assert_eq!(c.get("a.com"), Some(CacheHit::Stale(addrs(10))));
        let snap = c.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(snap[0].stale && snap[0].expires_in.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn negative_entries_expire_into_misses() {
        let c = DnsCache::new(10);
        c.put_negative("nx.com");
        assert_eq!(c.get("nx.com"), Some(CacheHit::Negative));
        assert!(c.snapshot()[0].negative);
        advance(NEGATIVE_TTL + Duration::from_secs(1)).await;
        assert_eq!(c.get("nx.com"), None);
        assert!(c.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn zero_ttl_is_not_cached_and_replaces_old_entries() {
        let c = DnsCache::new(10);
        c.put("a.com", addrs(10));
        c.put("a.com", addrs(0));
        assert_eq!(c.get("a.com"), None);
    }

    #[tokio::test(start_paused = true)]
    async fn lru_evicts_the_least_recently_used() {
        let c = DnsCache::new(2);
        c.put("a.com", addrs(10));
        c.put("b.com", addrs(10));
        assert!(c.get("a.com").is_some()); // touch a
        c.put("c.com", addrs(10)); // evicts b
        assert!(c.get("b.com").is_none());
        assert!(c.get("a.com").is_some() && c.get("c.com").is_some());
        assert_eq!(c.len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_slot_is_single_and_rate_limited() {
        let c = DnsCache::new(10);
        c.put("a.com", addrs(1));
        advance(Duration::from_secs(2)).await;
        assert!(c.begin_refresh("a.com"));
        assert!(!c.begin_refresh("a.com"), "already refreshing");
        c.end_refresh("a.com");
        assert!(!c.begin_refresh("a.com"), "retry within 60 s is refused");
        advance(REFRESH_RETRY_INTERVAL).await;
        assert!(c.begin_refresh("a.com"));
        c.put("a.com", addrs(5)); // a successful refresh replaces the entry and clears the slot
        advance(Duration::from_secs(6)).await;
        assert!(c.begin_refresh("a.com"));
        assert!(!c.begin_refresh("missing.com"));
    }

    #[tokio::test(start_paused = true)]
    async fn flush_and_snapshot_order() {
        let c = DnsCache::new(10);
        c.put("b.com", addrs(10));
        c.put("a.com", addrs(10));
        let names: Vec<String> = c.snapshot().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["a.com", "b.com"]);
        c.flush();
        assert!(c.is_empty());
    }
}
```

`lib.rs`：`pub mod cache;`。

> `tokio::time::advance` 需要 tokio 的 `test-util` feature —— Task 1 已在 `rurge-dns` 的 dev-dependencies 中启用。`LruCache::get_mut` 在 0.18 存在；若不存在，用 `pop` + `put` 改写 `begin_refresh` / `end_refresh`。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns cache
```

预期：6 个测试通过。

- [ ] **Step 3: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): LRU + TTL 缓存、乐观刷新与负缓存

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 11: `[Host]` 映射链 `HostMap`、hosts 文件与 `SystemDns`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/system.rs`
- Create: `crates/rurge-dns/src/hosts.rs`

**Interfaces:**
- Consumes: `rurge_config::host::{HostEntry, HostKey, HostValue, SystemMode}`、`rurge_config::{Diagnostic, Diagnostics, Glob, HostName, codes}`（`codes::W_HOST_SCRIPT_SKIPPED = "W0027"`、`codes::W_DNS_UPSTREAM_UNSUPPORTED = "W0026"`，Task 1 已定义）、`rurge_config::session::SessionInfo`、`rurge_rules::matcher::{EvalCtx, NoGeo, Verdict}`、`rurge_rules::registry::SetRegistry::get(&ResourceRef, SetKind) -> SetHandle`、`rurge_rules::set::SetHandle::load() -> Arc<CompiledSet>`、`CompiledSet::eval(&SessionInfo, &mut EvalCtx, no_resolve, extended) -> SetVerdict`、`rurge_rules::set_format::SetKind`、`crate::upstream::UpstreamSpec::from_dns_upstream(&DnsUpstream) -> Result<UpstreamSpec, String>`（Task 5）、`arc_swap::ArcSwap`。
- Produces（`rurge_dns::system::*`、`rurge_dns::hosts::*`）：
  - `trait SystemDns: Send + Sync { fn servers(&self) -> Vec<SocketAddr>; fn search_domains(&self) -> Vec<String>; fn hosts_path(&self) -> Option<PathBuf>; fn has_ipv6(&self) -> bool; }`；`NoSystemDns`（全部为空 / false）；`StaticSystemDns { servers, search_domains, hosts_path, has_ipv6 }`（`Default`）。
  - `HostAction::{Ips(Vec<IpAddr>), Alias(String), Servers(Vec<UpstreamSpec>), System(SystemMode)}`。
  - `HostLookup { action: HostAction, raw: String, etc_hosts: bool }`。
  - `HostMap::build(entries: &[HostEntry], sets: &SetRegistry, diags: &mut Diagnostics) -> HostMap`；`lookup(&self, name: &str) -> Option<HostLookup>`（`[Host]` 顺序首个命中，然后 hosts 文件）；`set_etc_hosts(&self, entries: Vec<(String, IpAddr)>)`；`etc_hosts_len(&self) -> usize`；`rules_len(&self) -> usize`。
  - `parse_hosts_file(text: &str) -> Vec<(String, IpAddr)>`（名字小写、去尾点；`#` 注释；非法 IP 行跳过）。

- [ ] **Step 1: 写 `system.rs`**

```rust
//! Access to the operating system's resolver configuration (design §7.1).
//! The binary implements it through `rurge-platform`; tests use the static kinds.

use std::net::SocketAddr;
use std::path::PathBuf;

pub trait SystemDns: Send + Sync {
    /// System DNS servers (port 53 unless the platform says otherwise).
    fn servers(&self) -> Vec<SocketAddr>;
    fn search_domains(&self) -> Vec<String>;
    fn hosts_path(&self) -> Option<PathBuf>;
    /// True when some interface has a global (non link-local, non unique-local) IPv6 address.
    fn has_ipv6(&self) -> bool;
}

/// No system information at all (tests, sandboxes).
pub struct NoSystemDns;

impl SystemDns for NoSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        Vec::new()
    }
    fn search_domains(&self) -> Vec<String> {
        Vec::new()
    }
    fn hosts_path(&self) -> Option<PathBuf> {
        None
    }
    fn has_ipv6(&self) -> bool {
        false
    }
}

/// Fixed values (tests and CLI overrides).
#[derive(Clone, Debug, Default)]
pub struct StaticSystemDns {
    pub servers: Vec<SocketAddr>,
    pub search_domains: Vec<String>,
    pub hosts_path: Option<PathBuf>,
    pub has_ipv6: bool,
}

impl SystemDns for StaticSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        self.servers.clone()
    }
    fn search_domains(&self) -> Vec<String> {
        self.search_domains.clone()
    }
    fn hosts_path(&self) -> Option<PathBuf> {
        self.hosts_path.clone()
    }
    fn has_ipv6(&self) -> bool {
        self.has_ipv6
    }
}
```

- [ ] **Step 2: 写 `hosts.rs`（含测试）**

```rust
//! `[Host]` mapping chain and the system hosts file (design §7.5).

use crate::upstream::UpstreamSpec;
use arc_swap::ArcSwap;
use rurge_config::host::{HostEntry, HostKey, HostValue, SystemMode};
use rurge_config::session::SessionInfo;
use rurge_config::{Diagnostic, Diagnostics, Glob, HostName, codes};
use rurge_rules::matcher::{EvalCtx, NoGeo, Verdict};
use rurge_rules::registry::SetRegistry;
use rurge_rules::set::SetHandle;
use rurge_rules::set_format::SetKind;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostAction {
    Ips(Vec<IpAddr>),
    Alias(String),
    Servers(Vec<UpstreamSpec>),
    System(SystemMode),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLookup {
    pub action: HostAction,
    /// The `[Host]` key text (or the matched hosts-file name).
    pub raw: String,
    pub etc_hosts: bool,
}

enum HostMatcher {
    Glob(Glob),
    Set(SetHandle),
}

struct HostRule {
    matcher: HostMatcher,
    action: HostAction,
    raw: String,
}

pub struct HostMap {
    rules: Vec<HostRule>,
    etc_hosts: ArcSwap<HashMap<String, Vec<IpAddr>>>,
}

impl HostMap {
    /// Compiles the `[Host]` section. `script:` values are skipped with W0027;
    /// unsupported `server:` upstreams are skipped with W0026.
    pub fn build(entries: &[HostEntry], sets: &SetRegistry, diags: &mut Diagnostics) -> HostMap {
        let mut rules = Vec::with_capacity(entries.len());
        for e in entries {
            let action = match &e.value {
                HostValue::Ips(v) => HostAction::Ips(v.clone()),
                HostValue::Alias(a) => HostAction::Alias(a.trim_end_matches('.').to_ascii_lowercase()),
                HostValue::Servers(list) => {
                    let mut specs = Vec::new();
                    for u in list {
                        match UpstreamSpec::from_dns_upstream(u) {
                            Ok(s) => specs.push(s),
                            Err(msg) => diags.push(
                                Diagnostic::warning(
                                    codes::W_DNS_UPSTREAM_UNSUPPORTED,
                                    format!("[Host] `{}`: {msg}", e.raw_key),
                                )
                                .at(e.span.clone()),
                            ),
                        }
                    }
                    if specs.is_empty() {
                        continue;
                    }
                    HostAction::Servers(specs)
                }
                HostValue::System(mode) => HostAction::System(*mode),
                HostValue::Script(name) => {
                    diags.push(
                        Diagnostic::warning(
                            codes::W_HOST_SCRIPT_SKIPPED,
                            format!("[Host] `{}`: script `{name}` is not supported in this version; entry skipped", e.raw_key),
                        )
                        .at(e.span.clone()),
                    );
                    continue;
                }
            };
            let matcher = match &e.key {
                HostKey::Pattern(g) => HostMatcher::Glob(g.clone()),
                HostKey::DomainSet(r) => HostMatcher::Set(sets.get(r, SetKind::DomainSet)),
                HostKey::RuleSet(r) => HostMatcher::Set(sets.get(r, SetKind::RuleSet)),
            };
            rules.push(HostRule {
                matcher,
                action,
                raw: e.raw_key.clone(),
            });
        }
        HostMap {
            rules,
            etc_hosts: ArcSwap::from_pointee(HashMap::new()),
        }
    }

    pub fn rules_len(&self) -> usize {
        self.rules.len()
    }

    pub fn etc_hosts_len(&self) -> usize {
        self.etc_hosts.load().len()
    }

    /// Replaces the hosts-file entries (called at start and on file change).
    pub fn set_etc_hosts(&self, entries: Vec<(String, IpAddr)>) {
        let mut map: HashMap<String, Vec<IpAddr>> = HashMap::new();
        for (name, ip) in entries {
            let v = map.entry(name).or_default();
            if !v.contains(&ip) {
                v.push(ip);
            }
        }
        self.etc_hosts.store(Arc::new(map));
    }

    /// `[Host]` rules in order, then the hosts file. `name` must be lowercase.
    pub fn lookup(&self, name: &str) -> Option<HostLookup> {
        for rule in &self.rules {
            let hit = match &rule.matcher {
                HostMatcher::Glob(g) => g.matches(name),
                HostMatcher::Set(handle) => set_matches(handle, name),
            };
            if hit {
                return Some(HostLookup {
                    action: rule.action.clone(),
                    raw: rule.raw.clone(),
                    etc_hosts: false,
                });
            }
        }
        self.etc_hosts.load().get(name).map(|ips| HostLookup {
            action: HostAction::Ips(ips.clone()),
            raw: name.to_string(),
            etc_hosts: true,
        })
    }
}

/// Only domain entries of a set can match here (matching happens before any
/// resolution), so the set is evaluated with `no-resolve` forced.
fn set_matches(handle: &SetHandle, name: &str) -> bool {
    let session = SessionInfo::tcp(HostName::Domain(name.to_string()), 0);
    let mut ctx = EvalCtx::new(&NoGeo);
    handle.load().eval(&session, &mut ctx, true, false).verdict == Verdict::Match
}

/// `/etc/hosts` format: `<ip> <name> [<name>...]`, `#` comments.
pub fn parse_hosts_file(text: &str) -> Vec<(String, IpAddr)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(ip) = parts.next().and_then(|s| s.parse::<IpAddr>().ok()) else {
            continue;
        };
        for name in parts {
            let n = name.trim_end_matches('.').to_ascii_lowercase();
            if !n.is_empty() {
                out.push((n, ip));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::http::{HttpClient, HttpClientConfig};
    use rurge_net::resource::{ResourceManager, ResourceOptions};
    use std::path::Path;

    fn build(dir: &Path, host_section: &str) -> (HostMap, Diagnostics) {
        let text = format!("[Host]\n{host_section}\n[Rule]\nFINAL,DIRECT\n");
        let loaded = from_text(&text, &dir.join("t.conf"), &LoadOptions::for_tests());
        assert!(!loaded.diagnostics.has_errors());
        let cfg = loaded.config;
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(dir.to_path_buf(), client, ResourceOptions { offline: true, ..ResourceOptions::default() });
        let (sets, _) = SetRegistry::build(&cfg, resources, dir);
        let mut diags = Diagnostics::default();
        let map = HostMap::build(&cfg.hosts, sets.as_ref(), &mut diags);
        (map, diags)
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn first_match_wins_and_wildcards_follow_the_manual() {
        let dir = tempfile::tempdir().unwrap();
        let (map, diags) = build(
            dir.path(),
            "exact.com = 1.2.3.4\n*google.com = 5.6.7.8\n*.dev = 6.7.8.9, ::1\nalias.com = exact.com\n",
        );
        assert!(diags.is_empty());
        assert_eq!(map.rules_len(), 4);
        assert_eq!(map.lookup("exact.com").unwrap().action, HostAction::Ips(vec![ip("1.2.3.4")]));
        assert_eq!(map.lookup("google.com").unwrap().action, HostAction::Ips(vec![ip("5.6.7.8")]));
        assert_eq!(map.lookup("foo.google.com").unwrap().action, HostAction::Ips(vec![ip("5.6.7.8")]));
        assert_eq!(map.lookup("bargoogle.com").unwrap().action, HostAction::Ips(vec![ip("5.6.7.8")]));
        assert_eq!(map.lookup("app.dev").unwrap().action, HostAction::Ips(vec![ip("6.7.8.9"), ip("::1")]));
        assert!(map.lookup("dev").is_none(), "*.dev must not match the bare name");
        assert_eq!(map.lookup("alias.com").unwrap().action, HostAction::Alias("exact.com".into()));
        assert_eq!(map.lookup("alias.com").unwrap().raw, "alias.com");
    }

    #[tokio::test]
    async fn server_system_and_script_values() {
        let dir = tempfile::tempdir().unwrap();
        let (map, diags) = build(
            dir.path(),
            "a.com = server:1.1.1.1, tls://dns.example.com\nb.com = server:system\nc.com = server:syslib\nd.com = script:my-script\ne.com = server:h3://dns.example.com/dns-query\n",
        );
        let codes: Vec<&str> = diags.iter().map(|d| d.code).collect();
        assert!(codes.contains(&codes::W_HOST_SCRIPT_SKIPPED));
        assert!(codes.contains(&codes::W_DNS_UPSTREAM_UNSUPPORTED));
        assert_eq!(map.rules_len(), 3, "script entry and all-unsupported entry are skipped");
        match map.lookup("a.com").unwrap().action {
            HostAction::Servers(specs) => {
                assert_eq!(specs.len(), 2);
                assert_eq!(specs[0], UpstreamSpec::Udp("1.1.1.1:53".parse().unwrap()));
                assert_eq!(specs[1].name(), "tls://dns.example.com:853");
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(map.lookup("b.com").unwrap().action, HostAction::System(SystemMode::System));
        assert_eq!(map.lookup("c.com").unwrap().action, HostAction::System(SystemMode::Syslib));
        assert!(map.lookup("d.com").is_none());
        assert!(map.lookup("e.com").is_none());
    }

    #[tokio::test]
    async fn set_keys_match_domain_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("d.txt"), ".set.example\nexact.example\n").unwrap();
        std::fs::write(dir.path().join("r.list"), "DOMAIN-SUFFIX,rule.example\nIP-CIDR,10.0.0.0/8\n").unwrap();
        let (map, diags) = build(dir.path(), "DOMAIN-SET:d.txt = 1.1.1.1\nRULE-SET:r.list = 2.2.2.2\n");
        assert!(diags.is_empty(), "{:?}", diags.iter().map(|d| d.code).collect::<Vec<_>>());
        assert_eq!(map.lookup("a.set.example").unwrap().action, HostAction::Ips(vec![ip("1.1.1.1")]));
        assert_eq!(map.lookup("exact.example").unwrap().action, HostAction::Ips(vec![ip("1.1.1.1")]));
        assert_eq!(map.lookup("x.rule.example").unwrap().action, HostAction::Ips(vec![ip("2.2.2.2")]));
        assert!(map.lookup("other.example").is_none());
    }

    #[tokio::test]
    async fn etc_hosts_come_after_host_rules_and_merge_families() {
        let dir = tempfile::tempdir().unwrap();
        let (map, _) = build(dir.path(), "dup.example = 9.9.9.9\n");
        let parsed = parse_hosts_file(
            "# comment\n127.0.0.1 localhost\n::1 localhost\n10.0.0.5   nas.lan nas   # trailing\nnot-an-ip name\n192.168.1.7 Printer.LAN.\n",
        );
        assert_eq!(parsed.len(), 5);
        map.set_etc_hosts(parsed);
        assert_eq!(map.etc_hosts_len(), 4);
        let hit = map.lookup("localhost").unwrap();
        assert!(hit.etc_hosts);
        assert_eq!(hit.action, HostAction::Ips(vec![ip("127.0.0.1"), ip("::1")]));
        assert_eq!(map.lookup("nas").unwrap().action, HostAction::Ips(vec![ip("10.0.0.5")]));
        assert_eq!(map.lookup("printer.lan").unwrap().action, HostAction::Ips(vec![ip("192.168.1.7")]));
        map.set_etc_hosts(vec![("dup.example".into(), ip("1.1.1.1"))]);
        assert_eq!(map.lookup("dup.example").unwrap().action, HostAction::Ips(vec![ip("9.9.9.9")]), "[Host] wins over hosts file");
        assert!(map.lookup("gone.example").is_none());
    }
}
```
`lib.rs`：`pub mod hosts; pub mod system;` 并再导出 `pub use system::{NoSystemDns, StaticSystemDns, SystemDns};`。

> `HostValue::Alias` 的值在 M1 中已是原始字符串；此处统一小写并去尾点。`Glob` 的通配语义由 M1 实现（`*google.com` 匹配 `google.com` 与 `bargoogle.com`，`*.dev` 不匹配 `dev`）；若断言失败，说明 M1 的 glob 与手册示例不一致，报告为计划外发现而不是改断言。`SystemMode` 派生 `Copy`；若没有，用 `mode.clone()`。

- [ ] **Step 3: 运行**

```bash
cargo test -p rurge-dns hosts
```

预期：4 个测试通过。

- [ ] **Step 4: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): [Host] 映射链（通配 / 别名 / 专属上游 / 集合键）、hosts 文件解析与 SystemDns 抽象

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 12: 解析器 `Resolver`

**Files:**
- Modify: `crates/rurge-dns/src/lib.rs`
- Create: `crates/rurge-dns/src/resolver.rs`

**Interfaces:**
- Consumes: 本 crate 的 `bootstrap::{Bootstrap, BootstrapConnector}`、`cache::{CachedAddrs, CacheHit, DnsCache, DEFAULT_CAPACITY}`、`fanout::{Answers, FanoutError, FanoutOpts, resolve_name}`、`hosts::{HostAction, HostMap, parse_hosts_file}`、`system::SystemDns`、`upstream::{UpstreamRef, UpstreamSpec, doh::DohUpstream, tcp::TcpUpstream, udp::UdpUpstream}`；`rurge_config::{Config, Diagnostic, Diagnostics, codes}`、`rurge_config::general::{DnsServer, EncryptedDns}`、`rurge_config::host::{HostEntry, SystemMode}`；`rurge_rules::engine::{LazyResolver, ResolveError}`、`rurge_rules::matcher::ResolvedAddrs`、`rurge_rules::registry::SetRegistry`；`rurge_net::connector::{Connector, Resolve}`、`rurge_net::http::{HttpClient, HttpClientConfig, tls_client_config}`、`rurge_net::resource::{ResourceManager, ResourceSource, ResourceSpec, ResourceState}`、`rurge_net::BoxFuture`；`tokio::sync::watch`。
- Produces（`rurge_dns::resolver::*`，crate 根再导出 `Resolver, ResolverConfig, ResolverDeps, LookupOpts, DnsResult, DnsError, Source, HostKind, UpstreamDelay`）：
  - `ResolverConfig { servers: Vec<DnsServer>, encrypted: Vec<EncryptedDns>, skip_cert_verification: bool, ipv6: bool, hosts: Vec<HostEntry>, read_etc_hosts: bool, proxy_hostnames: HashSet<String>, cache_capacity: usize, fanout: FanoutOpts }`；`ResolverConfig::from_config(cfg: &Config) -> ResolverConfig`。
  - `ResolverDeps { connector: Arc<dyn Connector>, sets: Arc<SetRegistry>, system: Arc<dyn SystemDns>, resources: Arc<ResourceManager> }`（DoH 的 `HttpClient` 由 `Resolver` 内部用 `BootstrapConnector` 构建 —— 与设计 §7.1 的差异，登记到计划修正表）。
  - `LookupOpts { bypass_cache: bool, want_v6: Option<bool> }`（`Default`）。
  - `DnsResult { v4: Vec<Ipv4Addr>, v6: Vec<Ipv6Addr>, ttl: Duration, source: Source, elapsed: Duration }`；`DnsResult::addrs(&self) -> Vec<IpAddr>`（v4 在前）。
  - `HostKind::{Ip, Alias, Server, System, EtcHosts}`；`Source::{Literal, Loopback, Host(HostKind), Cache { stale: bool }, Upstream(String), System}`（`Display`，如 `cache(stale)`、`upstream(udp://…)`、`host(alias)`）。
  - `DnsError::{Timeout, EmptyAnswer, AllFailed(Vec<(String, String)>), Bootstrap(String), NoUpstream, AliasLoop(String), Unsupported(String)}`（`Clone, PartialEq, Display`）。
  - `UpstreamDelay { upstream: String, result: Result<Duration, String> }`。
  - `Resolver::new(cfg: ResolverConfig, deps: ResolverDeps) -> (Arc<Resolver>, Diagnostics)`（须在 tokio 运行时内调用）；`lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError>`（async）；`flush(&self)`；`on_network_change(&self)`；`cache_snapshot(&self) -> Vec<CacheEntry>`；`measure_delay(&self, name: &str) -> Vec<UpstreamDelay>`（async）；`primary_upstreams(&self) -> Vec<String>`；`aaaa_suppressed(&self) -> bool`。
  - `impl LazyResolver for Resolver`（`lookup` 默认选项；`DnsError::Timeout → ResolveError::Timeout`、`EmptyAnswer → EmptyAnswer`、其余 → `Failed(msg)`）；`impl rurge_net::connector::Resolve for Resolver`（地址列表，错误转 `io::Error::other`）。
  - 上游选择（binding）：`encrypted` 中可用的条目（`https://` / `tls://` / `tcp://`）存在 → 普通查询只用它们，UDP 只作引导；否则用 `servers` 中的 UDP 条目（`system` 关键字展开为 `SystemDns::servers()`）；都没有 → 系统上游，系统上游为空时退化为 `tokio::net::lookup_host`；`ipv6 = false` 时丢弃 IPv6 地址的 UDP 服务器。

- [ ] **Step 1: 写 `resolver.rs`**

```rust
//! The resolver (design §7): upstream selection, bootstrap, cache, `[Host]`
//! chain, hosts file, special hostnames, AAAA suppression and in-flight
//! coalescing. Implements `LazyResolver` (rule engine) and `Resolve`
//! (connectors) so every other crate resolves through it.

use crate::bootstrap::{Bootstrap, BootstrapConnector};
use crate::cache::{CacheEntry, CacheHit, CachedAddrs, DEFAULT_CAPACITY, DnsCache};
use crate::fanout::{Answers, FanoutError, FanoutOpts, resolve_name};
use crate::hosts::{HostAction, HostMap, parse_hosts_file};
use crate::system::SystemDns;
use crate::upstream::doh::DohUpstream;
use crate::upstream::tcp::TcpUpstream;
use crate::upstream::udp::UdpUpstream;
use crate::upstream::{UpstreamRef, UpstreamSpec};
use arc_swap::ArcSwap;
use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::host::HostEntry;
use rurge_config::{Config, Diagnostic, Diagnostics, codes};
use rurge_net::BoxFuture;
use rurge_net::connector::{Connector, Resolve};
use rurge_net::http::{HttpClient, HttpClientConfig, tls_client_config};
use rurge_net::resource::{ResourceManager, ResourceSource, ResourceSpec, ResourceState};
use rurge_rules::engine::{LazyResolver, ResolveError};
use rurge_rules::matcher::ResolvedAddrs;
use rurge_rules::registry::SetRegistry;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;

pub const MAX_ALIAS_HOPS: usize = 8;
pub const AAAA_SUPPRESS_AFTER: u32 = 5;

#[derive(Clone, Debug)]
pub struct ResolverConfig {
    pub servers: Vec<DnsServer>,
    pub encrypted: Vec<EncryptedDns>,
    pub skip_cert_verification: bool,
    pub ipv6: bool,
    pub hosts: Vec<HostEntry>,
    pub read_etc_hosts: bool,
    pub proxy_hostnames: HashSet<String>,
    pub cache_capacity: usize,
    pub fanout: FanoutOpts,
}

impl ResolverConfig {
    pub fn from_config(cfg: &Config) -> ResolverConfig {
        ResolverConfig {
            servers: cfg.general.dns_server.clone(),
            encrypted: cfg.general.encrypted_dns_server.clone(),
            skip_cert_verification: cfg.general.encrypted_dns_skip_cert_verification,
            ipv6: cfg.general.ipv6,
            hosts: cfg.hosts.clone(),
            read_etc_hosts: cfg.general.read_etc_hosts,
            proxy_hostnames: cfg.proxy_hostnames(),
            cache_capacity: DEFAULT_CAPACITY,
            fanout: FanoutOpts::default(),
        }
    }
}

pub struct ResolverDeps {
    pub connector: Arc<dyn Connector>,
    pub sets: Arc<SetRegistry>,
    pub system: Arc<dyn SystemDns>,
    pub resources: Arc<ResourceManager>,
}

#[derive(Clone, Debug, Default)]
pub struct LookupOpts {
    pub bypass_cache: bool,
    /// `None` = follow the profile (`ipv6`) and the network (`has_ipv6`).
    pub want_v6: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    Ip,
    Alias,
    Server,
    System,
    EtcHosts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Literal,
    Loopback,
    Host(HostKind),
    Cache { stale: bool },
    Upstream(String),
    System,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Literal => f.write_str("literal"),
            Source::Loopback => f.write_str("loopback"),
            Source::Host(k) => match k {
                HostKind::Ip => f.write_str("host(ip)"),
                HostKind::Alias => f.write_str("host(alias)"),
                HostKind::Server => f.write_str("host(server)"),
                HostKind::System => f.write_str("host(system)"),
                HostKind::EtcHosts => f.write_str("hosts-file"),
            },
            Source::Cache { stale: false } => f.write_str("cache"),
            Source::Cache { stale: true } => f.write_str("cache(stale)"),
            Source::Upstream(u) => write!(f, "upstream({u})"),
            Source::System => f.write_str("system"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsResult {
    pub v4: Vec<Ipv4Addr>,
    pub v6: Vec<Ipv6Addr>,
    pub ttl: Duration,
    pub source: Source,
    pub elapsed: Duration,
}

impl DnsResult {
    pub fn addrs(&self) -> Vec<IpAddr> {
        self.v4
            .iter()
            .map(|a| IpAddr::V4(*a))
            .chain(self.v6.iter().map(|a| IpAddr::V6(*a)))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsError {
    Timeout,
    EmptyAnswer,
    AllFailed(Vec<(String, String)>),
    Bootstrap(String),
    NoUpstream,
    AliasLoop(String),
    Unsupported(String),
}

impl fmt::Display for DnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DnsError::Timeout => f.write_str("timeout"),
            DnsError::EmptyAnswer => f.write_str("empty answer"),
            DnsError::AllFailed(list) => {
                let parts: Vec<String> = list.iter().map(|(u, e)| format!("{u}: {e}")).collect();
                write!(f, "all upstreams failed ({})", parts.join("; "))
            }
            DnsError::Bootstrap(m) => write!(f, "bootstrap: {m}"),
            DnsError::NoUpstream => f.write_str("no upstream configured"),
            DnsError::AliasLoop(n) => write!(f, "[Host] alias chain too long at `{n}`"),
            DnsError::Unsupported(m) => write!(f, "unsupported: {m}"),
        }
    }
}

impl std::error::Error for DnsError {}

impl From<FanoutError> for DnsError {
    fn from(e: FanoutError) -> DnsError {
        match e {
            FanoutError::EmptyAnswer => DnsError::EmptyAnswer,
            FanoutError::Timeout => DnsError::Timeout,
            FanoutError::AllFailed(list) if list.is_empty() => DnsError::NoUpstream,
            FanoutError::AllFailed(list) => DnsError::AllFailed(list),
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpstreamDelay {
    pub upstream: String,
    pub result: Result<Duration, String>,
}

type Shared = Result<Answers, DnsError>;

pub struct Resolver {
    cfg: ResolverConfig,
    connector: Arc<dyn Connector>,
    system: Arc<dyn SystemDns>,
    bootstrap: Arc<Bootstrap>,
    http: Arc<HttpClient>,
    tls: Option<Arc<rustls::ClientConfig>>,
    primary_specs: Vec<UpstreamSpec>,
    primary: ArcSwap<Vec<UpstreamRef>>,
    system_upstreams: ArcSwap<Vec<UpstreamRef>>,
    host_upstreams: Mutex<HashMap<Vec<UpstreamSpec>, Arc<Vec<UpstreamRef>>>>,
    cache: DnsCache,
    hosts: HostMap,
    inflight: Mutex<HashMap<String, watch::Sender<Option<Shared>>>>,
    aaaa_failures: AtomicU32,
    aaaa_suppressed: AtomicBool,
    has_ipv6: AtomicBool,
    self_weak: Mutex<Weak<Resolver>>,
}

impl Resolver {
    pub fn new(cfg: ResolverConfig, deps: ResolverDeps) -> (Arc<Resolver>, Diagnostics) {
        let mut diags = Diagnostics::default();
        let has_ipv6 = deps.system.has_ipv6();

        // Traditional (plain UDP) servers; `system` expands to the platform's list.
        let mut traditional: Vec<UpstreamSpec> = Vec::new();
        let mut wants_system = false;
        for s in &cfg.servers {
            match UpstreamSpec::from_dns_server(s) {
                Some(UpstreamSpec::Udp(addr)) if addr.is_ipv6() && !cfg.ipv6 => {
                    tracing::info!(server = %addr, "IPv6 DNS server ignored because ipv6 is off");
                }
                Some(spec) => traditional.push(spec),
                None => wants_system = true,
            }
        }
        let system_specs: Vec<UpstreamSpec> = deps.system.servers().into_iter().map(UpstreamSpec::Udp).collect();
        if wants_system {
            traditional.extend(system_specs.iter().cloned());
        }

        // Encrypted subsystem: https:// tls:// tcp://
        let mut encrypted: Vec<UpstreamSpec> = Vec::new();
        for e in &cfg.encrypted {
            match UpstreamSpec::from_encrypted(e) {
                Ok(spec) => encrypted.push(spec),
                Err(msg) => diags.push(Diagnostic::warning(codes::W_DNS_UPSTREAM_UNSUPPORTED, msg)),
            }
        }
        if cfg.skip_cert_verification {
            tracing::warn!("encrypted-dns-skip-cert-verification is on: encrypted DNS certificates are not verified");
        }

        let bootstrap_specs = if traditional.is_empty() { system_specs.clone() } else { traditional.clone() };
        let bootstrap = Bootstrap::new(
            bootstrap_specs.iter().map(build_plain_udp).collect(),
            cfg.ipv6 && has_ipv6,
            cfg.fanout.clone(),
        );
        let bootstrap_connector: Arc<dyn Connector> =
            Arc::new(BootstrapConnector::new(deps.connector.clone(), bootstrap.clone()));
        let http = match HttpClient::new(
            bootstrap_connector.clone(),
            HttpClientConfig {
                skip_cert_verification: cfg.skip_cert_verification,
                ..HttpClientConfig::default()
            },
        ) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                diags.push(Diagnostic::warning(codes::W_DNS_UPSTREAM_UNSUPPORTED, format!("DoH client unavailable: {e}")));
                // A client that can never connect; DoH upstreams will fail per query.
                Arc::new(HttpClient::new(bootstrap_connector.clone(), HttpClientConfig::default()).expect("default http client"))
            }
        };
        let tls = match tls_client_config(cfg.skip_cert_verification) {
            Ok(c) => Some(c),
            Err(e) => {
                diags.push(Diagnostic::warning(codes::W_DNS_UPSTREAM_UNSUPPORTED, format!("DoT unavailable: {e}")));
                None
            }
        };

        let primary_specs = if !encrypted.is_empty() {
            encrypted
        } else if !traditional.is_empty() {
            traditional
        } else {
            system_specs.clone()
        };

        let hosts = HostMap::build(&cfg.hosts, deps.sets.as_ref(), &mut diags);

        let resolver = Arc::new(Resolver {
            connector: bootstrap_connector,
            system: deps.system.clone(),
            bootstrap,
            http,
            tls,
            primary: ArcSwap::from_pointee(Vec::new()),
            system_upstreams: ArcSwap::from_pointee(system_specs.iter().map(build_plain_udp).collect()),
            host_upstreams: Mutex::new(HashMap::new()),
            cache: DnsCache::new(cfg.cache_capacity),
            hosts,
            inflight: Mutex::new(HashMap::new()),
            aaaa_failures: AtomicU32::new(0),
            aaaa_suppressed: AtomicBool::new(false),
            has_ipv6: AtomicBool::new(has_ipv6),
            self_weak: Mutex::new(Weak::new()),
            primary_specs,
            cfg,
        });
        *resolver.self_weak.lock().expect("weak") = Arc::downgrade(&resolver);
        resolver.rebuild_primary();
        if resolver.cfg.read_etc_hosts {
            if let Some(path) = deps.system.hosts_path() {
                resolver.watch_etc_hosts(&deps.resources, path);
            }
        }
        (resolver, diags)
    }

    fn build_upstream(&self, spec: &UpstreamSpec) -> Option<UpstreamRef> {
        match spec {
            UpstreamSpec::Udp(addr) => Some(Arc::new(UdpUpstream::new(*addr))),
            UpstreamSpec::Tcp { host, port } => Some(Arc::new(TcpUpstream::plain(host, *port, self.connector.clone()))),
            UpstreamSpec::Tls { host, port } => self
                .tls
                .as_ref()
                .map(|tls| Arc::new(TcpUpstream::tls(host, *port, self.connector.clone(), tls.clone())) as UpstreamRef),
            UpstreamSpec::Https(url) => Some(Arc::new(DohUpstream::new(url.clone(), self.http.clone()))),
        }
    }

    fn rebuild_primary(&self) {
        let list: Vec<UpstreamRef> = self.primary_specs.iter().filter_map(|s| self.build_upstream(s)).collect();
        self.primary.store(Arc::new(list));
    }

    pub fn primary_upstreams(&self) -> Vec<String> {
        self.primary.load().iter().map(|u| u.name().to_string()).collect()
    }

    pub fn aaaa_suppressed(&self) -> bool {
        self.aaaa_suppressed.load(Ordering::Relaxed)
    }

    pub fn cache_snapshot(&self) -> Vec<CacheEntry> {
        self.cache.snapshot()
    }

    pub fn flush(&self) {
        self.cache.flush();
        self.bootstrap.flush();
        self.aaaa_failures.store(0, Ordering::Relaxed);
        self.aaaa_suppressed.store(false, Ordering::Relaxed);
    }

    /// Flush, re-read the system servers, rebuild sockets.
    pub fn on_network_change(&self) {
        self.flush();
        self.has_ipv6.store(self.system.has_ipv6(), Ordering::Relaxed);
        let system_specs: Vec<UpstreamSpec> = self.system.servers().into_iter().map(UpstreamSpec::Udp).collect();
        self.system_upstreams.store(Arc::new(system_specs.iter().map(build_plain_udp).collect()));
        let traditional: Vec<UpstreamSpec> = self.primary_specs.iter().filter(|s| s.is_traditional()).cloned().collect();
        let bootstrap_specs = if traditional.is_empty() { system_specs } else { traditional };
        self.bootstrap.set_upstreams(bootstrap_specs.iter().map(build_plain_udp).collect());
        self.rebuild_primary();
        self.host_upstreams.lock().expect("host upstreams").clear();
    }

    fn want_v6(&self, opts: &LookupOpts) -> bool {
        let configured = opts
            .want_v6
            .unwrap_or(self.cfg.ipv6 && self.has_ipv6.load(Ordering::Relaxed));
        configured && !self.aaaa_suppressed()
    }

    fn host_upstreams_for(&self, specs: &[UpstreamSpec]) -> Arc<Vec<UpstreamRef>> {
        let mut map = self.host_upstreams.lock().expect("host upstreams");
        if let Some(list) = map.get(specs) {
            return list.clone();
        }
        let list = Arc::new(specs.iter().filter_map(|s| self.build_upstream(s)).collect::<Vec<_>>());
        map.insert(specs.to_vec(), list.clone());
        list
    }

    pub async fn lookup(&self, host: &str, opts: LookupOpts) -> Result<DnsResult, DnsError> {
        let started = Instant::now();
        let trimmed = host.trim();
        let bare = trimmed.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return Ok(literal(ip, Source::Literal, started));
        }
        let lower = trimmed.to_ascii_lowercase();
        let no_search = lower.ends_with('.');
        let name = lower.trim_end_matches('.').to_string();
        if name.is_empty() {
            return Err(DnsError::Unsupported("empty hostname".to_string()));
        }
        let want_v6 = self.want_v6(&opts);

        // [Host] chain with alias restarts.
        let mut current = name;
        let mut hops = 0usize;
        loop {
            if current == "localhost" || current.ends_with(".localhost") {
                let mut r = literal(IpAddr::V4(Ipv4Addr::LOCALHOST), Source::Loopback, started);
                if want_v6 {
                    r.v6.push(Ipv6Addr::LOCALHOST);
                }
                return Ok(r);
            }
            if self.cfg.proxy_hostnames.contains(&current) {
                break;
            }
            let Some(hit) = self.hosts.lookup(&current) else { break };
            match hit.action {
                HostAction::Ips(ips) => {
                    let kind = if hit.etc_hosts { HostKind::EtcHosts } else { HostKind::Ip };
                    return Ok(from_ips(&ips, Duration::ZERO, Source::Host(kind), started));
                }
                HostAction::Alias(target) => {
                    hops += 1;
                    if hops > MAX_ALIAS_HOPS {
                        return Err(DnsError::AliasLoop(current));
                    }
                    if let Ok(ip) = target.parse::<IpAddr>() {
                        return Ok(literal(ip, Source::Host(HostKind::Alias), started));
                    }
                    current = target;
                    continue;
                }
                HostAction::Servers(specs) => {
                    let ups = self.host_upstreams_for(&specs);
                    let answers = self.query_coalesced(&ups, &current, want_v6, &opts).await?;
                    return Ok(from_answers(&answers, Source::Host(HostKind::Server), started));
                }
                HostAction::System(_) => {
                    return self.system_lookup(&current, want_v6, Source::Host(HostKind::System), started).await;
                }
            }
        }

        // Special names.
        if current.ends_with(".local") {
            return self.system_lookup(&current, want_v6, Source::System, started).await;
        }
        if !current.contains('.') && !no_search {
            let candidate = match self.system.search_domains().first() {
                Some(d) => format!("{current}.{}", d.trim_matches('.').to_ascii_lowercase()),
                None => current.clone(),
            };
            return self.system_lookup(&candidate, want_v6, Source::System, started).await;
        }

        // Cache.
        if !opts.bypass_cache {
            match self.cache.get(&current) {
                Some(CacheHit::Fresh(a)) => {
                    return Ok(from_cached(&a, Source::Cache { stale: false }, started));
                }
                Some(CacheHit::Stale(a)) => {
                    self.spawn_refresh(current.clone(), want_v6);
                    return Ok(from_cached(&a, Source::Cache { stale: true }, started));
                }
                Some(CacheHit::Negative) => return Err(DnsError::EmptyAnswer),
                None => {}
            }
        }

        let primary = self.primary.load_full();
        if primary.is_empty() {
            return self.system_lookup(&current, want_v6, Source::System, started).await;
        }
        let result = self.query_coalesced(&primary, &current, want_v6, &opts).await;
        self.record(&current, &result, want_v6);
        let answers = result?;
        Ok(from_answers(&answers, Source::Upstream(answers.upstream.clone()), started))
    }

    /// Stores the outcome in the cache and feeds AAAA suppression.
    fn record(&self, name: &str, result: &Result<Answers, DnsError>, want_v6: bool) {
        match result {
            Ok(a) => {
                self.cache.put(name, cached_from(a));
                if want_v6 {
                    if a.aaaa_timed_out {
                        let n = self.aaaa_failures.fetch_add(1, Ordering::Relaxed) + 1;
                        if n >= AAAA_SUPPRESS_AFTER && !self.aaaa_suppressed.swap(true, Ordering::Relaxed) {
                            tracing::warn!("AAAA answers timed out {n} times in a row; AAAA queries suppressed until the next flush or network change");
                        }
                    } else {
                        self.aaaa_failures.store(0, Ordering::Relaxed);
                    }
                }
            }
            Err(DnsError::EmptyAnswer) => self.cache.put_negative(name),
            Err(_) => {}
        }
    }

    fn spawn_refresh(&self, name: String, want_v6: bool) {
        if !self.cache.begin_refresh(&name) {
            return;
        }
        let weak = self.self_weak.lock().expect("weak").clone();
        tokio::spawn(async move {
            let Some(me) = weak.upgrade() else { return };
            let primary = me.primary.load_full();
            let result = me.query_coalesced(&primary, &name, want_v6, &LookupOpts::default()).await;
            match &result {
                Ok(_) | Err(DnsError::EmptyAnswer) => me.record(&name, &result, want_v6),
                Err(_) => me.cache.end_refresh(&name),
            }
        });
    }

    /// One in-flight query per (name, family); later callers wait for the first.
    async fn query_coalesced(&self, upstreams: &[UpstreamRef], name: &str, want_v6: bool, opts: &LookupOpts) -> Result<Answers, DnsError> {
        let key = format!("{name}#{}#{}", want_v6, upstreams.iter().map(|u| u.name()).collect::<Vec<_>>().join(","));
        let mut rx = {
            let map = self.inflight.lock().expect("inflight");
            map.get(&key).map(|tx| tx.subscribe())
        };
        if let Some(rx) = rx.as_mut() {
            if !opts.bypass_cache {
                loop {
                    let value = rx.borrow().clone();
                    if let Some(v) = value {
                        return v;
                    }
                    if rx.changed().await.is_err() {
                        break; // the first caller vanished: query ourselves
                    }
                }
            }
        }
        let (tx, _keep) = watch::channel(None);
        self.inflight.lock().expect("inflight").insert(key.clone(), tx);
        let result: Result<Answers, DnsError> = resolve_name(upstreams, name, want_v6, &self.cfg.fanout)
            .await
            .map_err(DnsError::from);
        if let Some(tx) = self.inflight.lock().expect("inflight").remove(&key) {
            let _ = tx.send(Some(result.clone()));
        }
        result
    }

    async fn system_lookup(&self, name: &str, want_v6: bool, source: Source, started: Instant) -> Result<DnsResult, DnsError> {
        let system = self.system_upstreams.load_full();
        if !system.is_empty() {
            let answers = self.query_coalesced(&system, name, want_v6, &LookupOpts::default()).await?;
            return Ok(from_answers(&answers, source, started));
        }
        let addrs = tokio::net::lookup_host((name, 0))
            .await
            .map_err(|e| DnsError::AllFailed(vec![("system".to_string(), e.to_string())]))?;
        let ips: Vec<IpAddr> = addrs.map(|sa| sa.ip()).collect();
        if ips.is_empty() {
            return Err(DnsError::EmptyAnswer);
        }
        Ok(from_ips(&ips, Duration::from_secs(60), source, started))
    }

    pub async fn measure_delay(&self, name: &str) -> Vec<UpstreamDelay> {
        let mut out = Vec::new();
        for up in self.primary.load_full().iter() {
            let started = Instant::now();
            let one = [up.clone()];
            let result = resolve_name(&one, name, false, &self.cfg.fanout)
                .await
                .map(|_| started.elapsed())
                .map_err(|e| e.to_string());
            out.push(UpstreamDelay {
                upstream: up.name().to_string(),
                result,
            });
        }
        out
    }

    fn watch_etc_hosts(self: &Arc<Self>, resources: &Arc<ResourceManager>, path: std::path::PathBuf) {
        let handle = resources.get(&ResourceSpec {
            source: ResourceSource::File(path.clone()),
            update_interval: None,
        });
        if let Some((data, _)) = handle.current().data() {
            self.hosts.set_etc_hosts(parse_hosts_file(&String::from_utf8_lossy(&data)));
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut rx = handle.subscribe();
            loop {
                let changed = tokio::select! {
                    r = rx.changed() => r.is_ok(),
                    _ = tokio::time::sleep(Duration::from_secs(60)) => { if weak.upgrade().is_none() { return; } continue; }
                };
                if !changed {
                    return;
                }
                let Some(me) = weak.upgrade() else { return };
                if let ResourceState::Available { data, .. } = handle.current() {
                    me.hosts.set_etc_hosts(parse_hosts_file(&String::from_utf8_lossy(&data)));
                    tracing::info!(path = %path.display(), entries = me.hosts.etc_hosts_len(), "hosts file reloaded");
                }
            }
        });
    }
}

fn build_plain_udp(spec: &UpstreamSpec) -> UpstreamRef {
    match spec {
        UpstreamSpec::Udp(addr) => Arc::new(UdpUpstream::new(*addr)),
        other => unreachable!("build_plain_udp called with {other:?}"),
    }
}

fn literal(ip: IpAddr, source: Source, started: Instant) -> DnsResult {
    from_ips(&[ip], Duration::ZERO, source, started)
}

fn from_ips(ips: &[IpAddr], ttl: Duration, source: Source, started: Instant) -> DnsResult {
    let mut r = DnsResult {
        v4: Vec::new(),
        v6: Vec::new(),
        ttl,
        source,
        elapsed: started.elapsed(),
    };
    for ip in ips {
        match ip {
            IpAddr::V4(a) => r.v4.push(*a),
            IpAddr::V6(a) => r.v6.push(*a),
        }
    }
    r
}

fn min_ttl(a: &Answers) -> Duration {
    let ttl = a.v4.iter().map(|(_, t)| *t).chain(a.v6.iter().map(|(_, t)| *t)).min().unwrap_or(0);
    Duration::from_secs(u64::from(ttl))
}

fn cached_from(a: &Answers) -> CachedAddrs {
    CachedAddrs {
        v4: a.v4.iter().map(|(ip, _)| *ip).collect(),
        v6: a.v6.iter().map(|(ip, _)| *ip).collect(),
        ttl: min_ttl(a),
        source: a.upstream.clone(),
    }
}

fn from_answers(a: &Answers, source: Source, started: Instant) -> DnsResult {
    DnsResult {
        v4: a.v4.iter().map(|(ip, _)| *ip).collect(),
        v6: a.v6.iter().map(|(ip, _)| *ip).collect(),
        ttl: min_ttl(a),
        source,
        elapsed: started.elapsed(),
    }
}

fn from_cached(a: &CachedAddrs, source: Source, started: Instant) -> DnsResult {
    DnsResult {
        v4: a.v4.clone(),
        v6: a.v6.clone(),
        ttl: a.ttl,
        source,
        elapsed: started.elapsed(),
    }
}

impl LazyResolver for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, Result<ResolvedAddrs, ResolveError>> {
        Box::pin(async move {
            match self.lookup(host, LookupOpts::default()).await {
                Ok(r) => Ok(ResolvedAddrs { v4: r.v4, v6: r.v6 }),
                Err(DnsError::Timeout) => Err(ResolveError::Timeout),
                Err(DnsError::EmptyAnswer) => Err(ResolveError::EmptyAnswer),
                Err(e) => Err(ResolveError::Failed(e.to_string())),
            }
        })
    }
}

impl Resolve for Resolver {
    fn resolve<'a>(&'a self, host: &'a str) -> BoxFuture<'a, io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let r = self.lookup(host, LookupOpts::default()).await.map_err(io::Error::other)?;
            let addrs = r.addrs();
            if addrs.is_empty() {
                return Err(io::Error::new(io::ErrorKind::NotFound, format!("no addresses for {host}")));
            }
            Ok(addrs)
        })
    }
}
```

> `DnsError` 实现 `std::error::Error`，`io::Error::other` 才能接受它。`Answers`、`DnsError` 均为 `Clone`。`ResolvedAddrs` 的字段名以 `rurge_rules::matcher` 为准（`v4` / `v6`）。

`lib.rs`：`pub mod resolver;` 并再导出 `pub use resolver::{DnsError, DnsResult, HostKind, LookupOpts, Resolver, ResolverConfig, ResolverDeps, Source, UpstreamDelay};`。

- [ ] **Step 2: 写测试**

在 `resolver.rs` 末尾追加：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Qtype;
    use crate::system::StaticSystemDns;
    use crate::testing::MockDns;
    use rurge_config::config::{LoadOptions, from_text};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use rurge_net::resource::ResourceOptions;
    use std::path::Path;

    fn fast() -> FanoutOpts {
        FanoutOpts {
            resend: Duration::from_millis(100),
            attempts: 3,
        }
    }

    struct Env {
        _dir: tempfile::TempDir,
        resources: Arc<ResourceManager>,
        sets: Arc<SetRegistry>,
        cfg: Config,
    }

    fn env(profile: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let loaded = from_text(profile, &dir.path().join("t.conf"), &LoadOptions::for_tests());
        let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
        assert!(!loaded.diagnostics.has_errors(), "{codes:?}");
        let cfg = loaded.config;
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector, HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(
            dir.path().to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                debounce: Duration::from_millis(50),
                ..ResourceOptions::default()
            },
        );
        let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir.path());
        Env { _dir: dir, resources, sets, cfg }
    }

    fn resolver(e: &Env, servers: &str, extra: &str, system: StaticSystemDns) -> (Arc<Resolver>, Diagnostics) {
        let _ = (servers, extra);
        let mut cfg = ResolverConfig::from_config(&e.cfg);
        cfg.fanout = fast();
        Resolver::new(
            cfg,
            ResolverDeps {
                connector: Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
                sets: e.sets.clone(),
                system: Arc::new(system),
                resources: e.resources.clone(),
            },
        )
    }

    fn profile(general: &str, hosts: &str) -> String {
        format!("[General]\n{general}\n[Proxy]\nProxyA = http, proxy.example, 8080\n[Host]\n{hosts}\n[Rule]\nFINAL,DIRECT\n")
    }

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    #[tokio::test]
    async fn literals_loopback_and_trailing_dot() {
        let mock = MockDns::spawn().await;
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, diags) = resolver(&e, "", "", StaticSystemDns::default());
        assert!(diags.is_empty(), "{:?}", diags.iter().map(|d| d.code).collect::<Vec<_>>());
        let lit = r.lookup("10.1.2.3", LookupOpts::default()).await.unwrap();
        assert_eq!((lit.v4, lit.source), (vec![v4("10.1.2.3")], Source::Literal));
        let v6 = r.lookup("[::1]", LookupOpts::default()).await.unwrap();
        assert_eq!(v6.v6, vec![Ipv6Addr::LOCALHOST]);
        let lo = r.lookup("LocalHost", LookupOpts::default()).await.unwrap();
        assert_eq!((lo.v4, lo.source), (vec![Ipv4Addr::LOCALHOST], Source::Loopback));
        assert_eq!(r.lookup("app.localhost", LookupOpts::default()).await.unwrap().source, Source::Loopback);
        mock.set("dotted.test", &["10.0.0.7"], &[], 60);
        let d = r.lookup("Dotted.Test.", LookupOpts::default()).await.unwrap();
        assert_eq!(d.v4, vec![v4("10.0.0.7")]);
        assert_eq!(mock.query_count("dotted.test", Qtype::A), 1);
        assert!(r.lookup("", LookupOpts::default()).await.is_err());
    }

    #[tokio::test]
    async fn host_chain_ips_aliases_loops_and_proxy_exemption() {
        let mock = MockDns::spawn().await;
        mock.set("proxy.example", &["10.9.9.9"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", mock.addr()),
            "fixed.test = 1.2.3.4, ::2\nalias.test = fixed.test\nloop-a.test = loop-b.test\nloop-b.test = loop-a.test\nproxy.example = 127.0.0.1\nip-alias.test = 9.9.9.9\n",
        ));
        let (r, _) = resolver(&e, "", "", StaticSystemDns::default());
        let f = r.lookup("fixed.test", LookupOpts::default()).await.unwrap();
        assert_eq!((f.v4.clone(), f.v6.clone(), f.source.clone()), (vec![v4("1.2.3.4")], vec!["::2".parse().unwrap()], Source::Host(HostKind::Ip)));
        assert_eq!(f.ttl, Duration::ZERO);
        let a = r.lookup("alias.test", LookupOpts::default()).await.unwrap();
        assert_eq!(a.v4, vec![v4("1.2.3.4")]);
        assert!(matches!(r.lookup("loop-a.test", LookupOpts::default()).await, Err(DnsError::AliasLoop(_))));
        // proxy hostnames bypass [Host] and go upstream
        let p = r.lookup("proxy.example", LookupOpts::default()).await.unwrap();
        assert_eq!(p.v4, vec![v4("10.9.9.9")]);
        assert!(matches!(p.source, Source::Upstream(_)));
        assert_eq!(r.lookup("ip-alias.test", LookupOpts::default()).await.unwrap().v4, vec![v4("9.9.9.9")]);
    }

    #[tokio::test]
    async fn host_server_and_system_modes_and_special_names() {
        let primary = MockDns::spawn().await;
        let dedicated = MockDns::spawn().await;
        let system = MockDns::spawn().await;
        dedicated.set("corp.test", &["10.10.0.1"], &[], 60);
        system.set("printer.local", &["10.20.0.1"], &[], 60);
        system.set("nas.home.lan", &["10.20.0.2"], &[], 60);
        system.set("sys.test", &["10.20.0.3"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}", primary.addr()),
            &format!("corp.test = server:{}\nsys.test = server:system\n", dedicated.addr()),
        ));
        let sys = StaticSystemDns {
            servers: vec![system.addr()],
            search_domains: vec!["home.lan".into()],
            ..StaticSystemDns::default()
        };
        let (r, _) = resolver(&e, "", "", sys);
        let c = r.lookup("corp.test", LookupOpts::default()).await.unwrap();
        assert_eq!((c.v4, c.source), (vec![v4("10.10.0.1")], Source::Host(HostKind::Server)));
        assert_eq!(primary.query_count("corp.test", Qtype::A), 0);
        let s = r.lookup("sys.test", LookupOpts::default()).await.unwrap();
        assert_eq!((s.v4, s.source), (vec![v4("10.20.0.3")], Source::Host(HostKind::System)));
        let l = r.lookup("printer.local", LookupOpts::default()).await.unwrap();
        assert_eq!((l.v4, l.source), (vec![v4("10.20.0.1")], Source::System));
        let n = r.lookup("nas", LookupOpts::default()).await.unwrap();
        assert_eq!(n.v4, vec![v4("10.20.0.2")], "single label + first search domain");
        assert_eq!(system.query_count("nas.home.lan", Qtype::A), 1);
        assert_eq!(primary.query_count("nas.home.lan", Qtype::A), 0);
    }

    #[tokio::test]
    async fn cache_fresh_stale_negative_and_coalescing() {
        let mock = MockDns::spawn().await;
        mock.set("c.test", &["10.0.0.1"], &[], 1);
        mock.set_empty("nx.test");
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver(&e, "", "", StaticSystemDns::default());
        let first = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert!(matches!(first.source, Source::Upstream(_)));
        let second = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert_eq!(second.source, Source::Cache { stale: false });
        assert_eq!(mock.query_count("c.test", Qtype::A), 1);
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let stale = r.lookup("c.test", LookupOpts::default()).await.unwrap();
        assert_eq!(stale.source, Source::Cache { stale: true });
        assert_eq!(stale.v4, vec![v4("10.0.0.1")]);
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(mock.query_count("c.test", Qtype::A), 2, "one background refresh");
        assert_eq!(r.lookup("c.test", LookupOpts::default()).await.unwrap().source, Source::Cache { stale: false });
        assert_eq!(r.lookup("nx.test", LookupOpts::default()).await, Err(DnsError::EmptyAnswer));
        assert_eq!(r.lookup("nx.test", LookupOpts::default()).await, Err(DnsError::EmptyAnswer));
        assert_eq!(mock.query_count("nx.test", Qtype::A), 1, "negative answer cached");
        let snap = r.cache_snapshot();
        assert_eq!(snap.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["c.test", "nx.test"]);
        // coalescing: two concurrent lookups of a new name → one upstream query
        mock.set("co.test", &["10.0.0.2"], &[], 60);
        mock.set_delay(Duration::from_millis(80));
        let r2 = r.clone();
        let (a, b) = tokio::join!(
            r.lookup("co.test", LookupOpts::default()),
            r2.lookup("co.test", LookupOpts::default())
        );
        assert_eq!(a.unwrap().v4, b.unwrap().v4);
        assert_eq!(mock.query_count("co.test", Qtype::A), 1);
        mock.set_delay(Duration::ZERO);
        r.flush();
        assert!(r.cache_snapshot().is_empty());
        let bypass = r.lookup("c.test", LookupOpts { bypass_cache: true, want_v6: None }).await.unwrap();
        assert!(matches!(bypass.source, Source::Upstream(_)));
    }

    #[tokio::test]
    async fn aaaa_suppression_after_five_timeouts_and_flush_resumes() {
        let mock = MockDns::spawn().await;
        for i in 0..7 {
            mock.set(&format!("h{i}.test"), &["10.0.0.1"], &["fd00::1"], 60);
        }
        mock.set_drop_qtype(Qtype::Aaaa, true);
        let e = env(&profile(&format!("dns-server = {}\nipv6 = true", mock.addr()), ""));
        let sys = StaticSystemDns { has_ipv6: true, ..StaticSystemDns::default() };
        let (r, _) = resolver(&e, "", "", sys);
        for i in 0..5 {
            let res = r.lookup(&format!("h{i}.test"), LookupOpts::default()).await.unwrap();
            assert_eq!(res.v4.len(), 1);
            assert!(res.v6.is_empty());
        }
        assert!(r.aaaa_suppressed());
        r.lookup("h5.test", LookupOpts::default()).await.unwrap();
        assert_eq!(mock.query_count("h5.test", Qtype::Aaaa), 0, "AAAA no longer asked");
        r.flush();
        assert!(!r.aaaa_suppressed());
        mock.set_drop_qtype(Qtype::Aaaa, false);
        let res = r.lookup("h6.test", LookupOpts::default()).await.unwrap();
        assert_eq!(res.v6.len(), 1);
        assert_eq!(mock.query_count("h6.test", Qtype::Aaaa), 1);
    }

    #[tokio::test]
    async fn encrypted_upstreams_take_over_and_udp_bootstraps_them() {
        let udp = MockDns::spawn().await;
        let tcp = MockDns::spawn().await;
        udp.set("dns.example", &["127.0.0.1"], &[], 60);
        tcp.set("a.test", &["10.0.0.42"], &[], 60);
        udp.set("a.test", &["10.0.0.1"], &[], 60);
        let e = env(&profile(
            &format!("dns-server = {}\nencrypted-dns-server = tcp://dns.example:{}", udp.addr(), tcp.addr().port()),
            "",
        ));
        let (r, diags) = resolver(&e, "", "", StaticSystemDns::default());
        assert!(diags.is_empty());
        assert_eq!(r.primary_upstreams(), vec![format!("tcp://dns.example:{}", tcp.addr().port())]);
        let a = r.lookup("a.test", LookupOpts::default()).await.unwrap();
        assert_eq!(a.v4, vec![v4("10.0.0.42")], "normal queries go to the encrypted subsystem only");
        assert_eq!(udp.query_count("dns.example", Qtype::A), 1, "UDP only bootstrapped the tcp:// hostname");
        assert_eq!(udp.query_count("a.test", Qtype::A), 0);
        assert_eq!(tcp.query_count("a.test", Qtype::A), 1);
    }

    #[tokio::test]
    async fn unsupported_upstreams_warn_and_ipv6_servers_drop_when_ipv6_is_off() {
        let mock = MockDns::spawn().await;
        let e = env(&profile(
            &format!("dns-server = {}, [::1]:5353\nencrypted-dns-server = h3://dns.example/dns-query", mock.addr()),
            "",
        ));
        let (r, diags) = resolver(&e, "", "", StaticSystemDns::default());
        assert!(diags.iter().any(|d| d.code == codes::W_DNS_UPSTREAM_UNSUPPORTED));
        assert_eq!(r.primary_upstreams(), vec![format!("udp://{}", mock.addr())]);
    }

    #[tokio::test]
    async fn no_servers_uses_system_upstreams_then_lookup_host() {
        let system = MockDns::spawn().await;
        system.set("s.test", &["10.30.0.1"], &[], 60);
        let e = env(&profile("", ""));
        let (r, _) = resolver(&e, "", "", StaticSystemDns { servers: vec![system.addr()], ..StaticSystemDns::default() });
        assert!(r.primary_upstreams().contains(&format!("udp://{}", system.addr())));
        let s = r.lookup("s.test", LookupOpts::default()).await.unwrap();
        assert_eq!(s.v4, vec![v4("10.30.0.1")]);
        let (r2, _) = resolver(&e, "", "", StaticSystemDns::default());
        assert!(r2.primary_upstreams().is_empty());
        let lo = r2.lookup("localhost", LookupOpts::default()).await.unwrap();
        assert_eq!(lo.source, Source::Loopback);
    }

    #[tokio::test]
    async fn etc_hosts_are_read_and_watched() {
        let mock = MockDns::spawn().await;
        let dir = tempfile::tempdir().unwrap();
        let hosts_path = dir.path().join("hosts");
        std::fs::write(&hosts_path, "10.40.0.1 nas.lan\n").unwrap();
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let sys = StaticSystemDns { hosts_path: Some(hosts_path.clone()), ..StaticSystemDns::default() };
        let (r, _) = resolver(&e, "", "", sys);
        let n = r.lookup("nas.lan", LookupOpts::default()).await.unwrap();
        assert_eq!((n.v4, n.source), (vec![v4("10.40.0.1")], Source::Host(HostKind::EtcHosts)));
        std::fs::write(&hosts_path, "10.40.0.2 nas.lan\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let n = r.lookup("nas.lan", LookupOpts::default()).await.unwrap();
            if n.v4 == vec![v4("10.40.0.2")] {
                break;
            }
            assert!(Instant::now() < deadline, "hosts file change not picked up");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    #[tokio::test]
    async fn implements_lazy_resolver_and_resolve() {
        let mock = MockDns::spawn().await;
        mock.set("t.test", &["10.0.0.1"], &[], 60);
        mock.set_empty("nx.test");
        let e = env(&profile(&format!("dns-server = {}", mock.addr()), ""));
        let (r, _) = resolver(&e, "", "", StaticSystemDns::default());
        let lazy: &dyn LazyResolver = r.as_ref();
        assert_eq!(lazy.resolve("t.test").await.unwrap().v4, vec![v4("10.0.0.1")]);
        assert_eq!(lazy.resolve("nx.test").await, Err(ResolveError::EmptyAnswer));
        let res: &dyn Resolve = r.as_ref();
        assert_eq!(res.resolve("t.test").await.unwrap(), vec![IpAddr::V4(v4("10.0.0.1"))]);
        assert!(res.resolve("nx.test").await.is_err());
        let delays = r.measure_delay("t.test").await;
        assert_eq!(delays.len(), 1);
        assert!(delays[0].result.is_ok());
    }
}
```

> `MockDns` 需要同时支持 UDP 与 TCP（Task 4）；`set_delay` 影响全部应答；`tokio::join!` 需要 tokio 的 `macros` feature（已启用）。`from_config` 依赖 M1 对 `dns-server` 中 `[::1]:5353` 的解析（若 M1 不接受带端口的 IPv6，改用 `::1`）。

- [ ] **Step 3: 运行**

```bash
cargo test -p rurge-dns resolver
```

预期：9 个测试通过（含约 1.5 s 的缓存过期等待与 AAAA 抑制用例 5 × 100 ms 的重发等待）。

- [ ] **Step 4: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add crates/rurge-dns
git commit -F - <<'EOF'
feat(dns): Resolver：上游选择、引导、缓存、[Host] 链、hosts 文件、特殊主机名、AAAA 抑制与查询合并

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 13: CLI —— `rurge dns lookup` / `rurge dns cache`、`rule match` 换用 `Resolver`

**Files:**
- Modify: `Cargo.toml`（workspace 追加 `tracing-subscriber = "0.3"`）
- Modify: `crates/rurge/Cargo.toml`
- Modify: `crates/rurge/src/main.rs`
- Modify: `crates/rurge/src/cli/mod.rs`
- Modify: `crates/rurge/src/cli/runtime.rs`
- Modify: `crates/rurge/src/cli/rule.rs:4-16`、`rule.rs:176`、`rule.rs:216-229`
- Create: `crates/rurge/src/cli/dns.rs`
- Modify: `crates/rurge/tests/cli.rs`（追加 `mod dns`）

**Interfaces:**
- Consumes: `rurge_dns::{Resolver, ResolverConfig, ResolverDeps, LookupOpts, DnsResult, DnsError}`（Task 12：`ResolverConfig::from_config(&Config)`、公开字段 `servers` / `encrypted` / `cache_capacity`；`Resolver::new(cfg, deps) -> (Arc<Resolver>, Diagnostics)`、`lookup(&self, &str, LookupOpts) -> Result<DnsResult, DnsError>`、`cache_snapshot() -> Vec<CacheEntry>`、`primary_upstreams() -> Vec<String>`；`DnsResult { v4, v6, ttl, source, elapsed }` + `addrs()`）；`rurge_dns::cache::{CacheEntry { name, v4, v6, expires_in, stale, negative, source }, DEFAULT_CAPACITY}`（Task 10）；`rurge_dns::system::SystemDns`（Task 11）；`rurge_platform::dns::{servers, search_domains, hosts_path, has_ipv6}`（Task 2）；`rurge_config::general::{DnsServer, EncryptedDns::parse}`；`rurge_config::Diagnostics::extend`；Task 8 的 `tracing` 事件（target `rurge_dns::fanout`）；`rurge_dns::testing::MockDns`（测试）。
- Produces：
  - `RuntimeArgs.dns_cache_size: Option<usize>`（`--dns-cache-size` / `RURGE_DNS_CACHE_SIZE`）→ `Runtime.dns_cache_size: usize`（默认 `DEFAULT_CAPACITY`，最小 1）。
  - `Stack.resolver: Arc<Resolver>`；`build_stack(cfg, rt, wait)` 保持签名，新增 `build_stack_with(cfg, rt, wait, customize: impl FnOnce(&mut ResolverConfig)) -> anyhow::Result<Stack>`；`PlatformSystemDns`（`SystemDns` 的平台适配器）。`SystemLazyResolver` 删除。
  - `rurge dns lookup -c <conf> <name> [--type a|aaaa|both] [--server <spec>...] [--no-cache] [--trace] [--json] [--wait <secs>] [--platform <p>] + RuntimeArgs`；`rurge dns cache -c <conf> [name...] [--server <spec>...] [--json] ...`。退出码：0 成功；1 `EmptyAnswer`（`--type aaaa` 且无 AAAA 记录同样算空）；2 其他错误（含配置错误、非法 `--server`）。
  - 文本输出：`name:`、`addresses:`（逗号分隔，v4 在前；无则 `(none)`）、`source:`、`ttl: <n>s`、`elapsed: <x.y>ms`、`upstreams:`（主上游列表，空则 `(system)`）；出错时 `error: <msg>`。JSON：`{ name, addresses, v4, v6, source, ttl_secs, elapsed_ms, error, upstreams, warnings }`。`--trace` 把 `rurge_dns` 的 debug 事件打印到 stderr。
  - `dns cache`：先解析给定名字（失败打 `warning:` 到 stderr），再打印快照：文本 `entries: <n>` + 每行 `<name> <state> <source> <addrs>`（state = `<n>s` 剩余 / `stale` / `negative` / `expired`）；JSON `{ entries: [{ name, v4, v6, expires_in_secs, stale, negative, source }], warnings }`。

- [ ] **Step 1: 依赖**

workspace `Cargo.toml` 的 `[workspace.dependencies]` 追加：

```toml
tracing-subscriber = "0.3"
```

`crates/rurge/Cargo.toml`：

```toml
[dependencies]
rurge-config.workspace = true
rurge-rules.workspace = true
rurge-net.workspace = true
rurge-dns.workspace = true
rurge-platform.workspace = true
anyhow.workspace = true
clap.workspace = true
serde.workspace = true
serde_json.workspace = true
tokio.workspace = true
tracing-subscriber.workspace = true
url.workspace = true

[dev-dependencies]
rurge-dns = { workspace = true, features = ["testing"] }
rurge-net = { workspace = true, features = ["testing"] }
assert_cmd.workspace = true
predicates.workspace = true
tempfile.workspace = true
```

- [ ] **Step 2: `runtime.rs`**

替换 `crates/rurge/src/cli/runtime.rs` 全文：

```rust
//! rurge-specific runtime options (FR-CFG-17): command-line flags and
//! environment variables only, never profile keys. Also assembles the shared
//! objects (resource manager, set registry, GeoIP, resolver) the offline
//! commands need.

use anyhow::Context;
use clap::Args;
use rurge_config::{Config, Diagnostics};
use rurge_dns::cache::DEFAULT_CAPACITY;
use rurge_dns::system::SystemDns;
use rurge_dns::{Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::{GeoDb, GeoUpdater, GeoUrls, SetRegistry};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

#[derive(Args, Clone, Debug)]
pub struct RuntimeArgs {
    /// Data directory for caches and databases (default: the platform data dir)
    #[arg(long, env = "RURGE_DATA_DIR", value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// GeoIP country database URL (overrides geoip-maxmind-url)
    #[arg(long, env = "RURGE_GEOIP_URL", value_name = "URL")]
    pub geoip_url: Option<Url>,
    /// GeoIP ASN database URL
    #[arg(long, env = "RURGE_GEOIP_ASN_URL", value_name = "URL")]
    pub geoip_asn_url: Option<Url>,
    /// Never touch the network: use cached resources only
    #[arg(long, env = "RURGE_NO_NETWORK")]
    pub no_network: bool,
    /// DNS cache capacity in entries (default 2000)
    #[arg(long, env = "RURGE_DNS_CACHE_SIZE", value_name = "N")]
    pub dns_cache_size: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Runtime {
    pub data_dir: PathBuf,
    pub geo_urls: GeoUrls,
    pub no_network: bool,
    pub dns_cache_size: usize,
}

impl RuntimeArgs {
    pub fn resolve(&self, cfg: &Config) -> anyhow::Result<Runtime> {
        let data_dir = self
            .data_dir
            .clone()
            .unwrap_or_else(rurge_platform::dirs::data_dir);
        std::fs::create_dir_all(&data_dir)
            .with_context(|| format!("cannot create data dir {}", data_dir.display()))?;
        let mut geo_urls = GeoUrls::default();
        if let Some(u) = &cfg.general.geoip_maxmind_url {
            match Url::parse(u) {
                Ok(parsed) => geo_urls.country = parsed,
                Err(e) => eprintln!("warning: ignoring geoip-maxmind-url `{u}`: {e}"),
            }
        }
        if let Some(u) = &self.geoip_url {
            geo_urls.country = u.clone();
        }
        if let Some(u) = &self.geoip_asn_url {
            geo_urls.asn = u.clone();
        }
        Ok(Runtime {
            data_dir,
            geo_urls,
            no_network: self.no_network,
            dns_cache_size: self.dns_cache_size.unwrap_or(DEFAULT_CAPACITY).max(1),
        })
    }
}

pub struct Stack {
    /// Kept alive so the manager's background refresh tasks keep running;
    /// callers reach individual resources through `registry` and `geo`.
    #[allow(dead_code)]
    pub resources: Arc<ResourceManager>,
    pub registry: Arc<SetRegistry>,
    pub geo: Arc<GeoDb>,
    /// Kept alive so its install tasks are not aborted by `Drop`.
    #[allow(dead_code)]
    pub geo_updater: Option<GeoUpdater>,
    pub resolver: Arc<Resolver>,
    pub diagnostics: Diagnostics,
}

/// Builds resources → set registry → GeoIP → resolver, then waits up to
/// `wait` for the first fetch of every resource (skipped in `--no-network` mode).
pub async fn build_stack(cfg: &Config, rt: &Runtime, wait: Duration) -> anyhow::Result<Stack> {
    build_stack_with(cfg, rt, wait, |_| {}).await
}

/// `build_stack` with a hook that edits the resolver configuration before
/// the resolver is built (`dns lookup --server`).
pub async fn build_stack_with(
    cfg: &Config,
    rt: &Runtime,
    wait: Duration,
    customize: impl FnOnce(&mut ResolverConfig),
) -> anyhow::Result<Stack> {
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default())?);
    let resources = ResourceManager::with_options(
        rt.data_dir.clone(),
        client,
        ResourceOptions {
            offline: rt.no_network,
            ..ResourceOptions::default()
        },
    );
    let base_dir = cfg
        .source
        .main
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let (registry, mut diagnostics) = SetRegistry::build(cfg, resources.clone(), &base_dir);
    let (geo, geo_diags) = GeoDb::open(&rt.data_dir.join("geoip"));
    for d in geo_diags {
        diagnostics.push(d);
    }
    let geo_updater = (!rt.no_network).then(|| {
        GeoUpdater::spawn(
            geo.clone(),
            resources.clone(),
            rt.geo_urls.clone(),
            !cfg.general.disable_geoip_db_auto_update,
        )
    });
    let mut resolver_cfg = ResolverConfig::from_config(cfg);
    resolver_cfg.cache_capacity = rt.dns_cache_size;
    customize(&mut resolver_cfg);
    let (resolver, dns_diags) = Resolver::new(
        resolver_cfg,
        ResolverDeps {
            connector,
            sets: registry.clone(),
            system: Arc::new(PlatformSystemDns),
            resources: resources.clone(),
        },
    );
    diagnostics.extend(dns_diags);
    if !rt.no_network && !wait.is_zero() {
        resources.wait_initial(wait).await;
        settle(&registry, &geo, &resources).await;
    }
    Ok(Stack {
        resources,
        registry,
        geo,
        geo_updater,
        resolver,
        diagnostics,
    })
}

/// Gives the background reload / install tasks up to two seconds to apply
/// resources that just arrived.
async fn settle(registry: &SetRegistry, geo: &GeoDb, resources: &ResourceManager) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        let sets_pending = registry
            .statuses()
            .iter()
            .any(|s| s.state == "missing" || s.state == "compiling");
        let geo_pending = {
            let info = geo.info();
            let available = |name: &str| {
                resources
                    .statuses()
                    .iter()
                    .any(|s| s.state == "available" && s.source.to_string().ends_with(name))
            };
            (info.country_epoch.is_none() && available("GeoLite2-Country.mmdb"))
                || (info.asn_epoch.is_none() && available("GeoLite2-ASN.mmdb"))
        };
        if (!sets_pending && !geo_pending) || tokio::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// `rurge-platform::dns` behind the `SystemDns` trait (AR-02: platform code
/// stays in rurge-platform; rurge-dns only sees the trait).
pub struct PlatformSystemDns;

impl SystemDns for PlatformSystemDns {
    fn servers(&self) -> Vec<SocketAddr> {
        rurge_platform::dns::servers()
    }

    fn search_domains(&self) -> Vec<String> {
        rurge_platform::dns::search_domains()
    }

    fn hosts_path(&self) -> Option<PathBuf> {
        let path = rurge_platform::dns::hosts_path();
        path.is_file().then_some(path)
    }

    fn has_ipv6(&self) -> bool {
        rurge_platform::dns::has_ipv6()
    }
}
```

（`settle` 函数原样保留；`SystemLazyResolver` 及其 `BoxFuture` / `LazyResolver` / `ResolvedAddrs` 导入随之删除。）

- [ ] **Step 3: `rule.rs` 换用 `Resolver`**

`crates/rurge/src/cli/rule.rs`：
- 第 4 行改为 `use super::runtime::{RuntimeArgs, build_stack};`，追加 `use std::sync::Arc;`。
- 第 176 行 `fn print_diagnostics` 改为 `pub(crate) fn print_diagnostics`（`dns.rs` 复用）。
- 第 216–229 行的解析器选择改为：

```rust
        let resolver: Arc<dyn LazyResolver> = if args.no_dns {
            Arc::new(NoResolve)
        } else if !args.resolve.is_empty() {
            let mut fixed = ResolvedAddrs::default();
            for ip in &args.resolve {
                match ip {
                    IpAddr::V4(v) => fixed.v4.push(*v),
                    IpAddr::V6(v) => fixed.v6.push(*v),
                }
            }
            Arc::new(FixedResolve(fixed))
        } else {
            stack.resolver.clone()
        };
```

其余（`resolver.as_ref()` 的两处调用）不变。

- [ ] **Step 4: 写 `dns.rs`**

```rust
//! `rurge dns lookup` / `rurge dns cache`: resolve names through the
//! profile's DNS settings without running the daemon (M2 design §10.2).

use super::rule::print_diagnostics;
use super::runtime::{RuntimeArgs, Stack, build_stack_with};
use crate::capabilities;
use clap::{Args, Subcommand, ValueEnum};
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::general::{DnsServer, EncryptedDns};
use rurge_config::{Config, Diagnostics};
use rurge_dns::{DnsError, DnsResult, LookupOpts, Resolver};
use serde_json::json;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Args)]
pub struct DnsArgs {
    #[command(subcommand)]
    pub command: DnsCommand,
}

#[derive(Subcommand)]
pub enum DnsCommand {
    /// Resolve a name through the profile's DNS settings
    Lookup(LookupArgs),
    /// Resolve the given names, then print this process's cache snapshot
    Cache(CacheArgs),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum QueryType {
    A,
    Aaaa,
    Both,
}

#[derive(Args)]
pub struct CommonArgs {
    /// Profile to load
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Override the profile's upstreams (dns-server / encrypted-dns-server syntax)
    #[arg(long, value_name = "SPEC")]
    pub server: Vec<String>,
    /// Seconds to wait for external resources ([Host] rule sets) to download
    #[arg(long, default_value = "30")]
    pub wait: u64,
    /// JSON output
    #[arg(long)]
    pub json: bool,
    /// Evaluate the profile as if running on this platform
    #[arg(long, value_parser = super::check::parse_platform)]
    pub platform: Option<Platform>,
    #[command(flatten)]
    pub runtime: RuntimeArgs,
}

#[derive(Args)]
pub struct LookupArgs {
    /// Name to resolve
    pub name: String,
    /// Record types to ask for
    #[arg(long = "type", value_enum, default_value = "both")]
    pub qtype: QueryType,
    /// Bypass the cache
    #[arg(long)]
    pub no_cache: bool,
    /// Print every attempt (upstream, record type, outcome, timing) to stderr
    #[arg(long)]
    pub trace: bool,
    #[command(flatten)]
    pub common: CommonArgs,
}

#[derive(Args)]
pub struct CacheArgs {
    /// Names to resolve before printing the snapshot
    pub names: Vec<String>,
    #[command(flatten)]
    pub common: CommonArgs,
}

pub fn run(args: DnsArgs) -> anyhow::Result<ExitCode> {
    match args.command {
        DnsCommand::Lookup(a) => run_lookup(a),
        DnsCommand::Cache(a) => run_cache(a),
    }
}

/// `--server` values follow the `dns-server` key: `system`, `ip[:port]`
/// (`[v6]:port`), or an encrypted URL (`https://`, `tls://`, `tcp://`).
fn parse_servers(specs: &[String]) -> anyhow::Result<(Vec<DnsServer>, Vec<EncryptedDns>)> {
    let mut servers = Vec::new();
    let mut encrypted = Vec::new();
    for spec in specs {
        let s = spec.trim();
        if s.eq_ignore_ascii_case("system") {
            servers.push(DnsServer::System);
        } else if let Some(enc) = EncryptedDns::parse(s) {
            encrypted.push(enc);
        } else if let Ok(sa) = s.parse::<SocketAddr>() {
            servers.push(DnsServer::Udp(sa));
        } else if let Ok(ip) = s.trim_matches(['[', ']']).parse::<IpAddr>() {
            servers.push(DnsServer::Udp(SocketAddr::new(ip, 53)));
        } else {
            anyhow::bail!(
                "invalid --server `{s}` (expected system, ip[:port] or an encrypted DNS URL)"
            );
        }
    }
    Ok((servers, encrypted))
}

/// Loads the profile; `None` means errors were printed and the caller exits 2.
fn load_profile(common: &CommonArgs) -> anyhow::Result<Option<(Config, Diagnostics)>> {
    let platform = common.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, capabilities::CORE_VERSION),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(&common.config, &opts)?;
    if loaded.diagnostics.has_errors() {
        print_diagnostics(&loaded.diagnostics.sorted());
        return Ok(None);
    }
    Ok(Some((loaded.config, loaded.diagnostics)))
}

async fn build(common: &CommonArgs, cfg: &Config) -> anyhow::Result<Stack> {
    let rt = common.runtime.resolve(cfg)?;
    let overrides = (!common.server.is_empty())
        .then(|| parse_servers(&common.server))
        .transpose()?;
    build_stack_with(cfg, &rt, Duration::from_secs(common.wait), move |rc| {
        if let Some((servers, encrypted)) = overrides {
            rc.servers = servers;
            rc.encrypted = encrypted;
        }
    })
    .await
}

/// `--trace`: rurge-dns debug events (one per attempt) on stderr.
fn install_trace() {
    use tracing_subscriber::filter::{LevelFilter, Targets};
    use tracing_subscriber::prelude::*;
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_target(true)
                .without_time(),
        )
        .with(Targets::new().with_target("rurge_dns", LevelFilter::DEBUG))
        .try_init();
}

fn warnings(config_diags: &Diagnostics, stack_diags: &Diagnostics) -> Vec<serde_json::Value> {
    config_diags
        .iter()
        .chain(stack_diags.iter())
        .map(|x| json!({ "code": x.code, "message": x.message }))
        .collect()
}

fn run_lookup(args: LookupArgs) -> anyhow::Result<ExitCode> {
    let Some((cfg, config_diags)) = load_profile(&args.common)? else {
        return Ok(ExitCode::from(2));
    };
    if args.trace {
        install_trace();
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stack = build(&args.common, &cfg).await?;
        let opts = LookupOpts {
            bypass_cache: args.no_cache,
            want_v6: match args.qtype {
                QueryType::A => Some(false),
                QueryType::Aaaa => Some(true),
                QueryType::Both => None,
            },
        };
        let mut result = stack.resolver.lookup(&args.name, opts).await;
        if args.qtype == QueryType::Aaaa {
            result = match result {
                Ok(r) if r.v6.is_empty() => Err(DnsError::EmptyAnswer),
                Ok(mut r) => {
                    r.v4.clear();
                    Ok(r)
                }
                Err(e) => Err(e),
            };
        }
        let code = match &result {
            Ok(_) => ExitCode::SUCCESS,
            Err(DnsError::EmptyAnswer) => ExitCode::from(1),
            Err(_) => ExitCode::from(2),
        };
        if args.common.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&lookup_json(
                    &args.name,
                    &result,
                    &stack.resolver,
                    &config_diags,
                    &stack.diagnostics
                ))?
            );
        } else {
            print_diagnostics(&config_diags.sorted());
            print_diagnostics(&stack.diagnostics);
            print_lookup(&args.name, &result, &stack.resolver);
        }
        drop(stack);
        Ok(code)
    })
}

fn print_lookup(name: &str, result: &Result<DnsResult, DnsError>, resolver: &Resolver) {
    println!("name: {name}");
    match result {
        Ok(r) => {
            let addrs: Vec<String> = r.addrs().iter().map(ToString::to_string).collect();
            println!(
                "addresses: {}",
                if addrs.is_empty() {
                    "(none)".to_string()
                } else {
                    addrs.join(", ")
                }
            );
            println!("source: {}", r.source);
            println!("ttl: {}s", r.ttl.as_secs());
            println!("elapsed: {:.1}ms", r.elapsed.as_secs_f64() * 1000.0);
        }
        Err(e) => println!("error: {e}"),
    }
    let ups = resolver.primary_upstreams();
    println!(
        "upstreams: {}",
        if ups.is_empty() {
            "(system)".to_string()
        } else {
            ups.join(", ")
        }
    );
}

fn lookup_json(
    name: &str,
    result: &Result<DnsResult, DnsError>,
    resolver: &Resolver,
    config_diags: &Diagnostics,
    stack_diags: &Diagnostics,
) -> serde_json::Value {
    let (addresses, v4, v6, source, ttl, elapsed, error) = match result {
        Ok(r) => (
            r.addrs().iter().map(ToString::to_string).collect::<Vec<_>>(),
            r.v4.iter().map(ToString::to_string).collect::<Vec<_>>(),
            r.v6.iter().map(ToString::to_string).collect::<Vec<_>>(),
            Some(r.source.to_string()),
            Some(r.ttl.as_secs()),
            Some(r.elapsed.as_secs_f64() * 1000.0),
            None,
        ),
        Err(e) => (Vec::new(), Vec::new(), Vec::new(), None, None, None, Some(e.to_string())),
    };
    json!({
        "name": name,
        "addresses": addresses,
        "v4": v4,
        "v6": v6,
        "source": source,
        "ttl_secs": ttl,
        "elapsed_ms": elapsed,
        "error": error,
        "upstreams": resolver.primary_upstreams(),
        "warnings": warnings(config_diags, stack_diags),
    })
}

fn run_cache(args: CacheArgs) -> anyhow::Result<ExitCode> {
    let Some((cfg, config_diags)) = load_profile(&args.common)? else {
        return Ok(ExitCode::from(2));
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let stack = build(&args.common, &cfg).await?;
        for name in &args.names {
            if let Err(e) = stack.resolver.lookup(name, LookupOpts::default()).await {
                eprintln!("warning: {name}: {e}");
            }
        }
        let snapshot = stack.resolver.cache_snapshot();
        if args.common.json {
            let entries: Vec<serde_json::Value> = snapshot
                .iter()
                .map(|e| {
                    json!({
                        "name": e.name,
                        "v4": e.v4.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "v6": e.v6.iter().map(ToString::to_string).collect::<Vec<_>>(),
                        "expires_in_secs": e.expires_in.map(|d| d.as_secs()),
                        "stale": e.stale,
                        "negative": e.negative,
                        "source": e.source,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "entries": entries,
                    "warnings": warnings(&config_diags, &stack.diagnostics),
                }))?
            );
        } else {
            print_diagnostics(&config_diags.sorted());
            print_diagnostics(&stack.diagnostics);
            println!("entries: {}", snapshot.len());
            for e in &snapshot {
                let state = if e.negative {
                    "negative".to_string()
                } else if e.stale {
                    "stale".to_string()
                } else {
                    match e.expires_in {
                        Some(d) => format!("{}s", d.as_secs()),
                        None => "expired".to_string(),
                    }
                };
                let mut addrs: Vec<String> = e.v4.iter().map(ToString::to_string).collect();
                addrs.extend(e.v6.iter().map(ToString::to_string));
                println!(
                    "{:<40} {:<10} {:<28} {}",
                    e.name,
                    state,
                    e.source,
                    addrs.join(", ")
                );
            }
        }
        drop(stack);
        Ok(ExitCode::SUCCESS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rurge_config::general::EncryptedDnsScheme;

    #[test]
    fn server_specs_follow_the_dns_server_syntax() {
        let specs: Vec<String> = [
            "system",
            "1.1.1.1",
            "8.8.8.8:5353",
            "[2001:db8::1]:53",
            "tcp://dns.example",
            "https://dns.example/dns-query",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (servers, encrypted) = parse_servers(&specs).unwrap();
        assert_eq!(
            servers,
            vec![
                DnsServer::System,
                DnsServer::Udp("1.1.1.1:53".parse().unwrap()),
                DnsServer::Udp("8.8.8.8:5353".parse().unwrap()),
                DnsServer::Udp("[2001:db8::1]:53".parse().unwrap()),
            ]
        );
        assert_eq!(encrypted.len(), 2);
        assert_eq!(encrypted[0].scheme, EncryptedDnsScheme::Tcp);
        assert_eq!(encrypted[1].scheme, EncryptedDnsScheme::Https);
        let err = parse_servers(&["nonsense".to_string()]).unwrap_err();
        assert!(err.to_string().contains("invalid --server"), "{err}");
    }
}
```

> `str::trim_matches(['[', ']'])` 需要 Rust ≥ 1.58；`Option::transpose` 把 `Option<Result<_>>` 变为 `Result<Option<_>>`。`tracing_subscriber::registry()` / `fmt::layer()` / `Targets` 都在 0.3 的默认 feature 内；`without_time()` 避免依赖 `time` crate。若 `EncryptedDns` 的 `scheme` 字段为私有，测试改为断言 `encrypted[0].url` 前缀。

- [ ] **Step 5: 挂子命令**

`crates/rurge/src/cli/mod.rs`：追加 `pub mod dns;`。

`crates/rurge/src/main.rs` 的 `Command`：

```rust
    /// Rule engine tools (offline)
    Rule(Box<cli::rule::RuleArgs>),
    /// DNS tools (offline)
    Dns(Box<cli::dns::DnsArgs>),
```

`main` 的 match 追加 `Command::Dns(args) => cli::dns::run(*args),`。

- [ ] **Step 6: CLI 测试**

`crates/rurge/tests/cli.rs` 末尾追加：

```rust
mod dns {
    use assert_cmd::Command;
    use rurge_dns::message::Qtype;
    use rurge_dns::testing::MockDns;
    use std::path::Path;

    const CONF: &str = "[General]\nipv6 = false\n[Proxy]\n[Host]\nfixed.test = 1.2.3.4\n[Rule]\nFINAL,DIRECT\n";

    fn workspace() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("t.conf"), CONF).unwrap();
        dir
    }

    fn dns_cmd(dir: &Path, sub: &str, server: &str, extra: &[&str]) -> Command {
        let mut cmd = Command::cargo_bin("rurge").unwrap();
        cmd.arg("dns")
            .arg(sub)
            .arg("-c")
            .arg(dir.join("t.conf"))
            .arg("--server")
            .arg(server)
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.join("data"))
            .args(extra);
        cmd
    }

    /// Runs the binary off the runtime thread so the mock upstream keeps serving.
    async fn output(mut cmd: Command) -> std::process::Output {
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
            .await
            .unwrap()
    }

    fn json_of(out: &std::process::Output) -> serde_json::Value {
        serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!("bad json: {e}\n{}", String::from_utf8_lossy(&out.stdout))
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lookup_over_udp_prints_addresses_source_and_ttl() {
        let mock = MockDns::spawn().await;
        mock.set("a.test", &["10.0.0.1", "10.0.0.2"], &[], 120);
        let dir = workspace();
        let server = mock.addr().to_string();
        let out = output(dns_cmd(dir.path(), "lookup", &server, &["a.test", "--json"])).await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json_of(&out);
        assert_eq!(v["addresses"], serde_json::json!(["10.0.0.1", "10.0.0.2"]));
        assert_eq!(v["source"], format!("upstream(udp://{server})"));
        assert_eq!(v["ttl_secs"], 120);
        assert!(v["error"].is_null());
        assert_eq!(v["upstreams"][0], format!("udp://{server}"));
        let text = output(dns_cmd(dir.path(), "lookup", &server, &["a.test"])).await;
        let stdout = String::from_utf8_lossy(&text.stdout);
        assert!(stdout.contains("addresses: 10.0.0.1, 10.0.0.2"), "{stdout}");
        assert!(stdout.contains("ttl: 120s"), "{stdout}");
        assert_eq!(mock.query_count("a.test", Qtype::Aaaa), 0, "ipv6 = false asks A only");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_answer_exits_one() {
        let mock = MockDns::spawn().await;
        mock.set_empty("nx.test");
        let dir = workspace();
        let server = mock.addr().to_string();
        let out = output(dns_cmd(dir.path(), "lookup", &server, &["nx.test", "--json"])).await;
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(json_of(&out)["error"], "empty answer");
        let text = output(dns_cmd(dir.path(), "lookup", &server, &["nx.test"])).await;
        assert_eq!(text.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&text.stdout).contains("error: empty answer"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn host_entries_short_circuit_the_upstream() {
        let mock = MockDns::spawn().await;
        let dir = workspace();
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &mock.addr().to_string(),
            &["fixed.test", "--json"],
        ))
        .await;
        let v = json_of(&out);
        assert_eq!(v["addresses"], serde_json::json!(["1.2.3.4"]));
        assert_eq!(v["source"], "host(ip)");
        assert!(mock.queries().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_upstream_with_trace_shows_attempts() {
        let mock = MockDns::spawn().await;
        mock.set("t.test", &["10.0.0.7"], &[], 60);
        let dir = workspace();
        let server = format!("tcp://{}", mock.addr());
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &server,
            &["t.test", "--type", "a", "--trace"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains(&format!("source: upstream({server})")), "{stdout}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("rurge_dns::fanout")
                && stderr.contains("send")
                && stderr.contains("answer"),
            "{stderr}"
        );
        assert_eq!(mock.queries()[0].1, "tcp");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dot_and_doh_upstreams_via_server_override() {
        use rurge_dns::message::{Question, Rcode, build_response};
        use rurge_net::testing::TestServer;
        let dir = workspace();
        std::fs::write(
            dir.path().join("t.conf"),
            CONF.replace(
                "ipv6 = false",
                "ipv6 = false
encrypted-dns-skip-cert-verification = true",
            ),
        )
        .unwrap();
        let dot = MockDns::spawn_tls().await;
        dot.set("s.test", &["10.0.0.5"], &[], 60);
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            &format!("tls://{}", dot.addr()),
            &["s.test", "--json"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(json_of(&out)["addresses"][0], "10.0.0.5");
        let doh = TestServer::spawn_tls().await;
        let q = Question {
            name: "s.test".to_string(),
            qtype: Qtype::A,
        };
        doh.set(
            "/dns-query",
            build_response(0, &q, Rcode::NoError, &[("10.0.0.6".parse().unwrap(), 60)], false)
                .unwrap(),
        );
        doh.set_header("/dns-query", "content-type", "application/dns-message");
        let out = output(dns_cmd(
            dir.path(),
            "lookup",
            doh.url("/dns-query").as_str(),
            &["s.test", "--json"],
        ))
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let v = json_of(&out);
        assert_eq!(v["addresses"][0], "10.0.0.6");
        assert_eq!(v["source"], format!("upstream({})", doh.url("/dns-query")));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dns_cache_lists_warmed_entries() {
        let mock = MockDns::spawn().await;
        mock.set("c.test", &["10.0.0.3"], &[], 60);
        mock.set_empty("nx.test");
        let dir = workspace();
        let server = mock.addr().to_string();
        let out = output(dns_cmd(
            dir.path(),
            "cache",
            &server,
            &["c.test", "nx.test", "--json"],
        ))
        .await;
        assert_eq!(out.status.code(), Some(0));
        let v = json_of(&out);
        let entries = v["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        let c = entries.iter().find(|e| e["name"] == "c.test").unwrap();
        assert_eq!(c["v4"][0], "10.0.0.3");
        assert_eq!(c["negative"], false);
        let nx = entries.iter().find(|e| e["name"] == "nx.test").unwrap();
        assert_eq!(nx["negative"], true);
        let text = output(dns_cmd(dir.path(), "cache", &server, &["c.test"])).await;
        let stdout = String::from_utf8_lossy(&text.stdout);
        assert!(stdout.contains("entries: 1") && stdout.contains("c.test"), "{stdout}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_server_spec_exits_two() {
        let dir = workspace();
        let out = output(dns_cmd(dir.path(), "lookup", "nonsense", &["a.test"])).await;
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("invalid --server"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rule_match_resolves_through_the_profile_resolver() {
        let mock = MockDns::spawn().await;
        mock.set("ip-rule.test", &["10.1.2.3"], &[], 60);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("t.conf"),
            format!(
                "[General]\ndns-server = {}\nipv6 = false\n[Proxy]\nP = direct\n[Rule]\nIP-CIDR,10.0.0.0/8,P\nFINAL,DIRECT\n",
                mock.addr()
            ),
        )
        .unwrap();
        let mut cmd = Command::cargo_bin("rurge").unwrap();
        cmd.arg("rule")
            .arg("match")
            .arg("-c")
            .arg(dir.path().join("t.conf"))
            .arg("--no-network")
            .arg("--data-dir")
            .arg(dir.path().join("data"))
            .arg("ip-rule.test")
            .arg("--json");
        let out = output(cmd).await;
        let v = json_of(&out);
        assert_eq!(v["policy"], "P");
        assert_eq!(v["resolved"]["v4"][0], "10.1.2.3");
        assert_eq!(mock.query_count("ip-rule.test", Qtype::A), 1);
    }
}
```

> 这些测试用 `#[tokio::test(flavor = "multi_thread")]` 让模拟上游在子进程运行期间继续服务；`tokio` 是 bin 的普通依赖（含 `macros` / `rt-multi-thread`）。`tcp://127.0.0.1:<port>` / `tls://127.0.0.1:<port>` 的主机是 IP 字面量，`Bootstrap::resolve` 直接返回（Task 9）。DoH 用例给 `TestServer` 一个静态应答体：`DohUpstream` 发送时把报文 ID 置 0、收到后恢复（Task 7），所以 ID 为 0 的固定应答对任意查询都匹配；`ipv6 = false` 保证只问 A。

- [ ] **Step 7: 运行**

```bash
cargo test -p rurge
```

预期：原有 CLI 测试全部通过（`rule match` 已改走 `Resolver`，`--no-dns` / `--resolve` 路径不变）、`dns` 模块 8 个测试与 `dns.rs` 的 1 个单元测试通过。

- [ ] **Step 8: 质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
git add Cargo.toml Cargo.lock crates/rurge
git commit -F - <<'EOF'
feat(cli): rurge dns lookup / dns cache；rule match 改用 rurge-dns Resolver

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

### Task 14: 端到端集成测试、基准、文档与兼容性清单

**Files:**
- Create: `crates/rurge-dns/tests/resolver.rs`
- Modify: `crates/rurge-dns/benches/dns.rs`（替换 Task 1 的占位）
- Modify: `README.md`、`CLAUDE.md`、`docs/surge-compatibility-matrix.md`、`docs/superpowers/specs/2026-09-04-phase1-m2-rules-dns-design.md`、`docs/superpowers/plans/2026-09-04-phase1-m2b-dns-plan.md`

**Interfaces:**
- Consumes: `rurge_dns::{Resolver, ResolverConfig, ResolverDeps, LookupOpts, Source, HostKind}`、`rurge_dns::message::{Qtype, Question, Rcode, build_response}`、`rurge_dns::cache::{DnsCache, CachedAddrs}`、`rurge_dns::system::StaticSystemDns`、`rurge_dns::testing::MockDns`（`spawn` / `spawn_tls` / `set` / `set_drop_first` / `set_delay` / `set_drop_qtype` / `queries` / `query_count`）、`rurge_net::testing::TestServer`（`spawn_tls` / `set` / `set_header` / `url` / `hits`）、`rurge_config::config::{LoadOptions, from_text}`、`rurge_rules::SetRegistry::build`、`rurge_net::resource::{ResourceManager, ResourceOptions}`。
- Produces: 验收产出（设计 §13 集成（M2b）、§16 第 6 ～ 9 条）；文档同步。

- [ ] **Step 1: 集成测试 `tests/resolver.rs`**

```rust
//! End to end: profile text → `Resolver` → in-process upstreams over every
//! transport (UDP, tcp://, tls://, https://), the manual's retry and
//! partial-result timing with the default 1 s / 5 attempts, `[Host]`
//! `server:` URLs bootstrapped by traditional upstreams, and set keys.

use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::message::{Qtype, Question, Rcode, build_response};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_dns::{HostKind, LookupOpts, Resolver, ResolverConfig, ResolverDeps, Source};
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_net::testing::TestServer;
use rurge_rules::SetRegistry;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn profile(general: &str, hosts: &str) -> String {
    format!("[General]\n{general}\n[Proxy]\n[Host]\n{hosts}\n[Rule]\nFINAL,DIRECT\n")
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// Builds a resolver whose profile lives in `dir` (local set files go there too).
fn resolver_in(dir: &Path, profile_text: &str, system: StaticSystemDns) -> Arc<Resolver> {
    let loaded = from_text(profile_text, &dir.join("t.conf"), &LoadOptions::for_tests());
    assert!(
        !loaded.diagnostics.has_errors(),
        "{:?}",
        loaded.diagnostics.iter().map(|d| d.code).collect::<Vec<_>>()
    );
    let cfg = loaded.config;
    let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
    let client = Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default()).unwrap());
    let resources = ResourceManager::with_options(
        dir.to_path_buf(),
        client,
        ResourceOptions {
            offline: true,
            ..ResourceOptions::default()
        },
    );
    let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir);
    let (resolver, diags) = Resolver::new(
        ResolverConfig::from_config(&cfg),
        ResolverDeps {
            connector,
            sets,
            system: Arc::new(system),
            resources,
        },
    );
    assert!(diags.is_empty(), "{:?}", diags.iter().map(|d| d.code).collect::<Vec<_>>());
    resolver
}

fn resolver(profile_text: &str, system: StaticSystemDns) -> (tempfile::TempDir, Arc<Resolver>) {
    let dir = tempfile::tempdir().unwrap();
    let r = resolver_in(dir.path(), profile_text, system);
    (dir, r)
}

async fn lookup(r: &Resolver, name: &str) -> rurge_dns::DnsResult {
    r.lookup(name, LookupOpts::default()).await.unwrap()
}

#[tokio::test]
async fn every_transport_resolves() {
    // UDP
    let udp = MockDns::spawn().await;
    udp.set("a.test", &["10.0.0.1"], &[], 60);
    let (_d, r) = resolver(
        &profile(&format!("dns-server = {}\nipv6 = false", udp.addr()), ""),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.1")]);
    assert_eq!(a.source, Source::Upstream(format!("udp://{}", udp.addr())));

    // tcp://
    let tcp = MockDns::spawn().await;
    tcp.set("a.test", &["10.0.0.2"], &[], 60);
    let (_d, r) = resolver(
        &profile(&format!("encrypted-dns-server = tcp://{}\nipv6 = false", tcp.addr()), ""),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.2")]);
    assert_eq!(a.source, Source::Upstream(format!("tcp://{}", tcp.addr())));
    assert_eq!(tcp.queries()[0].1, "tcp");

    // tls://
    let dot = MockDns::spawn_tls().await;
    dot.set("a.test", &["10.0.0.3"], &[], 60);
    let (_d, r) = resolver(
        &profile(
            &format!(
                "encrypted-dns-server = tls://{}\nencrypted-dns-skip-cert-verification = true\nipv6 = false",
                dot.addr()
            ),
            "",
        ),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.3")]);
    assert_eq!(a.source, Source::Upstream(format!("tls://{}", dot.addr())));

    // https:// (static body: DoH sends ID 0 and restores the caller's ID)
    let doh = TestServer::spawn_tls().await;
    let q = Question {
        name: "a.test".to_string(),
        qtype: Qtype::A,
    };
    doh.set(
        "/dns-query",
        build_response(0, &q, Rcode::NoError, &[(ip("10.0.0.4"), 60)], false).unwrap(),
    );
    doh.set_header("/dns-query", "content-type", "application/dns-message");
    let url = doh.url("/dns-query");
    let (_d, r) = resolver(
        &profile(
            &format!(
                "encrypted-dns-server = {url}\nencrypted-dns-skip-cert-verification = true\nipv6 = false"
            ),
            "",
        ),
        StaticSystemDns::default(),
    );
    let a = lookup(&r, "a.test").await;
    assert_eq!(a.addrs(), vec![ip("10.0.0.4")]);
    assert_eq!(a.source, Source::Upstream(url.to_string()));
    assert_eq!(doh.hits("/dns-query"), 1);
}

#[tokio::test]
async fn host_server_url_is_bootstrapped_by_traditional_upstreams() {
    let udp = MockDns::spawn().await;
    udp.set("dns.corp.test", &["127.0.0.1"], &[], 60);
    udp.set("corp.test", &["10.0.0.99"], &[], 60);
    let corp = MockDns::spawn().await;
    corp.set("corp.test", &["10.10.0.1"], &[], 60);
    let (_d, r) = resolver(
        &profile(
            &format!("dns-server = {}\nipv6 = false", udp.addr()),
            &format!("corp.test = server:tcp://dns.corp.test:{}", corp.addr().port()),
        ),
        StaticSystemDns::default(),
    );
    let c = lookup(&r, "corp.test").await;
    assert_eq!(c.addrs(), vec![ip("10.10.0.1")]);
    assert_eq!(c.source, Source::Host(HostKind::Server));
    assert_eq!(udp.query_count("dns.corp.test", Qtype::A), 1, "bootstrap went to the UDP upstream");
    assert_eq!(udp.query_count("corp.test", Qtype::A), 0, "the name itself never went to the UDP upstream");
    assert_eq!(corp.queries()[0].1, "tcp");
}

#[tokio::test]
async fn resend_after_one_second_and_first_answer_wins() {
    let a = MockDns::spawn().await;
    a.set("r.test", &["10.0.0.1"], &[], 60);
    a.set_drop_first(1);
    let b = MockDns::spawn().await;
    b.set("r.test", &["10.0.0.2"], &[], 60);
    b.set_delay(Duration::from_millis(1600));
    let (_d, r) = resolver(
        &profile(&format!("dns-server = {}, {}\nipv6 = false", a.addr(), b.addr()), ""),
        StaticSystemDns::default(),
    );
    let started = Instant::now();
    let res = lookup(&r, "r.test").await;
    let elapsed = started.elapsed();
    assert_eq!(res.addrs(), vec![ip("10.0.0.1")], "a's resent query wins before b's slow answer");
    assert_eq!(res.source, Source::Upstream(format!("udp://{}", a.addr())));
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(1500),
        "{elapsed:?}"
    );
    assert_eq!(a.query_count("r.test", Qtype::A), 2, "one resend after the 1 s timer");
    assert!(b.query_count("r.test", Qtype::A) >= 1);
}

#[tokio::test]
async fn partial_result_when_aaaa_lags() {
    let m = MockDns::spawn().await;
    m.set("p.test", &["10.0.0.1"], &["fd00::1"], 60);
    m.set_drop_qtype(Qtype::Aaaa, true);
    let (_d, r) = resolver(
        &profile(&format!("dns-server = {}\nipv6 = true", m.addr()), ""),
        StaticSystemDns {
            has_ipv6: true,
            ..StaticSystemDns::default()
        },
    );
    let started = Instant::now();
    let res = lookup(&r, "p.test").await;
    let elapsed = started.elapsed();
    assert_eq!(res.v4, vec!["10.0.0.1".parse::<std::net::Ipv4Addr>().unwrap()]);
    assert!(res.v6.is_empty());
    assert!(
        elapsed >= Duration::from_millis(900) && elapsed < Duration::from_millis(1500),
        "partial result at the first resend tick: {elapsed:?}"
    );
    m.set_drop_qtype(Qtype::Aaaa, false);
    let full = r
        .lookup(
            "p.test",
            LookupOpts {
                bypass_cache: true,
                want_v6: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(full.v6.len(), 1);
}

#[tokio::test]
async fn domain_set_and_rule_set_host_keys() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("d.txt"), "a.set.test\n.suffix.test\n").unwrap();
    std::fs::write(dir.path().join("r.list"), "DOMAIN,r.set.test\n").unwrap();
    let m = MockDns::spawn().await;
    m.set("other.test", &["10.0.0.9"], &[], 60);
    let r = resolver_in(
        dir.path(),
        &profile(
            &format!("dns-server = {}\nipv6 = false", m.addr()),
            "DOMAIN-SET:d.txt = 10.5.0.1\nRULE-SET:r.list = 10.5.0.2\n",
        ),
        StaticSystemDns::default(),
    );
    // local set files load asynchronously; give the registry a moment
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let a = lookup(&r, "a.set.test").await;
        if a.source == Source::Host(HostKind::Ip) {
            assert_eq!(a.addrs(), vec![ip("10.5.0.1")]);
            break;
        }
        assert!(Instant::now() < deadline, "DOMAIN-SET host key never matched: {a:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(lookup(&r, "x.suffix.test").await.addrs(), vec![ip("10.5.0.1")]);
    assert_eq!(lookup(&r, "r.set.test").await.addrs(), vec![ip("10.5.0.2")]);
    let other = lookup(&r, "other.test").await;
    assert_eq!(other.addrs(), vec![ip("10.0.0.9")]);
    assert!(matches!(other.source, Source::Upstream(_)));
}
```

> `MockDns` 在 `rurge-dns` 的集成测试里可用，因为 Task 1 的 `Cargo.toml` 通过自身 dev-dependency 打开了 `testing` feature。`DOMAIN-SET` 文件的 `.suffix.test` 行表示后缀匹配（M2a `set_format`）。两条依赖真实 1 s 定时器的用例合计约 2.5 s。

- [ ] **Step 2: 运行**

```bash
cargo test -p rurge-dns --test resolver
```

预期：5 个测试通过。

- [ ] **Step 3: 基准 `benches/dns.rs`**

替换占位内容：

```rust
//! NFR-01: a DNS cache hit completes in under 1 ms. Two numbers: the bare
//! cache lookup and a full `Resolver::lookup` that hits the cache.

use criterion::{Criterion, criterion_group, criterion_main};
use rurge_config::config::{LoadOptions, from_text};
use rurge_dns::cache::{CachedAddrs, DnsCache};
use rurge_dns::system::StaticSystemDns;
use rurge_dns::testing::MockDns;
use rurge_dns::{LookupOpts, Resolver, ResolverConfig, ResolverDeps};
use rurge_net::connector::{DirectConnector, SystemResolve};
use rurge_net::http::{HttpClient, HttpClientConfig};
use rurge_net::resource::{ResourceManager, ResourceOptions};
use rurge_rules::SetRegistry;
use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

fn cache_get(c: &mut Criterion) {
    let cache = DnsCache::new(2000);
    let names: Vec<String> = (0..2000u32).map(|i| format!("host{i}.example")).collect();
    for (i, name) in names.iter().enumerate() {
        cache.put(
            name,
            CachedAddrs {
                v4: vec![Ipv4Addr::new(10, 0, (i / 256) as u8, (i % 256) as u8)],
                v6: Vec::new(),
                ttl: Duration::from_secs(3600),
                source: "bench".to_string(),
            },
        );
    }
    let mut i = 0usize;
    c.bench_function("cache_get", |b| {
        b.iter(|| {
            i = (i + 1) % names.len();
            std::hint::black_box(cache.get(&names[i]))
        })
    });
}

fn resolver_cache_hit(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (resolver, _mock) = rt.block_on(async {
        let mock = MockDns::spawn().await;
        mock.set("bench.test", &["10.0.0.1"], &[], 3600);
        let text = format!(
            "[General]\ndns-server = {}\nipv6 = false\n[Proxy]\n[Rule]\nFINAL,DIRECT\n",
            mock.addr()
        );
        let cfg = from_text(&text, &dir.path().join("t.conf"), &LoadOptions::for_tests()).config;
        let connector = Arc::new(DirectConnector::new(Arc::new(SystemResolve)));
        let client = Arc::new(HttpClient::new(connector.clone(), HttpClientConfig::default()).unwrap());
        let resources = ResourceManager::with_options(
            dir.path().to_path_buf(),
            client,
            ResourceOptions {
                offline: true,
                ..ResourceOptions::default()
            },
        );
        let (sets, _) = SetRegistry::build(&cfg, resources.clone(), dir.path());
        let (resolver, _) = Resolver::new(
            ResolverConfig::from_config(&cfg),
            ResolverDeps {
                connector,
                sets,
                system: Arc::new(StaticSystemDns::default()),
                resources,
            },
        );
        resolver.lookup("bench.test", LookupOpts::default()).await.unwrap();
        (resolver, mock)
    });
    c.bench_function("resolver_cache_hit", |b| {
        b.iter(|| {
            rt.block_on(resolver.lookup("bench.test", LookupOpts::default()))
                .unwrap()
        })
    });
}

criterion_group!(benches, cache_get, resolver_cache_hit);
criterion_main!(benches);
```

运行 `cargo bench -p rurge-dns`，把两个数字记入本计划末尾的修正记录表（目标：`resolver_cache_hit` 中位数 < 1 ms；预期几微秒到几十微秒）。

- [ ] **Step 4: README**

`README.md` 中文部分：
- 第 18 行状态引言改为：`> **阶段 1 进行中：M1、M2a、M2b 完成**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、` `` `rurge rule match` `` `、` `` `rurge dns lookup` `` `）；M3、M4 未开始。` `` `rurge check` `` ` 可以校验任意 Surge 配置并给出带行号的诊断；` `` `rurge rule match` `` ` 可以离线测试一次会话会命中哪条规则；` `` `rurge dns lookup` `` ` 可以按配置的 DNS 设置离线解析域名；代理功能尚未实现。`
- 第 30 行 DNS 行的第二列末尾追加：`（普通 DNS / DoH / DoT / ` `` `tcp://` `` ` / ` `` `[Host]` `` ` / 系统 hosts 已实现，M2b）`。
- 第 56 行改为：`> ` `` `rurge check` `` `、` `` `rurge rule match` `` `（离线规则测试）与 ` `` `rurge dns lookup` `` `（离线 DNS 测试）已可用；` `` `rurge run` `` ` 将在 M3 提供。`
- 第 66 行之后追加一行示例：`rurge dns lookup -c surge.conf www.example.com --trace`。

英文部分对应第 140、151、177、187 行做同样修改（"M1, M2a and M2b are done (…, DNS client, `rurge rule match`, `rurge dns lookup`); M3 and M4 have not started."；"(plain DNS / DoH / DoT / `tcp://` / `[Host]` / system hosts implemented, M2b)"；"`rurge check`, `rurge rule match` (offline rule testing) and `rurge dns lookup` (offline DNS testing) work today; `rurge run` arrives with milestone M3."；示例行同上）。

- [ ] **Step 5: CLAUDE.md**

- 「当前状态」段改为：`阶段 1 进行中。M1、M2a、M2b 已完成：Cargo workspace、` `` `rurge-config` `` `（解析全部 Surge 语法为强类型 ` `` `Config` `` ` + 诊断）、` `` `rurge check` `` `、` `` `rurge-net` `` `（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、` `` `rurge-rules` `` `（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、` `` `rurge rule match` `` `（离线规则匹配开发命令）、` `` `rurge-dns` `` `（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、` `` `[Host]` `` ` 链、系统 hosts）、` `` `rurge-platform::dns` `` `、` `` `rurge dns lookup` `` `。M3（连接流水线）、M4（控制面与平台）未开始，` `` `rurge run` `` ` 尚不存在。`
- 「先读这些文档」在 M2a 计划条目后追加：`- ` `` `docs/superpowers/plans/2026-09-04-phase1-m2b-dns-plan.md` `` `：M2b 实施计划（14 个任务）。末尾「执行期修正记录」与「延后事项」同 M2a。`
- 「计划中的架构」里依赖方向一句的「（M2 设计文档确认，` `` `rurge-dns` `` ` 尚未实现）」改为「（M2 设计文档确认）」。
- 「常用命令」追加：

```bash
cargo run -p rurge -- dns lookup -c config.conf example.com --trace    # 离线 DNS 解析（--server 覆盖上游）
cargo bench -p rurge-dns                        # criterion 基准（DNS 缓存命中）
```

- [ ] **Step 6: 兼容性清单**

`docs/surge-compatibility-matrix.md`：
- 第 133 行（`encrypted-dns-server`）备注追加：「阶段 1 对 `h3` / `quic` 条目告警 W0026 并忽略」。
- 第 189 行（`read-etc-hosts`）状态改为 🟡，备注改为「手册标注 Mac only；rurge 三平台生效（Win: `System32\drivers\etc\hosts`）」；第 507 行同步改为 🟡、同一备注。
- 第 504 行（`server:force-syslib`）状态改为 🟡，备注「阶段 1 等同 `syslib`；M3 起区分」。
- 第 505 行（`script:`）备注追加「阶段 1 构建时告警 W0027 并跳过该条目」。
- 6.1 表末尾追加两行：

```
| `localhost` / `*.localhost` 直接返回回环地址，不查询上游 | | 🟡 | 1 | 手册未说明 |
| 空应答（NOERROR 无记录 / NXDOMAIN）负缓存 30 s；错误不缓存 | | 🟡 | 1 | 手册未说明 |
```

- 第 806 行（rurge 专有开发命令）之后追加一行：

```
| rurge 专有开发命令 | 离线按配置的 DNS 设置解析域名，`--server` 覆盖上游，`--trace` 打印每次尝试；`dns cache` 打印本进程缓存快照 | `rurge dns lookup -c <conf> <name> [--type a\|aaaa\|both] [--server <spec>...] [--no-cache] [--trace] [--json]`；`rurge dns cache -c <conf> [name...]` | 1 | 见 M2 设计文档 §10.2；阶段 6 的 `dns lookup` 经 HTTP API 查询守护进程 |
```

- [ ] **Step 7: 设计文档与计划的修正记录**

`docs/superpowers/specs/2026-09-04-phase1-m2-rules-dns-design.md`：
- §7.1 的 `ResolverDeps` 说明处追加一句：「实现时 DoH 的 `HttpClient` 由 `Resolver` 内部用 `BootstrapConnector` 构建（保证 URL 主机名只经传统上游解析），`ResolverDeps` 只注入 `connector` / `sets` / `system` / `resources`。」
- §10.2 命令行改为 `rurge dns cache -c <conf> [name...]`，并加注「先解析给定名字再打印快照（演示用）；`--trace` 通过 `rurge_dns::fanout` 的 `tracing` debug 事件实现」。

`docs/superpowers/plans/2026-09-04-phase1-m2b-dns-plan.md` 末尾追加两节（执行期间按实际情况填写；下面是执行前已知的条目）：

```markdown
## 执行期修正记录

| 任务 | 计划内容 | 实际处理 | 原因 |
| --- | --- | --- | --- |
| 12 | 设计 §7.1：`ResolverDeps` 注入 `HttpClient` | `Resolver` 内部用 `BootstrapConnector` 构建 DoH 客户端 | URL 型上游的主机名必须只经传统上游解析（引导豁免） |
| 13 | 设计 §10.2：`rurge dns cache -c <conf>` | 追加位置参数 `[name...]`，先解析再打印快照 | 进程刚启动时快照恒为空，命令无法演示 |
| 13 | 设计 §10.2：`--trace` 打印每次尝试 | 通过 `rurge_dns::fanout` 的 `tracing` debug 事件 + `tracing-subscriber` 实现 | 不改动 `resolve_name` 签名 |
| 14 | NFR-01 缓存命中 < 1 ms | `cache_get` / `resolver_cache_hit` 基准数字：（执行时填写） | |

## 延后事项

- `h3://` / `quic://` 上游（阶段 2，依赖 QUIC 栈）；`encrypted-dns-follow-outbound-mode`（M3，DNS 连接走规则）。
- `server:force-syslib` 与 `syslib` 的区分、`[SSID Setting]` 的 DNS 覆盖（M3）。
- `[Host]` 的 `script:` 值（阶段 5）。
- `dns cache` 查询运行中的守护进程、`GET /v1/dns` / `POST /v1/dns/flush` / 延迟测试端点（M4）。
- 系统 DNS 变化（网络切换）的自动 `on_network_change` 触发（M4 平台事件）。
```

- [ ] **Step 8: 全量质量门并提交**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace
cargo bench -p rurge-dns
git add crates/rurge-dns README.md CLAUDE.md docs
git commit -F - <<'EOF'
test(dns): 端到端集成测试与缓存命中基准；文档与兼容性清单同步 M2b

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01KrX7AhqrqqoQUDY3t5QPcW
EOF
```

---

## 执行期修正记录

| 任务 | 计划内容 | 实际处理 | 原因 |
| --- | --- | --- | --- |
| 1 | 全局约束 `rust-version = "1.85"`，不用 let-chains | workspace `rust-version` 提升到 1.88；clippy 随之要求把 11 处嵌套 `if let` 合并为 let-chains（c25907b） | hickory-proto 0.26 声明 MSRV 1.88，且编解码代码只对 0.26 的公开字段 API 核对过 |
| 2 | Windows `search_domains()` 样例代码吞掉错误 | 两处 ipconfig 错误路径补 `tracing::debug!` | 接口约定「出错返回空值并 debug 日志」 |
| 4 | `MockDns` 只有 TCP 监听随 oneshot 退出 | `Drop` 中 abort UDP / TCP 两个监听任务；recv/accept 错误记录并继续 | UDP 监听不随丢弃停止；Windows 上回复已关闭端口后 `recv_from` 会报 WSAECONNRESET |
| 5 | 设计 §7.2「TCP / DoT 按 ID 多路复用」 | 每个 `TcpUpstream` 一条串行化持久连接，`exchange_framed` 校验应答 ID；锁等待与建连纳入 `timeout_at(deadline)`；DoT 派生 TLS 配置并只提供 ALPN `dot` | 阶段 1 不做多路复用；共享 HTTP 客户端的 ALPN（h2 / http/1.1）会被严格的 DoT 服务器拒绝 |
| 7 | DoH 所有 `HttpError` 映射为 `Http` | `HttpError::Timeout` → `Timeout`；`LengthLimitError` → `BadResponse` | 接口约定 |
| 8 | 单次查询超时计入 `failures` | 超时视为「无应答」，不计入失败；发送门槛加 `now < deadline`；补多线程运行时用例 | 共享截止时间下多线程运行时会随机把 `EmptyAnswer` / `Timeout` 变成 `AllFailed` |
| 9 / 10 | 计划顺序 Task 9 → Task 10 | Task 10（缓存）先于 Task 9 实现（b05402e） | `Bootstrap` 依赖 `DnsCache`，预检漏掉 T9 ↔ T10 |
| 9 | 引导缓存过期后重新查询 | 过期先返回旧地址，经 `begin_refresh` / `end_refresh` 后台刷新一次（Weak 自引用） | 60 s 后每次拨号都要付一次完整 fanout |
| 9 | 引导测试真实等待 61 s | 用 `tokio::time::pause` / `advance` / `resume` | 全量测试时长 |
| 11 | `HostMap::lookup` 要求调用方归一化 | `lookup` 内部归一化（小写、去尾点，`Cow`） | 大写 / 尾点查询会漏掉 hosts 条目与字面量模式 |
| 11 | `force-syslib` 等同 `syslib` | 保留解析出的 `SystemMode`，解析器对三种模式一视同仁（M3 再区分） | M3 需要该区分 |
| 12 | 设计 §7.1：`ResolverDeps` 注入 `HttpClient` | `Resolver` 内部用 `BootstrapConnector` 构建 DoH 客户端（不可用时为 `None` 并告警 W0026） | 引导豁免必须覆盖 DoH URL |
| 12 | `on_network_change` 从 `primary_specs` 推导引导上游 | 保留 `configured_udp` + `wants_system`，`apply_system_servers` 由 `new` 与 `on_network_change` 共用；`dns-server = system` 在网络变化后重新展开；新增 `bootstrap_upstreams()` | 配置了加密上游时引导集合会错降级为系统 DNS；`system` 关键字在构造时被快照 |
| 12 | 缓存只按名字为键 | `CachedAddrs.v6_queried`：`want_v6` 的查询不命中未查过 AAAA 的条目 | v4-only 条目会被当作 v6 调用者的命中 |
| 12 | 在途查询无取消保护 | `Inflight` 守卫：首个调用者被取消时移除未发布的在途项 | 后续等待者会永久挂起 |
| 12 | hosts 文件先读后订阅；DoH 客户端构建失败 `expect` | 先 `subscribe` 再读；`http: Option<Arc<HttpClient>>`；`system_lookup` 透传 `bypass_cache` | 变更丢失窗口；守护进程启动 panic |
| 13 | 设计 §10.2：`rurge dns cache -c <conf>` | 追加位置参数 `[name...]`，先解析再打印快照 | 进程刚启动时快照恒为空 |
| 13 | 设计 §10.2：`--trace` 打印每次尝试 | `rurge_dns::fanout` 的 `tracing` debug 事件 + `tracing-subscriber` | 不改 `resolve_name` 签名 |
| 14 | NFR-01 缓存命中 < 1 ms | `cache_get` 中位数 229.24 ns；`resolver_cache_hit` 中位数 778.17 ns（`cargo bench -p rurge-dns --bench dns -- --warm-up-time 1 --measurement-time 3`） | 均远低于 1 ms 目标 |
| 14 | `cargo bench -p rurge-dns -- --warm-up-time 1 --measurement-time 3` | 改用 `cargo bench -p rurge-dns --bench dns -- --warm-up-time 1 --measurement-time 3` | 与 M2a Task 16 同一根因：`-p rurge-dns` 还会跑 lib 单元测试的默认 harness，不认识 criterion 的 CLI 参数 |

## 延后事项

- `h3://` / `quic://` 上游（阶段 2，依赖 QUIC 栈）；`encrypted-dns-follow-outbound-mode`（M3，DNS 连接走规则）。
- `server:force-syslib` 与 `syslib` 的区分、`[SSID Setting]` 的 DNS 覆盖（M3）。
- `[Host]` 的 `script:` 值（阶段 5）。
- `dns cache` 查询运行中的守护进程、`GET /v1/dns` / `POST /v1/dns/flush` / 延迟测试端点（M4）。
- 系统 DNS 变化（网络切换）的自动 `on_network_change` 触发（M4 平台事件）。
- `UdpUpstream`：被外部取消的查询会在 `pending` 中留下等待者直到迟到应答或 ID 复用（fanout 的 `abort_all` 会常规触发）；应加带令牌的 RAII 移除。
- `TcpUpstream`：同一连接上的 ID 多路复用（设计 §7.2）。
- `Bootstrap`：`want_v6` 在构造时固定，IPv6 上线后引导查询仍只问 A；冷未命中无 singleflight；刷新任务 panic 会滞留刷新槽。
- 负缓存条目不携带家族信息（30 s 内 `want_v6 = false` 的空应答会返回给 `want_v6 = true` 的调用者）。
- `ipv6 = false` 只过滤配置中的 IPv6 服务器，不过滤系统展开出来的服务器（待登记）。
- `fanout`：`empties` 按上游名去重（重复配置同一服务器时只能在截止时判空）；问题段不匹配的应答报为 `rcode NOERROR`；`AllFailed` 排序未测；JoinError 静默丢弃。
- `message.rs` 测试 `response_round_trip_with_records_and_ttls` 有一条恒真断言；`udp.rs` 错误文案 "receiver dropped" 应为 "sender dropped"、一处过期注释；`hosts.rs` 非 Windows `read()` 会解析两次 resolv.conf；`cache.rs` 容量 0 静默夹到 1。
- `resolver.rs`（约 1500 行）可拆出 `resolver/types.rs`。
