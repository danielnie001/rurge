# 阶段 1 / M1「workspace 骨架与配置解析器」实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建立 Cargo workspace，并实现 `rurge-config` crate：把任意 Surge `.conf` 解析为强类型 `Config` 与分级诊断，配套 `rurge check` 命令与语料库快照测试。

**Architecture:** 两层模型：文本层 `Profile`（忠实保留节、行、来源、Requirement）→ 语义层 `Config`（强类型；`[General]` 逐键、策略 / 组 / 规则 / Host / Keystore 完整解析，其余节延迟保存）。解析永不 panic，问题以 `Diagnostic { severity, code, span }` 报告；错误阻止生效，警告不阻止。二进制 crate `rurge` 本里程碑只提供 `version` 与 `check`。

**Tech Stack:** Rust stable / edition 2024、thiserror、fancy-regex、ipnet、serde + serde_json、clap（derive）、insta、tempfile、assert_cmd + predicates。

**Spec:** `docs/superpowers/specs/2026-09-03-phase1-core-skeleton-design.md`（第 1 ～ 5 节、第 13 ～ 16 节）；需求编号 FR-CFG-01 ～ 09、12、14（本地部分）、17、19；兼容性清单第 1 ～ 5 节。

## Global Constraints

- 工具链：`rust-toolchain.toml` 固定 `channel = "stable"`；workspace `edition = "2024"`，`rust-version = "1.85"`。
- 质量门：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test` 三者在每个任务结束时必须通过；workspace lints `unsafe_code = "forbid"`。
- crate 名称固定：`rurge-config`（库）、`rurge`（二进制）；路径 `crates/rurge-config`、`crates/rurge`。
- 诊断分级按设计文档 4.3：错误 = 拒绝生效；警告 / 信息 = 继续。诊断代码一经定义不得改号。
- 不改写用户配置文件；旧键迁移只在内存中进行（FR-CFG-02）。
- 未识别的键 / 节 / 规则参数：警告并保留（FR-CFG-02）。
- Surge 语法即 rurge 语法，不引入 Surge 没有的配置项（FR-CFG-17）。
- 语言：文档与提交信息中文；代码标识符、注释、日志、CLI 输出英文。
- 提交：每个任务一次提交，在分支 `m1-config-parser` 上进行，不推送、不合并；项目所有者审阅后合并。若所有者要求"先审阅再提交"，把每个任务的 `git commit` 步骤改为 `git add` 暂存。
- 手册基线：Surge 官方手册 2026-09 版；实现语义有疑问时以 `docs/surge-compatibility-matrix.md` 与手册为准，不凭记忆。

## 文件结构

```
Cargo.toml                                   workspace 根：成员、共享依赖版本、lints
rust-toolchain.toml
.github/workflows/ci.yml                     三平台 fmt / clippy / test
crates/rurge-config/
  Cargo.toml
  src/lib.rs                                 模块声明与再导出
  src/span.rs                                Span（文件 + 行号）
  src/diagnostic.rs                          Severity / Diagnostic / Diagnostics / codes
  src/text/mod.rs                            文本层：Profile / Section / Entry / parse_str
  src/text/include.rs                        #!include 展开（本地）
  src/value.rs                               split_list / unquote / ParamMap / parse_bool
  src/requirement.rs                         Requirement 表达式解析与求值、行级指令
  src/hostlist.rs                            Glob 与 Host List
  src/general.rs                             [General] 强类型解析与旧键迁移
  src/policy.rs                              [Proxy] / [Proxy Group] 解析、SubnetExpr
  src/rule.rs                                [Rule] 解析、子规则、PortExpr、ResourceRef
  src/host.rs                                [Host] 解析
  src/keystore.rs                            [Keystore] 解析
  src/deferred.rs                            阶段 1 无行为的节
  src/managed.rs                             #!MANAGED-CONFIG
  src/config.rs                              Config / LoadOptions / Platform / Capabilities / 交叉校验 / load
  tests/corpus.rs                            语料库快照测试
crates/rurge/
  Cargo.toml
  src/main.rs                                clap 入口
  src/capabilities.rs                        本版本已实现能力（供 check 告警）
  src/cli/check.rs                           check 子命令
  tests/cli.rs                               CLI 端到端测试
tests/corpus/README.md                       语料来源与许可
tests/corpus/valid/*.conf                    应通过的配置（含 include 子目录）
tests/corpus/invalid/*.conf + *.expect       应产生指定错误码的配置
```

依赖方向：`rurge` → `rurge-config`；`rurge-config` 不依赖内部 crate。模块内依赖：`config` → 其余全部；`text` → `requirement`（行级指令）、`value`；`general` / `policy` / `rule` / `host` / `keystore` → `value`、`hostlist`、`diagnostic`、`span`。

---

### Task 1: Workspace 骨架与 CI

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `crates/rurge-config/Cargo.toml`
- Create: `crates/rurge-config/src/lib.rs`
- Create: `crates/rurge/Cargo.toml`
- Create: `crates/rurge/src/main.rs`
- Create: `crates/rurge/tests/cli.rs`
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: 二进制 `rurge` 的 `version` 子命令，输出 `rurge <版本>`；后续任务在 `crates/rurge/src/main.rs` 的 `Command` 枚举上追加子命令。

- [ ] **Step 1: 写 workspace 与 crate 清单**

`Cargo.toml`：

```toml
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.85"
license = "MIT"
repository = "https://github.com/danielnie001/rurge"

[workspace.dependencies]
rurge-config = { path = "crates/rurge-config" }
thiserror = "2"
anyhow = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
clap = { version = "4", features = ["derive"] }
fancy-regex = "0.14"
ipnet = "2"
tempfile = "3"
insta = { version = "1", features = ["yaml"] }
assert_cmd = "2"
predicates = "3"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
```

`rust-toolchain.toml`：

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

`crates/rurge-config/Cargo.toml`：

```toml
[package]
name = "rurge-config"
description = "Surge-compatible profile parser for rurge"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
thiserror.workspace = true
serde.workspace = true
fancy-regex.workspace = true
ipnet.workspace = true

[dev-dependencies]
tempfile.workspace = true
insta.workspace = true

[lints]
workspace = true
```

`crates/rurge-config/src/lib.rs`：

```rust
//! Surge-compatible profile parser.
//!
//! Two layers: the *text layer* (`text::Profile`) keeps every section and line
//! with its origin; the *semantic layer* (`config::Config`) is the typed view
//! consumed by the engine. Parsing never panics; problems are reported as
//! `diagnostic::Diagnostic` values.

pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
```

`crates/rurge/Cargo.toml`：

```toml
[package]
name = "rurge"
description = "Cross-platform Surge-compatible network proxy"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "rurge"
path = "src/main.rs"

[dependencies]
rurge-config.workspace = true
anyhow.workspace = true
clap.workspace = true
serde_json.workspace = true

[dev-dependencies]
assert_cmd.workspace = true
predicates.workspace = true
tempfile.workspace = true

[lints]
workspace = true
```

`crates/rurge/src/main.rs`：

```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rurge", version, about = "Cross-platform Surge-compatible network proxy")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print version information
    Version,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version => {
            println!("rurge {} (config {})", env!("CARGO_PKG_VERSION"), rurge_config::CRATE_VERSION);
        }
    }
    Ok(())
}
```

- [ ] **Step 2: 写 CLI 冒烟测试**

`crates/rurge/tests/cli.rs`：

```rust
use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_prints_crate_version() {
    Command::cargo_bin("rurge")
        .unwrap()
        .arg("version")
        .assert()
        .success()
        .stdout(predicate::str::starts_with("rurge 0.1.0"));
}
```

- [ ] **Step 3: 运行，确认通过**

Run: `cargo test -p rurge`
Expected: `version_prints_crate_version ... ok`

- [ ] **Step 4: 写 CI**

`.github/workflows/ci.yml`：

```yaml
name: CI
on:
  push:
    branches: [main]
  pull_request:
jobs:
  check:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, windows-latest, macos-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 5: 本地跑质量门**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
Expected: 全部通过，无警告。

- [ ] **Step 6: 提交**

```bash
git checkout -b m1-config-parser
git add Cargo.toml Cargo.lock rust-toolchain.toml crates .github
git commit -m "chore: 建立 Cargo workspace、rurge-config 与 rurge crate 骨架及三平台 CI"
```

---

### Task 2: Span 与 Diagnostic

**Files:**
- Create: `crates/rurge-config/src/span.rs`
- Create: `crates/rurge-config/src/diagnostic.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Produces:
  - `Span { file: Arc<Path>, line: u32 }`，`Span::new(file, line)`，`Display` 为 `path:line`，`Serialize` 为 `{"file":"...","line":n}`。
  - `Severity { Info, Warning, Error }`（`Ord`：Info < Warning < Error）。
  - `Diagnostic { severity, code: &'static str, message, span: Option<Span>, hint: Option<String> }`，构造器 `Diagnostic::error/warning/info(code, message)`，链式 `.at(span)`、`.with_hint(text)`。
  - `Diagnostics`：`push`、`extend`、`has_errors()`、`iter()`、`len()`、`is_empty()`、`into_vec()`、`sorted()`（按文件、行、严重度降序）。
  - `codes::*` 常量（见下表），后续任务只用常量名。

| 常量 | 代码 | 含义 |
| --- | --- | --- |
| `E_SYNTAX` | E0001 | 行无法解析 |
| `E_INCLUDE_NOT_FOUND` | E0002 | include 目标不存在或不可读 |
| `E_REQUIREMENT_SYNTAX` | E0003 | Requirement 表达式语法错误 |
| `E_UNKNOWN_POLICY_TYPE` | E0004 | `[Proxy]` 未知协议关键字 |
| `E_BUILTIN_REDEFINED` | E0005 | 重定义内置策略名（DIRECT 除外） |
| `E_DUPLICATE_NAME` | E0006 | 策略 / 组重名 |
| `E_UNKNOWN_POLICY_REF` | E0007 | 规则引用不存在的策略 |
| `E_UNKNOWN_GROUP_MEMBER` | E0008 | 组成员不存在 |
| `E_GROUP_CYCLE` | E0009 | 组循环引用 |
| `E_MISSING_FINAL` | E0010 | 缺少启用的 FINAL |
| `E_INVALID_RULE_VALUE` | E0011 | 规则值非法（CIDR / 端口 / 正则 / ASN 等） |
| `E_UNKNOWN_RULE_TYPE` | E0012 | 未知规则类型 |
| `E_LISTENER_NOT_IP` | E0013 | 监听地址不是 IP 字面量 |
| `E_NOT_ALLOWED_HERE` | E0014 | FINAL / pre-matching 出现在子规则或规则集内 |
| `E_NESTING_TOO_DEEP` | E0015 | 逻辑规则嵌套超过 10 层 |
| `E_INCLUDE_CYCLE` | E0016 | include 循环 |
| `E_INVALID_DEFINITION` | E0017 | `Name = ...` 行缺少 `=` 或名字为空 |
| `W_UNKNOWN_KEY` | W0001 | 未知键 |
| `W_UNKNOWN_SECTION` | W0002 | 未知节 |
| `W_UNKNOWN_RULE_PARAM` | W0003 | 未知规则参数 |
| `W_PLATFORM_IGNORED` | W0004 | 平台不适用的键被忽略 |
| `W_INVALID_HOST_LIST_ENTRY` | W0005 | Host List 非法项 |
| `W_VANISHED_KEY` | W0006 | 已从手册消失的旧键 |
| `W_PROTOCOL_NOT_IMPLEMENTED` | W0007 | 本版本未实现的协议 |
| `W_GROUP_NOT_IMPLEMENTED` | W0008 | 本版本未实现的组类型（用第一个成员） |
| `W_IOS_BUILTIN_AS_DIRECT` | W0009 | iOS 专属内置策略视为 DIRECT |
| `W_DEVICE_POLICY_AS_REJECT` | W0010 | `DEVICE:` 策略视为 REJECT |
| `W_REMOTE_INCLUDE_UNSUPPORTED` | W0011 | 远程 include 尚未支持 |
| `W_INVALID_VALUE` | W0012 | 键值非法，已用默认值 |
| `W_RULE_NEVER_MATCHES` | W0013 | 规则在本平台 / 本版本永不匹配 |
| `W_UNKNOWN_DIRECTIVE` | W0014 | 未知 `#!` 指令 |
| `W_LINE_OUTSIDE_SECTION` | W0015 | 节外的行被忽略 |
| `W_DEFERRED_SECTION` | W0016 | 节已解析但本版本无行为 |
| `I_LEGACY_MIGRATED` | I0001 | 旧键已迁移 |
| `I_LINE_DISABLED` | I0002 | 行因 Requirement 未满足而禁用 |

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/diagnostic.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span(line: u32) -> Span {
        Span::new(Arc::from(Path::new("a.conf")), line)
    }

    #[test]
    fn builders_and_display() {
        let d = Diagnostic::error(codes::E_MISSING_FINAL, "missing FINAL rule")
            .at(span(12))
            .with_hint("add `FINAL,DIRECT` as the last rule");
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.code, "E0010");
        assert_eq!(
            d.to_string(),
            "error[E0010] a.conf:12: missing FINAL rule\n  hint: add `FINAL,DIRECT` as the last rule"
        );
    }

    #[test]
    fn diagnostics_has_errors_and_sorting() {
        let mut ds = Diagnostics::default();
        ds.push(Diagnostic::warning(codes::W_UNKNOWN_KEY, "w").at(span(5)));
        ds.push(Diagnostic::error(codes::E_SYNTAX, "e").at(span(2)));
        ds.push(Diagnostic::info(codes::I_LEGACY_MIGRATED, "i"));
        assert!(ds.has_errors());
        let sorted = ds.sorted().into_vec();
        assert_eq!(sorted[0].span, None); // spanless first
        assert_eq!(sorted[1].code, "E0001");
        assert_eq!(sorted[2].code, "W0001");
    }

    #[test]
    fn json_shape() {
        let d = Diagnostic::warning(codes::W_UNKNOWN_KEY, "unknown key").at(span(3));
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(json["severity"], "warning");
        assert_eq!(json["code"], "W0001");
        assert_eq!(json["span"]["line"], 3);
        assert_eq!(json["span"]["file"], "a.conf");
    }
}
```

给 `crates/rurge-config/Cargo.toml` 的 `[dev-dependencies]` 加 `serde_json.workspace = true`。

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config diagnostic`
Expected: 编译失败，`span` / `diagnostic` 模块不存在。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/span.rs`：

```rust
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use std::fmt;
use std::path::Path;
use std::sync::Arc;

/// Location of a profile line: file plus 1-based line number.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Span {
    pub file: Arc<Path>,
    pub line: u32,
}

impl Span {
    pub fn new(file: Arc<Path>, line: u32) -> Self {
        Self { file, line }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.file.display(), self.line)
    }
}

impl Serialize for Span {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("Span", 2)?;
        st.serialize_field("file", &self.file.display().to_string())?;
        st.serialize_field("line", &self.line)?;
        st.end()
    }
}
```

`crates/rurge-config/src/diagnostic.rs`：

```rust
use crate::span::Span;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        })
    }
}

/// One problem found while loading a profile.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
    pub span: Option<Span>,
    pub hint: Option<String>,
}

impl Diagnostic {
    pub fn new(severity: Severity, code: &'static str, message: impl Into<String>) -> Self {
        Self { severity, code, message: message.into(), span: None, hint: None }
    }
    pub fn error(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, code, message)
    }
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, code, message)
    }
    pub fn info(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Info, code, message)
    }
    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]", self.severity, self.code)?;
        if let Some(span) = &self.span {
            write!(f, " {span}")?;
        }
        write!(f, ": {}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

/// Ordered collection of diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diagnostics {
    items: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.items.push(d);
    }
    pub fn extend(&mut self, other: Diagnostics) {
        self.items.extend(other.items);
    }
    pub fn has_errors(&self) -> bool {
        self.items.iter().any(|d| d.severity == Severity::Error)
    }
    pub fn iter(&self) -> impl Iterator<Item = &Diagnostic> {
        self.items.iter()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.items
    }
    /// Sort by file, then line, then severity (errors first). Spanless items come first.
    pub fn sorted(mut self) -> Self {
        self.items.sort_by(|a, b| {
            a.span
                .cmp(&b.span)
                .then_with(|| b.severity.cmp(&a.severity))
        });
        self
    }
}

/// Stable diagnostic codes. Never renumber.
pub mod codes {
    pub const E_SYNTAX: &str = "E0001";
    pub const E_INCLUDE_NOT_FOUND: &str = "E0002";
    pub const E_REQUIREMENT_SYNTAX: &str = "E0003";
    pub const E_UNKNOWN_POLICY_TYPE: &str = "E0004";
    pub const E_BUILTIN_REDEFINED: &str = "E0005";
    pub const E_DUPLICATE_NAME: &str = "E0006";
    pub const E_UNKNOWN_POLICY_REF: &str = "E0007";
    pub const E_UNKNOWN_GROUP_MEMBER: &str = "E0008";
    pub const E_GROUP_CYCLE: &str = "E0009";
    pub const E_MISSING_FINAL: &str = "E0010";
    pub const E_INVALID_RULE_VALUE: &str = "E0011";
    pub const E_UNKNOWN_RULE_TYPE: &str = "E0012";
    pub const E_LISTENER_NOT_IP: &str = "E0013";
    pub const E_NOT_ALLOWED_HERE: &str = "E0014";
    pub const E_NESTING_TOO_DEEP: &str = "E0015";
    pub const E_INCLUDE_CYCLE: &str = "E0016";
    pub const E_INVALID_DEFINITION: &str = "E0017";
    pub const W_UNKNOWN_KEY: &str = "W0001";
    pub const W_UNKNOWN_SECTION: &str = "W0002";
    pub const W_UNKNOWN_RULE_PARAM: &str = "W0003";
    pub const W_PLATFORM_IGNORED: &str = "W0004";
    pub const W_INVALID_HOST_LIST_ENTRY: &str = "W0005";
    pub const W_VANISHED_KEY: &str = "W0006";
    pub const W_PROTOCOL_NOT_IMPLEMENTED: &str = "W0007";
    pub const W_GROUP_NOT_IMPLEMENTED: &str = "W0008";
    pub const W_IOS_BUILTIN_AS_DIRECT: &str = "W0009";
    pub const W_DEVICE_POLICY_AS_REJECT: &str = "W0010";
    pub const W_REMOTE_INCLUDE_UNSUPPORTED: &str = "W0011";
    pub const W_INVALID_VALUE: &str = "W0012";
    pub const W_RULE_NEVER_MATCHES: &str = "W0013";
    pub const W_UNKNOWN_DIRECTIVE: &str = "W0014";
    pub const W_LINE_OUTSIDE_SECTION: &str = "W0015";
    pub const W_DEFERRED_SECTION: &str = "W0016";
    pub const I_LEGACY_MIGRATED: &str = "I0001";
    pub const I_LINE_DISABLED: &str = "I0002";
}
```

`lib.rs` 增加：

```rust
pub mod diagnostic;
pub mod span;

pub use diagnostic::{Diagnostic, Diagnostics, Severity, codes};
pub use span::Span;
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config diagnostic`
Expected: 3 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): 增加 Span、Diagnostic 与稳定的诊断代码表"
```

---

### Task 3: 文本层解析 `parse_str`

**Files:**
- Create: `crates/rurge-config/src/text/mod.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `Span`、`Diagnostic`、`Diagnostics`、`codes`（Task 2）。本任务先内置一个最小的 `requirement::split_line`（Task 5 替换为完整实现，签名不变）：`pub fn split_line(line: &str) -> Result<(Option<String>, String), ReqError>`。
- Produces:
  - `Origin { Main, Include(Arc<Path>), Module(String) }`
  - `SectionKind { KeyValue, Ordered, Unknown }`，`section_kind(name: &str) -> SectionKind`
  - `Entry { raw: String, span: Span, origin: Origin, requirement: Option<String>, disabled: bool }`
  - `Section { name: String, kind: SectionKind, span: Span, entries: Vec<Entry> }`，`Section::active_entries()` 迭代未禁用条目
  - `Directive { raw: String, span: Span }`
  - `Profile { main: Option<Arc<Path>>, header: Vec<Directive>, sections: Vec<Section> }`，`Profile::section(name)`（不区分大小写）、`Profile::sections_with_prefix("Ruleset ")`
  - `parse_str(text: &str, file: Arc<Path>, origin: Origin) -> (Profile, Diagnostics)`
  - `strip_inline_comment(line: &str) -> &str`、`is_comment(line: &str) -> bool`

规则（设计文档 5.1，清单 1.1）：
- 行首 `#`（但 `#!` 除外）、`;`、`//`（但 `//!` 除外）为注释行。
- 行内注释：` #`、` ;`、` //` 且前面至少一个空白；` #!` 与 ` //!` 不是注释；双引号内不识别注释。
- `[Name]` 为节头；`#!include ...` 作为普通条目保留（Task 6 展开）；节外的 `#!` 行进入 `header`；节外普通行告警 W0015。
- 行尾 `\r` 去掉；空行跳过。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/text/mod.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn parse(text: &str) -> (Profile, Diagnostics) {
        parse_str(text, Arc::from(Path::new("t.conf")), Origin::Main)
    }

    #[test]
    fn sections_entries_and_comments() {
        let text = "\
#!MANAGED-CONFIG https://x/y interval=60
# a comment
[General]
loglevel = notify // inline
dns-server = 8.8.8.8 # inline
; another comment
[Rule]
DOMAIN,a.com,DIRECT
// comment
FINAL,DIRECT
[Weird Section]
whatever = 1
";
        let (p, d) = parse(text);
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(p.header.len(), 1);
        assert_eq!(p.header[0].raw, "#!MANAGED-CONFIG https://x/y interval=60");
        assert_eq!(p.sections.len(), 3);
        let g = p.section("general").unwrap();
        assert_eq!(g.kind, SectionKind::KeyValue);
        assert_eq!(g.entries[0].raw, "loglevel = notify");
        assert_eq!(g.entries[0].span.line, 4);
        assert_eq!(g.entries[1].raw, "dns-server = 8.8.8.8");
        let r = p.section("Rule").unwrap();
        assert_eq!(r.kind, SectionKind::Ordered);
        assert_eq!(r.entries.len(), 2);
        assert_eq!(p.sections[2].kind, SectionKind::Unknown);
        assert_eq!(p.sections[2].entries[0].raw, "whatever = 1");
    }

    #[test]
    fn inline_comment_rules() {
        assert_eq!(strip_inline_comment("dns-server = 8.8.8.8 // c"), "dns-server = 8.8.8.8");
        assert_eq!(strip_inline_comment("dns-server = 8.8.8.8 # c"), "dns-server = 8.8.8.8");
        assert_eq!(strip_inline_comment("dns-server = 8.8.8.8 ; c"), "dns-server = 8.8.8.8");
        assert_eq!(strip_inline_comment("url = http://a/b#c"), "url = http://a/b#c");
        assert_eq!(strip_inline_comment("x = \"a // b\" // c"), "x = \"a // b\"");
        assert_eq!(strip_inline_comment("DOMAIN,a,REJECT #!MACOS-ONLY"), "DOMAIN,a,REJECT #!MACOS-ONLY");
        assert_eq!(strip_inline_comment("G = url-test, A //!REQUIREMENT CORE_VERSION<22"), "G = url-test, A //!REQUIREMENT CORE_VERSION<22");
        assert!(is_comment("# x"));
        assert!(is_comment("; x"));
        assert!(is_comment("// x"));
        assert!(!is_comment("#!include a.dconf"));
        assert!(!is_comment("//!REQUIREMENT x"));
    }

    #[test]
    fn named_sections_and_prefix_lookup() {
        let (p, _) = parse("[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n[WireGuard home]\nmtu = 1280\n[Ruleset Other]\n");
        let names: Vec<_> = p.sections_with_prefix("Ruleset ").map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Ruleset Streaming", "Ruleset Other"]);
        assert_eq!(p.section("WireGuard home").unwrap().kind, SectionKind::KeyValue);
        assert_eq!(p.section("Ruleset Streaming").unwrap().kind, SectionKind::Ordered);
    }

    #[test]
    fn requirement_directives_are_attached_to_entries() {
        let (p, d) = parse("[Rule]\n#!MACOS-ONLY DOMAIN,a.com,REJECT\nDOMAIN,b.com,REJECT #!IOS-ONLY\nFINAL,DIRECT\n");
        assert!(d.is_empty());
        let r = p.section("Rule").unwrap();
        assert_eq!(r.entries[0].raw, "DOMAIN,a.com,REJECT");
        assert_eq!(r.entries[0].requirement.as_deref(), Some("SYSTEM == 'macOS'"));
        assert_eq!(r.entries[1].requirement.as_deref(), Some("SYSTEM == 'iOS'"));
        assert_eq!(r.entries[2].requirement, None);
    }

    #[test]
    fn lines_outside_sections_warn_and_crlf_is_stripped() {
        let (p, d) = parse("stray = 1\r\n[General]\r\nipv6 = true\r\n");
        assert_eq!(d.iter().next().unwrap().code, codes::W_LINE_OUTSIDE_SECTION);
        assert_eq!(p.section("General").unwrap().entries[0].raw, "ipv6 = true");
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config text`
Expected: 编译失败（模块不存在）。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/requirement.rs`（本任务的最小版本，Task 5 替换）：

```rust
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ReqError(pub String);

/// Split a profile line into (requirement expression, content).
/// Minimal version: only the simplified `#!IOS-ONLY` / `#!MACOS-ONLY` / `#!TVOS-ONLY`
/// prefix and suffix forms. Task 5 replaces this with the full implementation.
pub fn split_line(line: &str) -> Result<(Option<String>, String), ReqError> {
    const SIMPLIFIED: [(&str, &str); 3] = [
        ("#!IOS-ONLY", "SYSTEM == 'iOS'"),
        ("#!MACOS-ONLY", "SYSTEM == 'macOS'"),
        ("#!TVOS-ONLY", "SYSTEM == 'tvOS'"),
    ];
    for (tag, expr) in SIMPLIFIED {
        if let Some(rest) = line.strip_prefix(tag) {
            return Ok((Some(expr.to_string()), rest.trim().to_string()));
        }
        if let Some(body) = line.strip_suffix(tag) {
            let body = body.trim_end();
            if body.ends_with(char::is_whitespace) || body.is_empty() {
                return Ok((Some(expr.to_string()), body.trim().to_string()));
            }
            // e.g. "foo #!IOS-ONLY" -> body "foo " (already trimmed) ; require a separating space
            return Ok((Some(expr.to_string()), body.to_string()));
        }
    }
    Ok((None, line.trim().to_string()))
}
```

`crates/rurge-config/src/text/mod.rs`：

```rust
pub mod include;

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::requirement;
use crate::span::Span;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    Main,
    Include(Arc<Path>),
    Module(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectionKind {
    KeyValue,
    Ordered,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Trimmed line content without inline comment and without requirement directive.
    pub raw: String,
    pub span: Span,
    pub origin: Origin,
    /// Requirement expression source attached to this line, if any.
    pub requirement: Option<String>,
    /// Set by requirement evaluation when the expression is not satisfied.
    pub disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub kind: SectionKind,
    pub span: Span,
    pub entries: Vec<Entry>,
}

impl Section {
    pub fn active_entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.disabled)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    pub raw: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub main: Option<Arc<Path>>,
    pub header: Vec<Directive>,
    pub sections: Vec<Section>,
}

impl Profile {
    /// Case-insensitive lookup of the first section with this name.
    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
    pub fn section_mut(&mut self, name: &str) -> Option<&mut Section> {
        self.sections.iter_mut().find(|s| s.name.eq_ignore_ascii_case(name))
    }
    /// Sections whose name starts with `prefix` (e.g. "Ruleset "), in file order.
    pub fn sections_with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a Section> + 'a {
        self.sections.iter().filter(move |s| starts_with_ci(&s.name, prefix))
    }
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

const KEY_VALUE_SECTIONS: &[&str] = &[
    "General", "Proxy", "Proxy Group", "MITM", "Keystore", "Ponte", "Testing", "DHCP", "Snell Server", "MTProto",
];
const ORDERED_SECTIONS: &[&str] = &[
    "Rule", "Host", "URL Rewrite", "Header Rewrite", "Body Rewrite", "Map Local", "Panel", "Port Forwarding",
    "Script", "SSID Setting",
];

pub fn section_kind(name: &str) -> SectionKind {
    if KEY_VALUE_SECTIONS.iter().any(|k| k.eq_ignore_ascii_case(name))
        || starts_with_ci(name, "WireGuard ")
        || starts_with_ci(name, "Tailscale ")
    {
        SectionKind::KeyValue
    } else if ORDERED_SECTIONS.iter().any(|k| k.eq_ignore_ascii_case(name)) || starts_with_ci(name, "Ruleset ") {
        SectionKind::Ordered
    } else {
        SectionKind::Unknown
    }
}

/// A line is a comment if it starts with `#` (not `#!`), `;`, or `//` (not `//!`).
pub fn is_comment(line: &str) -> bool {
    (line.starts_with('#') && !line.starts_with("#!"))
        || line.starts_with(';')
        || (line.starts_with("//") && !line.starts_with("//!"))
}

/// Remove an inline comment (` #`, ` ;`, ` //` preceded by whitespace, outside double quotes).
/// ` #!` and ` //!` are requirement directives, not comments.
pub fn strip_inline_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_quotes {
            if c == b'\\' {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_quotes = false;
            }
            i += 1;
            continue;
        }
        if c == b'"' {
            in_quotes = true;
            i += 1;
            continue;
        }
        let prev_is_space = i > 0 && bytes[i - 1].is_ascii_whitespace();
        if prev_is_space {
            let next = bytes.get(i + 1).copied();
            let is_directive = |n: Option<u8>| n == Some(b'!');
            if c == b';' {
                return line[..i].trim_end();
            }
            if c == b'#' && !is_directive(next) {
                return line[..i].trim_end();
            }
            if c == b'/' && next == Some(b'/') && !is_directive(bytes.get(i + 2).copied()) {
                return line[..i].trim_end();
            }
        }
        i += 1;
    }
    line
}

fn section_header(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?.trim();
    (!inner.is_empty()).then_some(inner)
}

fn is_requirement_prefix(line: &str) -> bool {
    ["#!REQUIREMENT", "#!IOS-ONLY", "#!MACOS-ONLY", "#!TVOS-ONLY"].iter().any(|p| line.starts_with(p))
}

/// Parse profile text. Never fails; problems are reported as diagnostics.
pub fn parse_str(text: &str, file: Arc<Path>, origin: Origin) -> (Profile, Diagnostics) {
    let mut profile = Profile { main: Some(file.clone()), header: Vec::new(), sections: Vec::new() };
    let mut diags = Diagnostics::default();
    let mut current: Option<Section> = None;

    for (idx, line) in text.lines().enumerate() {
        let span = Span::new(file.clone(), idx as u32 + 1);
        let trimmed = line.trim_end_matches('\r').trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(name) = section_header(trimmed) {
            if let Some(sec) = current.take() {
                profile.sections.push(sec);
            }
            current = Some(Section { name: name.to_string(), kind: section_kind(name), span, entries: Vec::new() });
            continue;
        }
        if trimmed.starts_with("#!") && !trimmed.starts_with("#!include") && !is_requirement_prefix(trimmed) {
            match current.as_mut() {
                None => profile.header.push(Directive { raw: trimmed.to_string(), span }),
                Some(_) => diags.push(
                    Diagnostic::warning(codes::W_UNKNOWN_DIRECTIVE, format!("unknown directive ignored: {trimmed}"))
                        .at(span),
                ),
            }
            continue;
        }
        if is_comment(trimmed) {
            continue;
        }
        let Some(sec) = current.as_mut() else {
            diags.push(Diagnostic::warning(codes::W_LINE_OUTSIDE_SECTION, "line outside of any section is ignored").at(span));
            continue;
        };
        let content = strip_inline_comment(trimmed);
        match requirement::split_line(content) {
            Ok((requirement, body)) => {
                if body.is_empty() {
                    continue;
                }
                sec.entries.push(Entry { raw: body, span, origin: origin.clone(), requirement, disabled: false });
            }
            Err(e) => diags.push(Diagnostic::error(codes::E_REQUIREMENT_SYNTAX, e.to_string()).at(span)),
        }
    }
    if let Some(sec) = current.take() {
        profile.sections.push(sec);
    }
    (profile, diags)
}
```

`crates/rurge-config/src/text/include.rs` 先放一个空文件（Task 6 填充）：

```rust
//! `#!include` expansion. Implemented in Task 6.
```

`lib.rs` 增加 `pub mod requirement; pub mod text;` 与 `pub use text::{Entry, Origin, Profile, Section, SectionKind};`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config text`
Expected: 5 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): 文本层解析：节、条目、注释规则与 Requirement 指令附着"
```

---

### Task 4: 值拆分器与 ParamMap

**Files:**
- Create: `crates/rurge-config/src/value.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Produces:
  - `split_list(s: &str) -> Vec<String>`：顶层逗号拆分，尊重 `"..."` / `'...'` 与 `(...)` 嵌套，每项 trim 并去引号，空项丢弃。
  - `unquote(s: &str) -> String`：`"..."` 内 `\"` → `"`、`\\` → `\`；`'...'` 去引号不转义；其他原样。
  - `split_definition(raw: &str) -> Option<(&str, &str)>`：`Name = rest` 在第一个 `=` 处拆分，两侧 trim，名字非空。
  - `parse_key_value(field: &str) -> Option<(&str, &str)>`：同上，用于 `key=value` 参数。
  - `parse_bool(s: &str) -> Option<bool>`：`true/false/1/0/yes/no`（不区分大小写）。
  - `ParamMap`：`insert(key, value)`（键小写 trim，值 unquote trim；允许重复键）、`get(key) -> Option<&str>`（首个）、`get_all(key) -> Vec<&str>`、`contains(key)`、`bool(key) -> Option<bool>`、`u16(key)`、`u32(key)`、`iter()`、`len()`、`is_empty()`、`from_fields(fields: &[String]) -> (ParamMap, Vec<String> /* 非 key=value 的位置参数 */)`。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/value.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_list_cases() {
        let cases: &[(&str, &[&str])] = &[
            ("a, b ,c", &["a", "b", "c"]),
            ("", &[]),
            ("a,,b", &["a", "b"]),
            ("ss, 1.2.3.4, 8388, encrypt-method=aes-128-gcm, password=\"p,w\"", &["ss", "1.2.3.4", "8388", "encrypt-method=aes-128-gcm", "password=p,w"]),
            ("AND,((SRC-IP,1.2.3.4),(DOMAIN-SUFFIX,a.com)),DIRECT", &["AND", "((SRC-IP,1.2.3.4),(DOMAIN-SUFFIX,a.com))", "DIRECT"]),
            ("URL-REGEX,\"^http://x/(a|b),?c\",Proxy", &["URL-REGEX", "^http://x/(a|b),?c", "Proxy"]),
            ("peer = (public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1), (public-key = j, allowed-ips = 0.0.0.0/0, endpoint = b:2)",
             &["peer = (public-key = k, allowed-ips = \"10.0.0.0/8, 192.168.0.0/16\", endpoint = a:1)", "(public-key = j, allowed-ips = 0.0.0.0/0, endpoint = b:2)"]),
            ("x='a,b',c", &["x=a,b", "c"]),
        ];
        for (input, expected) in cases {
            let got = split_list(input);
            assert_eq!(got, *expected, "input: {input}");
        }
    }

    #[test]
    fn unquote_cases() {
        assert_eq!(unquote("\"a \\\"b\\\" c\\\\d\""), "a \"b\" c\\d");
        assert_eq!(unquote("'x'"), "x");
        assert_eq!(unquote("plain"), "plain");
        assert_eq!(unquote("\"unterminated"), "\"unterminated");
    }

    #[test]
    fn definitions_and_params() {
        assert_eq!(split_definition("Proxy = ss, a, 1"), Some(("Proxy", "ss, a, 1")));
        assert_eq!(split_definition("  a.b = c=d "), Some(("a.b", "c=d")));
        assert_eq!(split_definition("= x"), None);
        assert_eq!(split_definition("no equals"), None);
        assert_eq!(parse_key_value("Key = Value"), Some(("Key", "Value")));
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("maybe"), None);

        let fields = split_list("user, pass, tfo=true, args=a, args=b, Port=443");
        let (params, positional) = ParamMap::from_fields(&fields);
        assert_eq!(positional, ["user", "pass"]);
        assert_eq!(params.get("TFO"), Some("true"));
        assert_eq!(params.bool("tfo"), Some(true));
        assert_eq!(params.get_all("args"), ["a", "b"]);
        assert_eq!(params.u16("port"), Some(443));
        assert_eq!(params.len(), 4);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config value`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/value.rs`：

```rust
//! Value-level helpers shared by all section parsers.

/// Split a comma-separated list at top level, honouring quotes and parentheses.
/// Items are trimmed and unquoted; empty items are dropped.
pub fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                cur.push(c);
                if c == '\\' && q == '"' {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                    }
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    cur.push(c);
                }
                '(' => {
                    depth += 1;
                    cur.push(c);
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    cur.push(c);
                }
                ',' if depth == 0 => {
                    out.push(std::mem::take(&mut cur));
                }
                _ => cur.push(c),
            },
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|f| unquote(f.trim()))
        .filter(|f| !f.is_empty())
        .collect()
}

/// Strip surrounding quotes. Double quotes support `\"` and `\\` escapes.
pub fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    } else if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Split `Name = rest` at the first `=`.
pub fn split_definition(raw: &str) -> Option<(&str, &str)> {
    let (name, rest) = raw.split_once('=')?;
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    Some((name, rest.trim()))
}

/// Split `key=value` at the first `=`.
pub fn parse_key_value(field: &str) -> Option<(&str, &str)> {
    split_definition(field)
}

pub fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Ordered, case-insensitive `key=value` parameters. Duplicate keys are kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParamMap {
    entries: Vec<(String, String)>,
}

impl ParamMap {
    pub fn insert(&mut self, key: &str, value: &str) {
        self.entries.push((key.trim().to_ascii_lowercase(), unquote(value.trim())));
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        let key = key.to_ascii_lowercase();
        self.entries.iter().find(|(k, _)| *k == key).map(|(_, v)| v.as_str())
    }
    pub fn get_all(&self, key: &str) -> Vec<&str> {
        let key = key.to_ascii_lowercase();
        self.entries.iter().filter(|(k, _)| *k == key).map(|(_, v)| v.as_str()).collect()
    }
    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
    pub fn bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(parse_bool)
    }
    pub fn u16(&self, key: &str) -> Option<u16> {
        self.get(key).and_then(|v| v.parse().ok())
    }
    pub fn u32(&self, key: &str) -> Option<u32> {
        self.get(key).and_then(|v| v.parse().ok())
    }
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Split fields into `key=value` parameters and positional values.
    pub fn from_fields(fields: &[String]) -> (ParamMap, Vec<String>) {
        let mut params = ParamMap::default();
        let mut positional = Vec::new();
        for f in fields {
            match parse_key_value(f) {
                Some((k, v)) => params.insert(k, v),
                None => positional.push(f.clone()),
            }
        }
        (params, positional)
    }
}
```

注意 `split_list` 中的 `unquote(f.trim())`：`"password=\"p,w\""` 这类 `key="value"` 字段整体不带外层引号，所以 `unquote` 不动它；`ParamMap::insert` 再对值去引号。测试用例 `password=p,w` 的期望值依赖 `split_list` 保留 `password="p,w"` 原样——为满足测试，`split_list` 对形如 `key="..."` 的字段也去掉值上的引号：在 `map` 中把 `unquote(f.trim())` 换成 `unquote_field(f.trim())`：

```rust
fn unquote_field(f: &str) -> String {
    match f.split_once('=') {
        Some((k, v)) if !k.trim().is_empty() && (v.trim().starts_with('"') || v.trim().starts_with('\'')) => {
            format!("{}={}", k.trim(), unquote(v.trim()))
        }
        _ => unquote(f),
    }
}
```

`lib.rs` 增加 `pub mod value;` 与 `pub use value::ParamMap;`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config value`
Expected: 3 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): 值拆分器、引号处理与 ParamMap"
```

---

### Task 5: Glob 匹配器与 Requirement 表达式

**Files:**
- Create: `crates/rurge-config/src/glob.rs`
- Modify: `crates/rurge-config/src/requirement.rs`（替换 Task 3 的最小版本）
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `Profile` / `Entry`（Task 3）、`Diagnostics`、`codes`（Task 2）。
- Produces:
  - `glob::GlobOptions { case_insensitive: bool, classes: bool }`，`Glob::new(pattern, opts) -> Result<Glob, GlobError>`，`Glob::matches(&self, s) -> bool`，`Glob::source()`，`Glob::has_wildcards()`。`*` 匹配任意长度（含空，可跨 `.`），`?` 匹配一个字符，`classes=true` 时支持 `[abc]` `[a-z]` `[!x]`。
  - `requirement::Environment { core_version: u64, system: String, system_version: String, device_model: String, language: String, device_name: String }`，`Environment::fixed()`（测试用：`core_version 20`，`system "macOS"`，`system_version "14.5"`，`device_model "Mac15,6"`，`language "zh-CN"`，`device_name "test-mac"`）。
  - `requirement::Expr`（`Cmp(Var, CmpOp, Value)` / `Str(Var, StrOp, String)` / `And` / `Or` / `Not`），`parse(src: &str) -> Result<Expr, ReqError>`，`eval(&Expr, &Environment) -> bool`。
  - `requirement::split_line(line) -> Result<(Option<String>, String), ReqError>`（完整版：`#!REQUIREMENT <expr>` 行首、行尾 `#!REQUIREMENT` / `//!REQUIREMENT`、三种简写的行首与行尾形式；含空格的表达式用 `"` 包裹）。
  - `requirement::apply(profile: &mut Profile, env: &Environment, diags: &mut Diagnostics)`：求值所有带 `requirement` 的条目；不满足 → `disabled = true` + I0002；语法错误 → E0003 并禁用该行。

语义：`CORE_VERSION` 按整数比较；其他变量按字符串比较（`==`/`=`、`!=`/`<>` 精确；`>` `<` 等按字典序）；`BEGINSWITH` `CONTAINS` `ENDSWITH` 区分大小写；`LIKE` 用 `*` `?` 通配；`MATCHES` 用正则（fancy-regex）。关键字 `AND` `OR` `NOT` 与运算符 `&&` `||` `!` 等价，不区分大小写；优先级 `NOT` > `AND` > `OR`；括号分组。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/glob.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn ci(p: &str) -> Glob {
        Glob::new(p, GlobOptions { case_insensitive: true, classes: false }).unwrap()
    }
    fn cs(p: &str) -> Glob {
        Glob::new(p, GlobOptions { case_insensitive: false, classes: true }).unwrap()
    }

    #[test]
    fn wildcard_semantics() {
        assert!(ci("*.example.com").matches("a.b.example.com"));
        assert!(!ci("*.example.com").matches("example.com"));
        assert!(ci("*google.com").matches("bargoogle.com"));
        assert!(ci("api-*.example.com").matches("api-v2.example.com"));
        assert!(ci("cdn?.example.com").matches("cdn1.example.com"));
        assert!(!ci("cdn?.example.com").matches("cdn10.example.com"));
        assert!(ci("*").matches(""));
        assert!(ci("EXAMPLE.com").matches("example.COM"));
        assert!(!cs("Instagram*").matches("instagram 1.0"));
        assert!(cs("Instagram*").matches("Instagram 1.0"));
    }

    #[test]
    fn classes_and_errors() {
        assert!(cs("cdn[0-9].example.com").matches("cdn7.example.com"));
        assert!(!cs("cdn[!0-9].example.com").matches("cdn7.example.com"));
        assert!(cs("[abc]x").matches("bx"));
        assert!(Glob::new("[abc", GlobOptions { case_insensitive: false, classes: true }).is_err());
        // classes disabled: brackets are literals
        assert!(ci("[a]").matches("[a]"));
        assert!(!ci("a").has_wildcards());
        assert!(ci("a*").has_wildcards());
    }
}
```

`crates/rurge-config/src/requirement.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment::fixed()
    }

    #[test]
    fn manual_examples_evaluate() {
        let cases: &[(&str, bool)] = &[
            ("CORE_VERSION>=22", false),
            ("CORE_VERSION<22", true),
            ("CORE_VERSION>=20", true),
            ("SYSTEM=='macOS'", true),
            ("SYSTEM = 'iOS'", false),
            ("SYSTEM<>'iOS'", true),
            ("SYSTEM!='macOS'", false),
            ("CORE_VERSION>=22 AND SYSTEM=='iOS'", false),
            ("CORE_VERSION>=20 && (SYSTEM = 'iOS' || SYSTEM = 'macOS')", true),
            ("NOT SYSTEM=='iOS'", true),
            ("!(SYSTEM=='macOS')", false),
            ("DEVICE_NAME CONTAINS 'mac'", true),
            ("LANGUAGE BEGINSWITH 'zh'", true),
            ("LANGUAGE ENDSWITH 'US'", false),
            ("DEVICE_MODEL LIKE 'Mac1?,*'", true),
            ("SYSTEM_VERSION MATCHES '^14\\.'", true),
            ("CORE_VERSION=>20", true),
            ("CORE_VERSION=<19", false),
            ("system == 'macOS'", true),
            ("CORE_VERSION >= 6008000", false),
        ];
        for (src, expected) in cases {
            let expr = parse(src).unwrap_or_else(|e| panic!("{src}: {e}"));
            assert_eq!(eval(&expr, &env()), *expected, "expr: {src}");
        }
    }

    #[test]
    fn syntax_errors() {
        for src in ["", "CORE_VERSION >=", "UNKNOWN_VAR == 1", "SYSTEM == ", "(SYSTEM == 'a'", "CORE_VERSION ?? 1"] {
            assert!(parse(src).is_err(), "should fail: {src:?}");
        }
    }

    #[test]
    fn split_line_forms() {
        let ok = |l: &str| split_line(l).unwrap();
        assert_eq!(ok("#!REQUIREMENT CORE_VERSION>=22 Group = smart, a, b"), (Some("CORE_VERSION>=22".into()), "Group = smart, a, b".into()));
        assert_eq!(ok("Group = url-test, a, b //!REQUIREMENT CORE_VERSION<22"), (Some("CORE_VERSION<22".into()), "Group = url-test, a, b".into()));
        assert_eq!(ok("Group = url-test, a #!REQUIREMENT CORE_VERSION<22"), (Some("CORE_VERSION<22".into()), "Group = url-test, a".into()));
        assert_eq!(ok("#!REQUIREMENT \"CORE_VERSION>=22 AND SYSTEM=='iOS'\" Group = smart, a"), (Some("CORE_VERSION>=22 AND SYSTEM=='iOS'".into()), "Group = smart, a".into()));
        assert_eq!(ok("#!REQUIREMENT SYSTEM=='macOS'"), (Some("SYSTEM=='macOS'".into()), String::new()));
        assert_eq!(ok("DOMAIN,reject.com,REJECT #!MACOS-ONLY"), (Some("SYSTEM == 'macOS'".into()), "DOMAIN,reject.com,REJECT".into()));
        assert_eq!(ok("#!IOS-ONLY DOMAIN,a,REJECT"), (Some("SYSTEM == 'iOS'".into()), "DOMAIN,a,REJECT".into()));
        assert_eq!(ok("DOMAIN,a,REJECT //!TVOS-ONLY"), (Some("SYSTEM == 'tvOS'".into()), "DOMAIN,a,REJECT".into()));
        assert_eq!(ok("plain line"), (None, "plain line".into()));
        assert!(split_line("x #!REQUIREMENT").is_err());
    }

    #[test]
    fn apply_disables_unmet_lines() {
        use crate::text::{Origin, parse_str};
        use std::path::Path;
        use std::sync::Arc;
        let text = "[Rule]\n#!REQUIREMENT CORE_VERSION>=22 DOMAIN,a,REJECT\nDOMAIN,b,REJECT #!MACOS-ONLY\nDOMAIN,c,REJECT #!REQUIREMENT SYSTEM ??\nFINAL,DIRECT\n";
        let (mut p, mut d) = parse_str(text, Arc::from(Path::new("t.conf")), Origin::Main);
        apply(&mut p, &env(), &mut d);
        let r = p.section("Rule").unwrap();
        assert!(r.entries[0].disabled);
        assert!(!r.entries[1].disabled);
        assert!(r.entries[2].disabled);
        assert!(!r.entries[3].disabled);
        let codes: Vec<_> = d.iter().map(|x| x.code).collect();
        assert!(codes.contains(&codes::I_LINE_DISABLED));
        assert!(codes.contains(&codes::E_REQUIREMENT_SYNTAX));
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config glob requirement`
Expected: 编译失败（`glob` 不存在；`Environment` / `parse` / `apply` 不存在）。

- [ ] **Step 3: 实现 glob**

`crates/rurge-config/src/glob.rs`：

```rust
//! Minimal wildcard matcher: `*` (any run, crosses dots), `?` (one char), optional `[...]` classes.

use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobOptions {
    pub case_insensitive: bool,
    pub classes: bool,
}

#[derive(Debug, Error, PartialEq, Eq)]
#[error("invalid glob pattern: {0}")]
pub struct GlobError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Lit(char),
    Any,
    One,
    Class { negate: bool, ranges: Vec<(char, char)> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob {
    source: String,
    toks: Vec<Tok>,
    ci: bool,
}

fn fold(c: char, ci: bool) -> char {
    if ci { c.to_ascii_lowercase() } else { c }
}

impl Glob {
    pub fn new(pattern: &str, opts: GlobOptions) -> Result<Glob, GlobError> {
        let mut toks = Vec::new();
        let chars: Vec<char> = pattern.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '*' => {
                    if toks.last() != Some(&Tok::Any) {
                        toks.push(Tok::Any);
                    }
                }
                '?' => toks.push(Tok::One),
                '[' if opts.classes => {
                    let mut j = i + 1;
                    let negate = j < chars.len() && (chars[j] == '!' || chars[j] == '^');
                    if negate {
                        j += 1;
                    }
                    let mut ranges = Vec::new();
                    let mut closed = false;
                    while j < chars.len() {
                        if chars[j] == ']' && !ranges.is_empty() {
                            closed = true;
                            break;
                        }
                        let lo = chars[j];
                        if j + 2 < chars.len() && chars[j + 1] == '-' && chars[j + 2] != ']' {
                            ranges.push((fold(lo, opts.case_insensitive), fold(chars[j + 2], opts.case_insensitive)));
                            j += 3;
                        } else {
                            let f = fold(lo, opts.case_insensitive);
                            ranges.push((f, f));
                            j += 1;
                        }
                    }
                    if !closed {
                        return Err(GlobError(format!("unterminated character class in `{pattern}`")));
                    }
                    toks.push(Tok::Class { negate, ranges });
                    i = j;
                }
                _ => toks.push(Tok::Lit(fold(c, opts.case_insensitive))),
            }
            i += 1;
        }
        Ok(Glob { source: pattern.to_string(), toks, ci: opts.case_insensitive })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn has_wildcards(&self) -> bool {
        self.toks.iter().any(|t| !matches!(t, Tok::Lit(_)))
    }

    fn tok_matches(tok: &Tok, c: char) -> bool {
        match tok {
            Tok::Lit(l) => *l == c,
            Tok::One => true,
            Tok::Class { negate, ranges } => ranges.iter().any(|(a, b)| *a <= c && c <= *b) != *negate,
            Tok::Any => false,
        }
    }

    pub fn matches(&self, s: &str) -> bool {
        let input: Vec<char> = s.chars().map(|c| fold(c, self.ci)).collect();
        let (mut si, mut pi) = (0usize, 0usize);
        let mut star: Option<(usize, usize)> = None;
        while si < input.len() {
            if pi < self.toks.len() && Self::tok_matches(&self.toks[pi], input[si]) {
                si += 1;
                pi += 1;
            } else if pi < self.toks.len() && self.toks[pi] == Tok::Any {
                star = Some((pi, si));
                pi += 1;
            } else if let Some((sp, ss)) = star {
                pi = sp + 1;
                si = ss + 1;
                star = Some((sp, ss + 1));
            } else {
                return false;
            }
        }
        while pi < self.toks.len() && self.toks[pi] == Tok::Any {
            pi += 1;
        }
        pi == self.toks.len()
    }
}
```

- [ ] **Step 4: 实现 requirement（整文件替换）**

`crates/rurge-config/src/requirement.rs`：

```rust
//! Line requirement expressions (`#!REQUIREMENT`, `#!IOS-ONLY`, ...).

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::glob::{Glob, GlobOptions};
use crate::text::Profile;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct ReqError(pub String);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Environment {
    pub core_version: u64,
    pub system: String,
    pub system_version: String,
    pub device_model: String,
    pub language: String,
    pub device_name: String,
}

impl Environment {
    /// Deterministic environment for tests and snapshots.
    pub fn fixed() -> Self {
        Self {
            core_version: 20,
            system: "macOS".into(),
            system_version: "14.5".into(),
            device_model: "Mac15,6".into(),
            language: "zh-CN".into(),
            device_name: "test-mac".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Var {
    CoreVersion,
    System,
    SystemVersion,
    DeviceModel,
    Language,
    DeviceName,
}

impl Var {
    fn parse(ident: &str) -> Option<Var> {
        Some(match ident.to_ascii_uppercase().as_str() {
            "CORE_VERSION" => Var::CoreVersion,
            "SYSTEM" => Var::System,
            "SYSTEM_VERSION" => Var::SystemVersion,
            "DEVICE_MODEL" => Var::DeviceModel,
            "LANGUAGE" => Var::Language,
            "DEVICE_NAME" => Var::DeviceName,
            _ => return None,
        })
    }
    fn value(&self, env: &Environment) -> Value {
        match self {
            Var::CoreVersion => Value::Int(env.core_version as i64),
            Var::System => Value::Str(env.system.clone()),
            Var::SystemVersion => Value::Str(env.system_version.clone()),
            Var::DeviceModel => Value::Str(env.device_model.clone()),
            Var::Language => Value::Str(env.language.clone()),
            Var::DeviceName => Value::Str(env.device_name.clone()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrOp {
    BeginsWith,
    Contains,
    EndsWith,
    Like,
    Matches,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    Cmp(Var, CmpOp, Value),
    Str(Var, StrOp, String),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Num(i64),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
}

const OPS: [&str; 13] = ["==", "!=", "<>", ">=", "=>", "<=", "=<", "&&", "||", "=", ">", "<", "!"];

fn tokenize(src: &str) -> Result<Vec<Tok>, ReqError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            toks.push(Tok::LParen);
            i += 1;
        } else if c == ')' {
            toks.push(Tok::RParen);
            i += 1;
        } else if c == '\'' || c == '"' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != c {
                j += 1;
            }
            if j >= chars.len() {
                return Err(ReqError("unterminated string".into()));
            }
            toks.push(Tok::Str(chars[i + 1..j].iter().collect()));
            i = j + 1;
        } else if c.is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let n: String = chars[i..j].iter().collect();
            toks.push(Tok::Num(n.parse().map_err(|_| ReqError(format!("bad number `{n}`")))?));
            i = j;
        } else if c.is_alphabetic() || c == '_' {
            let mut j = i;
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            toks.push(Tok::Ident(chars[i..j].iter().collect()));
            i = j;
        } else {
            let rest: String = chars[i..].iter().take(2).collect();
            let op = OPS.iter().find(|op| rest.starts_with(*op)).ok_or_else(|| ReqError(format!("unexpected `{c}`")))?;
            toks.push(Tok::Op(op));
            i += op.len();
        }
    }
    Ok(toks)
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        self.pos += 1;
        t
    }
    fn peek_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(i)) if i.eq_ignore_ascii_case(kw))
    }
    fn or(&mut self) -> Result<Expr, ReqError> {
        let mut left = self.and()?;
        while self.peek() == Some(&Tok::Op("||")) || self.peek_keyword("OR") {
            self.next();
            let right = self.and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn and(&mut self) -> Result<Expr, ReqError> {
        let mut left = self.not()?;
        while self.peek() == Some(&Tok::Op("&&")) || self.peek_keyword("AND") {
            self.next();
            let right = self.not()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }
    fn not(&mut self) -> Result<Expr, ReqError> {
        if self.peek() == Some(&Tok::Op("!")) || self.peek_keyword("NOT") {
            self.next();
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.primary()
    }
    fn primary(&mut self) -> Result<Expr, ReqError> {
        match self.next() {
            Some(Tok::LParen) => {
                let e = self.or()?;
                if self.next() != Some(Tok::RParen) {
                    return Err(ReqError("expected `)`".into()));
                }
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                let var = Var::parse(&name).ok_or_else(|| ReqError(format!("unknown variable `{name}`")))?;
                match self.next() {
                    Some(Tok::Op(op)) => {
                        let op = match op {
                            "==" | "=" => CmpOp::Eq,
                            "!=" | "<>" => CmpOp::Ne,
                            ">" => CmpOp::Gt,
                            ">=" | "=>" => CmpOp::Ge,
                            "<" => CmpOp::Lt,
                            "<=" | "=<" => CmpOp::Le,
                            other => return Err(ReqError(format!("unexpected operator `{other}`"))),
                        };
                        let value = match self.next() {
                            Some(Tok::Num(n)) => Value::Int(n),
                            Some(Tok::Str(s)) | Some(Tok::Ident(s)) => Value::Str(s),
                            _ => return Err(ReqError("expected a value after comparison operator".into())),
                        };
                        Ok(Expr::Cmp(var, op, value))
                    }
                    Some(Tok::Ident(kw)) => {
                        let op = match kw.to_ascii_uppercase().as_str() {
                            "BEGINSWITH" => StrOp::BeginsWith,
                            "CONTAINS" => StrOp::Contains,
                            "ENDSWITH" => StrOp::EndsWith,
                            "LIKE" => StrOp::Like,
                            "MATCHES" => StrOp::Matches,
                            other => return Err(ReqError(format!("unknown operator `{other}`"))),
                        };
                        match self.next() {
                            Some(Tok::Str(s)) | Some(Tok::Ident(s)) => Ok(Expr::Str(var, op, s)),
                            Some(Tok::Num(n)) => Ok(Expr::Str(var, op, n.to_string())),
                            _ => Err(ReqError("expected a string after string operator".into())),
                        }
                    }
                    _ => Err(ReqError(format!("expected an operator after `{name}`"))),
                }
            }
            Some(t) => Err(ReqError(format!("unexpected token {t:?}"))),
            None => Err(ReqError("unexpected end of expression".into())),
        }
    }
}

pub fn parse(src: &str) -> Result<Expr, ReqError> {
    let toks = tokenize(src.trim())?;
    if toks.is_empty() {
        return Err(ReqError("empty expression".into()));
    }
    let mut p = Parser { toks, pos: 0 };
    let e = p.or()?;
    if p.pos != p.toks.len() {
        return Err(ReqError("trailing tokens in expression".into()));
    }
    Ok(e)
}

fn value_str(v: &Value) -> String {
    match v {
        Value::Int(n) => n.to_string(),
        Value::Str(s) => s.clone(),
    }
}

pub fn eval(expr: &Expr, env: &Environment) -> bool {
    match expr {
        Expr::And(a, b) => eval(a, env) && eval(b, env),
        Expr::Or(a, b) => eval(a, env) || eval(b, env),
        Expr::Not(a) => !eval(a, env),
        Expr::Cmp(var, op, rhs) => {
            let lhs = var.value(env);
            let ord = match (&lhs, rhs) {
                (Value::Int(a), Value::Int(b)) => a.cmp(b),
                (Value::Int(a), Value::Str(b)) => match b.trim().parse::<i64>() {
                    Ok(b) => a.cmp(&b),
                    Err(_) => return false,
                },
                (Value::Str(a), _) => a.as_str().cmp(value_str(rhs).as_str()),
            };
            match op {
                CmpOp::Eq => ord.is_eq(),
                CmpOp::Ne => ord.is_ne(),
                CmpOp::Gt => ord.is_gt(),
                CmpOp::Ge => ord.is_ge(),
                CmpOp::Lt => ord.is_lt(),
                CmpOp::Le => ord.is_le(),
            }
        }
        Expr::Str(var, op, rhs) => {
            let lhs = value_str(&var.value(env));
            match op {
                StrOp::BeginsWith => lhs.starts_with(rhs.as_str()),
                StrOp::Contains => lhs.contains(rhs.as_str()),
                StrOp::EndsWith => lhs.ends_with(rhs.as_str()),
                StrOp::Like => Glob::new(rhs, GlobOptions { case_insensitive: false, classes: false })
                    .map(|g| g.matches(&lhs))
                    .unwrap_or(false),
                StrOp::Matches => fancy_regex::Regex::new(rhs).map(|re| re.is_match(&lhs).unwrap_or(false)).unwrap_or(false),
            }
        }
    }
}

const SIMPLIFIED: [(&str, &str); 3] = [
    ("IOS-ONLY", "SYSTEM == 'iOS'"),
    ("MACOS-ONLY", "SYSTEM == 'macOS'"),
    ("TVOS-ONLY", "SYSTEM == 'tvOS'"),
];

/// Take the expression at the start of `rest`: a quoted string, or the first whitespace-delimited token.
fn take_expr(rest: &str) -> Result<(String, &str), ReqError> {
    let rest = rest.trim_start();
    if let Some(inner) = rest.strip_prefix('"') {
        let end = inner.find('"').ok_or_else(|| ReqError("unterminated quoted requirement".into()))?;
        return Ok((inner[..end].to_string(), &inner[end + 1..]));
    }
    if rest.is_empty() {
        return Err(ReqError("missing requirement expression".into()));
    }
    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    Ok((rest[..end].to_string(), &rest[end..]))
}

/// Find ` <marker>` (preceded by whitespace) searching from the end.
fn find_suffix_marker(line: &str, marker: &str) -> Option<usize> {
    let pos = line.rfind(marker)?;
    let before = &line[..pos];
    (before.ends_with(char::is_whitespace)).then_some(pos)
}

/// Split a profile line into (requirement expression source, content).
pub fn split_line(line: &str) -> Result<(Option<String>, String), ReqError> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix("#!REQUIREMENT") {
        let (expr, content) = take_expr(rest)?;
        return Ok((Some(expr), content.trim().to_string()));
    }
    for (tag, expr) in SIMPLIFIED {
        let prefix = format!("#!{tag}");
        if let Some(rest) = line.strip_prefix(&prefix) {
            if rest.is_empty() || rest.starts_with(char::is_whitespace) {
                return Ok((Some(expr.to_string()), rest.trim().to_string()));
            }
        }
    }
    for marker in ["#!REQUIREMENT", "//!REQUIREMENT"] {
        if let Some(pos) = find_suffix_marker(line, marker) {
            let expr = line[pos + marker.len()..].trim();
            if expr.is_empty() {
                return Err(ReqError("missing requirement expression".into()));
            }
            let expr = expr.strip_prefix('"').and_then(|e| e.strip_suffix('"')).unwrap_or(expr);
            return Ok((Some(expr.to_string()), line[..pos].trim().to_string()));
        }
    }
    for (tag, expr) in SIMPLIFIED {
        for prefix in ["#!", "//!"] {
            let marker = format!("{prefix}{tag}");
            if let Some(body) = line.strip_suffix(&marker) {
                if body.ends_with(char::is_whitespace) {
                    return Ok((Some(expr.to_string()), body.trim().to_string()));
                }
            }
        }
    }
    Ok((None, line.to_string()))
}

/// Evaluate every entry's requirement; unmet or invalid lines are disabled.
pub fn apply(profile: &mut Profile, env: &Environment, diags: &mut Diagnostics) {
    for section in &mut profile.sections {
        for entry in &mut section.entries {
            let Some(src) = &entry.requirement else { continue };
            match parse(src) {
                Ok(expr) => {
                    if !eval(&expr, env) {
                        entry.disabled = true;
                        diags.push(
                            Diagnostic::info(codes::I_LINE_DISABLED, format!("line disabled: requirement `{src}` not met"))
                                .at(entry.span.clone()),
                        );
                    }
                }
                Err(e) => {
                    entry.disabled = true;
                    diags.push(
                        Diagnostic::error(codes::E_REQUIREMENT_SYNTAX, format!("invalid requirement `{src}`: {e}"))
                            .at(entry.span.clone()),
                    );
                }
            }
        }
    }
}
```

`lib.rs` 增加 `pub mod glob;` 与 `pub use glob::{Glob, GlobOptions}; pub use requirement::Environment;`。Task 3 测试中 `requirement_directives_are_attached_to_entries` 期望值不变。

- [ ] **Step 5: 运行，确认通过**

Run: `cargo test -p rurge-config`
Expected: 全部通过（含 Task 3 的文本层测试）。

- [ ] **Step 6: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): Glob 匹配器与 Requirement 表达式解析、求值、行级指令"
```

---

### Task 6: `#!include` 展开（本地文件）

**Files:**
- Modify: `crates/rurge-config/src/text/include.rs`
- Modify: `crates/rurge-config/src/diagnostic.rs`（新增 `W_INCLUDE_SECTION_MISSING = "W0017"`）
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `Profile` / `Section` / `Entry` / `Origin` / `parse_str`（Task 3），`split_list`（Task 4）。
- Produces: `include::IncludeOptions { base_dir: PathBuf, max_depth: usize }`（`Default`：`base_dir = "."`，`max_depth = 8`），`include::expand(profile: &mut Profile, opts: &IncludeOptions, diags: &mut Diagnostics)`。

语义（清单 1.3）：
- `#!include a.dconf` / `#!include a.dconf, b.dconf`：把目标文件中**同名节**的条目在指令位置展开；可与普通内容混合，位置决定顺序。
- 相对路径相对 `base_dir`（主配置所在目录）；目标文件本身可含 `#!include`（递归，深度上限 8，循环 → E0016）。
- 节名为 `Ruleset *` / `WireGuard *` / `Tailscale *`（通配命名节）时，把目标文件中所有前缀匹配的命名节整体追加到 profile 末尾，并移除通配节本身。
- 目标为 `http://` / `https://` → W0011 跳过（M2 接入外部资源管理器）；文件不存在 → E0002；目标文件没有该节 → W0017。
- 展开的条目 `origin = Origin::Include(path)`，`span` 指向被包含文件的行。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/text/include.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::fs;
    use std::sync::Arc;

    fn load(dir: &std::path::Path, main: &str) -> (Profile, Diagnostics) {
        let path = dir.join(main);
        let text = fs::read_to_string(&path).unwrap();
        let (mut p, mut d) = parse_str(&text, Arc::from(path.as_path()), Origin::Main);
        expand(&mut p, &IncludeOptions { base_dir: dir.to_path_buf(), max_depth: 8 }, &mut d);
        (p, d)
    }

    #[test]
    fn single_multiple_and_mixed_includes() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.conf"), "[Proxy]\n#!include proxy.dconf\n[Rule]\n#!include a.dconf\nDEST-PORT,123,DIRECT\n#!include b.dconf, c.dconf\nFINAL,DIRECT\n").unwrap();
        fs::write(dir.path().join("proxy.dconf"), "[Proxy]\nP = direct\n[Rule]\nDOMAIN,ignored,DIRECT\n").unwrap();
        fs::write(dir.path().join("a.dconf"), "[Rule]\nDOMAIN,a,DIRECT\n").unwrap();
        fs::write(dir.path().join("b.dconf"), "[Rule]\nDOMAIN,b,DIRECT\n").unwrap();
        fs::write(dir.path().join("c.dconf"), "[Rule]\nDOMAIN,c,DIRECT\n").unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert_eq!(p.section("Proxy").unwrap().entries[0].raw, "P = direct");
        let rules: Vec<_> = p.section("Rule").unwrap().entries.iter().map(|e| e.raw.as_str()).collect();
        assert_eq!(rules, ["DOMAIN,a,DIRECT", "DEST-PORT,123,DIRECT", "DOMAIN,b,DIRECT", "DOMAIN,c,DIRECT", "FINAL,DIRECT"]);
        let e = &p.section("Rule").unwrap().entries[0];
        assert!(matches!(&e.origin, Origin::Include(path) if path.ends_with("a.dconf")));
        assert_eq!(e.span.line, 2);
    }

    #[test]
    fn wildcard_named_sections() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.conf"), "[Ruleset *]\n#!include shared.conf\n[Rule]\nRULE-SET,Streaming,DIRECT\nFINAL,DIRECT\n").unwrap();
        fs::write(dir.path().join("shared.conf"), "[Ruleset Streaming]\nDOMAIN-SUFFIX,netflix.com\n[Ruleset Music]\nDOMAIN-SUFFIX,spotify.com\n[General]\nipv6 = true\n").unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        assert!(p.section("Ruleset *").is_none());
        let names: Vec<_> = p.sections_with_prefix("Ruleset ").map(|s| s.name.clone()).collect();
        assert_eq!(names, ["Ruleset Streaming", "Ruleset Music"]);
        assert!(p.section("General").is_none());
    }

    #[test]
    fn errors_and_warnings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.conf"), "[Rule]\n#!include missing.dconf\n#!include https://example.com/x.conf\n#!include nosec.dconf\n#!include loop.dconf\nFINAL,DIRECT\n").unwrap();
        fs::write(dir.path().join("nosec.dconf"), "[Proxy]\nP = direct\n").unwrap();
        fs::write(dir.path().join("loop.dconf"), "[Rule]\n#!include loop.dconf\n").unwrap();
        let (p, d) = load(dir.path(), "main.conf");
        let codes_seen: Vec<_> = d.iter().map(|x| x.code).collect();
        assert!(codes_seen.contains(&codes::E_INCLUDE_NOT_FOUND));
        assert!(codes_seen.contains(&codes::W_REMOTE_INCLUDE_UNSUPPORTED));
        assert!(codes_seen.contains(&codes::W_INCLUDE_SECTION_MISSING));
        assert!(codes_seen.contains(&codes::E_INCLUDE_CYCLE));
        assert_eq!(p.section("Rule").unwrap().entries.last().unwrap().raw, "FINAL,DIRECT");
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config include`
Expected: 编译失败（`expand` / `IncludeOptions` 不存在）。

- [ ] **Step 3: 实现**

`diagnostic.rs` 的 `codes` 模块追加：

```rust
    pub const W_INCLUDE_SECTION_MISSING: &str = "W0017";
```

`crates/rurge-config/src/text/include.rs`：

```rust
//! `#!include` expansion for local files. Remote includes are handled by the
//! resource manager in a later milestone; here they only produce a warning.

use super::{Entry, Origin, Profile, Section, parse_str};
use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::value::split_list;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct IncludeOptions {
    pub base_dir: PathBuf,
    pub max_depth: usize,
}

impl Default for IncludeOptions {
    fn default() -> Self {
        Self { base_dir: PathBuf::from("."), max_depth: 8 }
    }
}

fn is_url(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

fn wildcard_prefix(section_name: &str) -> Option<&str> {
    let trimmed = section_name.trim_end();
    let prefix = trimmed.strip_suffix('*')?;
    (prefix.ends_with(' ')).then_some(prefix)
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

pub fn expand(profile: &mut Profile, opts: &IncludeOptions, diags: &mut Diagnostics) {
    let mut stack: Vec<PathBuf> = Vec::new();
    if let Some(main) = &profile.main {
        stack.push(canonical(main));
    }
    expand_inner(profile, opts, diags, &mut stack, 0);
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn expand_inner(profile: &mut Profile, opts: &IncludeOptions, diags: &mut Diagnostics, stack: &mut Vec<PathBuf>, depth: usize) {
    let mut appended: Vec<Section> = Vec::new();
    let mut remove_wildcards: Vec<usize> = Vec::new();

    for (idx, section) in profile.sections.iter_mut().enumerate() {
        let wildcard = wildcard_prefix(&section.name).map(str::to_string);
        let mut new_entries: Vec<Entry> = Vec::new();
        for entry in std::mem::take(&mut section.entries) {
            let Some(rest) = entry.raw.strip_prefix("#!include") else {
                new_entries.push(entry);
                continue;
            };
            for target in split_list(rest) {
                if is_url(&target) {
                    diags.push(
                        Diagnostic::warning(codes::W_REMOTE_INCLUDE_UNSUPPORTED, format!("remote include not supported yet: {target}"))
                            .at(entry.span.clone()),
                    );
                    continue;
                }
                let path = if Path::new(&target).is_absolute() { PathBuf::from(&target) } else { opts.base_dir.join(&target) };
                let canon = canonical(&path);
                if stack.contains(&canon) {
                    diags.push(Diagnostic::error(codes::E_INCLUDE_CYCLE, format!("include cycle: {}", path.display())).at(entry.span.clone()));
                    continue;
                }
                if depth >= opts.max_depth {
                    diags.push(
                        Diagnostic::error(codes::E_INCLUDE_CYCLE, format!("include nesting deeper than {}: {}", opts.max_depth, path.display()))
                            .at(entry.span.clone()),
                    );
                    continue;
                }
                let text = match fs::read_to_string(&path) {
                    Ok(t) => t,
                    Err(e) => {
                        diags.push(
                            Diagnostic::error(codes::E_INCLUDE_NOT_FOUND, format!("cannot read include `{}`: {e}", path.display()))
                                .at(entry.span.clone()),
                        );
                        continue;
                    }
                };
                let file: Arc<Path> = Arc::from(path.as_path());
                let (mut sub, sub_diags) = parse_str(&text, file.clone(), Origin::Include(file.clone()));
                diags.extend(sub_diags);
                stack.push(canon);
                expand_inner(&mut sub, opts, diags, stack, depth + 1);
                stack.pop();

                if let Some(prefix) = &wildcard {
                    let (matched, _): (Vec<Section>, Vec<Section>) =
                        sub.sections.into_iter().partition(|s| starts_with_ci(&s.name, prefix));
                    appended.extend(matched);
                } else if let Some(s) = sub.section_mut(&section.name) {
                    new_entries.append(&mut s.entries);
                } else {
                    diags.push(
                        Diagnostic::warning(codes::W_INCLUDE_SECTION_MISSING, format!("`{}` has no [{}] section", path.display(), section.name))
                            .at(entry.span.clone()),
                    );
                }
            }
        }
        section.entries = new_entries;
        if wildcard.is_some() {
            remove_wildcards.push(idx);
        }
    }

    for idx in remove_wildcards.into_iter().rev() {
        profile.sections.remove(idx);
    }
    profile.sections.extend(appended);
}
```

`lib.rs` 增加 `pub use text::include::IncludeOptions;`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config include`
Expected: 3 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): 本地 #!include 展开：单文件、多文件、混合、通配命名节、循环检测"
```

---

### Task 7: HostName 与 Host List 参数类型

**Files:**
- Create: `crates/rurge-config/src/types.rs`
- Create: `crates/rurge-config/src/hostlist.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `Glob`（Task 5）、`ipnet::IpNet`。
- Produces:
  - `types::HostName { Domain(String), Ip(IpAddr) }`，`HostName::parse(&str) -> HostName`（IP 字面量 → `Ip`；否则小写、去尾部 `.` 的 `Domain`），`HostName::is_simple()`（无 `.` 的域名），`Display`。
  - `hostlist::HostPattern { Glob(Glob), Cidr(IpNet), AnyIp, AnyV4, AnyV6, SimpleHostname }`
  - `hostlist::PortSpec { Default, Any, Port(u16) }`
  - `hostlist::HostListEntry { negate: bool, pattern: HostPattern, port: PortSpec, raw: String }`
  - `hostlist::HostList { entries, default_port: Option<u16>, invalid: Vec<String> }`，`HostList::parse(text: &str, default_port: Option<u16>) -> HostList`，`HostList::matches(&self, host: &HostName, port: u16) -> Option<bool>`（`Some(true)` 命中、`Some(false)` 被 `-` 排除、`None` 无条目命中），`HostList::is_empty()`，`HostList::empty()`。

条目语法（清单 1.6）：`-` 前缀排除；`*` `?` 通配（不区分大小写，无字符类）；`host:port`、`host:0`（全端口）、无端口 = 参数默认端口（`default_port = None` 时不限端口）；IPv6 带端口写 `[v6]:port`；含 `/` 为 CIDR；`<ip-address>` `<ipv4-address>` `<ipv6-address>` `<simple-hostname>` 特殊记号（可带 `:port`）；`*:0` 全部主机全部端口。非法项进入 `invalid`，由调用方报 W0005。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/hostlist.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::HostName;

    fn h(s: &str) -> HostName {
        HostName::parse(s)
    }

    #[test]
    fn force_http_engine_hosts_examples() {
        let list = HostList::parse("-*.apple.com, www.google.com, www.google.com:8080, api.example.com:0, *:0", Some(80));
        assert!(list.invalid.is_empty());
        assert_eq!(list.matches(&h("x.apple.com"), 80), Some(false));
        assert_eq!(list.matches(&h("www.google.com"), 80), Some(true));
        assert_eq!(list.matches(&h("www.google.com"), 8080), Some(true));
        assert_eq!(list.matches(&h("www.google.com"), 443), Some(true)); // *:0 catches it
        assert_eq!(list.matches(&h("api.example.com"), 12345), Some(true));
        let narrow = HostList::parse("www.google.com", Some(80));
        assert_eq!(narrow.matches(&h("www.google.com"), 443), None);
        assert_eq!(narrow.matches(&h("WWW.GOOGLE.COM"), 80), Some(true));
    }

    #[test]
    fn mitm_hostname_example_and_special_tokens() {
        let list = HostList::parse("-*icloud*, -*.mzstatic.com, -<ip-address>, *", Some(443));
        assert_eq!(list.matches(&h("gateway.icloud.com"), 443), Some(false));
        assert_eq!(list.matches(&h("a.mzstatic.com"), 443), Some(false));
        assert_eq!(list.matches(&h("1.2.3.4"), 443), Some(false));
        assert_eq!(list.matches(&h("example.com"), 443), Some(true));
        assert_eq!(list.matches(&h("example.com"), 8443), None);
        let v4 = HostList::parse("<ipv4-address>, <simple-hostname>", None);
        assert_eq!(v4.matches(&h("10.0.0.1"), 1), Some(true));
        assert_eq!(v4.matches(&h("::1"), 1), None);
        assert_eq!(v4.matches(&h("nas"), 1), Some(true));
        assert_eq!(v4.matches(&h("nas.lan"), 1), None);
    }

    #[test]
    fn skip_proxy_with_cidr_and_ipv6() {
        let list = HostList::parse("127.0.0.1, 192.168.0.0/16, 10.0.0.0/8, localhost, *.local, [::1]:0, fe80::/10", None);
        assert!(list.invalid.is_empty(), "{:?}", list.invalid);
        assert_eq!(list.matches(&h("127.0.0.1"), 80), Some(true));
        assert_eq!(list.matches(&h("192.168.1.9"), 80), Some(true));
        assert_eq!(list.matches(&h("172.16.0.1"), 80), None);
        assert_eq!(list.matches(&h("localhost"), 80), Some(true));
        assert_eq!(list.matches(&h("printer.local"), 80), Some(true));
        assert_eq!(list.matches(&h("::1"), 80), Some(true));
        assert_eq!(list.matches(&h("fe80::1"), 80), Some(true));
    }

    #[test]
    fn invalid_entries_are_collected() {
        let list = HostList::parse("ok.com, -, :80, 10.0.0.0/99, [::1", Some(80));
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.invalid, ["-", ":80", "10.0.0.0/99", "[::1"]);
    }

    #[test]
    fn hostname_parse() {
        assert_eq!(h("Example.COM."), HostName::Domain("example.com".into()));
        assert!(matches!(h("1.2.3.4"), HostName::Ip(std::net::IpAddr::V4(_))));
        assert!(h("nas").is_simple());
        assert!(!h("nas.lan").is_simple());
        assert_eq!(h("::1").to_string(), "::1");
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config hostlist`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/types.rs`：

```rust
//! Small value types shared by every layer.

use std::fmt;
use std::net::IpAddr;

/// A connection target: a domain name (lowercase, no trailing dot) or an IP literal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HostName {
    Domain(String),
    Ip(IpAddr),
}

impl HostName {
    pub fn parse(s: &str) -> HostName {
        let s = s.trim();
        let bare = s.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(s);
        if let Ok(ip) = bare.parse::<IpAddr>() {
            return HostName::Ip(ip);
        }
        HostName::Domain(s.trim_end_matches('.').to_ascii_lowercase())
    }
    /// A hostname without a dot, such as `localhost` or `nas`.
    pub fn is_simple(&self) -> bool {
        matches!(self, HostName::Domain(d) if !d.contains('.'))
    }
    pub fn as_domain(&self) -> Option<&str> {
        match self {
            HostName::Domain(d) => Some(d),
            HostName::Ip(_) => None,
        }
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HostName::Domain(d) => f.write_str(d),
            HostName::Ip(ip) => write!(f, "{ip}"),
        }
    }
}
```

`crates/rurge-config/src/hostlist.rs`：

```rust
//! The "Host List" parameter type shared by skip-proxy, always-real-ip,
//! force-http-engine-hosts, always-raw-tcp-hosts and MITM hostname.

use crate::glob::{Glob, GlobOptions};
use crate::types::HostName;
use ipnet::IpNet;
use std::net::IpAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostPattern {
    Glob(Glob),
    Cidr(IpNet),
    AnyIp,
    AnyV4,
    AnyV6,
    SimpleHostname,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortSpec {
    Default,
    Any,
    Port(u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostListEntry {
    pub negate: bool,
    pub pattern: HostPattern,
    pub port: PortSpec,
    pub raw: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostList {
    pub entries: Vec<HostListEntry>,
    pub default_port: Option<u16>,
    pub invalid: Vec<String>,
}

impl HostList {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn parse(text: &str, default_port: Option<u16>) -> HostList {
        let mut list = HostList { entries: Vec::new(), default_port, invalid: Vec::new() };
        for raw in text.split(',') {
            let raw = raw.trim();
            if raw.is_empty() {
                continue;
            }
            match parse_entry(raw) {
                Some(e) => list.entries.push(e),
                None => list.invalid.push(raw.to_string()),
            }
        }
        list
    }

    /// First entry whose host and port both match decides: `Some(!negate)`.
    pub fn matches(&self, host: &HostName, port: u16) -> Option<bool> {
        for e in &self.entries {
            let port_ok = match e.port {
                PortSpec::Any => true,
                PortSpec::Port(p) => p == port,
                PortSpec::Default => self.default_port.is_none_or(|d| d == port),
            };
            if port_ok && pattern_matches(&e.pattern, host) {
                return Some(!e.negate);
            }
        }
        None
    }
}

fn pattern_matches(p: &HostPattern, host: &HostName) -> bool {
    match (p, host) {
        (HostPattern::Glob(g), h) => g.matches(&h.to_string()),
        (HostPattern::Cidr(net), HostName::Ip(ip)) => net.contains(ip),
        (HostPattern::AnyIp, HostName::Ip(_)) => true,
        (HostPattern::AnyV4, HostName::Ip(IpAddr::V4(_))) => true,
        (HostPattern::AnyV6, HostName::Ip(IpAddr::V6(_))) => true,
        (HostPattern::SimpleHostname, h) => h.is_simple(),
        _ => false,
    }
}

/// Split `host[:port]`, handling `[v6]:port` and bare IPv6.
fn split_host_port(s: &str) -> Option<(&str, PortSpec)> {
    if let Some(rest) = s.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        return match after.strip_prefix(':') {
            None if after.is_empty() => Some((host, PortSpec::Default)),
            Some(p) => Some((host, parse_port(p)?)),
            _ => None,
        };
    }
    if s.matches(':').count() >= 2 {
        return Some((s, PortSpec::Default)); // bare IPv6 literal
    }
    match s.rsplit_once(':') {
        Some((host, p)) => Some((host, parse_port(p)?)),
        None => Some((s, PortSpec::Default)),
    }
}

fn parse_port(p: &str) -> Option<PortSpec> {
    let n: u16 = p.parse().ok()?;
    Some(if n == 0 { PortSpec::Any } else { PortSpec::Port(n) })
}

fn parse_entry(raw: &str) -> Option<HostListEntry> {
    let (negate, body) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest.trim()),
        None => (false, raw),
    };
    if body.is_empty() {
        return None;
    }
    let lower = body.to_ascii_lowercase();
    for (token, pattern) in [
        ("<ip-address>", HostPattern::AnyIp),
        ("<ipv4-address>", HostPattern::AnyV4),
        ("<ipv6-address>", HostPattern::AnyV6),
        ("<simple-hostname>", HostPattern::SimpleHostname),
    ] {
        if let Some(rest) = lower.strip_prefix(token) {
            let port = match rest.strip_prefix(':') {
                None if rest.is_empty() => PortSpec::Default,
                Some(p) => parse_port(p)?,
                _ => return None,
            };
            return Some(HostListEntry { negate, pattern, port, raw: raw.to_string() });
        }
    }
    if body.contains('/') {
        let net: IpNet = body.parse().ok()?;
        return Some(HostListEntry { negate, pattern: HostPattern::Cidr(net), port: PortSpec::Default, raw: raw.to_string() });
    }
    let (host, port) = split_host_port(body)?;
    if host.is_empty() {
        return None;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(HostListEntry { negate, pattern: HostPattern::Cidr(IpNet::from(ip)), port, raw: raw.to_string() });
    }
    let glob = Glob::new(host, GlobOptions { case_insensitive: true, classes: false }).ok()?;
    Some(HostListEntry { negate, pattern: HostPattern::Glob(glob), port, raw: raw.to_string() })
}
```

`lib.rs` 增加 `pub mod hostlist; pub mod types;` 与 `pub use hostlist::HostList; pub use types::HostName;`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config hostlist`
Expected: 5 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): HostName 与 Host List 参数类型（排除、通配、端口、特殊记号、CIDR）"
```

---

### Task 8: `[General]` 强类型解析与旧键迁移

**Files:**
- Create: `crates/rurge-config/src/general.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `Section` / `Entry`（Task 3），`split_definition` / `split_list` / `parse_bool`（Task 4），`HostList`（Task 7），`Diagnostics` / `codes`（Task 2）。
- Produces: `general::General`（字段表如下）、`general::parse_general(section: Option<&Section>, diags: &mut Diagnostics) -> General` 与各值类型。

字段表（键 → 字段 → 类型 → 默认；平台列为 Surge 标注，rurge 对 iOS 专属键解析后报 W0004）：

| 键 | 字段 | 类型 / 默认 |
| --- | --- | --- |
| loglevel | `loglevel` | `LogLevel { Verbose, Info, Notify, Warning }` / Notify |
| debug-cpu-usage, debug-memory-usage | `debug_cpu_usage`, `debug_memory_usage` | bool / false |
| dns-server | `dns_server: Vec<DnsServer { System, Udp(SocketAddr) }>` | 空；默认端口 53；出现加密 URL（含 `tcp://`）时移入 `encrypted_dns_server` |
| encrypted-dns-server | `encrypted_dns_server: Vec<EncryptedDns { scheme: EncryptedDnsScheme { Https, H3, Quic, Tls, Tcp }, url: String }>` | 空 |
| encrypted-dns-follow-outbound-mode, encrypted-dns-skip-cert-verification, allow-dns-svcb, use-local-host-item-for-proxy | 同名 snake_case bool | false |
| hijack-dns | `hijack_dns: Vec<HijackTarget { addr: Option<Ipv4Addr>, port: u16 }>` | 空；`*` → addr None；默认端口 53 |
| always-real-ip | `always_real_ip: HostList` | 空（default_port None） |
| geoip-maxmind-url | `geoip_maxmind_url: Option<String>` | None |
| disable-geoip-db-auto-update | bool | false |
| ipv6 | bool | false |
| ipv6-vif | `Ipv6Vif { Disabled, Auto, Always }` | Disabled；旧值 `off` → Disabled（I0001） |
| tun-excluded-routes, tun-included-routes | `Vec<IpNet>` | 空 |
| icmp-forwarding | bool | true |
| skip-proxy | `HostList` | 空 |
| exclude-simple-hostnames | bool | false |
| proxy-restricted-to-lan, gateway-restricted-to-lan | bool | true |
| external-controller-access, http-api | `Option<ControllerAccess { key: String, addr: SocketAddr }>` | None |
| http-api-tls, http-api-web-dashboard | bool | false |
| internet-test-url, proxy-test-url | `String` | `http://bing.com/` |
| test-timeout | `Duration` | 5s |
| proxy-test-udp | `Option<UdpTest { hostname: String, server: Ipv4Addr }>` | None |
| force-http-engine-hosts | `HostList`（default_port 80） | 空 |
| always-raw-tcp-hosts | `HostList` | 空 |
| always-raw-tcp-keywords | `Vec<String>` | 空 |
| udp-policy-not-supported-behaviour | `UdpFallback { Reject, Direct }` | Reject |
| udp-priority | bool | true |
| block-quic | `BlockQuicGlobal { PerPolicy, AllProxy, All, AlwaysAllow }` | PerPolicy |
| show-error-page | bool | true |
| show-error-page-for-reject | bool | false |
| compatibility-mode | `u8` | 0（iOS） |
| auto-suspend | bool | true（iOS） |
| allow-wifi-access, allow-hotspot-access, wifi-assist, all-hybrid, hide-vpn-icon, include-all-networks, include-local-networks, include-apns, include-cellular-services | bool | false（iOS） |
| wifi-access-http-port, wifi-access-socks5-port | `u16` | 6152 / 6153（iOS） |
| wifi-access-http-auth | `Option<(String, String)>` | None（iOS） |
| http-listen | `Vec<Listener { password: Option<String>, addr: SocketAddr }>` | 空；默认端口 6152；地址必须是 IP 字面量（E0013） |
| socks5-listen | `Vec<Listener>` | 空；默认端口 6153；带密码 → W0012 并丢弃密码 |
| set-system-socks-proxy, read-etc-hosts, subnet-exp-wifi-always-match | bool | true |
| 旧键 doh-server / doh-follow-outbound-mode / doh-skip-cert-verification | 迁移到 encrypted-dns-* | I0001 |
| 旧键 interface + port / socks-interface + socks-port | 迁移到 http-listen / socks5-listen（interface 默认 `127.0.0.1`，port 默认 6152 / 6153） | I0001 |
| 旧键 use-default-policy-if-wifi-not-primary | `subnet_exp_wifi_always_match = !value` | I0001 |
| 已消失的键 vif-mode, tls-provider, network-framework, bypass-system, bypass-tun, enhanced-mode-by-rule, allow-udp-proxy | 忽略 | W0006 |
| 其他 | `unknown: Vec<UnknownKey { key, value, span }>` | W0001 |

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/general.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    fn parse(text: &str) -> (General, Diagnostics) {
        let (p, mut d) = parse_str(text, Arc::from(Path::new("g.conf")), Origin::Main);
        let g = parse_general(p.section("General"), &mut d);
        (g, d)
    }

    fn codes_of(d: &Diagnostics) -> Vec<&'static str> {
        d.iter().map(|x| x.code).collect()
    }

    #[test]
    fn typical_section() {
        let (g, d) = parse(
            "[General]\nloglevel = warning\ndns-server = system, 8.8.8.8, 192.0.2.53:5353, https://doh.example/dns-query\nencrypted-dns-server = tls://1.1.1.1, tcp://dns.example.com\nhijack-dns = 8.8.8.8:53, *\nskip-proxy = 127.0.0.1, 192.168.0.0/16, localhost, *.local\nexclude-simple-hostnames = true\nhttp-api = key@0.0.0.0:6171\nhttp-listen = 0.0.0.0:6152, pw@127.0.0.1:7000, [::1]:6152\nsocks5-listen = 127.0.0.1:6153\ntest-timeout = 8\nproxy-test-udp = apple.com@8.8.8.8\nblock-quic = all-proxy\nudp-policy-not-supported-behaviour = DIRECT\nipv6-vif = auto\ntun-excluded-routes = 192.168.0.0/16, 10.0.0.0/8\nforce-http-engine-hosts = *.example.com, api.test:8080\nalways-raw-tcp-keywords = kw1, kw2\n",
        );
        assert!(!d.has_errors(), "{:?}", d.into_vec());
        assert_eq!(g.loglevel, LogLevel::Warning);
        assert_eq!(g.dns_server, vec![DnsServer::System, DnsServer::Udp("8.8.8.8:53".parse().unwrap()), DnsServer::Udp("192.0.2.53:5353".parse().unwrap())]);
        assert_eq!(g.encrypted_dns_server.len(), 3);
        assert_eq!(g.encrypted_dns_server[0].scheme, EncryptedDnsScheme::Https);
        assert_eq!(g.encrypted_dns_server[1].scheme, EncryptedDnsScheme::Tls);
        assert_eq!(g.encrypted_dns_server[2].scheme, EncryptedDnsScheme::Tcp);
        assert_eq!(g.hijack_dns, vec![HijackTarget { addr: Some("8.8.8.8".parse().unwrap()), port: 53 }, HijackTarget { addr: None, port: 53 }]);
        assert_eq!(g.skip_proxy.entries.len(), 4);
        assert!(g.exclude_simple_hostnames);
        assert_eq!(g.http_api.as_ref().unwrap().key, "key");
        assert_eq!(g.http_api.as_ref().unwrap().addr, "0.0.0.0:6171".parse().unwrap());
        assert_eq!(g.http_listen.len(), 3);
        assert_eq!(g.http_listen[1].password.as_deref(), Some("pw"));
        assert_eq!(g.http_listen[2].addr, "[::1]:6152".parse().unwrap());
        assert_eq!(g.test_timeout, Duration::from_secs(8));
        assert_eq!(g.proxy_test_udp.as_ref().unwrap().hostname, "apple.com");
        assert_eq!(g.block_quic, BlockQuicGlobal::AllProxy);
        assert_eq!(g.udp_policy_not_supported_behaviour, UdpFallback::Direct);
        assert_eq!(g.ipv6_vif, Ipv6Vif::Auto);
        assert_eq!(g.tun_excluded_routes.len(), 2);
        assert_eq!(g.force_http_engine_hosts.default_port, Some(80));
        assert_eq!(g.always_raw_tcp_keywords, ["kw1", "kw2"]);
    }

    #[test]
    fn defaults_when_section_missing() {
        let (g, d) = parse("[Rule]\nFINAL,DIRECT\n");
        assert!(d.is_empty());
        assert_eq!(g.loglevel, LogLevel::Notify);
        assert_eq!(g.test_timeout, Duration::from_secs(5));
        assert_eq!(g.internet_test_url, "http://bing.com/");
        assert!(g.icmp_forwarding);
        assert!(g.proxy_restricted_to_lan);
        assert_eq!(g.udp_policy_not_supported_behaviour, UdpFallback::Reject);
        assert!(g.http_listen.is_empty());
    }

    #[test]
    fn legacy_keys_migrate() {
        let (g, d) = parse("[General]\ndoh-server = https://a/dns-query\ndoh-follow-outbound-mode = true\ndoh-skip-cert-verification = true\ninterface = 0.0.0.0\nport = 6152\nsocks-interface = 127.0.0.1\nsocks-port = 6153\nuse-default-policy-if-wifi-not-primary = true\nipv6-vif = off\nvif-mode = v2\n");
        assert!(!d.has_errors());
        assert_eq!(g.encrypted_dns_server[0].url, "https://a/dns-query");
        assert!(g.encrypted_dns_follow_outbound_mode);
        assert!(g.encrypted_dns_skip_cert_verification);
        assert_eq!(g.http_listen[0].addr, "0.0.0.0:6152".parse().unwrap());
        assert_eq!(g.socks5_listen[0].addr, "127.0.0.1:6153".parse().unwrap());
        assert!(!g.subnet_exp_wifi_always_match);
        assert_eq!(g.ipv6_vif, Ipv6Vif::Disabled);
        let c = codes_of(&d);
        assert_eq!(c.iter().filter(|x| **x == codes::I_LEGACY_MIGRATED).count(), 7);
        assert!(c.contains(&codes::W_VANISHED_KEY));
    }

    #[test]
    fn invalid_values_unknown_keys_and_platform_keys() {
        let (g, d) = parse("[General]\nipv6 = maybe\nloglevel = loud\nhttp-listen = example.com:6152\nsocks5-listen = pw@127.0.0.1:6153\nhttp-api = 0.0.0.0:6171\nmystery-key = 1\ncompatibility-mode = 3\nallow-wifi-access = true\nwifi-access-http-auth = user:pass\n");
        assert!(!g.ipv6);
        assert_eq!(g.loglevel, LogLevel::Notify);
        assert!(g.http_listen.is_empty());
        assert_eq!(g.socks5_listen.len(), 1);
        assert_eq!(g.socks5_listen[0].password, None);
        assert!(g.http_api.is_none());
        assert_eq!(g.unknown.len(), 1);
        assert_eq!(g.unknown[0].key, "mystery-key");
        assert_eq!(g.compatibility_mode, 3);
        assert!(g.allow_wifi_access);
        assert_eq!(g.wifi_access_http_auth, Some(("user".into(), "pass".into())));
        let c = codes_of(&d);
        assert!(c.contains(&codes::W_INVALID_VALUE));
        assert!(c.contains(&codes::E_LISTENER_NOT_IP));
        assert!(c.contains(&codes::W_UNKNOWN_KEY));
        assert_eq!(c.iter().filter(|x| **x == codes::W_PLATFORM_IGNORED).count(), 3);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config general`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/general.rs`：

```rust
//! Typed view of the `[General]` section.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::hostlist::HostList;
use crate::span::Span;
use crate::text::Section;
use crate::value::{parse_bool, split_definition, split_list};
use ipnet::IpNet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogLevel {
    Verbose,
    Info,
    #[default]
    Notify,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsServer {
    System,
    Udp(SocketAddr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncryptedDnsScheme {
    Https,
    H3,
    Quic,
    Tls,
    Tcp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedDns {
    pub scheme: EncryptedDnsScheme,
    pub url: String,
}

impl EncryptedDns {
    /// Recognise `https://`, `h3://`, `quic://`, `tls://`, `tcp://`.
    pub fn parse(s: &str) -> Option<EncryptedDns> {
        let lower = s.to_ascii_lowercase();
        let scheme = if lower.starts_with("https://") {
            EncryptedDnsScheme::Https
        } else if lower.starts_with("h3://") {
            EncryptedDnsScheme::H3
        } else if lower.starts_with("quic://") {
            EncryptedDnsScheme::Quic
        } else if lower.starts_with("tls://") {
            EncryptedDnsScheme::Tls
        } else if lower.starts_with("tcp://") {
            EncryptedDnsScheme::Tcp
        } else {
            return None;
        };
        Some(EncryptedDns { scheme, url: s.to_string() })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HijackTarget {
    /// `None` means `*` (any destination address).
    pub addr: Option<Ipv4Addr>,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ipv6Vif {
    #[default]
    Disabled,
    Auto,
    Always,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerAccess {
    pub key: String,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpTest {
    pub hostname: String,
    pub server: Ipv4Addr,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UdpFallback {
    #[default]
    Reject,
    Direct,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlockQuicGlobal {
    #[default]
    PerPolicy,
    AllProxy,
    All,
    AlwaysAllow,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listener {
    pub password: Option<String>,
    pub addr: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownKey {
    pub key: String,
    pub value: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct General {
    pub loglevel: LogLevel,
    pub debug_cpu_usage: bool,
    pub debug_memory_usage: bool,
    pub dns_server: Vec<DnsServer>,
    pub encrypted_dns_server: Vec<EncryptedDns>,
    pub encrypted_dns_follow_outbound_mode: bool,
    pub encrypted_dns_skip_cert_verification: bool,
    pub allow_dns_svcb: bool,
    pub use_local_host_item_for_proxy: bool,
    pub hijack_dns: Vec<HijackTarget>,
    pub always_real_ip: HostList,
    pub geoip_maxmind_url: Option<String>,
    pub disable_geoip_db_auto_update: bool,
    pub ipv6: bool,
    pub ipv6_vif: Ipv6Vif,
    pub tun_excluded_routes: Vec<IpNet>,
    pub tun_included_routes: Vec<IpNet>,
    pub icmp_forwarding: bool,
    pub skip_proxy: HostList,
    pub exclude_simple_hostnames: bool,
    pub proxy_restricted_to_lan: bool,
    pub gateway_restricted_to_lan: bool,
    pub external_controller_access: Option<ControllerAccess>,
    pub http_api: Option<ControllerAccess>,
    pub http_api_tls: bool,
    pub http_api_web_dashboard: bool,
    pub internet_test_url: String,
    pub proxy_test_url: String,
    pub test_timeout: Duration,
    pub proxy_test_udp: Option<UdpTest>,
    pub force_http_engine_hosts: HostList,
    pub always_raw_tcp_hosts: HostList,
    pub always_raw_tcp_keywords: Vec<String>,
    pub udp_policy_not_supported_behaviour: UdpFallback,
    pub udp_priority: bool,
    pub block_quic: BlockQuicGlobal,
    pub show_error_page: bool,
    pub show_error_page_for_reject: bool,
    // iOS only (parsed, ignored at runtime on desktop)
    pub compatibility_mode: u8,
    pub auto_suspend: bool,
    pub allow_wifi_access: bool,
    pub allow_hotspot_access: bool,
    pub wifi_access_http_port: u16,
    pub wifi_access_socks5_port: u16,
    pub wifi_access_http_auth: Option<(String, String)>,
    pub wifi_assist: bool,
    pub all_hybrid: bool,
    pub hide_vpn_icon: bool,
    pub include_all_networks: bool,
    pub include_local_networks: bool,
    pub include_apns: bool,
    pub include_cellular_services: bool,
    // macOS only in Surge; supported everywhere in rurge
    pub http_listen: Vec<Listener>,
    pub socks5_listen: Vec<Listener>,
    pub set_system_socks_proxy: bool,
    pub read_etc_hosts: bool,
    pub subnet_exp_wifi_always_match: bool,
    pub unknown: Vec<UnknownKey>,
}

impl Default for General {
    fn default() -> Self {
        Self {
            loglevel: LogLevel::Notify,
            debug_cpu_usage: false,
            debug_memory_usage: false,
            dns_server: Vec::new(),
            encrypted_dns_server: Vec::new(),
            encrypted_dns_follow_outbound_mode: false,
            encrypted_dns_skip_cert_verification: false,
            allow_dns_svcb: false,
            use_local_host_item_for_proxy: false,
            hijack_dns: Vec::new(),
            always_real_ip: HostList::empty(),
            geoip_maxmind_url: None,
            disable_geoip_db_auto_update: false,
            ipv6: false,
            ipv6_vif: Ipv6Vif::Disabled,
            tun_excluded_routes: Vec::new(),
            tun_included_routes: Vec::new(),
            icmp_forwarding: true,
            skip_proxy: HostList::empty(),
            exclude_simple_hostnames: false,
            proxy_restricted_to_lan: true,
            gateway_restricted_to_lan: true,
            external_controller_access: None,
            http_api: None,
            http_api_tls: false,
            http_api_web_dashboard: false,
            internet_test_url: "http://bing.com/".into(),
            proxy_test_url: "http://bing.com/".into(),
            test_timeout: Duration::from_secs(5),
            proxy_test_udp: None,
            force_http_engine_hosts: HostList { default_port: Some(80), ..HostList::empty() },
            always_raw_tcp_hosts: HostList::empty(),
            always_raw_tcp_keywords: Vec::new(),
            udp_policy_not_supported_behaviour: UdpFallback::Reject,
            udp_priority: true,
            block_quic: BlockQuicGlobal::PerPolicy,
            show_error_page: true,
            show_error_page_for_reject: false,
            compatibility_mode: 0,
            auto_suspend: true,
            allow_wifi_access: false,
            allow_hotspot_access: false,
            wifi_access_http_port: 6152,
            wifi_access_socks5_port: 6153,
            wifi_access_http_auth: None,
            wifi_assist: false,
            all_hybrid: false,
            hide_vpn_icon: false,
            include_all_networks: false,
            include_local_networks: false,
            include_apns: false,
            include_cellular_services: false,
            http_listen: Vec::new(),
            socks5_listen: Vec::new(),
            set_system_socks_proxy: true,
            read_etc_hosts: true,
            subnet_exp_wifi_always_match: true,
            unknown: Vec::new(),
        }
    }
}

const IOS_ONLY_KEYS: &[&str] = &[
    "compatibility-mode", "auto-suspend", "allow-wifi-access", "allow-hotspot-access", "wifi-access-http-port",
    "wifi-access-socks5-port", "wifi-access-http-auth", "wifi-assist", "all-hybrid", "hide-vpn-icon",
    "include-all-networks", "include-local-networks", "include-apns", "include-cellular-services",
];

const VANISHED_KEYS: &[&str] = &[
    "vif-mode", "tls-provider", "network-framework", "bypass-system", "bypass-tun", "enhanced-mode-by-rule", "allow-udp-proxy",
];

fn invalid(diags: &mut Diagnostics, span: &Span, key: &str, value: &str) {
    diags.push(
        Diagnostic::warning(codes::W_INVALID_VALUE, format!("invalid value `{value}` for `{key}`, using default")).at(span.clone()),
    );
}

fn bool_or(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, current: bool) -> bool {
    match parse_bool(value) {
        Some(b) => b,
        None => {
            invalid(diags, span, key, value);
            current
        }
    }
}

fn u16_or(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, current: u16) -> u16 {
    match value.trim().parse() {
        Ok(v) => v,
        Err(_) => {
            invalid(diags, span, key, value);
            current
        }
    }
}

/// Parse `[password@]address[:port]`; the address must be an IP literal.
fn parse_listener(s: &str, default_port: u16) -> Result<Listener, &'static str> {
    let (password, rest) = match s.rsplit_once('@') {
        Some((pw, rest)) => (Some(pw.to_string()), rest),
        None => (None, s),
    };
    let addr = parse_socket_addr(rest, default_port).ok_or("address must be an IP literal like 0.0.0.0:6152")?;
    Ok(Listener { password, addr })
}

/// `ip`, `ip:port`, `[v6]`, `[v6]:port`.
fn parse_socket_addr(s: &str, default_port: u16) -> Option<SocketAddr> {
    let s = s.trim();
    if let Ok(sa) = s.parse::<SocketAddr>() {
        return Some(sa);
    }
    let bare = s.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(s);
    let ip: IpAddr = bare.parse().ok()?;
    Some(SocketAddr::new(ip, default_port))
}

fn parse_controller(s: &str) -> Option<ControllerAccess> {
    let (key, rest) = s.rsplit_once('@')?;
    if key.is_empty() {
        return None;
    }
    let addr = rest.parse::<SocketAddr>().ok()?;
    Some(ControllerAccess { key: key.to_string(), addr })
}

fn parse_listeners(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, default_port: u16, allow_password: bool) -> Vec<Listener> {
    let mut out = Vec::new();
    for item in split_list(value) {
        match parse_listener(&item, default_port) {
            Ok(mut l) => {
                if l.password.is_some() && !allow_password {
                    diags.push(
                        Diagnostic::warning(codes::W_INVALID_VALUE, format!("`{key}` does not support a password; ignoring it in `{item}`"))
                            .at(span.clone()),
                    );
                    l.password = None;
                }
                out.push(l);
            }
            Err(msg) => diags.push(Diagnostic::error(codes::E_LISTENER_NOT_IP, format!("`{key}`: `{item}`: {msg}")).at(span.clone())),
        }
    }
    out
}

fn parse_nets(diags: &mut Diagnostics, span: &Span, key: &str, value: &str) -> Vec<IpNet> {
    let mut out = Vec::new();
    for item in split_list(value) {
        match item.parse::<IpNet>() {
            Ok(n) => out.push(n),
            Err(_) => invalid(diags, span, key, &item),
        }
    }
    out
}

fn parse_host_list(diags: &mut Diagnostics, span: &Span, key: &str, value: &str, default_port: Option<u16>) -> HostList {
    let list = HostList::parse(value, default_port);
    for bad in &list.invalid {
        diags.push(Diagnostic::warning(codes::W_INVALID_HOST_LIST_ENTRY, format!("`{key}`: invalid entry `{bad}` ignored")).at(span.clone()));
    }
    list
}

fn migrated(diags: &mut Diagnostics, span: &Span, old: &str, new: &str) {
    diags.push(Diagnostic::info(codes::I_LEGACY_MIGRATED, format!("legacy key `{old}` migrated to `{new}`")).at(span.clone()));
}

pub fn parse_general(section: Option<&Section>, diags: &mut Diagnostics) -> General {
    let mut g = General::default();
    let Some(section) = section else { return g };
    let mut legacy_http: (Option<String>, Option<u16>, Option<Span>) = (None, None, None);
    let mut legacy_socks: (Option<String>, Option<u16>, Option<Span>) = (None, None, None);

    for e in section.active_entries() {
        let span = &e.span;
        let Some((key, value)) = split_definition(&e.raw) else {
            diags.push(Diagnostic::error(codes::E_INVALID_DEFINITION, format!("expected `key = value`, found `{}`", e.raw)).at(span.clone()));
            continue;
        };
        let key = key.to_ascii_lowercase();
        let key = key.as_str();
        macro_rules! set_bool {
            ($field:ident) => {
                g.$field = bool_or(diags, span, key, value, g.$field)
            };
        }
        match key {
            "loglevel" => {
                g.loglevel = match value.to_ascii_lowercase().as_str() {
                    "verbose" => LogLevel::Verbose,
                    "info" => LogLevel::Info,
                    "notify" => LogLevel::Notify,
                    "warning" => LogLevel::Warning,
                    _ => {
                        invalid(diags, span, key, value);
                        g.loglevel
                    }
                }
            }
            "debug-cpu-usage" => set_bool!(debug_cpu_usage),
            "debug-memory-usage" => set_bool!(debug_memory_usage),
            "dns-server" => {
                for item in split_list(value) {
                    if item.eq_ignore_ascii_case("system") {
                        g.dns_server.push(DnsServer::System);
                    } else if let Some(enc) = EncryptedDns::parse(&item) {
                        g.encrypted_dns_server.push(enc);
                    } else if let Some(sa) = parse_socket_addr(&item, 53) {
                        g.dns_server.push(DnsServer::Udp(sa));
                    } else {
                        invalid(diags, span, key, &item);
                    }
                }
            }
            "encrypted-dns-server" | "doh-server" => {
                if key == "doh-server" {
                    migrated(diags, span, key, "encrypted-dns-server");
                }
                for item in split_list(value) {
                    match EncryptedDns::parse(&item) {
                        Some(enc) => g.encrypted_dns_server.push(enc),
                        None => invalid(diags, span, key, &item),
                    }
                }
            }
            "encrypted-dns-follow-outbound-mode" | "doh-follow-outbound-mode" => {
                if key == "doh-follow-outbound-mode" {
                    migrated(diags, span, key, "encrypted-dns-follow-outbound-mode");
                }
                set_bool!(encrypted_dns_follow_outbound_mode)
            }
            "encrypted-dns-skip-cert-verification" | "doh-skip-cert-verification" => {
                if key == "doh-skip-cert-verification" {
                    migrated(diags, span, key, "encrypted-dns-skip-cert-verification");
                }
                set_bool!(encrypted_dns_skip_cert_verification)
            }
            "allow-dns-svcb" => set_bool!(allow_dns_svcb),
            "use-local-host-item-for-proxy" => set_bool!(use_local_host_item_for_proxy),
            "hijack-dns" => {
                for item in split_list(value) {
                    let (host, port) = match item.rsplit_once(':') {
                        Some((h, p)) => (h, p.parse::<u16>().ok()),
                        None => (item.as_str(), Some(53)),
                    };
                    let addr = if host == "*" { Ok(None) } else { host.parse::<Ipv4Addr>().map(Some) };
                    match (addr, port) {
                        (Ok(addr), Some(port)) => g.hijack_dns.push(HijackTarget { addr, port }),
                        _ => invalid(diags, span, key, &item),
                    }
                }
            }
            "always-real-ip" => g.always_real_ip = parse_host_list(diags, span, key, value, None),
            "geoip-maxmind-url" => g.geoip_maxmind_url = Some(value.to_string()),
            "disable-geoip-db-auto-update" => set_bool!(disable_geoip_db_auto_update),
            "ipv6" => set_bool!(ipv6),
            "ipv6-vif" => {
                g.ipv6_vif = match value.to_ascii_lowercase().as_str() {
                    "disabled" => Ipv6Vif::Disabled,
                    "auto" => Ipv6Vif::Auto,
                    "always" => Ipv6Vif::Always,
                    "off" => {
                        migrated(diags, span, "ipv6-vif = off", "ipv6-vif = disabled");
                        Ipv6Vif::Disabled
                    }
                    _ => {
                        invalid(diags, span, key, value);
                        g.ipv6_vif
                    }
                }
            }
            "tun-excluded-routes" => g.tun_excluded_routes = parse_nets(diags, span, key, value),
            "tun-included-routes" => g.tun_included_routes = parse_nets(diags, span, key, value),
            "icmp-forwarding" => set_bool!(icmp_forwarding),
            "skip-proxy" => g.skip_proxy = parse_host_list(diags, span, key, value, None),
            "exclude-simple-hostnames" => set_bool!(exclude_simple_hostnames),
            "proxy-restricted-to-lan" => set_bool!(proxy_restricted_to_lan),
            "gateway-restricted-to-lan" => set_bool!(gateway_restricted_to_lan),
            "external-controller-access" => {
                g.external_controller_access = parse_controller(value);
                if g.external_controller_access.is_none() {
                    invalid(diags, span, key, value);
                }
            }
            "http-api" => {
                g.http_api = parse_controller(value);
                if g.http_api.is_none() {
                    invalid(diags, span, key, value);
                }
            }
            "http-api-tls" => set_bool!(http_api_tls),
            "http-api-web-dashboard" => set_bool!(http_api_web_dashboard),
            "internet-test-url" => g.internet_test_url = value.to_string(),
            "proxy-test-url" => g.proxy_test_url = value.to_string(),
            "test-timeout" => match value.trim().parse::<u64>() {
                Ok(s) => g.test_timeout = Duration::from_secs(s),
                Err(_) => invalid(diags, span, key, value),
            },
            "proxy-test-udp" => match value.split_once('@').and_then(|(h, ip)| ip.trim().parse::<Ipv4Addr>().ok().map(|ip| (h.trim().to_string(), ip))) {
                Some((hostname, server)) => g.proxy_test_udp = Some(UdpTest { hostname, server }),
                None => invalid(diags, span, key, value),
            },
            "force-http-engine-hosts" => g.force_http_engine_hosts = parse_host_list(diags, span, key, value, Some(80)),
            "always-raw-tcp-hosts" => g.always_raw_tcp_hosts = parse_host_list(diags, span, key, value, None),
            "always-raw-tcp-keywords" => g.always_raw_tcp_keywords = split_list(value),
            "udp-policy-not-supported-behaviour" => {
                g.udp_policy_not_supported_behaviour = match value.to_ascii_uppercase().as_str() {
                    "REJECT" => UdpFallback::Reject,
                    "DIRECT" => UdpFallback::Direct,
                    _ => {
                        invalid(diags, span, key, value);
                        g.udp_policy_not_supported_behaviour
                    }
                }
            }
            "udp-priority" => set_bool!(udp_priority),
            "block-quic" => {
                g.block_quic = match value.to_ascii_lowercase().as_str() {
                    "per-policy" => BlockQuicGlobal::PerPolicy,
                    "all-proxy" => BlockQuicGlobal::AllProxy,
                    "all" => BlockQuicGlobal::All,
                    "always-allow" => BlockQuicGlobal::AlwaysAllow,
                    _ => {
                        invalid(diags, span, key, value);
                        g.block_quic
                    }
                }
            }
            "show-error-page" => set_bool!(show_error_page),
            "show-error-page-for-reject" => set_bool!(show_error_page_for_reject),
            "compatibility-mode" => g.compatibility_mode = u16_or(diags, span, key, value, u16::from(g.compatibility_mode)) as u8,
            "auto-suspend" => set_bool!(auto_suspend),
            "allow-wifi-access" => set_bool!(allow_wifi_access),
            "allow-hotspot-access" => set_bool!(allow_hotspot_access),
            "wifi-access-http-port" => g.wifi_access_http_port = u16_or(diags, span, key, value, g.wifi_access_http_port),
            "wifi-access-socks5-port" => g.wifi_access_socks5_port = u16_or(diags, span, key, value, g.wifi_access_socks5_port),
            "wifi-access-http-auth" => match value.split_once(':') {
                Some((u, p)) => g.wifi_access_http_auth = Some((u.to_string(), p.to_string())),
                None => invalid(diags, span, key, value),
            },
            "wifi-assist" => set_bool!(wifi_assist),
            "all-hybrid" => set_bool!(all_hybrid),
            "hide-vpn-icon" => set_bool!(hide_vpn_icon),
            "include-all-networks" => set_bool!(include_all_networks),
            "include-local-networks" => set_bool!(include_local_networks),
            "include-apns" => set_bool!(include_apns),
            "include-cellular-services" => set_bool!(include_cellular_services),
            "http-listen" => g.http_listen = parse_listeners(diags, span, key, value, 6152, true),
            "socks5-listen" => g.socks5_listen = parse_listeners(diags, span, key, value, 6153, false),
            "set-system-socks-proxy" => set_bool!(set_system_socks_proxy),
            "read-etc-hosts" => set_bool!(read_etc_hosts),
            "subnet-exp-wifi-always-match" => set_bool!(subnet_exp_wifi_always_match),
            "use-default-policy-if-wifi-not-primary" => {
                migrated(diags, span, key, "subnet-exp-wifi-always-match (inverted)");
                let v = bool_or(diags, span, key, value, !g.subnet_exp_wifi_always_match);
                g.subnet_exp_wifi_always_match = !v;
            }
            "interface" => {
                legacy_http.0 = Some(value.to_string());
                legacy_http.2 = Some(span.clone());
            }
            "port" => {
                legacy_http.1 = Some(u16_or(diags, span, key, value, 6152));
                legacy_http.2 = Some(span.clone());
            }
            "socks-interface" => {
                legacy_socks.0 = Some(value.to_string());
                legacy_socks.2 = Some(span.clone());
            }
            "socks-port" => {
                legacy_socks.1 = Some(u16_or(diags, span, key, value, 6153));
                legacy_socks.2 = Some(span.clone());
            }
            k if VANISHED_KEYS.contains(&k) => {
                diags.push(Diagnostic::warning(codes::W_VANISHED_KEY, format!("`{k}` is no longer supported by Surge and is ignored")).at(span.clone()));
            }
            _ => {
                diags.push(Diagnostic::warning(codes::W_UNKNOWN_KEY, format!("unknown [General] key `{key}` ignored")).at(span.clone()));
                g.unknown.push(UnknownKey { key: key.to_string(), value: value.to_string(), span: span.clone() });
            }
        }
        if IOS_ONLY_KEYS.contains(&key) {
            diags.push(Diagnostic::warning(codes::W_PLATFORM_IGNORED, format!("`{key}` is iOS-only and has no effect on desktop platforms")).at(span.clone()));
        }
    }

    if let (Some(span), true) = (&legacy_http.2, g.http_listen.is_empty()) {
        let addr = format!("{}:{}", legacy_http.0.as_deref().unwrap_or("127.0.0.1"), legacy_http.1.unwrap_or(6152));
        migrated(diags, span, "interface/port", "http-listen");
        g.http_listen = parse_listeners(diags, span, "http-listen", &addr, 6152, true);
    }
    if let (Some(span), true) = (&legacy_socks.2, g.socks5_listen.is_empty()) {
        let addr = format!("{}:{}", legacy_socks.0.as_deref().unwrap_or("127.0.0.1"), legacy_socks.1.unwrap_or(6153));
        migrated(diags, span, "socks-interface/socks-port", "socks5-listen");
        g.socks5_listen = parse_listeners(diags, span, "socks5-listen", &addr, 6153, false);
    }
    g
}
```

关于 `legacy_keys_migrate` 测试中 `I_LEGACY_MIGRATED` 计数为 7：`doh-server`、`doh-follow-outbound-mode`、`doh-skip-cert-verification`、`use-default-policy-if-wifi-not-primary`、`ipv6-vif = off`、`interface/port`、`socks-interface/socks-port`。

`lib.rs` 增加 `pub mod general;` 与 `pub use general::General;`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config general`
Expected: 4 个测试通过。

- [ ] **Step 5: 运行质量门并提交**

Run: `cargo clippy -p rurge-config --all-targets -- -D warnings && cargo fmt --all --check`
Expected: 无警告。

```bash
git add crates/rurge-config
git commit -m "feat(config): [General] 全部选项的强类型解析、旧键迁移与平台提示"
```

---

### Task 9: `[Proxy]` 与 `[Proxy Group]` 解析

**Files:**
- Create: `crates/rurge-config/src/policy.rs`
- Modify: `crates/rurge-config/src/diagnostic.rs`（新增 `ParseError`）
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `split_list` / `ParamMap` / `parse_key_value`（Task 4），`Glob`（Task 5），`HostName`（Task 7），`Span`、`codes`（Task 2）。
- Produces:
  - `diagnostic::ParseError { code: &'static str, message: String }`，`ParseError::new(code, message)`，`Diagnostic::from_parse(err: ParseError, span: Span) -> Diagnostic`（严重度 Error）。所有节解析函数都返回 `Result<T, ParseError>`，由 Task 12 附加 span。
  - `policy::PolicyKind`（24 个变体：16 种协议 + `Direct` `Reject` `RejectDrop` `RejectNoDrop` `RejectTinyGif` 别名类型），`PolicyKind::parse(keyword) -> Option<PolicyKind>`，`keyword() -> &'static str`，`is_builtin_alias()`，`takes_server()`（`wireguard` `tailscale` `external` 与别名类型为 false）。
  - `policy::Builtin { Direct, Reject, RejectDrop, RejectNoDrop, RejectTinyGif, Cellular, CellularOnly, Hybrid, NoHybrid }`，`Builtin::parse(name)`（大小写敏感的大写名），`name()`，`is_reject()`，`is_ios_only()`。
  - `policy::ProxyPolicy { name, kind, server: Option<HostName>, port: Option<u16>, positional: Vec<String>, params: ParamMap, span }`，`parse_policy(name: &str, definition: &str, span: &Span) -> Result<ProxyPolicy, ParseError>`。
  - `policy::GroupKind { Select, UrlTest, Fallback, LoadBalance, Smart, Subnet }`，`GroupKind::parse`（含 `ssid` 别名 → `Subnet`），`keyword()`。
  - `policy::NetType { Wifi, Wired, Cellular }`、`policy::SubnetExpr { Ssid(Glob), Bssid(Glob), Router(IpAddr), Type(NetType), Mccmnc(String), Bare(String) }`，`SubnetExpr::parse(&str) -> Result<SubnetExpr, ParseError>`。
  - `policy::PolicyGroup { name, kind, members: Vec<String>, params: ParamMap, conditions: Vec<(SubnetExpr, String)>, legacy_keyword: bool, span }`，`parse_group(name, definition, span) -> Result<PolicyGroup, ParseError>`。

语法：策略行 `type, [server, port,] [positional...], key=value...`；组行 `type, member..., key=value...`；`subnet` 组中带 `=` 的字段若键不是 `default` / `cellular` / `hidden` / `icon-url`，则是条件 `子网表达式 = 策略`。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/policy.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("p.conf")), 1)
    }

    #[test]
    fn manual_policy_examples() {
        let p = parse_policy("ProxyHTTPS", "https, 1.2.3.4, 443, username, password", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::Https);
        assert_eq!(p.server, Some(HostName::parse("1.2.3.4")));
        assert_eq!(p.port, Some(443));
        assert_eq!(p.positional, ["username", "password"]);

        let p = parse_policy("ProxySS", "ss, 1.2.3.4, 8388, encrypt-method=chacha20-ietf-poly1305, password=pwd, udp-relay=true", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::Shadowsocks);
        assert_eq!(p.params.get("encrypt-method"), Some("chacha20-ietf-poly1305"));
        assert_eq!(p.params.bool("udp-relay"), Some(true));

        let p = parse_policy("Office WG", "wireguard, section-name=office-wg", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::WireGuard);
        assert_eq!(p.server, None);
        assert_eq!(p.params.get("section-name"), Some("office-wg"));

        let p = parse_policy("ext", "external, exec = \"/usr/bin/ssh\", args = \"1.2.3.4\", args = \"-D\", local-port = 1080, addresses = 1.2.3.4", &span()).unwrap();
        assert_eq!(p.kind, PolicyKind::External);
        assert_eq!(p.params.get_all("args"), ["1.2.3.4", "-D"]);
        assert_eq!(p.params.u16("local-port"), Some(1080));

        let p = parse_policy("Corp-VPN", "direct, interface = utun0", &span()).unwrap();
        assert!(p.kind.is_builtin_alias());
        assert_eq!(p.params.get("interface"), Some("utun0"));

        let p = parse_policy("Exit", "snell, exit.example.com, 443, psk=pwd, version=5, underlying-proxy=Entry", &span()).unwrap();
        assert_eq!(p.server, Some(HostName::Domain("exit.example.com".into())));
        assert_eq!(p.params.get("underlying-proxy"), Some("Entry"));
    }

    #[test]
    fn policy_errors() {
        assert_eq!(parse_policy("X", "vless, 1.2.3.4, 443", &span()).unwrap_err().code, codes::E_UNKNOWN_POLICY_TYPE);
        assert_eq!(parse_policy("X", "ss, 1.2.3.4", &span()).unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse_policy("X", "ss, 1.2.3.4, notaport, password=x", &span()).unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse_policy("X", "", &span()).unwrap_err().code, codes::E_SYNTAX);
    }

    #[test]
    fn builtin_names() {
        assert_eq!(Builtin::parse("REJECT-TINYGIF"), Some(Builtin::RejectTinyGif));
        assert_eq!(Builtin::parse("reject"), None);
        assert!(Builtin::RejectDrop.is_reject());
        assert!(Builtin::CellularOnly.is_ios_only());
        assert!(!Builtin::Direct.is_ios_only());
    }

    #[test]
    fn manual_group_examples() {
        let g = parse_group("Proxy", "select, ProxyA, ProxyB, DIRECT", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::Select);
        assert_eq!(g.members, ["ProxyA", "ProxyB", "DIRECT"]);

        let g = parse_group("Auto", "url-test, ProxyA, ProxyB, interval=600, tolerance=100, no-alert=true", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::UrlTest);
        assert_eq!(g.params.u32("interval"), Some(600));
        assert_eq!(g.params.bool("no-alert"), Some(true));

        let g = parse_group("Smart", "smart, ProxyA, ProxyB, policy-priority=\"Premium:0.9;Backup:1.3\"", &span()).unwrap();
        assert_eq!(g.params.get("policy-priority"), Some("Premium:0.9;Backup:1.3"));

        let g = parse_group("egroup", "select, policy-path=proxies.txt, policy-regex-filter=^HK, include-other-group=\"group1,group2\"", &span()).unwrap();
        assert!(g.members.is_empty());
        assert_eq!(g.params.get("include-other-group"), Some("group1,group2"));

        let g = parse_group("Subnet Group", "subnet, default = ProxyHTTP, SSID:MyHome = ProxySOCKS5, TYPE:WIFI = ProxyHTTP, BSSID:aa:bb:cc:* = A, ROUTER:192.168.1.1 = B, MCCMNC:310260 = C, OldName = D, hidden=true", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::Subnet);
        assert_eq!(g.params.get("default"), Some("ProxyHTTP"));
        assert_eq!(g.params.bool("hidden"), Some(true));
        assert_eq!(g.conditions.len(), 6);
        assert!(matches!(&g.conditions[0].0, SubnetExpr::Ssid(gl) if gl.source() == "MyHome"));
        assert_eq!(g.conditions[1].0, SubnetExpr::Type(NetType::Wifi));
        assert!(matches!(&g.conditions[2].0, SubnetExpr::Bssid(_)));
        assert_eq!(g.conditions[3].0, SubnetExpr::Router("192.168.1.1".parse().unwrap()));
        assert_eq!(g.conditions[4].0, SubnetExpr::Mccmnc("310260".into()));
        assert_eq!(g.conditions[5].0, SubnetExpr::Bare("OldName".into()));
        assert_eq!(g.conditions[5].1, "D");

        let g = parse_group("Old", "ssid, default = A, MyWifi = B", &span()).unwrap();
        assert_eq!(g.kind, GroupKind::Subnet);
        assert!(g.legacy_keyword);
    }

    #[test]
    fn group_errors() {
        assert_eq!(parse_group("G", "round-robin, A, B", &span()).unwrap_err().code, codes::E_UNKNOWN_POLICY_TYPE);
        assert_eq!(parse_group("G", "subnet, default = A, TYPE:SATELLITE = B", &span()).unwrap_err().code, codes::E_INVALID_RULE_VALUE);
        assert_eq!(parse_group("G", "subnet, default = A, ROUTER:not-an-ip = B", &span()).unwrap_err().code, codes::E_INVALID_RULE_VALUE);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config policy`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`diagnostic.rs` 追加：

```rust
/// Error from a section-level parser; the caller attaches the span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub code: &'static str,
    pub message: String,
}

impl ParseError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

impl Diagnostic {
    pub fn from_parse(err: ParseError, span: Span) -> Diagnostic {
        Diagnostic::error(err.code, err.message).at(span)
    }
}
```

`crates/rurge-config/src/policy.rs`：

```rust
//! `[Proxy]` policies and `[Proxy Group]` groups.

use crate::diagnostic::{ParseError, codes};
use crate::glob::{Glob, GlobOptions};
use crate::span::Span;
use crate::types::HostName;
use crate::value::{ParamMap, parse_key_value, split_list};
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PolicyKind {
    Http,
    Https,
    H2Connect,
    Socks5,
    Socks5Tls,
    Shadowsocks,
    Snell,
    Vmess,
    Trojan,
    Tuic,
    TuicV5,
    Hysteria2,
    Masque,
    AnyTls,
    TrustTunnel,
    Ssh,
    WireGuard,
    Tailscale,
    External,
    Direct,
    Reject,
    RejectDrop,
    RejectNoDrop,
    RejectTinyGif,
}

const POLICY_KEYWORDS: &[(&str, PolicyKind)] = &[
    ("http", PolicyKind::Http),
    ("https", PolicyKind::Https),
    ("h2-connect", PolicyKind::H2Connect),
    ("socks5", PolicyKind::Socks5),
    ("socks5-tls", PolicyKind::Socks5Tls),
    ("ss", PolicyKind::Shadowsocks),
    ("snell", PolicyKind::Snell),
    ("vmess", PolicyKind::Vmess),
    ("trojan", PolicyKind::Trojan),
    ("tuic", PolicyKind::Tuic),
    ("tuic-v5", PolicyKind::TuicV5),
    ("hysteria2", PolicyKind::Hysteria2),
    ("masque", PolicyKind::Masque),
    ("anytls", PolicyKind::AnyTls),
    ("trust-tunnel", PolicyKind::TrustTunnel),
    ("ssh", PolicyKind::Ssh),
    ("wireguard", PolicyKind::WireGuard),
    ("tailscale", PolicyKind::Tailscale),
    ("external", PolicyKind::External),
    ("direct", PolicyKind::Direct),
    ("reject", PolicyKind::Reject),
    ("reject-drop", PolicyKind::RejectDrop),
    ("reject-no-drop", PolicyKind::RejectNoDrop),
    ("reject-tinygif", PolicyKind::RejectTinyGif),
];

impl PolicyKind {
    pub fn parse(keyword: &str) -> Option<PolicyKind> {
        let kw = keyword.trim().to_ascii_lowercase();
        POLICY_KEYWORDS.iter().find(|(k, _)| *k == kw).map(|(_, v)| *v)
    }
    pub fn keyword(&self) -> &'static str {
        POLICY_KEYWORDS.iter().find(|(_, v)| v == self).map(|(k, _)| *k).unwrap_or("unknown")
    }
    pub fn is_builtin_alias(&self) -> bool {
        matches!(self, PolicyKind::Direct | PolicyKind::Reject | PolicyKind::RejectDrop | PolicyKind::RejectNoDrop | PolicyKind::RejectTinyGif)
    }
    pub fn takes_server(&self) -> bool {
        !self.is_builtin_alias() && !matches!(self, PolicyKind::WireGuard | PolicyKind::Tailscale | PolicyKind::External)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    Direct,
    Reject,
    RejectDrop,
    RejectNoDrop,
    RejectTinyGif,
    Cellular,
    CellularOnly,
    Hybrid,
    NoHybrid,
}

const BUILTIN_NAMES: &[(&str, Builtin)] = &[
    ("DIRECT", Builtin::Direct),
    ("REJECT", Builtin::Reject),
    ("REJECT-DROP", Builtin::RejectDrop),
    ("REJECT-NO-DROP", Builtin::RejectNoDrop),
    ("REJECT-TINYGIF", Builtin::RejectTinyGif),
    ("CELLULAR", Builtin::Cellular),
    ("CELLULAR-ONLY", Builtin::CellularOnly),
    ("HYBRID", Builtin::Hybrid),
    ("NO-HYBRID", Builtin::NoHybrid),
];

impl Builtin {
    /// Built-in names are case-sensitive upper-case keywords.
    pub fn parse(name: &str) -> Option<Builtin> {
        BUILTIN_NAMES.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
    }
    pub fn name(&self) -> &'static str {
        BUILTIN_NAMES.iter().find(|(_, v)| v == self).map(|(k, _)| *k).unwrap_or("DIRECT")
    }
    pub fn is_reject(&self) -> bool {
        matches!(self, Builtin::Reject | Builtin::RejectDrop | Builtin::RejectNoDrop | Builtin::RejectTinyGif)
    }
    pub fn is_ios_only(&self) -> bool {
        matches!(self, Builtin::Cellular | Builtin::CellularOnly | Builtin::Hybrid | Builtin::NoHybrid)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyPolicy {
    pub name: String,
    pub kind: PolicyKind,
    pub server: Option<HostName>,
    pub port: Option<u16>,
    pub positional: Vec<String>,
    pub params: ParamMap,
    pub span: Span,
}

pub fn parse_policy(name: &str, definition: &str, span: &Span) -> Result<ProxyPolicy, ParseError> {
    let fields = split_list(definition);
    let Some(type_kw) = fields.first() else {
        return Err(ParseError::new(codes::E_SYNTAX, format!("policy `{name}`: missing type")));
    };
    let kind = PolicyKind::parse(type_kw)
        .ok_or_else(|| ParseError::new(codes::E_UNKNOWN_POLICY_TYPE, format!("policy `{name}`: unknown type `{type_kw}`")))?;
    let (server, port, rest) = if kind.takes_server() {
        let server = fields
            .get(1)
            .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("policy `{name}`: expected `{}, <server>, <port>`", kind.keyword())))?;
        let port_str = fields
            .get(2)
            .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("policy `{name}`: expected `{}, <server>, <port>`", kind.keyword())))?;
        let port: u16 = port_str
            .parse()
            .map_err(|_| ParseError::new(codes::E_SYNTAX, format!("policy `{name}`: invalid port `{port_str}`")))?;
        (Some(HostName::parse(server)), Some(port), &fields[3..])
    } else {
        (None, None, &fields[1..])
    };
    let (params, positional) = ParamMap::from_fields(rest);
    Ok(ProxyPolicy { name: name.to_string(), kind, server, port, positional, params, span: span.clone() })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Select,
    UrlTest,
    Fallback,
    LoadBalance,
    Smart,
    Subnet,
}

impl GroupKind {
    /// Returns the kind and whether the legacy `ssid` keyword was used.
    pub fn parse(keyword: &str) -> Option<(GroupKind, bool)> {
        Some(match keyword.trim().to_ascii_lowercase().as_str() {
            "select" => (GroupKind::Select, false),
            "url-test" => (GroupKind::UrlTest, false),
            "fallback" => (GroupKind::Fallback, false),
            "load-balance" => (GroupKind::LoadBalance, false),
            "smart" => (GroupKind::Smart, false),
            "subnet" => (GroupKind::Subnet, false),
            "ssid" => (GroupKind::Subnet, true),
            _ => return None,
        })
    }
    pub fn keyword(&self) -> &'static str {
        match self {
            GroupKind::Select => "select",
            GroupKind::UrlTest => "url-test",
            GroupKind::Fallback => "fallback",
            GroupKind::LoadBalance => "load-balance",
            GroupKind::Smart => "smart",
            GroupKind::Subnet => "subnet",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetType {
    Wifi,
    Wired,
    Cellular,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubnetExpr {
    Ssid(Glob),
    Bssid(Glob),
    Router(IpAddr),
    Type(NetType),
    Mccmnc(String),
    Bare(String),
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &s[prefix.len()..])
}

impl SubnetExpr {
    pub fn parse(s: &str) -> Result<SubnetExpr, ParseError> {
        let s = s.trim();
        let bad = |what: &str| ParseError::new(codes::E_INVALID_RULE_VALUE, format!("invalid subnet expression `{s}`: {what}"));
        if let Some(v) = strip_prefix_ci(s, "SSID:") {
            return Glob::new(v, GlobOptions { case_insensitive: false, classes: false }).map(SubnetExpr::Ssid).map_err(|e| bad(&e.to_string()));
        }
        if let Some(v) = strip_prefix_ci(s, "BSSID:") {
            return Glob::new(v, GlobOptions { case_insensitive: true, classes: false }).map(SubnetExpr::Bssid).map_err(|e| bad(&e.to_string()));
        }
        if let Some(v) = strip_prefix_ci(s, "ROUTER:") {
            return v.parse::<IpAddr>().map(SubnetExpr::Router).map_err(|_| bad("expected an IP address"));
        }
        if let Some(v) = strip_prefix_ci(s, "TYPE:") {
            return match v.to_ascii_uppercase().as_str() {
                "WIFI" => Ok(SubnetExpr::Type(NetType::Wifi)),
                "WIRED" => Ok(SubnetExpr::Type(NetType::Wired)),
                "CELLULAR" => Ok(SubnetExpr::Type(NetType::Cellular)),
                _ => Err(bad("expected WIFI, WIRED or CELLULAR")),
            };
        }
        if let Some(v) = strip_prefix_ci(s, "MCCMNC:") {
            if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit()) {
                return Err(bad("expected MCC+MNC digits"));
            }
            return Ok(SubnetExpr::Mccmnc(v.to_string()));
        }
        if s.is_empty() {
            return Err(bad("empty"));
        }
        Ok(SubnetExpr::Bare(s.to_string()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolicyGroup {
    pub name: String,
    pub kind: GroupKind,
    pub members: Vec<String>,
    pub params: ParamMap,
    pub conditions: Vec<(SubnetExpr, String)>,
    pub legacy_keyword: bool,
    pub span: Span,
}

const SUBNET_GROUP_PARAMS: &[&str] = &["default", "cellular", "hidden", "icon-url"];

pub fn parse_group(name: &str, definition: &str, span: &Span) -> Result<PolicyGroup, ParseError> {
    let fields = split_list(definition);
    let Some(type_kw) = fields.first() else {
        return Err(ParseError::new(codes::E_SYNTAX, format!("policy group `{name}`: missing type")));
    };
    let (kind, legacy_keyword) = GroupKind::parse(type_kw)
        .ok_or_else(|| ParseError::new(codes::E_UNKNOWN_POLICY_TYPE, format!("policy group `{name}`: unknown type `{type_kw}`")))?;
    let mut members = Vec::new();
    let mut params = ParamMap::default();
    let mut conditions = Vec::new();
    for field in &fields[1..] {
        match parse_key_value(field) {
            Some((k, v)) => {
                if kind == GroupKind::Subnet && !SUBNET_GROUP_PARAMS.contains(&k.to_ascii_lowercase().as_str()) {
                    conditions.push((SubnetExpr::parse(k)?, v.to_string()));
                } else {
                    params.insert(k, v);
                }
            }
            None => members.push(field.clone()),
        }
    }
    Ok(PolicyGroup { name: name.to_string(), kind, members, params, conditions, legacy_keyword, span: span.clone() })
}
```

`lib.rs` 增加 `pub mod policy;` 与 `pub use diagnostic::ParseError; pub use policy::{Builtin, GroupKind, PolicyGroup, PolicyKind, ProxyPolicy, SubnetExpr};`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config policy`
Expected: 5 个测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): [Proxy] 与 [Proxy Group] 解析、内置策略名、子网表达式"
```

---

### Task 10: `[Rule]` 解析（29 种类型、10 个参数、逻辑规则与子规则）

**Files:**
- Create: `crates/rurge-config/src/rule.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `split_list`（Task 4），`Glob`（Task 5），`Builtin` / `SubnetExpr`（Task 9），`ParseError` / `codes`（Task 9 / 2），`fancy_regex`、`ipnet`。
- Produces:
  - `rule::ParseCtx<'a> { inline_rulesets: &'a HashSet<String>, base_dir: &'a Path }`
  - `rule::PortExpr { Single(u16), Range(u16, u16), Gt(u16), Lt(u16), Ge(u16), Le(u16) }`，`PortExpr::parse(&str) -> Option<PortExpr>`，`matches(port) -> bool`
  - `rule::InternalSet { System, Lan }`，`rule::ResourceRef { Internal(InternalSet), Inline(String), File(PathBuf), Url(String) }`，`ResourceRef::parse(raw, ctx) -> ResourceRef`（顺序：内部名 → 内联节名 → URL → 文件）
  - `rule::Pattern { source: String, regex: fancy_regex::Regex }`（`PartialEq` 按 `source`），`Pattern::new(&str) -> Result<Pattern, String>`
  - `rule::HostnameType { IPv4, IPv6, Domain, Simple }`、`rule::ProtocolKind { Http, Https, Tcp, Udp, Quic, Stun, MtProto, Doh, Doh3, Doq, Dot, Dns }`、`rule::ProcessPattern { Name(Glob), Path(Glob), Prefix(String) }`
  - `rule::RuleKind`（29 个变体，见实现）、`RuleKind::type_name() -> &'static str`、`RuleKind::is_ip_based()`
  - `rule::SubRule { kind: RuleKind, no_resolve: bool, extended_matching: bool, raw: String }`
  - `rule::RuleParams { no_resolve, dns_failed, extended_matching, pre_matching, requires_resolve: bool, notification_text: Option<String>, notification_interval: Option<u32>, update_interval: Option<i64>, always_capture: Option<String>, unknown: Vec<String> }`
  - `rule::PolicyRef { Builtin(Builtin), Named(String), Device(String) }`，`PolicyRef::parse(&str)`，`Display`
  - `rule::Rule { kind: RuleKind, policy: PolicyRef, params: RuleParams, span: Span, raw: String }`，`Display` 输出 `raw`
  - `parse_rule(raw, ctx, span) -> Result<Rule, ParseError>`；`parse_subrule(raw, ctx) -> Result<SubRule, ParseError>`（供逻辑规则、内联规则集与 M2 的外部集合文件复用）

校验：`FINAL` 不能作子规则（E0014）；集合 / 子规则里不允许 `pre-matching`（E0014）；逻辑规则嵌套 > 10 层（E0015）；`NOT` 必须恰好一个子规则（E0001）；`pre-matching` 的策略必须是 REJECT 系（E0014）；未知类型（E0012）；值非法（E0011）；未知参数进入 `params.unknown`（Task 12 报 W0003）。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/rule.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::Arc;

    fn ctx_and_span() -> (HashSet<String>, Span) {
        (HashSet::from(["Streaming".to_string()]), Span::new(Arc::from(Path::new("r.conf")), 1))
    }

    fn parse(raw: &str) -> Result<Rule, ParseError> {
        let (inline, span) = ctx_and_span();
        let ctx = ParseCtx { inline_rulesets: &inline, base_dir: Path::new("/profiles") };
        parse_rule(raw, &ctx, &span)
    }

    #[test]
    fn manual_examples_parse_with_expected_types() {
        let cases: &[(&str, &str)] = &[
            ("DOMAIN,www.apple.com,Proxy", "DOMAIN"),
            ("DOMAIN-SUFFIX,apple.com,DIRECT", "DOMAIN-SUFFIX"),
            ("DOMAIN-KEYWORD,google,Proxy", "DOMAIN-KEYWORD"),
            ("DOMAIN-WILDCARD,api-*.example.com,Proxy", "DOMAIN-WILDCARD"),
            ("DOMAIN-SET,https://example.com/adblock.txt,REJECT", "DOMAIN-SET"),
            ("DOMAIN-SET,my-domains.txt,Proxy,update-interval=43200", "DOMAIN-SET"),
            ("IP-CIDR,192.168.0.0/16,DIRECT,no-resolve", "IP-CIDR"),
            ("IP-CIDR,8.8.8.8,Proxy", "IP-CIDR"),
            ("IP-CIDR6,2001:db8:abcd:8000::/50,DIRECT", "IP-CIDR6"),
            ("IP-CIDR6,2404:6800::,DIRECT", "IP-CIDR6"),
            ("GEOIP,cn,DIRECT", "GEOIP"),
            ("IP-ASN,AS13335,Proxy", "IP-ASN"),
            ("USER-AGENT,Instagram*,DIRECT", "USER-AGENT"),
            ("URL-REGEX,\"^http://example\\.com/(a|b),?c\",Proxy", "URL-REGEX"),
            ("URL-REGEX,^https://example\\.com,Proxy,extended-matching", "URL-REGEX"),
            ("PROCESS-NAME,Google*,Proxy", "PROCESS-NAME"),
            ("PROCESS-NAME,/Applications/*.app/Contents/MacOS/*,Proxy", "PROCESS-NAME"),
            ("PROCESS-NAME,/Applications/ChatGPT.app/,Proxy", "PROCESS-NAME"),
            ("DEST-PORT,80-81,DIRECT", "DEST-PORT"),
            ("SRC-PORT,>=50000,DIRECT", "SRC-PORT"),
            ("IN-PORT,6152,DIRECT", "IN-PORT"),
            ("SRC-IP,192.168.20.0/24,DIRECT", "SRC-IP"),
            ("SRC-IP,192.168.20.100,DIRECT", "SRC-IP"),
            ("DEVICE-NAME,Kids-iPad,REJECT", "DEVICE-NAME"),
            ("MAC-ADDRESS,A4:83:E7:11:22:33,Proxy", "MAC-ADDRESS"),
            ("PROTOCOL,STUN,REJECT", "PROTOCOL"),
            ("PROTOCOL,MTProto,Proxy", "PROTOCOL"),
            ("HOSTNAME-TYPE,IPv6,REJECT", "HOSTNAME-TYPE"),
            ("SUBNET,SSID:Office-*,DIRECT", "SUBNET"),
            ("SUBNET,TYPE:CELLULAR,DIRECT", "SUBNET"),
            ("CELLULAR-RADIO,LTE,DIRECT", "CELLULAR-RADIO"),
            ("CELLULAR-CARRIER,310260,Proxy", "CELLULAR-CARRIER"),
            ("AND,((SRC-IP,192.168.1.110),(DOMAIN-SUFFIX,example.com)),DIRECT", "AND"),
            ("AND,((NOT,((SRC-IP,192.168.1.110))),(DOMAIN-SUFFIX,example.com)),DIRECT", "AND"),
            ("OR,((DOMAIN,a.com),(DOMAIN,b.com)),Proxy", "OR"),
            ("NOT,((RULE-SET,LAN)),Proxy", "NOT"),
            ("AND,((PROTOCOL,UDP),(RULE-SET,https://example.com/streaming.list)),REJECT", "AND"),
            ("AND,((DOMAIN-SUFFIX,tracker.example.com),(DEST-PORT,443)),REJECT,pre-matching", "AND"),
            ("SCRIPT,ssid-rule,DIRECT,requires-resolve", "SCRIPT"),
            ("RULE-SET,SYSTEM,DIRECT", "RULE-SET"),
            ("RULE-SET,LAN,DIRECT,no-resolve", "RULE-SET"),
            ("RULE-SET,Streaming,StreamingProxy", "RULE-SET"),
            ("RULE-SET,https://example.com/social.list,Proxy,no-resolve,extended-matching,update-interval=43200", "RULE-SET"),
            ("RULE-SET,rules/local.list,Proxy", "RULE-SET"),
            ("DOMAIN,ad.example.com,REJECT,pre-matching", "DOMAIN"),
            ("DOMAIN-SUFFIX,example.com,Proxy,notification-text=Example matched,notification-interval=600", "DOMAIN-SUFFIX"),
            ("FINAL,ProxyB,dns-failed", "FINAL"),
            ("FINAL,DIRECT", "FINAL"),
        ];
        for (raw, ty) in cases {
            let r = parse(raw).unwrap_or_else(|e| panic!("{raw}: {}", e.message));
            assert_eq!(r.kind.type_name(), *ty, "{raw}");
            assert_eq!(r.to_string(), *raw);
        }
    }

    #[test]
    fn values_and_params() {
        let r = parse("IP-CIDR,8.8.8.8,Proxy").unwrap();
        assert!(matches!(r.kind, RuleKind::IpCidr(n) if n.prefix_len() == 32));
        let r = parse("IP-ASN,AS13335,Proxy").unwrap();
        assert_eq!(r.kind, RuleKind::IpAsn(13335));
        let r = parse("GEOIP,cn,DIRECT").unwrap();
        assert_eq!(r.kind, RuleKind::GeoIp("CN".into()));
        let r = parse("DEST-PORT,10000-20000,DIRECT").unwrap();
        assert_eq!(r.kind, RuleKind::DestPort(PortExpr::Range(10000, 20000)));
        assert!(PortExpr::Ge(50000).matches(50000));
        assert!(!PortExpr::Lt(80).matches(80));
        let r = parse("RULE-SET,Streaming,P").unwrap();
        assert_eq!(r.kind, RuleKind::RuleSet(ResourceRef::Inline("Streaming".into())));
        let r = parse("RULE-SET,rules/local.list,P").unwrap();
        assert_eq!(r.kind, RuleKind::RuleSet(ResourceRef::File(Path::new("/profiles/rules/local.list").to_path_buf())));
        let r = parse("RULE-SET,SYSTEM,P").unwrap();
        assert_eq!(r.kind, RuleKind::RuleSet(ResourceRef::Internal(InternalSet::System)));
        let r = parse("DOMAIN-SUFFIX,example.com,Proxy,notification-text=Example matched,notification-interval=600,bogus,update-interval=-1").unwrap();
        assert_eq!(r.params.notification_text.as_deref(), Some("Example matched"));
        assert_eq!(r.params.notification_interval, Some(600));
        assert_eq!(r.params.update_interval, Some(-1));
        assert_eq!(r.params.unknown, ["bogus"]);
        let r = parse("FINAL,ProxyB,dns-failed").unwrap();
        assert!(r.params.dns_failed);
        assert_eq!(r.policy, PolicyRef::Named("ProxyB".into()));
        let r = parse("DOMAIN,a,DEVICE:Home Mac").unwrap();
        assert_eq!(r.policy, PolicyRef::Device("Home Mac".into()));
        let r = parse("DOMAIN,a,REJECT-TINYGIF").unwrap();
        assert_eq!(r.policy, PolicyRef::Builtin(Builtin::RejectTinyGif));
        let r = parse("PROCESS-NAME,/Applications/ChatGPT.app/,Proxy").unwrap();
        assert_eq!(r.kind, RuleKind::ProcessName(ProcessPattern::Prefix("/Applications/ChatGPT.app/".into())));
        let r = parse("AND,((NOT,((SRC-IP,192.168.1.110))),(DOMAIN-SUFFIX,example.com)),DIRECT").unwrap();
        let RuleKind::And(subs) = &r.kind else { panic!() };
        assert_eq!(subs.len(), 2);
        assert!(matches!(&subs[0].kind, RuleKind::Not(inner) if matches!(inner.kind, RuleKind::SrcIp(_))));
    }

    #[test]
    fn errors() {
        let code = |raw: &str| parse(raw).unwrap_err().code;
        assert_eq!(code("DOMAINZ,a,DIRECT"), codes::E_UNKNOWN_RULE_TYPE);
        assert_eq!(code("IP-CIDR,10.0.0.0/99,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("URL-REGEX,\"[unclosed\",DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("DEST-PORT,abc,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("PROTOCOL,http,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("HOSTNAME-TYPE,ipv4,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("IP-ASN,ASX,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("MAC-ADDRESS,zz:zz,DIRECT"), codes::E_INVALID_RULE_VALUE);
        assert_eq!(code("DOMAIN,a"), codes::E_SYNTAX);
        assert_eq!(code("FINAL"), codes::E_SYNTAX);
        assert_eq!(code("AND,((FINAL,DIRECT),(DOMAIN,a)),DIRECT"), codes::E_NOT_ALLOWED_HERE);
        assert_eq!(code("AND,((DOMAIN,a,pre-matching),(DOMAIN,b)),REJECT"), codes::E_NOT_ALLOWED_HERE);
        assert_eq!(code("NOT,((DOMAIN,a),(DOMAIN,b)),DIRECT"), codes::E_SYNTAX);
        assert_eq!(code("DOMAIN,a,Proxy,pre-matching"), codes::E_NOT_ALLOWED_HERE);
        let deep = format!("{}DOMAIN,a{}", "NOT,((".repeat(11), "))".repeat(11));
        assert_eq!(code(&format!("{deep},DIRECT")), codes::E_NESTING_TOO_DEEP);
    }

    #[test]
    fn extra_positional_field_is_unknown_param_not_error() {
        let r = parse("IP-CIDR6,2001:db8::/50,DIRECT,no-resolve,extra").unwrap();
        assert!(r.params.no_resolve);
        assert_eq!(r.params.unknown, ["extra"]);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config rule`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/rule.rs`：

```rust
//! `[Rule]` lines, sub-rules (logical rules, inline rule sets, set files).

use crate::diagnostic::{ParseError, codes};
use crate::glob::{Glob, GlobOptions};
use crate::policy::{Builtin, SubnetExpr};
use crate::span::Span;
use crate::value::split_list;
use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

pub const MAX_LOGICAL_DEPTH: usize = 10;

pub struct ParseCtx<'a> {
    pub inline_rulesets: &'a HashSet<String>,
    pub base_dir: &'a Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortExpr {
    Single(u16),
    Range(u16, u16),
    Gt(u16),
    Lt(u16),
    Ge(u16),
    Le(u16),
}

impl PortExpr {
    pub fn parse(s: &str) -> Option<PortExpr> {
        let s = s.trim();
        let num = |x: &str| x.trim().parse::<u16>().ok();
        if let Some(v) = s.strip_prefix(">=") {
            return num(v).map(PortExpr::Ge);
        }
        if let Some(v) = s.strip_prefix("<=") {
            return num(v).map(PortExpr::Le);
        }
        if let Some(v) = s.strip_prefix('>') {
            return num(v).map(PortExpr::Gt);
        }
        if let Some(v) = s.strip_prefix('<') {
            return num(v).map(PortExpr::Lt);
        }
        if let Some((a, b)) = s.split_once('-') {
            let (a, b) = (num(a)?, num(b)?);
            return (a <= b).then_some(PortExpr::Range(a, b));
        }
        num(s).map(PortExpr::Single)
    }
    pub fn matches(&self, port: u16) -> bool {
        match *self {
            PortExpr::Single(p) => port == p,
            PortExpr::Range(a, b) => (a..=b).contains(&port),
            PortExpr::Gt(p) => port > p,
            PortExpr::Lt(p) => port < p,
            PortExpr::Ge(p) => port >= p,
            PortExpr::Le(p) => port <= p,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InternalSet {
    System,
    Lan,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceRef {
    Internal(InternalSet),
    Inline(String),
    File(PathBuf),
    Url(String),
}

impl ResourceRef {
    pub fn parse(raw: &str, ctx: &ParseCtx) -> ResourceRef {
        let raw = raw.trim();
        match raw {
            "SYSTEM" => return ResourceRef::Internal(InternalSet::System),
            "LAN" => return ResourceRef::Internal(InternalSet::Lan),
            _ => {}
        }
        if let Some(name) = ctx.inline_rulesets.iter().find(|n| n.as_str() == raw || n.eq_ignore_ascii_case(raw)) {
            return ResourceRef::Inline(name.clone());
        }
        let lower = raw.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return ResourceRef::Url(raw.to_string());
        }
        let path = Path::new(raw);
        ResourceRef::File(if path.is_absolute() { path.to_path_buf() } else { ctx.base_dir.join(path) })
    }
}

/// A compiled regular expression that compares by source text.
#[derive(Clone)]
pub struct Pattern {
    pub source: String,
    pub regex: fancy_regex::Regex,
}

impl Pattern {
    pub fn new(source: &str) -> Result<Pattern, String> {
        fancy_regex::Regex::new(source).map(|regex| Pattern { source: source.to_string(), regex }).map_err(|e| e.to_string())
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}
impl Eq for Pattern {}
impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pattern({:?})", self.source)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostnameType {
    IPv4,
    IPv6,
    Domain,
    Simple,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolKind {
    Http,
    Https,
    Tcp,
    Udp,
    Quic,
    Stun,
    MtProto,
    Doh,
    Doh3,
    Doq,
    Dot,
    Dns,
}

impl ProtocolKind {
    /// Keywords are case-sensitive, exactly as the manual lists them.
    pub fn parse(s: &str) -> Option<ProtocolKind> {
        Some(match s {
            "HTTP" => ProtocolKind::Http,
            "HTTPS" => ProtocolKind::Https,
            "TCP" => ProtocolKind::Tcp,
            "UDP" => ProtocolKind::Udp,
            "QUIC" => ProtocolKind::Quic,
            "STUN" => ProtocolKind::Stun,
            "MTProto" => ProtocolKind::MtProto,
            "DOH" => ProtocolKind::Doh,
            "DOH3" => ProtocolKind::Doh3,
            "DOQ" => ProtocolKind::Doq,
            "DOT" => ProtocolKind::Dot,
            "DNS" => ProtocolKind::Dns,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessPattern {
    Name(Glob),
    Path(Glob),
    Prefix(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleKind {
    Domain(String),
    DomainSuffix(String),
    DomainKeyword(String),
    DomainWildcard(Glob),
    DomainSet(ResourceRef),
    IpCidr(Ipv4Net),
    IpCidr6(Ipv6Net),
    GeoIp(String),
    IpAsn(u32),
    UserAgent(Glob),
    UrlRegex(Pattern),
    ProcessName(ProcessPattern),
    DestPort(PortExpr),
    SrcPort(PortExpr),
    InPort(PortExpr),
    SrcIp(IpNet),
    DeviceName(Glob),
    MacAddress(String),
    Protocol(ProtocolKind),
    HostnameType(HostnameType),
    Subnet(SubnetExpr),
    CellularRadio(String),
    CellularCarrier(String),
    And(Vec<SubRule>),
    Or(Vec<SubRule>),
    Not(Box<SubRule>),
    Script(String),
    RuleSet(ResourceRef),
    Final,
}

impl RuleKind {
    pub fn type_name(&self) -> &'static str {
        match self {
            RuleKind::Domain(_) => "DOMAIN",
            RuleKind::DomainSuffix(_) => "DOMAIN-SUFFIX",
            RuleKind::DomainKeyword(_) => "DOMAIN-KEYWORD",
            RuleKind::DomainWildcard(_) => "DOMAIN-WILDCARD",
            RuleKind::DomainSet(_) => "DOMAIN-SET",
            RuleKind::IpCidr(_) => "IP-CIDR",
            RuleKind::IpCidr6(_) => "IP-CIDR6",
            RuleKind::GeoIp(_) => "GEOIP",
            RuleKind::IpAsn(_) => "IP-ASN",
            RuleKind::UserAgent(_) => "USER-AGENT",
            RuleKind::UrlRegex(_) => "URL-REGEX",
            RuleKind::ProcessName(_) => "PROCESS-NAME",
            RuleKind::DestPort(_) => "DEST-PORT",
            RuleKind::SrcPort(_) => "SRC-PORT",
            RuleKind::InPort(_) => "IN-PORT",
            RuleKind::SrcIp(_) => "SRC-IP",
            RuleKind::DeviceName(_) => "DEVICE-NAME",
            RuleKind::MacAddress(_) => "MAC-ADDRESS",
            RuleKind::Protocol(_) => "PROTOCOL",
            RuleKind::HostnameType(_) => "HOSTNAME-TYPE",
            RuleKind::Subnet(_) => "SUBNET",
            RuleKind::CellularRadio(_) => "CELLULAR-RADIO",
            RuleKind::CellularCarrier(_) => "CELLULAR-CARRIER",
            RuleKind::And(_) => "AND",
            RuleKind::Or(_) => "OR",
            RuleKind::Not(_) => "NOT",
            RuleKind::Script(_) => "SCRIPT",
            RuleKind::RuleSet(_) => "RULE-SET",
            RuleKind::Final => "FINAL",
        }
    }
    pub fn is_ip_based(&self) -> bool {
        matches!(self, RuleKind::IpCidr(_) | RuleKind::IpCidr6(_) | RuleKind::GeoIp(_) | RuleKind::IpAsn(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubRule {
    pub kind: RuleKind,
    pub no_resolve: bool,
    pub extended_matching: bool,
    pub raw: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RuleParams {
    pub no_resolve: bool,
    pub dns_failed: bool,
    pub extended_matching: bool,
    pub pre_matching: bool,
    pub requires_resolve: bool,
    pub notification_text: Option<String>,
    pub notification_interval: Option<u32>,
    pub update_interval: Option<i64>,
    pub always_capture: Option<String>,
    pub unknown: Vec<String>,
}

impl RuleParams {
    fn parse(fields: &[String]) -> RuleParams {
        let mut p = RuleParams::default();
        for f in fields {
            let (k, v) = match f.split_once('=') {
                Some((k, v)) => (k.trim(), Some(v.trim())),
                None => (f.trim(), None),
            };
            match (k.to_ascii_lowercase().as_str(), v) {
                ("no-resolve", None) => p.no_resolve = true,
                ("dns-failed", None) => p.dns_failed = true,
                ("extended-matching", None) => p.extended_matching = true,
                ("pre-matching", None) => p.pre_matching = true,
                ("requires-resolve", None) => p.requires_resolve = true,
                ("notification-text", Some(v)) => p.notification_text = Some(v.to_string()),
                ("notification-interval", Some(v)) if v.parse::<u32>().is_ok() => p.notification_interval = v.parse().ok(),
                ("update-interval", Some(v)) if v.parse::<i64>().is_ok() => p.update_interval = v.parse().ok(),
                ("always-capture", Some(v)) => p.always_capture = Some(v.to_string()),
                _ => p.unknown.push(f.clone()),
            }
        }
        p
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyRef {
    Builtin(Builtin),
    Named(String),
    Device(String),
}

impl PolicyRef {
    pub fn parse(s: &str) -> PolicyRef {
        let s = s.trim();
        if let Some(b) = Builtin::parse(s) {
            return PolicyRef::Builtin(b);
        }
        if let Some(d) = s.strip_prefix("DEVICE:") {
            return PolicyRef::Device(d.trim().to_string());
        }
        PolicyRef::Named(s.to_string())
    }
    pub fn name(&self) -> String {
        self.to_string()
    }
}

impl fmt::Display for PolicyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyRef::Builtin(b) => f.write_str(b.name()),
            PolicyRef::Named(n) => f.write_str(n),
            PolicyRef::Device(d) => write!(f, "DEVICE:{d}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    pub policy: PolicyRef,
    pub params: RuleParams,
    pub span: Span,
    pub raw: String,
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

fn invalid(ty: &str, value: &str, why: &str) -> ParseError {
    ParseError::new(codes::E_INVALID_RULE_VALUE, format!("{ty}: invalid value `{value}`: {why}"))
}

fn glob(value: &str, ci: bool, classes: bool) -> Result<Glob, String> {
    Glob::new(value, GlobOptions { case_insensitive: ci, classes }).map_err(|e| e.to_string())
}

fn parse_mac(value: &str) -> Option<String> {
    let parts: Vec<&str> = value.split([':', '-']).collect();
    if parts.len() != 6 || !parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    Some(parts.iter().map(|p| p.to_ascii_uppercase()).collect::<Vec<_>>().join(":"))
}

fn parse_kind(ty: &str, value: &str, ctx: &ParseCtx, depth: usize) -> Result<RuleKind, ParseError> {
    let kind = match ty {
        "DOMAIN" => RuleKind::Domain(value.trim_end_matches('.').to_ascii_lowercase()),
        "DOMAIN-SUFFIX" => RuleKind::DomainSuffix(value.trim_matches('.').to_ascii_lowercase()),
        "DOMAIN-KEYWORD" => RuleKind::DomainKeyword(value.to_ascii_lowercase()),
        "DOMAIN-WILDCARD" => RuleKind::DomainWildcard(glob(value, true, true).map_err(|e| invalid(ty, value, &e))?),
        "DOMAIN-SET" => RuleKind::DomainSet(ResourceRef::parse(value, ctx)),
        "IP-CIDR" => RuleKind::IpCidr(
            value.parse::<Ipv4Net>().or_else(|_| value.parse::<Ipv4Addr>().map(|ip| Ipv4Net::new(ip, 32).unwrap())).map_err(|_| invalid(ty, value, "expected IPv4 CIDR"))?,
        ),
        "IP-CIDR6" => RuleKind::IpCidr6(
            value.parse::<Ipv6Net>().or_else(|_| value.parse::<Ipv6Addr>().map(|ip| Ipv6Net::new(ip, 128).unwrap())).map_err(|_| invalid(ty, value, "expected IPv6 CIDR"))?,
        ),
        "GEOIP" => {
            if value.is_empty() || !value.chars().all(|c| c.is_ascii_alphabetic()) {
                return Err(invalid(ty, value, "expected an ISO country code"));
            }
            RuleKind::GeoIp(value.to_ascii_uppercase())
        }
        "IP-ASN" => {
            let digits = value.strip_prefix("AS").or_else(|| value.strip_prefix("as")).unwrap_or(value);
            RuleKind::IpAsn(digits.parse().map_err(|_| invalid(ty, value, "expected a decimal ASN"))?)
        }
        "USER-AGENT" => RuleKind::UserAgent(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?),
        "URL-REGEX" => RuleKind::UrlRegex(Pattern::new(value).map_err(|e| invalid(ty, value, &e))?),
        "PROCESS-NAME" => RuleKind::ProcessName(if value.starts_with('/') && value.ends_with('/') && value.len() > 1 {
            ProcessPattern::Prefix(value.to_string())
        } else if value.starts_with('/') {
            ProcessPattern::Path(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?)
        } else {
            ProcessPattern::Name(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?)
        }),
        "DEST-PORT" => RuleKind::DestPort(PortExpr::parse(value).ok_or_else(|| invalid(ty, value, "expected a port expression"))?),
        "SRC-PORT" => RuleKind::SrcPort(PortExpr::parse(value).ok_or_else(|| invalid(ty, value, "expected a port expression"))?),
        "IN-PORT" => RuleKind::InPort(PortExpr::parse(value).ok_or_else(|| invalid(ty, value, "expected a port expression"))?),
        "SRC-IP" => RuleKind::SrcIp(
            value.parse::<IpNet>().or_else(|_| value.parse::<IpAddr>().map(IpNet::from)).map_err(|_| invalid(ty, value, "expected an IP address or CIDR"))?,
        ),
        "DEVICE-NAME" => RuleKind::DeviceName(glob(value, false, false).map_err(|e| invalid(ty, value, &e))?),
        "MAC-ADDRESS" => RuleKind::MacAddress(parse_mac(value).ok_or_else(|| invalid(ty, value, "expected a MAC address"))?),
        "PROTOCOL" => RuleKind::Protocol(ProtocolKind::parse(value).ok_or_else(|| invalid(ty, value, "unknown protocol keyword (case-sensitive)"))?),
        "HOSTNAME-TYPE" => RuleKind::HostnameType(match value {
            "IPv4" => HostnameType::IPv4,
            "IPv6" => HostnameType::IPv6,
            "DOMAIN" => HostnameType::Domain,
            "SIMPLE" => HostnameType::Simple,
            _ => return Err(invalid(ty, value, "expected IPv4, IPv6, DOMAIN or SIMPLE (case-sensitive)")),
        }),
        "SUBNET" => RuleKind::Subnet(SubnetExpr::parse(value)?),
        "CELLULAR-RADIO" => RuleKind::CellularRadio(value.to_string()),
        "CELLULAR-CARRIER" => RuleKind::CellularCarrier(value.to_string()),
        "AND" | "OR" | "NOT" => {
            let subs = parse_logical_value(value, ctx, depth)?;
            match ty {
                "AND" => RuleKind::And(subs),
                "OR" => RuleKind::Or(subs),
                _ => {
                    if subs.len() != 1 {
                        return Err(ParseError::new(codes::E_SYNTAX, "NOT takes exactly one sub-rule"));
                    }
                    RuleKind::Not(Box::new(subs.into_iter().next().unwrap()))
                }
            }
        }
        "SCRIPT" => RuleKind::Script(value.to_string()),
        "RULE-SET" => RuleKind::RuleSet(ResourceRef::parse(value, ctx)),
        _ => return Err(ParseError::new(codes::E_UNKNOWN_RULE_TYPE, format!("unknown rule type `{ty}`"))),
    };
    Ok(kind)
}

/// `((R1),(R2),...)` -> sub-rules.
fn parse_logical_value(value: &str, ctx: &ParseCtx, depth: usize) -> Result<Vec<SubRule>, ParseError> {
    if depth > MAX_LOGICAL_DEPTH {
        return Err(ParseError::new(codes::E_NESTING_TOO_DEEP, format!("logical rules nested deeper than {MAX_LOGICAL_DEPTH}")));
    }
    let inner = value
        .trim()
        .strip_prefix('(')
        .and_then(|v| v.strip_suffix(')'))
        .ok_or_else(|| ParseError::new(codes::E_SYNTAX, "logical rule value must be wrapped in parentheses"))?;
    let mut subs = Vec::new();
    for item in split_list(inner) {
        let sub = item
            .strip_prefix('(')
            .and_then(|v| v.strip_suffix(')'))
            .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("sub-rule `{item}` must be wrapped in parentheses")))?;
        subs.push(parse_subrule_depth(sub, ctx, depth + 1)?);
    }
    if subs.is_empty() {
        return Err(ParseError::new(codes::E_SYNTAX, "logical rule needs at least one sub-rule"));
    }
    Ok(subs)
}

fn parse_subrule_depth(raw: &str, ctx: &ParseCtx, depth: usize) -> Result<SubRule, ParseError> {
    let fields = split_list(raw);
    let Some(ty) = fields.first() else {
        return Err(ParseError::new(codes::E_SYNTAX, "empty sub-rule"));
    };
    let ty = ty.to_ascii_uppercase();
    if ty == "FINAL" {
        return Err(ParseError::new(codes::E_NOT_ALLOWED_HERE, "FINAL is not allowed as a sub-rule or inside a rule set"));
    }
    let value = fields.get(1).ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("{ty}: missing value")))?;
    let kind = parse_kind(&ty, value, ctx, depth)?;
    let mut sub = SubRule { kind, no_resolve: false, extended_matching: false, raw: raw.trim().to_string() };
    for flag in &fields[2..] {
        match flag.to_ascii_lowercase().as_str() {
            "no-resolve" => sub.no_resolve = true,
            "extended-matching" => sub.extended_matching = true,
            "pre-matching" => return Err(ParseError::new(codes::E_NOT_ALLOWED_HERE, "pre-matching is only allowed on top-level rules")),
            _ => {} // unknown flags are ignored, as Surge does
        }
    }
    Ok(sub)
}

/// Parse a rule without a policy (logical sub-rule, inline rule set line, set file line).
pub fn parse_subrule(raw: &str, ctx: &ParseCtx) -> Result<SubRule, ParseError> {
    parse_subrule_depth(raw, ctx, 1)
}

/// Parse a full `[Rule]` line: `TYPE,VALUE,POLICY[,params...]` (FINAL has no value).
pub fn parse_rule(raw: &str, ctx: &ParseCtx, span: &Span) -> Result<Rule, ParseError> {
    let fields = split_list(raw);
    let Some(ty) = fields.first() else {
        return Err(ParseError::new(codes::E_SYNTAX, "empty rule"));
    };
    let ty = ty.to_ascii_uppercase();
    let (kind, policy_idx) = if ty == "FINAL" {
        (RuleKind::Final, 1)
    } else {
        let value = fields.get(1).ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("{ty}: missing value")))?;
        (parse_kind(&ty, value, ctx, 1)?, 2)
    };
    let policy = fields.get(policy_idx).ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("{ty}: missing policy")))?;
    let policy = PolicyRef::parse(policy);
    let params = RuleParams::parse(&fields[policy_idx + 1..]);
    if params.pre_matching {
        let reject = matches!(&policy, PolicyRef::Builtin(b) if b.is_reject());
        if !reject {
            return Err(ParseError::new(codes::E_NOT_ALLOWED_HERE, "pre-matching requires a REJECT-family policy"));
        }
        if matches!(kind, RuleKind::Protocol(_) | RuleKind::ProcessName(_) | RuleKind::Script(_) | RuleKind::Final) {
            return Err(ParseError::new(codes::E_NOT_ALLOWED_HERE, format!("{ty} does not support pre-matching")));
        }
    }
    Ok(Rule { kind, policy, params, span: span.clone(), raw: raw.trim().to_string() })
}
```

`lib.rs` 增加 `pub mod rule;` 与 `pub use rule::{ParseCtx, PolicyRef, ResourceRef, Rule, RuleKind, RuleParams, SubRule};`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config rule`
Expected: 4 个测试通过。若 `manual_examples_parse_with_expected_types` 因 `to_string()` 与原文差异失败，检查 `raw.trim()` 与测试输入是否有多余空格；canonical 形式即 trim 后的原文。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): [Rule] 解析：29 种规则类型、10 个参数、逻辑规则与子规则"
```

---

### Task 11: `[Host]`、`[Keystore]`、延迟节与 `#!MANAGED-CONFIG`

**Files:**
- Create: `crates/rurge-config/src/host.rs`
- Create: `crates/rurge-config/src/keystore.rs`
- Create: `crates/rurge-config/src/deferred.rs`
- Create: `crates/rurge-config/src/managed.rs`
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: `ParseCtx` / `ResourceRef`（Task 10），`EncryptedDns`（Task 8），`Glob`（Task 5），`split_definition` / `split_list` / `ParamMap`（Task 4），`Section` / `Entry` / `Directive` / `Profile`（Task 3），`ParseError`（Task 9）。
- Produces:
  - `host::SystemMode { System, Syslib, ForceSyslib }`，`host::DnsUpstream { Udp(SocketAddr), Encrypted(EncryptedDns) }`，`host::HostValue { Ips(Vec<IpAddr>), Alias(String), Servers(Vec<DnsUpstream>), System(SystemMode), Script(String) }`，`host::HostKey { Pattern(Glob), Set(ResourceRef) }`，`host::HostEntry { key, raw_key: String, value, span }`，`parse_host_entry(raw, ctx, span) -> Result<HostEntry, ParseError>`。
  - `keystore::KeystoreType { P12, OpensshPrivateKey }`，`keystore::KeystoreItem { name, kind, base64, password: Option<String>, span }`，`parse_keystore_item(name, definition, span) -> Result<KeystoreItem, ParseError>`。
  - `deferred::DeferredSection { name: String, entries: Vec<Entry> }`，`deferred::DeferredSections { sections: Vec<DeferredSection> }`，`DeferredSections::get(name)`，`deferred::is_deferred(name) -> bool`（`MITM` `URL Rewrite` `Header Rewrite` `Body Rewrite` `Map Local` `Script` `Panel` `SSID Setting` `Port Forwarding` `Ponte` `Testing` `DHCP` `Snell Server` `MTProto` 及 `WireGuard ` / `Tailscale ` 前缀）。
  - `managed::ManagedConfig { url: String, interval: Duration, strict: bool }`，`parse_managed(header: &[Directive], diags: &mut Diagnostics) -> Option<ManagedConfig>`（`#!FORBIDDEN-AUTO-UPGRADE` 静默忽略；其他未知头指令 W0014）。

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/host.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::ParseCtx;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::Arc;

    fn parse(raw: &str) -> Result<HostEntry, ParseError> {
        let inline = HashSet::new();
        let ctx = ParseCtx { inline_rulesets: &inline, base_dir: Path::new("/p") };
        parse_host_entry(raw, &ctx, &Span::new(Arc::from(Path::new("h.conf")), 1))
    }

    #[test]
    fn manual_examples() {
        let e = parse("abc.com = 1.2.3.4, 5.6.7.8, ::1").unwrap();
        assert!(matches!(e.value, HostValue::Ips(ref v) if v.len() == 3));
        assert!(matches!(e.key, HostKey::Pattern(_)));
        let e = parse("*.dev = 6.7.8.9").unwrap();
        assert!(matches!(&e.key, HostKey::Pattern(g) if g.matches("x.dev")));
        assert_eq!(parse("foo.com = bar.com").unwrap().value, HostValue::Alias("bar.com".into()));
        let e = parse("bar.com = server:8.8.8.8,1.1.1.1").unwrap();
        assert!(matches!(e.value, HostValue::Servers(ref s) if s.len() == 2));
        let e = parse("example.com = server:https://cloudflare-dns.com/dns-query").unwrap();
        assert!(matches!(e.value, HostValue::Servers(ref s) if matches!(s[0], DnsUpstream::Encrypted(_))));
        assert_eq!(parse("Macbook = server:system").unwrap().value, HostValue::System(SystemMode::System));
        assert_eq!(parse("x = server:syslib").unwrap().value, HostValue::System(SystemMode::Syslib));
        assert_eq!(parse("x = server:force-syslib").unwrap().value, HostValue::System(SystemMode::ForceSyslib));
        assert_eq!(parse("*.example.com = script:dnspod").unwrap().value, HostValue::Script("dnspod".into()));
        let e = parse("DOMAIN-SET:https://example.com/domains.txt = server:https://doh.example.com/dns-query").unwrap();
        assert!(matches!(e.key, HostKey::Set(ResourceRef::Url(_))));
        let e = parse("RULE-SET:https://example.com/rules.txt = 10.0.0.10").unwrap();
        assert!(matches!(e.key, HostKey::Set(ResourceRef::Url(_))));
        assert_eq!(e.value, HostValue::Ips(vec!["10.0.0.10".parse().unwrap()]));
    }

    #[test]
    fn errors() {
        assert_eq!(parse("no-equals").unwrap_err().code, codes::E_INVALID_DEFINITION);
        assert_eq!(parse("a.com = ").unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse("a.com = server:").unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse("a.com = server:not an ip").unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse("a.com = 1.2.3.4, not-ip").unwrap_err().code, codes::E_SYNTAX);
    }
}
```

`crates/rurge-config/src/keystore.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    fn span() -> Span {
        Span::new(Arc::from(Path::new("k.conf")), 1)
    }

    #[test]
    fn items() {
        let i = parse_keystore_item("cert1", "type=p12, base64=AAAA, password=123456", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::P12);
        assert_eq!(i.password.as_deref(), Some("123456"));
        let i = parse_keystore_item("key1", "type=openssh-private-key, base64=BBBB", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::OpensshPrivateKey);
        let i = parse_keystore_item("cert2", "base64=CCCC, password=x", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::P12);
        let i = parse_keystore_item("key2", "base64=DDDD", &span()).unwrap();
        assert_eq!(i.kind, KeystoreType::OpensshPrivateKey);
        assert_eq!(parse_keystore_item("bad", "type=p12, password=x", &span()).unwrap_err().code, codes::E_SYNTAX);
        assert_eq!(parse_keystore_item("bad", "type=jks, base64=x", &span()).unwrap_err().code, codes::E_INVALID_RULE_VALUE);
    }
}
```

`crates/rurge-config/src/managed.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Origin, parse_str};
    use std::path::Path;
    use std::sync::Arc;

    fn header(text: &str) -> (Option<ManagedConfig>, Diagnostics) {
        let (p, mut d) = parse_str(text, Arc::from(Path::new("m.conf")), Origin::Main);
        let m = parse_managed(&p.header, &mut d);
        (m, d)
    }

    #[test]
    fn managed_directive() {
        let (m, d) = header("#!MANAGED-CONFIG http://test.com/surge.conf interval=60 strict=true\n#!FORBIDDEN-AUTO-UPGRADE smart-group\n[General]\n");
        assert!(d.is_empty(), "{:?}", d.into_vec());
        let m = m.unwrap();
        assert_eq!(m.url, "http://test.com/surge.conf");
        assert_eq!(m.interval, Duration::from_secs(60));
        assert!(m.strict);
        let (m, _) = header("#!MANAGED-CONFIG https://x/y\n[General]\n");
        let m = m.unwrap();
        assert_eq!(m.interval, Duration::from_secs(86400));
        assert!(!m.strict);
        let (m, d) = header("#!SOMETHING-ELSE\n[General]\n");
        assert!(m.is_none());
        assert_eq!(d.iter().next().unwrap().code, codes::W_UNKNOWN_DIRECTIVE);
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config host keystore managed`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`crates/rurge-config/src/host.rs`：

```rust
//! `[Host]` local DNS mapping entries.

use crate::diagnostic::{ParseError, codes};
use crate::general::EncryptedDns;
use crate::glob::{Glob, GlobOptions};
use crate::rule::{ParseCtx, ResourceRef};
use crate::span::Span;
use crate::value::{split_definition, split_list};
use std::net::{IpAddr, SocketAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemMode {
    System,
    Syslib,
    ForceSyslib,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsUpstream {
    Udp(SocketAddr),
    Encrypted(EncryptedDns),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostValue {
    Ips(Vec<IpAddr>),
    Alias(String),
    Servers(Vec<DnsUpstream>),
    System(SystemMode),
    Script(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostKey {
    Pattern(Glob),
    Set(ResourceRef),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub key: HostKey,
    pub raw_key: String,
    pub value: HostValue,
    pub span: Span,
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    (s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &s[prefix.len()..])
}

fn parse_upstream(item: &str) -> Option<DnsUpstream> {
    if let Some(enc) = EncryptedDns::parse(item) {
        return Some(DnsUpstream::Encrypted(enc));
    }
    if let Ok(sa) = item.parse::<SocketAddr>() {
        return Some(DnsUpstream::Udp(sa));
    }
    let bare = item.strip_prefix('[').and_then(|x| x.strip_suffix(']')).unwrap_or(item);
    bare.parse::<IpAddr>().ok().map(|ip| DnsUpstream::Udp(SocketAddr::new(ip, 53)))
}

pub fn parse_host_entry(raw: &str, ctx: &ParseCtx, span: &Span) -> Result<HostEntry, ParseError> {
    let (key, value) = split_definition(raw)
        .ok_or_else(|| ParseError::new(codes::E_INVALID_DEFINITION, format!("expected `<host> = <value>`, found `{raw}`")))?;
    if value.is_empty() {
        return Err(ParseError::new(codes::E_SYNTAX, format!("`{key}`: empty value")));
    }
    let host_key = if let Some(r) = strip_prefix_ci(key, "DOMAIN-SET:").or_else(|| strip_prefix_ci(key, "RULE-SET:")) {
        HostKey::Set(ResourceRef::parse(r, ctx))
    } else {
        HostKey::Pattern(
            Glob::new(key, GlobOptions { case_insensitive: true, classes: false })
                .map_err(|e| ParseError::new(codes::E_SYNTAX, format!("`{key}`: {e}")))?,
        )
    };
    let host_value = if let Some(rest) = strip_prefix_ci(value, "server:") {
        match rest.trim().to_ascii_lowercase().as_str() {
            "system" => HostValue::System(SystemMode::System),
            "syslib" => HostValue::System(SystemMode::Syslib),
            "force-syslib" => HostValue::System(SystemMode::ForceSyslib),
            "" => return Err(ParseError::new(codes::E_SYNTAX, format!("`{key}`: `server:` needs at least one server"))),
            _ => {
                let mut servers = Vec::new();
                for item in split_list(rest) {
                    servers.push(parse_upstream(&item).ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("`{key}`: invalid DNS server `{item}`")))?);
                }
                HostValue::Servers(servers)
            }
        }
    } else if let Some(name) = strip_prefix_ci(value, "script:") {
        HostValue::Script(name.trim().to_string())
    } else {
        let items = split_list(value);
        let ips: Vec<IpAddr> = items.iter().filter_map(|i| i.parse::<IpAddr>().ok()).collect();
        if ips.len() == items.len() && !ips.is_empty() {
            HostValue::Ips(ips)
        } else if items.len() == 1 && !items[0].contains(' ') && items[0].contains('.') && ips.is_empty() {
            HostValue::Alias(items[0].to_ascii_lowercase())
        } else {
            return Err(ParseError::new(codes::E_SYNTAX, format!("`{key}`: expected IP addresses, a hostname alias, `server:` or `script:`")));
        }
    };
    Ok(HostEntry { key: host_key, raw_key: key.to_string(), value: host_value, span: span.clone() })
}
```

`crates/rurge-config/src/keystore.rs`：

```rust
//! `[Keystore]` certificates and private keys.

use crate::diagnostic::{ParseError, codes};
use crate::span::Span;
use crate::value::{ParamMap, split_list};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeystoreType {
    P12,
    OpensshPrivateKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeystoreItem {
    pub name: String,
    pub kind: KeystoreType,
    pub base64: String,
    pub password: Option<String>,
    pub span: Span,
}

pub fn parse_keystore_item(name: &str, definition: &str, span: &Span) -> Result<KeystoreItem, ParseError> {
    let (params, _) = ParamMap::from_fields(&split_list(definition));
    let base64 = params
        .get("base64")
        .ok_or_else(|| ParseError::new(codes::E_SYNTAX, format!("keystore item `{name}`: missing `base64`")))?
        .to_string();
    let password = params.get("password").map(str::to_string);
    let kind = match params.get("type").map(|t| t.to_ascii_lowercase()) {
        Some(t) if t == "p12" => KeystoreType::P12,
        Some(t) if t == "openssh-private-key" => KeystoreType::OpensshPrivateKey,
        Some(t) => return Err(ParseError::new(codes::E_INVALID_RULE_VALUE, format!("keystore item `{name}`: unknown type `{t}`"))),
        None if password.is_some() => KeystoreType::P12,
        None => KeystoreType::OpensshPrivateKey,
    };
    Ok(KeystoreItem { name: name.to_string(), kind, base64, password, span: span.clone() })
}
```

`crates/rurge-config/src/deferred.rs`：

```rust
//! Sections that are parsed and kept but have no behaviour in this version.

use crate::text::{Entry, Profile};

const DEFERRED: &[&str] = &[
    "MITM", "URL Rewrite", "Header Rewrite", "Body Rewrite", "Map Local", "Script", "Panel", "SSID Setting",
    "Port Forwarding", "Ponte", "Testing", "DHCP", "Snell Server", "MTProto",
];

pub fn is_deferred(name: &str) -> bool {
    DEFERRED.iter().any(|d| d.eq_ignore_ascii_case(name))
        || (name.len() > 10 && name[..10].eq_ignore_ascii_case("WireGuard "))
        || (name.len() > 10 && name[..10].eq_ignore_ascii_case("Tailscale "))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredSection {
    pub name: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeferredSections {
    pub sections: Vec<DeferredSection>,
}

impl DeferredSections {
    pub fn get(&self, name: &str) -> Option<&DeferredSection> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }
    pub fn collect(profile: &Profile) -> DeferredSections {
        DeferredSections {
            sections: profile
                .sections
                .iter()
                .filter(|s| is_deferred(&s.name))
                .map(|s| DeferredSection { name: s.name.clone(), entries: s.active_entries().cloned().collect() })
                .collect(),
        }
    }
}
```

`crates/rurge-config/src/managed.rs`：

```rust
//! `#!MANAGED-CONFIG` and other header directives.

use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::text::Directive;
use crate::value::parse_bool;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedConfig {
    pub url: String,
    pub interval: Duration,
    pub strict: bool,
}

pub fn parse_managed(header: &[Directive], diags: &mut Diagnostics) -> Option<ManagedConfig> {
    let mut managed = None;
    for d in header {
        if let Some(rest) = d.raw.strip_prefix("#!MANAGED-CONFIG") {
            let mut parts = rest.split_whitespace();
            let Some(url) = parts.next() else {
                diags.push(Diagnostic::warning(codes::W_INVALID_VALUE, "#!MANAGED-CONFIG without a URL is ignored").at(d.span.clone()));
                continue;
            };
            let mut cfg = ManagedConfig { url: url.to_string(), interval: Duration::from_secs(86400), strict: false };
            for p in parts {
                match p.split_once('=') {
                    Some(("interval", v)) => match v.parse::<u64>() {
                        Ok(s) => cfg.interval = Duration::from_secs(s),
                        Err(_) => diags.push(Diagnostic::warning(codes::W_INVALID_VALUE, format!("invalid interval `{v}`")).at(d.span.clone())),
                    },
                    Some(("strict", v)) => match parse_bool(v) {
                        Some(b) => cfg.strict = b,
                        None => diags.push(Diagnostic::warning(codes::W_INVALID_VALUE, format!("invalid strict `{v}`")).at(d.span.clone())),
                    },
                    _ => diags.push(Diagnostic::warning(codes::W_INVALID_VALUE, format!("unknown MANAGED-CONFIG parameter `{p}`")).at(d.span.clone())),
                }
            }
            managed = Some(cfg);
        } else if d.raw.starts_with("#!FORBIDDEN-AUTO-UPGRADE") {
            // rurge never auto-upgrades profiles; nothing to do.
        } else {
            diags.push(Diagnostic::warning(codes::W_UNKNOWN_DIRECTIVE, format!("unknown directive ignored: {}", d.raw)).at(d.span.clone()));
        }
    }
    managed
}
```

`lib.rs` 增加 `pub mod deferred; pub mod host; pub mod keystore; pub mod managed;` 与 `pub use host::{HostEntry, HostKey, HostValue}; pub use keystore::KeystoreItem; pub use managed::ManagedConfig; pub use deferred::DeferredSections;`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): [Host]、[Keystore]、延迟节收集与 #!MANAGED-CONFIG 解析"
```

---

### Task 12: `Config` 组装与交叉校验

**Files:**
- Create: `crates/rurge-config/src/config.rs`
- Modify: `crates/rurge-config/src/diagnostic.rs`（新增 `W_RULESET_LINE_SKIPPED = "W0018"`、`W_RULES_AFTER_FINAL = "W0019"`）
- Modify: `crates/rurge-config/src/lib.rs`

**Interfaces:**
- Consumes: 全部前序任务。
- Produces:
  - `config::Platform { Windows, Linux, MacOs }`，`Platform::current()`，`system_name() -> &'static str`（`Windows` / `Linux` / `macOS`），`Platform::parse(&str)`。
  - `config::Capabilities { policy_kinds: HashSet<PolicyKind>, group_kinds: HashSet<GroupKind>, rule_types: HashSet<&'static str> }`，`Capabilities::all()`，`Capabilities::ALL_RULE_TYPES: [&str; 29]`。
  - `config::LoadOptions { environment: Environment, platform: Platform, capabilities: Capabilities }`，`LoadOptions::for_tests()`（`Environment::fixed()`、`Platform::Linux`、`Capabilities::all()`）。
  - `config::InlineRuleset { name, rules: Vec<SubRule>, span }`，`config::SourceInfo { main: PathBuf, includes: Vec<PathBuf> }`。
  - `config::Config { general, policies, groups, rules, rulesets, hosts, keystore, deferred, managed, unknown_sections: Vec<String>, source }`，`Config::resolve_policy(name) -> Option<PolicyTarget>`（`PolicyTarget::{Builtin(Builtin), Proxy(&ProxyPolicy), Group(&PolicyGroup)}`），`Config::summary() -> ConfigSummary`（`Serialize`，供快照与 API）。
  - `config::Loaded { config: Config, diagnostics: Diagnostics }`，`config::LoadError::Io { path, source }`。
  - `config::load(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError>`，`config::from_text(text: &str, path: &Path, opts) -> Loaded`，`config::from_profile(profile: Profile, base_dir: &Path, opts) -> Loaded`。

交叉校验规则（设计文档 4.3）：

| 检查 | 诊断 |
| --- | --- |
| `[Proxy]` / `[Proxy Group]` 行缺 `=` | E0017 |
| 策略名重定义 `DIRECT` | 静默丢弃该行 |
| 策略名重定义其他内置名 | E0005 |
| 策略 / 组重名 | E0006 |
| 规则引用不存在的策略 | E0007 |
| 组成员 / subnet 条件引用不存在的策略 | E0008 |
| 组循环引用 | E0009 |
| 没有启用的 FINAL | E0010 |
| FINAL 之后还有规则 | W0019 |
| 规则未知参数 | W0003 |
| 内联规则集中的非法行 | W0018（跳过该行） |
| 未知节 | W0002（每个节一次） |
| 存在延迟节 | W0016（一条，列出全部） |
| 协议 / 组类型 / 规则类型不在 `capabilities` 中 | W0007 / W0008 / W0013（每种一次） |
| `DEVICE:` 策略 | W0010 |
| iOS 专属内置策略 | W0009（每种一次） |

- [ ] **Step 1: 写测试**

`crates/rurge-config/src/config.rs` 末尾：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_text(text: &str) -> Loaded {
        from_text(text, Path::new("/profiles/t.conf"), &LoadOptions::for_tests())
    }

    fn codes_of(l: &Loaded) -> Vec<&'static str> {
        l.diagnostics.iter().map(|d| d.code).collect()
    }

    const QUICK_START: &str = "[General]\ndns-server = system, 1.1.1.1, 8.8.8.8\n\n[Proxy]\nProxyA = https, proxy.example.com, 443, username, password\n\n[Proxy Group]\nProxy = select, ProxyA, DIRECT\n\n[Rule]\nDOMAIN-SUFFIX,example.com,Proxy\nGEOIP,CN,DIRECT\nFINAL,Proxy\n";

    #[test]
    fn quick_start_loads_cleanly() {
        let l = load_text(QUICK_START);
        assert!(!l.diagnostics.has_errors(), "{:?}", l.diagnostics.into_vec());
        let c = l.config;
        assert_eq!(c.policies.len(), 1);
        assert_eq!(c.groups.len(), 1);
        assert_eq!(c.rules.len(), 3);
        assert!(matches!(c.resolve_policy("Proxy"), Some(PolicyTarget::Group(_))));
        assert!(matches!(c.resolve_policy("ProxyA"), Some(PolicyTarget::Proxy(_))));
        assert!(matches!(c.resolve_policy("REJECT"), Some(PolicyTarget::Builtin(Builtin::Reject))));
        assert!(c.resolve_policy("nope").is_none());
        assert_eq!(c.source.main, Path::new("/profiles/t.conf"));
        let s = c.summary();
        assert_eq!(s.rules.len(), 3);
        assert_eq!(s.policies, ["ProxyA (https)"]);
    }

    #[test]
    fn reference_errors() {
        let l = load_text("[Proxy]\nA = direct\n[Proxy Group]\nG1 = select, G2, A\nG2 = select, G1\nG3 = select, Missing\n[Rule]\nDOMAIN,a,Nope\nFINAL,DIRECT\n");
        let c = codes_of(&l);
        assert!(c.contains(&codes::E_UNKNOWN_POLICY_REF));
        assert!(c.contains(&codes::E_UNKNOWN_GROUP_MEMBER));
        assert!(c.contains(&codes::E_GROUP_CYCLE));
        assert!(l.diagnostics.has_errors());
    }

    #[test]
    fn final_rules() {
        let l = load_text("[Rule]\nDOMAIN,a,DIRECT\n");
        assert!(codes_of(&l).contains(&codes::E_MISSING_FINAL));
        let l = load_text("[Rule]\nFINAL,DIRECT\nDOMAIN,a,DIRECT\n");
        assert!(!l.diagnostics.has_errors());
        assert!(codes_of(&l).contains(&codes::W_RULES_AFTER_FINAL));
        let l = load_text("[Rule]\nFINAL,DIRECT #!IOS-ONLY\n");
        assert!(codes_of(&l).contains(&codes::E_MISSING_FINAL));
    }

    #[test]
    fn names_and_builtins() {
        let l = load_text("[Proxy]\nDIRECT = direct\nREJECT = direct\nA = direct\nA = reject\n[Proxy Group]\nA = select, DIRECT\n[Rule]\nFINAL,DIRECT\n");
        let c = codes_of(&l);
        assert_eq!(l.config.policies.len(), 1, "DIRECT redefinition dropped silently, duplicates rejected");
        assert!(c.contains(&codes::E_BUILTIN_REDEFINED));
        assert_eq!(c.iter().filter(|x| **x == codes::E_DUPLICATE_NAME).count(), 2);
    }

    #[test]
    fn warnings() {
        let l = load_text("[General]\nloglevel = notify\n[Weird]\nx = 1\n[MITM]\nhostname = *\n[Script]\ns = type=generic, script-path=a.js\n[Ruleset Inline]\nDOMAIN,a.com\nBOGUS,b\nFINAL,DIRECT\nDOMAIN-SUFFIX,b.com,extended-matching\n[Rule]\nRULE-SET,Inline,DIRECT,mystery\nDOMAIN,a,CELLULAR\nDOMAIN,b,DEVICE:Home\nDOMAIN,c,HYBRID\nFINAL,DIRECT\n");
        assert!(!l.diagnostics.has_errors(), "{:?}", l.diagnostics.into_vec());
        let c = codes_of(&l);
        assert!(c.contains(&codes::W_UNKNOWN_SECTION));
        assert_eq!(c.iter().filter(|x| **x == codes::W_DEFERRED_SECTION).count(), 1);
        assert_eq!(c.iter().filter(|x| **x == codes::W_RULESET_LINE_SKIPPED).count(), 2);
        assert_eq!(l.config.rulesets[0].rules.len(), 2);
        assert!(c.contains(&codes::W_UNKNOWN_RULE_PARAM));
        assert_eq!(c.iter().filter(|x| **x == codes::W_IOS_BUILTIN_AS_DIRECT).count(), 2);
        assert!(c.contains(&codes::W_DEVICE_POLICY_AS_REJECT));
        assert_eq!(l.config.unknown_sections, ["Weird"]);
        assert_eq!(l.config.deferred.sections.len(), 2);
    }

    #[test]
    fn capability_warnings() {
        let mut opts = LoadOptions::for_tests();
        opts.capabilities.policy_kinds = HashSet::from([PolicyKind::Direct]);
        opts.capabilities.group_kinds = HashSet::from([GroupKind::Select]);
        opts.capabilities.rule_types.remove("PROCESS-NAME");
        let l = from_text("[Proxy]\nA = ss, 1.2.3.4, 1, encrypt-method=aes-128-gcm, password=x\nB = ss, 1.2.3.4, 2, encrypt-method=aes-128-gcm, password=x\n[Proxy Group]\nG = url-test, A, B\n[Rule]\nPROCESS-NAME,ssh,DIRECT\nPROCESS-NAME,curl,DIRECT\nFINAL,G\n", Path::new("/p/t.conf"), &opts);
        let c = codes_of(&l);
        assert_eq!(c.iter().filter(|x| **x == codes::W_PROTOCOL_NOT_IMPLEMENTED).count(), 1);
        assert_eq!(c.iter().filter(|x| **x == codes::W_GROUP_NOT_IMPLEMENTED).count(), 1);
        assert_eq!(c.iter().filter(|x| **x == codes::W_RULE_NEVER_MATCHES).count(), 1);
        assert!(!l.diagnostics.has_errors());
    }

    #[test]
    fn includes_are_recorded_in_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("main.conf"), "[Proxy]\n#!include p.dconf\n[Rule]\nFINAL,A\n").unwrap();
        std::fs::write(dir.path().join("p.dconf"), "[Proxy]\nA = direct\n").unwrap();
        let l = load(&dir.path().join("main.conf"), &LoadOptions::for_tests()).unwrap();
        assert!(!l.diagnostics.has_errors());
        assert_eq!(l.config.source.includes.len(), 1);
        assert!(l.config.source.includes[0].ends_with("p.dconf"));
        assert!(matches!(load(Path::new("/definitely/missing.conf"), &LoadOptions::for_tests()), Err(LoadError::Io { .. })));
    }
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config config`
Expected: 编译失败。

- [ ] **Step 3: 实现**

`diagnostic.rs` 的 `codes` 追加：

```rust
    pub const W_RULESET_LINE_SKIPPED: &str = "W0018";
    pub const W_RULES_AFTER_FINAL: &str = "W0019";
```

`crates/rurge-config/src/config.rs`：

```rust
//! Semantic layer: `Config` assembled from a `Profile`, plus cross validation.

use crate::deferred::DeferredSections;
use crate::diagnostic::{Diagnostic, Diagnostics, codes};
use crate::general::{General, parse_general};
use crate::host::{HostEntry, parse_host_entry};
use crate::keystore::{KeystoreItem, parse_keystore_item};
use crate::managed::{ManagedConfig, parse_managed};
use crate::policy::{Builtin, GroupKind, PolicyGroup, PolicyKind, ProxyPolicy, parse_group, parse_policy};
use crate::requirement::{self, Environment};
use crate::rule::{ParseCtx, PolicyRef, Rule, RuleKind, SubRule, parse_rule, parse_subrule};
use crate::span::Span;
use crate::text::include::{self, IncludeOptions};
use crate::text::{Origin, Profile, SectionKind, parse_str};
use crate::value::split_definition;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    Linux,
    MacOs,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else {
            Platform::Linux
        }
    }
    pub fn system_name(&self) -> &'static str {
        match self {
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
            Platform::MacOs => "macOS",
        }
    }
    pub fn parse(s: &str) -> Option<Platform> {
        Some(match s.to_ascii_lowercase().as_str() {
            "windows" => Platform::Windows,
            "linux" => Platform::Linux,
            "macos" => Platform::MacOs,
            _ => return None,
        })
    }
}

/// What the running engine actually implements; used to warn about inactive config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub policy_kinds: HashSet<PolicyKind>,
    pub group_kinds: HashSet<GroupKind>,
    pub rule_types: HashSet<&'static str>,
}

impl Capabilities {
    pub const ALL_RULE_TYPES: [&'static str; 29] = [
        "DOMAIN", "DOMAIN-SUFFIX", "DOMAIN-KEYWORD", "DOMAIN-WILDCARD", "DOMAIN-SET", "IP-CIDR", "IP-CIDR6", "GEOIP", "IP-ASN",
        "USER-AGENT", "URL-REGEX", "PROCESS-NAME", "DEST-PORT", "SRC-PORT", "IN-PORT", "SRC-IP", "DEVICE-NAME", "MAC-ADDRESS",
        "PROTOCOL", "HOSTNAME-TYPE", "SUBNET", "CELLULAR-RADIO", "CELLULAR-CARRIER", "AND", "OR", "NOT", "SCRIPT", "RULE-SET",
        "FINAL",
    ];

    pub fn all() -> Self {
        use PolicyKind::*;
        Self {
            policy_kinds: HashSet::from([
                Http, Https, H2Connect, Socks5, Socks5Tls, Shadowsocks, Snell, Vmess, Trojan, Tuic, TuicV5, Hysteria2, Masque, AnyTls,
                TrustTunnel, Ssh, WireGuard, Tailscale, External, Direct, Reject, RejectDrop, RejectNoDrop, RejectTinyGif,
            ]),
            group_kinds: HashSet::from([GroupKind::Select, GroupKind::UrlTest, GroupKind::Fallback, GroupKind::LoadBalance, GroupKind::Smart, GroupKind::Subnet]),
            rule_types: Self::ALL_RULE_TYPES.iter().copied().collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadOptions {
    pub environment: Environment,
    pub platform: Platform,
    pub capabilities: Capabilities,
}

impl LoadOptions {
    pub fn for_tests() -> Self {
        Self { environment: Environment::fixed(), platform: Platform::Linux, capabilities: Capabilities::all() }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRuleset {
    pub name: String,
    pub rules: Vec<SubRule>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub main: PathBuf,
    pub includes: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub general: General,
    pub policies: Vec<ProxyPolicy>,
    pub groups: Vec<PolicyGroup>,
    pub rules: Vec<Rule>,
    pub rulesets: Vec<InlineRuleset>,
    pub hosts: Vec<HostEntry>,
    pub keystore: Vec<KeystoreItem>,
    pub deferred: DeferredSections,
    pub managed: Option<ManagedConfig>,
    pub unknown_sections: Vec<String>,
    pub source: SourceInfo,
}

pub enum PolicyTarget<'a> {
    Builtin(Builtin),
    Proxy(&'a ProxyPolicy),
    Group(&'a PolicyGroup),
}

/// Stable, serialisable overview used by snapshots and the API.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ConfigSummary {
    pub listeners: Vec<String>,
    pub policies: Vec<String>,
    pub groups: Vec<String>,
    pub rules: Vec<String>,
    pub rulesets: Vec<String>,
    pub hosts: Vec<String>,
    pub keystore: Vec<String>,
    pub deferred: Vec<String>,
    pub unknown_sections: Vec<String>,
    pub managed: Option<String>,
}

impl Config {
    pub fn resolve_policy(&self, name: &str) -> Option<PolicyTarget<'_>> {
        if let Some(p) = self.policies.iter().find(|p| p.name == name) {
            return Some(PolicyTarget::Proxy(p));
        }
        if let Some(g) = self.groups.iter().find(|g| g.name == name) {
            return Some(PolicyTarget::Group(g));
        }
        Builtin::parse(name).map(PolicyTarget::Builtin)
    }

    pub fn summary(&self) -> ConfigSummary {
        ConfigSummary {
            listeners: self
                .general
                .http_listen
                .iter()
                .map(|l| format!("http {}", l.addr))
                .chain(self.general.socks5_listen.iter().map(|l| format!("socks5 {}", l.addr)))
                .collect(),
            policies: self.policies.iter().map(|p| format!("{} ({})", p.name, p.kind.keyword())).collect(),
            groups: self.groups.iter().map(|g| format!("{} ({}) -> [{}]", g.name, g.kind.keyword(), g.members.join(", "))).collect(),
            rules: self.rules.iter().map(|r| r.to_string()).collect(),
            rulesets: self.rulesets.iter().map(|s| format!("{}: {} rules", s.name, s.rules.len())).collect(),
            hosts: self.hosts.iter().map(|h| h.raw_key.clone()).collect(),
            keystore: self.keystore.iter().map(|k| k.name.clone()).collect(),
            deferred: self.deferred.sections.iter().map(|s| s.name.clone()).collect(),
            unknown_sections: self.unknown_sections.clone(),
            managed: self.managed.as_ref().map(|m| m.url.clone()),
        }
    }
}

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub diagnostics: Diagnostics,
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("cannot read `{path}`: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn load(path: &Path, opts: &LoadOptions) -> Result<Loaded, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|source| LoadError::Io { path: path.to_path_buf(), source })?;
    Ok(from_text(&text, path, opts))
}

pub fn from_text(text: &str, path: &Path, opts: &LoadOptions) -> Loaded {
    let file: Arc<Path> = Arc::from(path);
    let (mut profile, mut diags) = parse_str(text, file, Origin::Main);
    let base_dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new(".")).to_path_buf();
    include::expand(&mut profile, &IncludeOptions { base_dir: base_dir.clone(), max_depth: 8 }, &mut diags);
    requirement::apply(&mut profile, &opts.environment, &mut diags);
    let mut loaded = from_profile(profile, &base_dir, opts);
    diags.extend(loaded.diagnostics);
    loaded.diagnostics = diags;
    loaded
}

fn definition_entries<'a>(profile: &'a Profile, section: &str, diags: &mut Diagnostics) -> Vec<(&'a str, &'a str, &'a Span)> {
    let mut out = Vec::new();
    if let Some(sec) = profile.section(section) {
        for e in sec.active_entries() {
            match split_definition(&e.raw) {
                Some((name, def)) => out.push((name, def, &e.span)),
                None => diags.push(Diagnostic::error(codes::E_INVALID_DEFINITION, format!("[{section}]: expected `Name = ...`, found `{}`", e.raw)).at(e.span.clone())),
            }
        }
    }
    out
}

pub fn from_profile(profile: Profile, base_dir: &Path, opts: &LoadOptions) -> Loaded {
    let mut diags = Diagnostics::default();
    let main = profile.main.as_deref().map(Path::to_path_buf).unwrap_or_default();

    let managed = parse_managed(&profile.header, &mut diags);
    let general = parse_general(profile.section("General"), &mut diags);

    let inline_names: HashSet<String> = profile.sections_with_prefix("Ruleset ").map(|s| s.name["Ruleset ".len()..].trim().to_string()).collect();
    let ctx = ParseCtx { inline_rulesets: &inline_names, base_dir };

    // Policies and groups with name registry.
    let mut names: HashMap<String, &'static str> = HashMap::new();
    let mut policies = Vec::new();
    for (name, def, span) in definition_entries(&profile, "Proxy", &mut diags) {
        match Builtin::parse(name) {
            Some(Builtin::Direct) => continue,
            Some(_) => {
                diags.push(Diagnostic::error(codes::E_BUILTIN_REDEFINED, format!("`{name}` is a built-in policy and cannot be redefined")).at(span.clone()));
                continue;
            }
            None => {}
        }
        if names.insert(name.to_string(), "policy").is_some() {
            diags.push(Diagnostic::error(codes::E_DUPLICATE_NAME, format!("duplicate policy name `{name}`")).at(span.clone()));
            continue;
        }
        match parse_policy(name, def, span) {
            Ok(p) => policies.push(p),
            Err(e) => diags.push(Diagnostic::from_parse(e, span.clone())),
        }
    }
    let mut groups = Vec::new();
    for (name, def, span) in definition_entries(&profile, "Proxy Group", &mut diags) {
        if Builtin::parse(name).is_some() {
            diags.push(Diagnostic::error(codes::E_BUILTIN_REDEFINED, format!("`{name}` is a built-in policy and cannot be a group name")).at(span.clone()));
            continue;
        }
        if names.insert(name.to_string(), "group").is_some() {
            diags.push(Diagnostic::error(codes::E_DUPLICATE_NAME, format!("duplicate policy group name `{name}`")).at(span.clone()));
            continue;
        }
        match parse_group(name, def, span) {
            Ok(g) => groups.push(g),
            Err(e) => diags.push(Diagnostic::from_parse(e, span.clone())),
        }
    }

    // Inline rule sets.
    let mut rulesets = Vec::new();
    for sec in profile.sections_with_prefix("Ruleset ") {
        let name = sec.name["Ruleset ".len()..].trim().to_string();
        let mut rules = Vec::new();
        for e in sec.active_entries() {
            match parse_subrule(&e.raw, &ctx) {
                Ok(r) => rules.push(r),
                Err(err) => diags.push(Diagnostic::warning(codes::W_RULESET_LINE_SKIPPED, format!("[{}]: line skipped: {}", sec.name, err.message)).at(e.span.clone())),
            }
        }
        rulesets.push(InlineRuleset { name, rules, span: sec.span.clone() });
    }

    // Rules.
    let mut rules = Vec::new();
    if let Some(sec) = profile.section("Rule") {
        for e in sec.active_entries() {
            match parse_rule(&e.raw, &ctx, &e.span) {
                Ok(r) => {
                    for u in &r.params.unknown {
                        diags.push(Diagnostic::warning(codes::W_UNKNOWN_RULE_PARAM, format!("unknown rule parameter `{u}` ignored")).at(e.span.clone()));
                    }
                    rules.push(r);
                }
                Err(err) => diags.push(Diagnostic::from_parse(err, e.span.clone())),
            }
        }
    }

    // Hosts and keystore.
    let mut hosts = Vec::new();
    if let Some(sec) = profile.section("Host") {
        for e in sec.active_entries() {
            match parse_host_entry(&e.raw, &ctx, &e.span) {
                Ok(h) => hosts.push(h),
                Err(err) => diags.push(Diagnostic::from_parse(err, e.span.clone())),
            }
        }
    }
    let mut keystore = Vec::new();
    for (name, def, span) in definition_entries(&profile, "Keystore", &mut diags) {
        match parse_keystore_item(name, def, span) {
            Ok(k) => keystore.push(k),
            Err(err) => diags.push(Diagnostic::from_parse(err, span.clone())),
        }
    }

    // Deferred and unknown sections.
    let deferred = DeferredSections::collect(&profile);
    if !deferred.sections.is_empty() {
        let list: Vec<String> = deferred.sections.iter().map(|s| format!("[{}]", s.name)).collect();
        diags.push(Diagnostic::warning(codes::W_DEFERRED_SECTION, format!("sections parsed but inactive in this version: {}", list.join(", "))));
    }
    let mut unknown_sections = Vec::new();
    for sec in profile.sections.iter().filter(|s| s.kind == SectionKind::Unknown) {
        if !unknown_sections.contains(&sec.name) {
            diags.push(Diagnostic::warning(codes::W_UNKNOWN_SECTION, format!("unknown section [{}] is kept but ignored", sec.name)).at(sec.span.clone()));
            unknown_sections.push(sec.name.clone());
        }
    }

    let mut includes: Vec<PathBuf> = Vec::new();
    for sec in &profile.sections {
        for e in &sec.entries {
            if let Origin::Include(p) = &e.origin {
                let p = p.to_path_buf();
                if !includes.contains(&p) {
                    includes.push(p);
                }
            }
        }
    }

    let mut config = Config {
        general,
        policies,
        groups,
        rules,
        rulesets,
        hosts,
        keystore,
        deferred,
        managed,
        unknown_sections,
        source: SourceInfo { main, includes },
    };
    validate(&mut config, opts, &mut diags);
    Loaded { config, diagnostics: diags }
}

fn validate(config: &mut Config, opts: &LoadOptions, diags: &mut Diagnostics) {
    let exists = |name: &str, config: &Config| config.resolve_policy(name).is_some();

    // Rule policy references.
    let mut ios_warned: HashSet<Builtin> = HashSet::new();
    for r in &config.rules {
        match &r.policy {
            PolicyRef::Named(n) if !exists(n, config) => {
                diags.push(Diagnostic::error(codes::E_UNKNOWN_POLICY_REF, format!("rule references unknown policy `{n}`")).at(r.span.clone()));
            }
            PolicyRef::Device(d) => {
                diags.push(Diagnostic::warning(codes::W_DEVICE_POLICY_AS_REJECT, format!("Ponte policy `DEVICE:{d}` is not supported; treated as REJECT")).at(r.span.clone()));
            }
            PolicyRef::Builtin(b) if b.is_ios_only() && ios_warned.insert(*b) => {
                diags.push(Diagnostic::warning(codes::W_IOS_BUILTIN_AS_DIRECT, format!("`{}` is iOS-only; treated as DIRECT", b.name())).at(r.span.clone()));
            }
            _ => {}
        }
    }

    // Group members, subnet conditions and `default` all reference policies.
    fn group_refs(g: &PolicyGroup) -> Vec<String> {
        g.members
            .iter()
            .cloned()
            .chain(g.conditions.iter().map(|(_, p)| p.clone()))
            .chain(g.params.get("default").map(str::to_string))
            .collect()
    }
    for g in &config.groups {
        for m in group_refs(g) {
            if !exists(&m, config) {
                diags.push(Diagnostic::error(codes::E_UNKNOWN_GROUP_MEMBER, format!("policy group `{}` references unknown policy `{m}`", g.name)).at(g.span.clone()));
            }
        }
    }

    // Group cycles (DFS with colours) over every reference kind.
    let index: HashMap<&str, usize> = config.groups.iter().enumerate().map(|(i, g)| (g.name.as_str(), i)).collect();
    let mut colour = vec![0u8; config.groups.len()];
    fn dfs(i: usize, groups: &[PolicyGroup], index: &HashMap<&str, usize>, colour: &mut [u8], diags: &mut Diagnostics) {
        colour[i] = 1;
        for m in group_refs(&groups[i]) {
            if let Some(&j) = index.get(m.as_str()) {
                if colour[j] == 1 {
                    diags.push(Diagnostic::error(codes::E_GROUP_CYCLE, format!("policy group `{}` and `{}` reference each other", groups[i].name, groups[j].name)).at(groups[i].span.clone()));
                } else if colour[j] == 0 {
                    dfs(j, groups, index, colour, diags);
                }
            }
        }
        colour[i] = 2;
    }
    for i in 0..config.groups.len() {
        if colour[i] == 0 {
            dfs(i, &config.groups, &index, &mut colour, diags);
        }
    }

    // FINAL.
    match config.rules.iter().rposition(|r| matches!(r.kind, RuleKind::Final)) {
        None => {
            let span = config.rules.last().map(|r| r.span.clone());
            let d = Diagnostic::error(codes::E_MISSING_FINAL, "the [Rule] section must end with an enabled FINAL rule").with_hint("add `FINAL,DIRECT` as the last rule");
            diags.push(match span {
                Some(s) => d.at(s),
                None => d,
            });
        }
        Some(pos) if pos + 1 < config.rules.len() => {
            diags.push(Diagnostic::warning(codes::W_RULES_AFTER_FINAL, format!("{} rule(s) after FINAL never take effect", config.rules.len() - pos - 1)).at(config.rules[pos + 1].span.clone()));
        }
        _ => {}
    }

    // Capabilities.
    let mut seen_kinds: HashSet<PolicyKind> = HashSet::new();
    for p in &config.policies {
        if !opts.capabilities.policy_kinds.contains(&p.kind) && seen_kinds.insert(p.kind) {
            diags.push(Diagnostic::warning(codes::W_PROTOCOL_NOT_IMPLEMENTED, format!("policy type `{}` is not implemented in this version; such policies behave as REJECT", p.kind.keyword())).at(p.span.clone()));
        }
    }
    let mut seen_groups: HashSet<GroupKind> = HashSet::new();
    for g in &config.groups {
        if !opts.capabilities.group_kinds.contains(&g.kind) && seen_groups.insert(g.kind) {
            diags.push(Diagnostic::warning(codes::W_GROUP_NOT_IMPLEMENTED, format!("policy group type `{}` is not implemented in this version; the first member is used", g.kind.keyword())).at(g.span.clone()));
        }
    }
    let mut seen_rules: HashSet<&'static str> = HashSet::new();
    for r in &config.rules {
        let t = r.kind.type_name();
        if !opts.capabilities.rule_types.contains(t) && seen_rules.insert(t) {
            diags.push(Diagnostic::warning(codes::W_RULE_NEVER_MATCHES, format!("`{t}` rules never match on {} in this version", opts.platform.system_name())).at(r.span.clone()));
        }
    }
}
```

`lib.rs` 增加 `pub mod config;` 与 `pub use config::{Capabilities, Config, LoadOptions, Loaded, LoadError, Platform, load};`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config`
Expected: 全部通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge-config
git commit -m "feat(config): Config 组装、名字表、组循环检测、FINAL 与能力校验"
```

---

### Task 13: `rurge check` 子命令

**Files:**
- Create: `crates/rurge/src/capabilities.rs`
- Create: `crates/rurge/src/cli/mod.rs`
- Create: `crates/rurge/src/cli/check.rs`
- Modify: `crates/rurge/src/main.rs`
- Modify: `crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: `rurge_config::config::{Capabilities, LoadOptions, Platform, load}`、`rurge_config::requirement::Environment`、`Diagnostics`。
- Produces:
  - `capabilities::CORE_VERSION: u64 = 20`，`capabilities::current() -> Capabilities`（M1：策略只有别名类型；组只有 `select`；规则类型去掉 `PROCESS-NAME` `SCRIPT` `DEVICE-NAME` `MAC-ADDRESS` `SUBNET` `CELLULAR-RADIO` `CELLULAR-CARRIER`）。
  - `cli::environment(platform: Platform) -> Environment`（`system_version = "unknown"`，`device_model = std::env::consts::ARCH`，`language` 取 `LANG` 环境变量前缀否则 `en-US`，`device_name` 取 `COMPUTERNAME` / `HOSTNAME` 否则 `rurge`）。
  - `rurge check -c <FILE> [--json] [--strict] [--platform windows|linux|macos] [--core-version N]`；退出码 0 通过、1 有警告且 `--strict`、2 有错误或无法读取。

- [ ] **Step 1: 写测试**

`crates/rurge/tests/cli.rs` 追加：

```rust
use std::fs;

const VALID: &str = "[General]\nloglevel = notify\n[Rule]\nGEOIP,CN,DIRECT\nFINAL,DIRECT\n";
const WARNING: &str = "[General]\nmystery = 1\n[Rule]\nFINAL,DIRECT\n";
const ERROR: &str = "[Rule]\nDOMAIN,a,DIRECT\n";

fn write(dir: &tempfile::TempDir, name: &str, text: &str) -> std::path::PathBuf {
    let p = dir.path().join(name);
    fs::write(&p, text).unwrap();
    p
}

#[test]
fn check_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let valid = write(&dir, "valid.conf", VALID);
    let warning = write(&dir, "warning.conf", WARNING);
    let error = write(&dir, "error.conf", ERROR);

    Command::cargo_bin("rurge").unwrap().args(["check", "-c"]).arg(&valid).assert().success().stdout(predicate::str::contains("0 error(s)"));
    Command::cargo_bin("rurge").unwrap().args(["check", "-c"]).arg(&warning).assert().success().stdout(predicate::str::contains("W0001"));
    Command::cargo_bin("rurge").unwrap().args(["check", "--strict", "-c"]).arg(&warning).assert().code(1);
    Command::cargo_bin("rurge").unwrap().args(["check", "-c"]).arg(&error).assert().code(2).stdout(predicate::str::contains("E0010"));
    Command::cargo_bin("rurge").unwrap().args(["check", "-c", "/no/such/file.conf"]).assert().code(2).stderr(predicate::str::contains("cannot read"));
}

#[test]
fn check_json_and_platform() {
    let dir = tempfile::tempdir().unwrap();
    let conf = write(&dir, "p.conf", "[Rule]\nDOMAIN,a,REJECT #!MACOS-ONLY\nFINAL,DIRECT\n");
    let out = Command::cargo_bin("rurge").unwrap().args(["check", "--json", "--platform", "linux", "-c"]).arg(&conf).assert().success().get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["errors"], 0);
    assert!(v["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "I0002"));
    let out = Command::cargo_bin("rurge").unwrap().args(["check", "--json", "--platform", "macos", "-c"]).arg(&conf).assert().success().get_output().stdout.clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert!(!v["diagnostics"].as_array().unwrap().iter().any(|d| d["code"] == "I0002"));
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge`
Expected: `check` 子命令不存在，测试失败。

- [ ] **Step 3: 实现**

`crates/rurge/src/capabilities.rs`：

```rust
//! What this build of rurge actually implements. Milestone 1: parsing only,
//! so only the built-in alias policies and `select` groups are "implemented".

use rurge_config::config::Capabilities;
use rurge_config::policy::{GroupKind, PolicyKind};
use std::collections::HashSet;

/// Reported as `CORE_VERSION` to requirement expressions (see FR-CFG-08).
pub const CORE_VERSION: u64 = 20;

const INACTIVE_RULE_TYPES: [&str; 7] = ["PROCESS-NAME", "SCRIPT", "DEVICE-NAME", "MAC-ADDRESS", "SUBNET", "CELLULAR-RADIO", "CELLULAR-CARRIER"];

pub fn current() -> Capabilities {
    Capabilities {
        policy_kinds: HashSet::from([PolicyKind::Direct, PolicyKind::Reject, PolicyKind::RejectDrop, PolicyKind::RejectNoDrop, PolicyKind::RejectTinyGif]),
        group_kinds: HashSet::from([GroupKind::Select]),
        rule_types: Capabilities::ALL_RULE_TYPES.iter().copied().filter(|t| !INACTIVE_RULE_TYPES.contains(t)).collect(),
    }
}
```

`crates/rurge/src/cli/mod.rs`：

```rust
pub mod check;

use rurge_config::config::Platform;
use rurge_config::requirement::Environment;

pub fn environment(platform: Platform, core_version: u64) -> Environment {
    let language = std::env::var("LANG")
        .ok()
        .and_then(|l| l.split('.').next().map(|s| s.replace('_', "-")))
        .filter(|l| !l.is_empty() && l != "C")
        .unwrap_or_else(|| "en-US".to_string());
    let device_name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "rurge".to_string());
    Environment {
        core_version,
        system: platform.system_name().to_string(),
        system_version: "unknown".to_string(),
        device_model: std::env::consts::ARCH.to_string(),
        language,
        device_name,
    }
}
```

`crates/rurge/src/cli/check.rs`：

```rust
use crate::capabilities;
use anyhow::Context;
use clap::Args;
use rurge_config::config::{LoadOptions, Platform, load};
use rurge_config::diagnostic::Severity;
use serde::Serialize;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Args)]
pub struct CheckArgs {
    /// Profile to validate
    #[arg(short = 'c', long = "config", value_name = "FILE")]
    pub config: PathBuf,
    /// Print diagnostics as JSON
    #[arg(long)]
    pub json: bool,
    /// Exit with code 1 when there are warnings
    #[arg(long)]
    pub strict: bool,
    /// Evaluate the profile as if running on this platform (windows, linux, macos)
    #[arg(long, value_parser = parse_platform)]
    pub platform: Option<Platform>,
    /// Override the CORE_VERSION reported to requirement expressions
    #[arg(long)]
    pub core_version: Option<u64>,
}

fn parse_platform(s: &str) -> Result<Platform, String> {
    Platform::parse(s).ok_or_else(|| format!("unknown platform `{s}` (expected windows, linux or macos)"))
}

#[derive(Serialize)]
struct Report<'a> {
    file: String,
    ok: bool,
    errors: usize,
    warnings: usize,
    infos: usize,
    diagnostics: Vec<&'a rurge_config::Diagnostic>,
}

pub fn run(args: CheckArgs) -> anyhow::Result<ExitCode> {
    let platform = args.platform.unwrap_or_else(Platform::current);
    let opts = LoadOptions {
        environment: super::environment(platform, args.core_version.unwrap_or(capabilities::CORE_VERSION)),
        platform,
        capabilities: capabilities::current(),
    };
    let loaded = load(&args.config, &opts).with_context(|| format!("cannot read `{}`", args.config.display()))?;
    let diags = loaded.diagnostics.sorted();
    let count = |s: Severity| diags.iter().filter(|d| d.severity == s).count();
    let (errors, warnings, infos) = (count(Severity::Error), count(Severity::Warning), count(Severity::Info));

    if args.json {
        let report = Report { file: args.config.display().to_string(), ok: errors == 0, errors, warnings, infos, diagnostics: diags.iter().collect() };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for d in diags.iter() {
            println!("{d}");
        }
        println!("{}: {errors} error(s), {warnings} warning(s), {infos} note(s)", args.config.display());
    }

    Ok(if errors > 0 {
        ExitCode::from(2)
    } else if warnings > 0 && args.strict {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
```

`crates/rurge/src/main.rs`（整文件替换）：

```rust
mod capabilities;
mod cli;

use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "rurge", version, about = "Cross-platform Surge-compatible network proxy")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print version information
    Version,
    /// Validate a Surge-format profile and print diagnostics
    Check(cli::check::CheckArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Version => {
            println!("rurge {} (config {})", env!("CARGO_PKG_VERSION"), rurge_config::CRATE_VERSION);
            Ok(ExitCode::SUCCESS)
        }
        Command::Check(args) => cli::check::run(args),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(2)
        }
    }
}
```

`crates/rurge/Cargo.toml` 的 `[dependencies]` 加 `serde.workspace = true`。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge`
Expected: 3 个 CLI 测试通过。

- [ ] **Step 5: 提交**

```bash
git add crates/rurge
git commit -m "feat(cli): rurge check：诊断输出、JSON、--strict 与 --platform"
```

---

### Task 14: 语料库、快照测试与文档更新

**Files:**
- Create: `tests/corpus/README.md`
- Create: `tests/corpus/valid/quick-start.conf`、`format-examples.conf`、`kitchen-sink.conf`、`legacy-keys.conf`、`requirement.conf`、`include-main.conf`、`include/proxy.dconf`、`include/rules.dconf`、`include/shared-rulesets.conf`
- Create: `tests/corpus/invalid/missing-final.conf` + `.expect`、`unknown-policy.conf` + `.expect`、`group-cycle.conf` + `.expect`、`bad-cidr.conf` + `.expect`、`unknown-type.conf` + `.expect`、`final-in-set.conf` + `.expect`
- Create: `crates/rurge-config/tests/corpus.rs`（快照文件生成于 `crates/rurge-config/tests/snapshots/`）
- Modify: `README.md`、`CLAUDE.md`

**Interfaces:**
- Consumes: `rurge_config::config::{LoadOptions, load}`、`Config::summary()`、`Diagnostics::sorted()`。
- Produces: 语料库目录约定：`valid/*.conf` 必须无错误并与快照一致；`invalid/<name>.conf` 配 `invalid/<name>.expect`（每行一个必须出现的诊断代码）。

- [ ] **Step 1: 写语料文件**

`tests/corpus/README.md`：

```markdown
# 兼容性语料库

`valid/` 中的配置必须能被 rurge 加载且没有错误级诊断；每个文件对应一份快照（`crates/rurge-config/tests/snapshots/`），记录解析结果摘要与全部诊断。
`invalid/` 中的配置故意包含错误，`<name>.expect` 列出必须出现的诊断代码（每行一个）。

来源与许可：
- `quick-start.conf`、`format-examples.conf`、`requirement.conf`、`legacy-keys.conf`：改写自 Surge 官方手册中的示例（<https://manual.nssurge.com/>），仅保留语法结构。
- `kitchen-sink.conf`、`include-*`：本项目自写，覆盖全部节与规则类型，服务器地址均为示例地址。

添加真实配置前请脱敏：删除密码、PSK、UUID、私钥、订阅地址与内网地址。
```

`tests/corpus/valid/quick-start.conf`：

```ini
[General]
dns-server = system, 1.1.1.1, 8.8.8.8

[Proxy]
ProxyA = https, proxy.example.com, 443, username, password

[Proxy Group]
Proxy = select, ProxyA, DIRECT

[Rule]
DOMAIN-SUFFIX,example.com,Proxy
GEOIP,CN,DIRECT
FINAL,Proxy
```

`tests/corpus/valid/format-examples.conf`：

```ini
# This is a comment line.
; This is a comment line.
// This is a comment line.
[General]
loglevel = notify
dns-server = 8.8.8.8 // Inline comment
skip-proxy = 192.168.0.0/16, 10.0.0.0/8, localhost, *.local ; Inline comment
test-timeout = 5 # Inline comment
example-quoted = "a quoted value: \"text\"; path: C:\\Proxy"

[Proxy]
ProxyA = http, 1.2.3.4, 80

[Rule]
DOMAIN-SUFFIX,example.com,ProxyA
URL-REGEX,"^http://example\.com/(a|b),?c",ProxyA
FINAL,DIRECT
```

`tests/corpus/valid/kitchen-sink.conf`：

```ini
#!MANAGED-CONFIG https://example.com/kitchen-sink.conf interval=3600 strict=false

[General]
loglevel = notify
dns-server = system, 223.5.5.5, 119.29.29.29
encrypted-dns-server = https://dns.alidns.com/dns-query, h3://dns.alidns.com/dns-query, quic://dns.adguard.com, tls://1.1.1.1, tcp://dns.example.com
encrypted-dns-follow-outbound-mode = false
encrypted-dns-skip-cert-verification = false
allow-dns-svcb = false
use-local-host-item-for-proxy = false
hijack-dns = 8.8.8.8:53, 8.8.4.4:53
always-real-ip = *.srv.nintendo.net, *.stun.playstation.net, xbox.*.microsoft.com, *.xboxlive.com
geoip-maxmind-url = https://example.com/geoip.mmdb
disable-geoip-db-auto-update = false
ipv6 = true
ipv6-vif = auto
tun-excluded-routes = 192.168.0.0/16, 10.0.0.0/8, 172.16.0.0/12
tun-included-routes = 192.168.1.12/32
icmp-forwarding = true
skip-proxy = 127.0.0.1, 192.168.0.0/16, 10.0.0.0/8, 172.16.0.0/12, 100.64.0.0/10, localhost, *.local, [::1]:0
exclude-simple-hostnames = true
proxy-restricted-to-lan = true
gateway-restricted-to-lan = true
external-controller-access = key@127.0.0.1:6165
http-api = key@127.0.0.1:6171
http-api-tls = false
http-api-web-dashboard = true
internet-test-url = http://connectivitycheck.platform.hicloud.com/generate_204
proxy-test-url = http://cp.cloudflare.com/generate_204
test-timeout = 5
proxy-test-udp = apple.com@8.8.8.8
force-http-engine-hosts = *.example.com:8080
always-raw-tcp-hosts = *.stream.example
always-raw-tcp-keywords = rtmp, sip
udp-policy-not-supported-behaviour = REJECT
udp-priority = true
block-quic = per-policy
show-error-page = true
show-error-page-for-reject = true
compatibility-mode = 3
allow-wifi-access = true
wifi-access-http-port = 6152
wifi-access-socks5-port = 6153
http-listen = 0.0.0.0:6152
socks5-listen = 0.0.0.0:6153
set-system-socks-proxy = true
read-etc-hosts = true
subnet-exp-wifi-always-match = true

[Proxy]
Direct-Alias = direct, interface = eth0, allow-other-interface=true
HTTP-Proxy = http, proxy.example.com, 8080, user, pass, always-use-connect=true, headers=X-Client:rurge;X-Token:abc
HTTPS-Proxy = https, proxy.example.com, 443, user, pass, sni=cdn.example.com, server-cert-verify-name=proxy.example.com, tfo=true
H2-Proxy = h2-connect, proxy.example.com, 443, max-streams=3, udp-relay=true
SOCKS = socks5, 192.0.2.10, 1080, user, pass, udp-relay=true
SOCKS-TLS = socks5-tls, proxy.example.com, 443, user, pass, skip-cert-verify=false
SS = ss, 192.0.2.11, 8388, encrypt-method=chacha20-ietf-poly1305, password=secret, udp-relay=true, udp-port=8389, obfs=tls, obfs-host=cdn.example.com
SS-2022 = ss, 192.0.2.12, 8388, encrypt-method=2022-blake3-aes-128-gcm, password=YctPZ6U7xPPcU+gp3u+0tx/tRizJN9K8y+uKlW2qjlI=
Snell = snell, 192.0.2.13, 443, psk=secret, version=4, reuse=true, shadow-tls-password=stls, shadow-tls-version=3, shadow-tls-sni=example.com
VMess = vmess, 192.0.2.14, 443, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true, tls=true, ws=true, ws-path=/ws, ws-headers=Host:example.com|X-Token:abc
Trojan = trojan, 192.0.2.15, 443, password=secret, sni=example.com
TUIC = tuic, 192.0.2.16, 443, token=secret, alpn=h3
TUIC5 = tuic-v5, 192.0.2.17, 443, uuid=0233d11c-15a4-47d3-ade3-48ffca0ce119, password=secret, alpn=h3, port-hopping=5000-6000, port-hopping-interval=30
Hysteria = hysteria2, 192.0.2.18, 443, password=secret, download-bandwidth=100, salamander-password=obfs
MASQUE = masque, proxy.example.com, 443, username=user, password=pass
AnyTLS = anytls, 192.0.2.19, 443, password=secret, reuse=true
Trust = trust-tunnel, 192.0.2.20, 443, username=user, password=pass, h3=true
SSH = ssh, 192.0.2.21, 22, username=root, private-key=key1, idle-timeout=180, server-fingerprint="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBk2No6KBq2m9VTCcHXXJBX4/A3RNr+L+yDBl5+TF9qz"
WG = wireguard, section-name=home, underlying-proxy=SS, test-url=http://example.com/, ecn=false
TS = tailscale, section-name=tailnet
External = external, exec = "/usr/bin/ssh", args = "192.0.2.22", args = "-D", args = "127.0.0.1:1080", local-port = 1080, addresses = 192.0.2.22, udp-relay=true

[Proxy Group]
Select = select, Auto, Fallback, Balance, Smart, Subnet, SS, Trojan, DIRECT, icon-url=https://example.com/icon.png
Auto = url-test, SS, Trojan, Hysteria, interval=600, tolerance=100, timeout=5, evaluate-before-use=true, no-alert=true
Fallback = fallback, SS, Trojan, interval=600, timeout=5
Balance = load-balance, SS, Trojan, TUIC5, persistent=true
Smart = smart, SS, Trojan, policy-priority="SS:0.9;Trojan:1.3"
Subnet = subnet, default = SS, SSID:Home-* = DIRECT, TYPE:WIRED = DIRECT, ROUTER:192.168.1.1 = DIRECT, hidden=true
External-Group = select, policy-path=https://example.com/proxies.txt, update-interval=86400, policy-regex-filter=^HK, external-policy-modifier="tfo=true", external-policy-name-prefix=Sub-, include-all-proxies=true, include-other-group="Auto,Fallback", underlying-proxy=SS

[Keystore]
cert1 = type=p12, base64=AAAA, password=123456
key1 = type=openssh-private-key, base64=BBBB

[Ruleset Streaming]
DOMAIN-SUFFIX,netflix.com
DOMAIN-SUFFIX,netflix.net
DOMAIN,netflixdnstest0.com
IP-CIDR,203.0.113.0/24,no-resolve
IP-ASN,13335

[Ruleset Media]
RULE-SET,Streaming
DOMAIN-SUFFIX,video.example

[Host]
abc.com = 1.2.3.4, 5.6.7.8, ::1
*.dev = 6.7.8.9
foo.com = bar.com
bar.com = server:8.8.8.8, 1.1.1.1
secure.example = server:https://cloudflare-dns.com/dns-query
Macbook = server:system
printer.local = server:force-syslib
dyn.example.com = script:dnspod
DOMAIN-SET:https://example.com/domains.txt = server:https://doh.example.com/dns-query
RULE-SET:https://example.com/rules.txt = 10.0.0.10

[Rule]
DOMAIN,ad.example.com,REJECT,pre-matching
DOMAIN,www.apple.com,Select
DOMAIN-SUFFIX,apple.com,DIRECT,extended-matching
DOMAIN-KEYWORD,google,Select
DOMAIN-WILDCARD,api-*.example.com,Select
DOMAIN-SET,https://example.com/adblock.txt,REJECT,update-interval=43200
IP-CIDR,192.168.0.0/16,DIRECT,no-resolve
IP-CIDR,8.8.8.8,Select
IP-CIDR6,2001:db8:abcd:8000::/50,DIRECT
GEOIP,US,DIRECT,no-resolve
IP-ASN,AS13335,Select
USER-AGENT,Instagram*,Select
URL-REGEX,^https://example\.com,Select,extended-matching
PROCESS-NAME,Telegram,Select
PROCESS-NAME,/Applications/ChatGPT.app/,Select
DEST-PORT,10000-20000,DIRECT
SRC-PORT,>=50000,DIRECT
IN-PORT,6153,DIRECT
SRC-IP,192.168.20.0/24,DIRECT
DEVICE-NAME,Kids-iPad,REJECT
MAC-ADDRESS,A4:83:E7:11:22:33,Select
PROTOCOL,STUN,REJECT
PROTOCOL,MTProto,Select
HOSTNAME-TYPE,IPv6,REJECT
SUBNET,SSID:MyHome,DIRECT
SUBNET,TYPE:CELLULAR,DIRECT
CELLULAR-RADIO,LTE,DIRECT
CELLULAR-CARRIER,310260,Select
AND,((SRC-IP,192.168.1.110),(DOMAIN-SUFFIX,example.com)),DIRECT
AND,((NOT,((SRC-IP,192.168.1.110))),(DOMAIN-SUFFIX,example.com)),DIRECT
OR,((DOMAIN,a.example),(DOMAIN,b.example)),Select
AND,((PROTOCOL,UDP),(RULE-SET,Streaming)),REJECT
AND,((DOMAIN-SUFFIX,tracker.example.com),(DEST-PORT,443)),REJECT,pre-matching
SCRIPT,ssid-rule,DIRECT,requires-resolve
RULE-SET,Media,Select
RULE-SET,https://example.com/social.list,Select,no-resolve,extended-matching,update-interval=43200
RULE-SET,SYSTEM,DIRECT
RULE-SET,LAN,DIRECT
DOMAIN-SUFFIX,notify.example,Select,notification-text=Example matched,notification-interval=600
DOMAIN,capture.example,Select,always-capture=debug
FINAL,Select,dns-failed

[MITM]
ca-p12 = MIIJtQ
ca-passphrase = password
hostname = -*.apple.com, -*.icloud.com, *.example.com, api.example.org:8443
h2 = true
auto-quic-block = true

[URL Rewrite]
^http://www\.google\.cn http://www.google.com header
^http://yachen\.com https://yach.me 302
^http://ad\.com/ad\.png _ reject

[Header Rewrite]
http-request ^http://example.com header-add DNT 1
http-response ^http://example.com header-replace-regex Date 2022 2023

[Body Rewrite]
http-response ^https?://example\.com/ documents Surge
http-response-jq ^http://httpbingo.org/anything '.headers |= with_entries(select(.key | test("^X-") | not))'

[Map Local]
^http://surgetest\.com/json data-type=text data="{}" status-code=500
^http://surgetest\.com/gif data-type=tiny-gif status-code=200

[Script]
ssid-rule = type=rule, script-path=ssid-rule.js
dnspod = type=dns, script-path=dnspod.js
nightly = type=cron, cronexp="0 2 * * *", script-path=cron.js
panel = type=generic, script-path=panel.js

[Panel]
PanelA = title="Panel Title",content="Panel Content\nSecondLine",style=info
PanelB = title="External IP",content="",style=info,script-name=panel,update-interval=60

[SSID Setting]
SSID:MyHome suspend=true
SSID:Office dns-server=192.168.1.1,encrypted-dns-server=off

[Port Forwarding]
0.0.0.0:6841 localhost:3306 policy=SSH

[WireGuard home]
private-key = cHJpdmF0ZS1rZXktZXhhbXBsZS1iYXNlNjQtc3RyaW5nMTIzNA==
self-ip = 10.20.0.2
self-ip-v6 = fd00::2
dns-server = 10.20.0.1
mtu = 1280
peer = (public-key = cHVibGljLWtleS1leGFtcGxlLWJhc2U2NC1zdHJpbmctMTIzNA==, allowed-ips = "0.0.0.0/0, ::/0", endpoint = vpn.example.com:51820, keepalive = 25, client-id = 83/12/235)

[Tailscale tailnet]
auth-key = tskey-auth-example
hostname = rurge-node

[DHCP]
max-lease-time = 86400
default-lease-time = 43200

[Snell Server]
interface = 0.0.0.0
port = 6160
psk = RANDOM_KEY_HERE

[MTProto]
interface = 127.0.0.1
port = 5753
secret = 0123456789abcdef0123456789abcdef

[Testing]
download-concurrency = 4
upload-duration-limit = 10s
```

`tests/corpus/valid/legacy-keys.conf`：

```ini
[General]
doh-server = https://dns.alidns.com/dns-query
doh-follow-outbound-mode = true
doh-skip-cert-verification = false
interface = 0.0.0.0
port = 6152
socks-interface = 0.0.0.0
socks-port = 6153
ipv6-vif = off
use-default-policy-if-wifi-not-primary = true
vif-mode = v2
enhanced-mode-by-rule = false

[Proxy Group]
Old = ssid, default = DIRECT, MyWifi = DIRECT

[Rule]
SUBNET,MyWifi,DIRECT
FINAL,DIRECT
```

`tests/corpus/valid/requirement.conf`：

```ini
[Proxy]
A = direct
B = direct

[Proxy Group]
#!REQUIREMENT CORE_VERSION>=22 Group = smart, A, B
Group = url-test, A, B //!REQUIREMENT CORE_VERSION<22
#!REQUIREMENT "CORE_VERSION>=22 AND SYSTEM=='iOS'" Group2 = smart, A, B
Group2 = select, A, B #!REQUIREMENT "CORE_VERSION<22 OR SYSTEM!='iOS'"

[Rule]
DOMAIN,reject.com,REJECT #!MACOS-ONLY
#!IOS-ONLY DOMAIN,ios.example,REJECT
DOMAIN,linux.example,REJECT #!REQUIREMENT SYSTEM=='Linux'
FINAL,Group
```

`tests/corpus/valid/include-main.conf`：

```ini
[Proxy]
#!include include/proxy.dconf

[Ruleset *]
#!include include/shared-rulesets.conf

[Rule]
#!include include/rules.dconf
DEST-PORT,123,DIRECT
RULE-SET,Streaming,IncludedProxy
FINAL,IncludedProxy
```

`tests/corpus/valid/include/proxy.dconf`：

```ini
[Proxy]
IncludedProxy = direct
```

`tests/corpus/valid/include/rules.dconf`：

```ini
[Rule]
DOMAIN,included.example,IncludedProxy
```

`tests/corpus/valid/include/shared-rulesets.conf`：

```ini
[Ruleset Streaming]
DOMAIN-SUFFIX,stream.example

[Ruleset Music]
DOMAIN-SUFFIX,music.example
```

`tests/corpus/invalid/`（每个 `.expect` 文件内容即右侧代码，每行一个）：

| 文件 | 内容 | `.expect` |
| --- | --- | --- |
| `missing-final.conf` | `[Rule]\nDOMAIN,a,DIRECT\n` | `E0010` |
| `unknown-policy.conf` | `[Rule]\nDOMAIN,a,Nope\nFINAL,DIRECT\n` | `E0007` |
| `group-cycle.conf` | `[Proxy Group]\nA = select, B\nB = select, A\n[Rule]\nFINAL,A\n` | `E0009` |
| `bad-cidr.conf` | `[Rule]\nIP-CIDR,10.0.0.0/99,DIRECT\nFINAL,DIRECT\n` | `E0011` |
| `unknown-type.conf` | `[Proxy]\nX = vless, 1.2.3.4, 443\n[Rule]\nFINAL,DIRECT\n` | `E0004` |
| `final-in-set.conf` | `[Rule]\nAND,((FINAL,DIRECT),(DOMAIN,a)),DIRECT\nFINAL,DIRECT\n` | `E0014` |

- [ ] **Step 2: 写语料测试**

`crates/rurge-config/tests/corpus.rs`：

```rust
use rurge_config::config::{LoadOptions, load};
use rurge_config::diagnostic::Diagnostic;
use std::fs;
use std::path::{Path, PathBuf};

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus").canonicalize().expect("tests/corpus exists")
}

fn conf_files(sub: &str) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(corpus_dir().join(sub))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "conf"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .conf files in {sub}");
    files
}

/// Render a diagnostic with the file path relative to the corpus directory so snapshots are portable.
fn render(d: &Diagnostic, root: &Path) -> String {
    let location = d
        .span
        .as_ref()
        .map(|s| {
            let rel = s.file.strip_prefix(root).unwrap_or(&s.file).to_string_lossy().replace('\\', "/");
            format!(" {rel}:{}", s.line)
        })
        .unwrap_or_default();
    format!("{}[{}]{location}: {}", d.severity, d.code, d.message)
}

#[test]
fn valid_corpus_loads_without_errors_and_matches_snapshots() {
    let root = corpus_dir();
    for file in conf_files("valid") {
        let loaded = load(&file, &LoadOptions::for_tests()).unwrap();
        let diags: Vec<String> = loaded.diagnostics.clone().sorted().iter().map(|d| render(d, &root)).collect();
        assert!(!loaded.diagnostics.has_errors(), "{}: {diags:#?}", file.display());
        let name = file.file_stem().unwrap().to_string_lossy().to_string();
        insta::assert_yaml_snapshot!(format!("corpus__{name}"), (loaded.config.summary(), diags));
    }
}

#[test]
fn invalid_corpus_reports_expected_codes() {
    for file in conf_files("invalid") {
        let expect = fs::read_to_string(file.with_extension("expect")).unwrap_or_else(|_| panic!("{}: missing .expect", file.display()));
        let loaded = load(&file, &LoadOptions::for_tests()).unwrap();
        assert!(loaded.diagnostics.has_errors(), "{} should have errors", file.display());
        let codes: Vec<&str> = loaded.diagnostics.iter().map(|d| d.code).collect();
        for code in expect.lines().map(str::trim).filter(|l| !l.is_empty()) {
            assert!(codes.contains(&code), "{}: expected {code}, got {codes:?}", file.display());
        }
    }
}
```

- [ ] **Step 3: 生成快照并审阅**

Run: `cargo install cargo-insta`（一次性），然后 `cargo insta test -p rurge-config --accept`
Expected: 生成 `crates/rurge-config/tests/snapshots/corpus__*.snap`。逐个打开审阅：`kitchen-sink` 的规则数应为 41，策略 21 个，组 7 个，延迟节列表包含 `MITM` 到 `Testing` 共 15 个；`requirement` 在 Linux 环境下 `Group` 与 `Group2` 各只剩一条启用定义，`DOMAIN,linux.example` 启用，`reject.com` / `ios.example` 被禁用（I0002）；`legacy-keys` 有 7 条 I0001 与 2 条 W0006；`include-main` 的规则顺序为 included → DEST-PORT → RULE-SET → FINAL。若数字不符，先修实现再接受快照。

Run: `cargo test --workspace`
Expected: 全部通过。

- [ ] **Step 4: 更新文档**

`README.md` 中文「当前状态」段替换为：

```markdown
> **阶段 1 进行中。** 里程碑 M1（配置解析器）已完成：`rurge check` 可以校验任意 Surge 配置并给出带行号的诊断；代理功能尚未实现。
```

英文「Status」段替换为：

```markdown
> **Phase 1 in progress.** Milestone M1 (profile parser) is done: `rurge check` validates any Surge profile with line-numbered diagnostics; proxying is not implemented yet.
```

`README.md` 两处「快速开始」代码块前的引文改为「`rurge check` 已可用；`rurge run` 将在 M3 提供。」/「`rurge check` works today; `rurge run` arrives with milestone M3.」。

`CLAUDE.md`「当前状态」一节替换为：

```markdown
## 当前状态（2026-09）

阶段 1 进行中。M1 已完成：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`。M2（规则引擎与 DNS）、M3（连接流水线）、M4（控制面与平台）未开始，`rurge run` 尚不存在。
```

「计划中的命令」一节标题改为「常用命令」，内容替换为：

```bash
cargo test --workspace                          # 全部测试
cargo test -p rurge-config <name>               # 单个 crate / 用例
cargo insta test -p rurge-config --review       # 语料库快照有变化时审阅
cargo clippy --all-targets -- -D warnings       # 零警告
cargo fmt --all
cargo run -p rurge -- check -c config.conf      # 校验 Surge 配置（--json / --strict / --platform）
```

- [ ] **Step 5: 质量门与提交**

Run: `cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --workspace`
Expected: 全部通过。

```bash
git add tests/corpus crates/rurge-config/tests README.md CLAUDE.md
git commit -m "test(config): 兼容性语料库与快照测试；文档更新到 M1 完成状态"
```

---

## 自查记录

- **设计覆盖**：设计文档 4.1 ～ 4.3（Task 2、3、12）、5.1（Task 3 ～ 11）、5.2（Task 13）、14 的 M1 测试项（Task 3 ～ 14）、15 的验收标准 1 与 7（Task 13、14、1）。远程 `#!include`、托管配置的下载与更新（5.1 末段）按设计推迟到 M2，本计划只解析 `#!MANAGED-CONFIG` 并对远程 include 报 W0011。
- **类型一致性**：`ParseError` 在 Task 9 引入，Task 10、11 复用；`ParseCtx` 在 Task 10 定义，Task 11、12 复用；`HostList::parse` 签名在 Task 7、8 一致；`Environment::fixed()` 在 Task 5 定义，Task 12 的 `LoadOptions::for_tests()` 使用；`Capabilities::ALL_RULE_TYPES` 在 Task 12 定义，Task 13 使用。
- **诊断代码**：Task 2 定义 E0001 ～ E0017、W0001 ～ W0016、I0001 ～ I0002；Task 6 追加 W0017；Task 12 追加 W0018、W0019。
