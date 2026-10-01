# 阶段 2 / M6c「HTTP/2 族：h2-connect 与 trust-tunnel」Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `h2-connect`（HTTP/2 CONNECT 多路复用、`max-streams`、`headers`、`udp-relay` 下的 CONNECT-UDP）与 `trust-tunnel`（HTTP/2 模式、Basic 认证、只有 TCP）两种出站。M6 至此完成。

**Architecture:** `rurge-config` 新增 `spec::h2`（`H2ConnectSpec` / `TrustTunnelSpec`）。`rurge-proto` 新增共用的 `h2pool`（每个出站一个 HTTP/2 连接池：自己数每条连接上的流、单飞拨号、等服务端 SETTINGS、GOAWAY 后不再分配、空闲 60 秒关闭；`H2Stream` 把一条流包成字节流）、`h2connect`（CONNECT；`capsule` 编解码与 `udp` 载体：每个目标一条 extended CONNECT 流）与 `trust_tunnel`，外加 `FakeH2Proxy`。TLS 层能报出协商到的 ALPN。引擎的工厂装上两种出站，能力表翻转。互操作对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版），它同时充当 `h2-connect` 普通 CONNECT 的参考服务端。

**Tech Stack:** Rust 1.89 / edition 2024；**不新增 crate**：`h2` 0.4.19 已经经 `hyper` 在锁文件里（这里给工作区与 `rurge-proto` 各加一条依赖边）。

**Spec:** `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`（M6-D1 ～ D8；第 5 节；第 6、7 节中 M6c 的部分；第 9 节 V1、V7 ～ V9；第 10 节 M6c 草图；第 12 ～ 15 节的订正）与总设计。与本计划「计划期决定」表不一致处，以该表为准；执行开始时一并写进设计文档新增的第 16 节。

## Global Constraints

- MSRV **1.89**，edition 2024。`unsafe_code`：全工作区 `forbid`；只有 `rurge-platform` 的两个函数例外（本计划不碰）。**本计划不新增任何 unsafe**。
- 依赖方向不变：`rurge-proto → rurge-net → rurge-config`；`rurge-engine → { rurge-inbound → rurge-proto, rurge-policy → rurge-proto, rurge-dns }`。**不新增 crate**，也不给已有 crate 引入第二个大版本。
- **测试绝不碰公网**：只用回环 + 端口 0 + 有界等待；不用固定 sleep 当同步手段（轮询 + 截止时间；只有"断言这段时间里什么也没发生"或"等一段时间过去"本身就是被测条件时才等一段固定时间）。**任何带 `url-test` / `fallback` / `load-balance` / `smart` 组的测试配置，`proxy-test-url` 与 `internet-test-url` 都必须指向回环**——引擎用例的 `Profile::text` 已默认指向 `http://127.0.0.1:9/`，不要删掉。
- **测试与任何命令都不得修改本机的系统代理、注册表、网络设置，不得注册真实服务 / 计划任务**：不在 CLI 测试夹具之外运行 `rurge run --system-proxy`；不运行不带 `--dry-run` 的 `rurge service install | uninstall`。
- **不在本机下载或安装任何东西**——哪怕只是为了算一个校验和也不行（不装 TrustTunnel endpoint、sing-box，不 `rustup target add`、不 `cargo install`）。互操作用例在本机没有二进制时按既有约定跳过。
- **凭据与载荷永不外泄**：`username` / `password`、`proxy-authorization` 的值、`headers` 的值、TCP 与 UDP 的载荷不进日志、错误文本与 `Debug`（配置里用 `Secret<T>`）。
- 锁里只做内存操作，不跨 `.await`；后台任务随所属对象结束（`AbortOnDrop` 的做法），不留孤儿任务。
- rurge 专有的运行时选项只经命令行与环境变量提供，不扩展 Surge 配置格式（FR-CFG-17）。与 Surge 的每一处行为差异都登记进 `docs/surge-compatibility-matrix.md`（Task 7）。
- 日志、错误文本、CLI 输出、代码注释用英文；文档与提交标题用中文。注释密度、命名、惯用法与相邻代码保持一致；注释里不写评审轮次的标签。
- 提交信息结尾两行：`Co-Authored-By: <执行者自己的署名>` 与 `Claude-Session: https://claude.ai/code/session_01NH7PhdXCocrpjwdkmGk5th`。**不 push、不 merge**，不 amend。
- 每个任务结束的门禁（全部通过才算完成）：

  ```bash
  RUSTFMT="C:\Users\SZV01065\.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe" cargo fmt --all --check \
    && cargo clippy --all-targets -- -D warnings \
    && timeout 1500 cargo test --workspace --no-fail-fast
  ```

  `timeout` 不能省：`rurge-dns` 的一个用例曾让测试进程以 100% CPU 空转数小时（M3a「延后事项」#20）。测试二进制异常退出而没有失败用例时（`STATUS_ACCESS_VIOLATION`、`STATUS_HEAP_CORRUPTION` / `0xc0000374`、段错误——本机已知的既有问题，M3b 计划 P21），或整轮被 `timeout` 杀掉时，重跑一次并保留两次的日志，**不要在任务里去修它**。已知偶发失败的用例（`rurge-dns` 的 `a_partial_result_completes_aaaa_in_the_background` 与 `bootstrap::tests::stale_entries_are_served_and_refreshed_once`、`rurge` 的 `run::watch_reloads_rules_on_change` 与 `run::run_system_proxy_is_applied_switched_and_restored`、`rurge-engine` 的 `udp::a_closed_port_does_not_break_the_carrier`）同样重跑。**链接器报 LNK1180 / LNK1318 / LNK1285 或 "no space on device" 时是磁盘满了**：先看 `df -h /d`，删 `target/debug/incremental` 与 `target/debug/deps` 里旧的 `*.exe` / `*.pdb` 再重跑。
- 第三方 crate 的 API 以本机源码为准：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/h2-0.4.19`。
- 本机的 bash 处理不了超过约 8 KB 或含反斜杠的 heredoc（`\\` 会被改写）：新文件与含反斜杠的改动一律用写文件的工具落盘，不用 heredoc。

## Review Focus

设计没有逐条写到、而最可能伤到使用者的五类输入或失败方式；每一条都在负责它的任务里配了用例。

1. **服务端的流数上限低于 `max-streams`、几个请求同时到来**（启动时常见）：`h2` 过了服务端上限会把请求默默排队，不能让它们卡到超时。用例：Task 4 `concurrent_tunnels_do_not_wait_behind_the_servers_stream_limit`、`a_fresh_connection_carries_one_stream_until_the_servers_limit_is_known`。
2. **认证失败**：要以清楚的文字失败（`h2-connect: proxy authentication required` / `trust-tunnel: authentication failed`），不泄露凭据。用例：Task 3 与 Task 4 的认证用例；Task 6 `refused_credentials_fail_the_session`、`a_refused_trust_tunnel_login_fails_the_session`。
3. **服务端不支持 extended CONNECT 却开了 `udp-relay`**：UDP 要以清楚的文字失败，不能拖垮同一条连接上的 TCP。用例：Task 5 的服务端不支持 extended CONNECT 用例；Task 6 `udp_without_extended_connect_fails_the_flow`。
4. **服务端发来不认识的 capsule、context id 不是 0、长度写法不是最短**：都要按 RFC 跳过或接受，下一个数据报照常到达。用例：Task 5 的 capsule 编解码与 `udp_extra_capsules` 用例。
5. **服务端发 GOAWAY（重启、升级）**：之后的新请求要转到新连接，已有的流走完。用例：Task 2 与 Task 3 的 GOAWAY 用例。

## 计划期决定

写计划时对照设计、RFC 9113 / 8441 / 9298 / 9297、`h2` 0.4.19 源码、TrustTunnel 的协议文档、配置说明与 endpoint 源码（PROTOCOL.md、CONFIGURATION.md、`lib/src`，2026-10-01 只读查阅）、Surge 手册（`policies/http.html`、`policies/trust-tunnel.html`）与本仓库源码核对后定下的事；与设计文档文字不同的，写进设计文档第 16 节。

**本计划里的代码不是凭空写的。** 全部 7 个任务的改动在仓库的一份副本上按任务顺序真实做了一遍（副本用自己的构建目录），最后一次全工作区门禁见 Task 7 的 Step 5。计划里新文件的全文取自副本上该任务的提交，修改处的"把 … 换成 …"由脚本从相邻两个任务提交的差异生成，并在拼好之后按计划的顺序套到开工前的源码上逐字核对过。每个任务 Step 2 的"预期失败"是只把该任务的用例块（及写明的前置改动）套到上一个任务的状态上、真实跑出来的。

| # | 事项 | 决定与依据 |
| - | ---- | ---------- |
| P1 | V1：`h2-connect` 的凭据 | 手册自相矛盾（声明语法没有位置凭据，参数说明又说可以按位置写）：位置与命名写法都接受，命名的优先（复用 `read_credentials`，同 `http` / `https`）。`trust-tunnel` 的 `username` / `password` 只认命名写法且必填（`E0018`，不引用取值）。`max-streams` 是至少 1 的整数，缺省 3。`trust-tunnel` 的 `h3=true` 解析并 `W0029`，在 M7 之前按 HTTP/2 连；它上面的 `udp-relay` 是 `W0028` |
| P2 | ALPN 与禁用的头 | 两种协议的 `TlsOpts.alpn` 固定为 `["h2"]`，用户写了别的 `alpn` 是 `W0028`。HTTP/2 禁止的连接级头（`connection` `keep-alive` `proxy-connection` `transfer-encoding` `upgrade` `te`）在配置层从 `headers` 去掉、各报一条 `W0028`——否则 `h2` 以 malformed headers 拒绝整个请求 |
| P3 | 分步落地的门 | 同 M6a / M6b：Task 1 里两种行读完并检查但暂不产出 spec，引擎工厂先放 `BuildError` 分支；Task 6 去掉 |
| P4 | V7：`h2` 0.4.19 的四个坑 | (1) `handshake()` 不等服务端 SETTINGS——每条连接发一个 PING，只有 extended CONNECT 请求等它回来；(2) 客户端发 `:protocol` 前不检查服务端是否开了 SETTINGS_ENABLE_CONNECT_PROTOCOL——由池检查，没开时报 `h2-connect: the server does not support extended CONNECT`；(3) 到了服务端的流数上限 `send_request` 仍然成功、请求被默默排队——池自己数每条连接上的流，上限取 `max-streams` 与服务端上限的较小者；(4) GOAWAY 之后新请求失败——这样的连接不再分配新流，已有的流走完 |
| P5 | 会话池 | 选最早的、有空位且没在排空的连接，没有就拨号（同一时刻只拨一次，等着的请求拿到同一次拨号的结果）。窗口：每条流 1 MiB、每条连接 4 MiB。没有流的连接空闲 60 秒关闭（回收任务每 30 秒检查一次，第一次 `open` 时才起），用 `h2` 自己的 GOAWAY 体面关闭（最多 5 秒）。`H2Stream`：写受流控限制，写完把没用掉的发送容量还给连接；读到多少释放多少；`poll_shutdown` 发 END_STREAM。池的 `open` 本身不限时，由出站套 `opts.timeout` |
| P6 | 新连接的 SETTINGS 之前只放一条流（写计划时发现） | 服务端 SETTINGS 到达前池不知道服务端的流数上限：一条新连接在此之前只放一条流，同时进来的其它请求（在拨号锁之外）等它的 SETTINGS，再重新挑连接——放得下就用这条，放不下就按单飞规则另拨。代价是这些请求多一个往返；没选"每个并发请求各拨一条"（启动时会一下子多出几条连接） |
| P7 | V1 / V7：`h2-connect` 的 TCP | `:method CONNECT`、`:authority host:port`（IPv6 带方括号，IDN 转 A-label，发不出去的名字不拨号）；有凭据时 `proxy-authorization: Basic …`；`headers` 每个请求渲染一次（`<random-string>` 每次不同），同名的覆盖我们生成的（同 `http::merge`），只发一份；不发 `user-agent`。2xx 即隧道；407 → `h2-connect: proxy authentication required`；其它 → `h2-connect: the proxy answered <状态码>`。服务端不参与 ALPN 时报 `h2-connect: the server does not speak HTTP/2`；服务端有 ALPN 但与 `h2` 不重合时 rustls 直接中止握手，用户看到的是 `tls: received fatal alert: NoApplicationProtocol`（登记）。整个隧道（池、拨号、TLS、HTTP/2 握手、CONNECT 应答）共用一个 `opts.timeout` |
| P8 | V8：`trust-tunnel` | 同一套 CONNECT 与 Basic 认证；`user-agent`：规范标为必需、endpoint 实际可选（冲突）——总是发固定值 `rurge`（不带平台与版本，不冒充浏览器或 Surge），`headers` 写了 `User-Agent` 就替换它。2xx 隧道；407 → `trust-tunnel: authentication failed`；其它（含目标连不上的 502）→ `trust-tunnel: the server answered <状态码>`。endpoint 要求 SNI 精确匹配主机名：共用的 TLS 层发服务器主机名（IP 时用 `sni=`）。不实现 `_udp2` / `_icmp` / `_check` |
| P9 | CONNECT-UDP | `udp-relay=true` 时 `udp()` 为 `Native`；`open_udp` 不拨号，每个目标第一次发送时开一条 extended CONNECT 流（对称型，M6-D8；载体结构照 M5b 的 vmess UDP）：`:protocol connect-udp`、路径 `/.well-known/masque/udp/<host>/<port>/`（IPv6 不带方括号、冒号写成 `%3A`，名字转 A-label）、`capsule-protocol: ?1`。**服务端没开 extended CONNECT 时报错出现在第一个数据报**（不往服务端发任何东西，这条连接照常承载 TCP），而不是"载体打不开"（订正设计 5.3：为了看 SETTINGS 而专门开一条连接没有采用）。数据报放在 DATAGRAM capsule（类型 0、context id 0）里；不认识的 capsule 类型边读边丢、context id 不为 0 的跳过、varint 接受非最短写法；DATAGRAM 超过 8 + 65527 字节在缓冲前就报错；格式错误或截断即结束该目标的流，下一个数据报重开。超过 65527 字节的数据报发送时报 `InvalidInput`、不开流。每目标的流空闲 60 秒丢弃（RST_STREAM(CANCEL)） |
| P10 | V9 / 订正 M6-D5：互操作 | sing-box 1.14.2 的 `http` 入站只说 HTTP/1，`naive` 入站总带填充，都不能当 HTTP/2 CONNECT 的参考服务端。**TrustTunnel endpoint v1.1.0 同时充当 `trust-tunnel` 与 `h2-connect` 普通 CONNECT 的参考服务端**（它的 TCP 模式就是标准 HTTP/2 CONNECT 加 Basic 认证）；不另加 gost。它只有 Linux / macOS 版，只在这两个平台的 CI 按 SHA-256 安装（`RURGE_TEST_TRUSTTUNNEL`）。CONNECT-UDP over HTTP/2 没有可用的参考服务端，只有 `FakeH2Proxy` 与手工验收。`h2-connect` 对 endpoint 不发 `user-agent`（资料说可选，未实测；CI 若报 400 就在那几行加 `headers=User-Agent:…`） |
| P11 | 任务的切分 | 设计第 10 节草图的 6 个任务拆成 7 个：1 配置；2 会话池；3 `h2-connect` 的 TCP 与 `FakeH2Proxy`；4 `trust-tunnel`（连同 P6 的池修正）；5 CONNECT-UDP；6 引擎装配；7 互操作与文档 |

## 承接事项

| # | 来源 | 事项 | 处理 | 任务 |
| - | ---- | ---- | ---- | ---- |
| C1 | M6 设计第 10 节 | M6c 的全部内容 | 本计划 | 1–7 |
| C2 | M5a 延后事项（`udp-relay` 行） | `udp-relay` 对 HTTP/2 CONNECT 生效 | 本计划（P9） | 5–7 |
| C3 | 总设计 Q6 | MASQUE 与 trust-tunnel 的参考服务端 | trust-tunnel 的 h2 模式定为 TrustTunnel endpoint；MASQUE 与 h3 仍留给 M7 | 7 |
| C4 | M3b #7（P21） | 测试二进制偶发崩溃 | 照旧：门禁遇到就重跑 | — |

## File Structure

新建：

| 文件 | 职责 | 任务 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/spec/h2.rs` | `H2ConnectSpec` / `TrustTunnelSpec` 与读取（与用例） | 1 |
| `crates/rurge-proto/src/h2pool/{mod.rs, stream.rs}` | HTTP/2 会话池、`H2Stream`、`StackDial`（与用例） | 2、3、4 |
| `crates/rurge-proto/src/h2connect.rs` → `h2connect/{mod.rs, capsule.rs, udp.rs}` | `h2-connect` 出站；Task 5 改成目录，加 capsule 编解码与 UDP 载体 | 3、5 |
| `crates/rurge-proto/src/trust_tunnel.rs` | `trust-tunnel` 出站（与用例） | 4 |
| `crates/rurge-proto/src/testing/h2proxy.rs` | `FakeH2Proxy`（CONNECT、Basic、GOAWAY、CONNECT-UDP） | 3、4、5 |
| `crates/rurge-engine/tests/outbounds_http2.rs` | 经引擎的端到端用例 | 6 |
| `tests/interop/src/trusttunnel.rs`、`tests/interop/tests/http2.rs` | TrustTunnel endpoint 夹具与互操作用例 | 7 |

修改：`crates/rurge-config/src/spec/mod.rs`（`ProtoSpec`、`read_headers` 抽出）、`redact.rs` 与配置的用例与快照（1、6）；`Cargo.toml`、`crates/rurge-proto/{Cargo.toml, src/lib.rs, src/http.rs, src/transport/{stack.rs, tls.rs}, src/testing/{mod.rs, tls.rs}}`（2、3、4）；`crates/rurge-engine/src/outbounds.rs`、`crates/rurge/src/capabilities.rs`、`crates/rurge-engine/tests/common/mod.rs`、`crates/rurge/tests/cli.rs`（1、6）；`tests/interop/{src/lib.rs, README.md}`、`.github/workflows/ci.yml` 与文档（7）。

## 任务一览

| 任务 | 交付物 | 依赖 |
| ---- | ------ | ---- |
| 1 | `H2ConnectSpec` / `TrustTunnelSpec` | — |
| 2 | HTTP/2 会话池与 `H2Stream` | — |
| 3 | `h2-connect` 的 TCP、`FakeH2Proxy`、TLS 报出 ALPN | 1、2 |
| 4 | `trust-tunnel`、SETTINGS 之前只放一条流 | 3 |
| 5 | CONNECT-UDP | 3、4 |
| 6 | 引擎装配、能力表翻转、经引擎的用例 | 1–5 |
| 7 | 互操作（TrustTunnel endpoint）与文档 | 1–6 |

---

### Task 1: 两种协议的配置

`H2ConnectSpec` / `TrustTunnelSpec`（P1、P2）；原来 `http` 分支里解析 `headers` 的代码抽成 `read_headers`（报错文字不变）；两种行读完并检查但暂不产出 spec，引擎工厂先放占位分支（P3）。

**Files:**
- Create: `crates/rurge-config/src/spec/h2.rs`（自带用例）
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`src/redact.rs`（用例）、`tests/policy_spec.rs`、`tests/snapshots/corpus__corpus__kitchen-sink.snap`、`crates/rurge-engine/src/outbounds.rs`

**Interfaces:**
- Consumes: 既有的 `read_tls`、`read_credentials`、`HeaderTemplate::parse_list`、`Secret<T>`、诊断码。
- Produces: `rurge_config::spec::h2`：`pub const DEFAULT_MAX_STREAMS: u32 = 3`、`pub struct H2ConnectSpec { tls, username, password, headers, max_streams, udp_relay }`、`pub struct TrustTunnelSpec { tls, username, password, headers, max_streams }`、`pub struct TrustTunnelRead { pub spec, pub h3: bool }`、`read_h2_connect`、`read_trust_tunnel`；`ProtoSpec::H2Connect` / `ProtoSpec::TrustTunnel`

- [ ] **Step 1: 先写用例**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            None
        );
    }
}

```

换成

```rust
            None
        );
    }

    /// `h2-connect` and `trust-tunnel` lines are read and checked in full;
    /// they have no spec until the engine builds them (M6c task 6).
    #[test]
    fn h2_connect_and_trust_tunnel_lines_are_checked_but_have_no_spec_yet() {
        for def in [
            "h2-connect, 1.2.3.4, 443, max-streams=5",
            "h2-connect, h.test, 443, user, pass, udp-relay=true, client-cert=cert1, shadow-tls-password=st, underlying-proxy=Entry",
            "trust-tunnel, 192.168.20.62, 443, username=test, password=test",
            "trust-tunnel, h.test, 443, username=u, password=p, client-cert=cert1, shadow-tls-password=st, underlying-proxy=Pick",
        ] {
            let o = outcome("T", def);
            assert!(o.diagnostics.is_empty(), "{def}: {:?}", o.diagnostics);
            assert!(o.inert.is_empty(), "{def}: {:?}", o.inert);
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{def}");
        }
        let o = outcome(
            "T",
            "trust-tunnel, h.test, 443, username=u, password=p, h3=true",
        );
        assert!(o.diagnostics.is_empty(), "{:?}", o.diagnostics);
        assert_eq!(o.inert, ["h3"]);
        let o = outcome("T", "trust-tunnel, h.test, 443, max-streams=0");
        let found: Vec<(&str, &str)> = o
            .diagnostics
            .iter()
            .map(|d| (d.code, d.message.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `username` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: `password` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `T`: invalid value `0` for `max-streams` (expected an integer, at least 1)"
                ),
            ]
        );
        // both always run over TLS: the client certificate is theirs
        let tls = TlsOpts {
            client_cert: Some("cert1".into()),
            ..TlsOpts::default()
        };
        let h2 = ProtoSpec::H2Connect(H2ConnectSpec {
            tls: tls.clone(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: h2::DEFAULT_MAX_STREAMS,
            udp_relay: false,
        });
        assert_eq!(h2.keystore_item(), Some("cert1"));
        let tt = ProtoSpec::TrustTunnel(TrustTunnelSpec {
            tls,
            username: "u".into(),
            password: "p".into(),
            headers: Vec::new(),
            max_streams: h2::DEFAULT_MAX_STREAMS,
        });
        assert_eq!(tt.keystore_item(), Some("cert1"));
        assert!(tt.tls().is_some());
    }
}

```

`crates/rurge-config/src/redact.rs`——把

```rust
            "snell, 1.2.3.4, 8000, psk=***, version=5, obfs=http, obfs-host=***"
        );
    }
}

```

换成

```rust
            "snell, 1.2.3.4, 8000, psk=***, version=5, obfs=http, obfs-host=***"
        );
    }

    /// The HTTP/2 family (phase 2 M6 design 6): named and positional
    /// credentials and the whole `headers` list; `max-streams` and `h3` stay.
    #[test]
    fn an_h2_line_loses_its_credentials_and_headers() {
        assert_eq!(
            redact_definition(
                "h2-connect, example.com, 443, user, pass, headers=X-Token:abc, max-streams=5, udp-relay=true"
            ),
            "h2-connect, example.com, 443, ***, ***, headers=***, max-streams=5, udp-relay=true"
        );
        assert_eq!(
            redact_definition(
                "trust-tunnel, 192.168.20.62, 443, username=test, password=s3cret, headers=X-Padding:<random-string(16-32)>, h3=true"
            ),
            "trust-tunnel, 192.168.20.62, 443, username=***, password=***, headers=***, h3=true"
        );
    }
}

```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
            ),
        ]
    );
```

换成

```rust
            ),
        ]
    );
}

/// `h3=true` on `trust-tunnel` is said once per load and the policy keeps
/// HTTP/2 (phase 2 M6 design 5.1).
#[test]
fn trust_tunnel_h3_is_reported_once_per_load() {
    let loaded = load(
        "A = trust-tunnel, a.example, 443, username=u, password=p, h3=true
B = trust-tunnel, b.example, 443, username=u, password=p, h3=true",
        "",
    );
    assert!(!loaded.diagnostics.has_errors(), "{:?}", loaded.diagnostics);
    let inert: Vec<(String, u32)> = loaded
        .diagnostics
        .iter()
        .filter(|d| d.code == codes::W_PARAM_NOT_EFFECTIVE)
        .map(|d| (d.message.clone(), d.span.as_ref().unwrap().line))
        .collect();
    assert_eq!(
        inert,
        [(
            "policy parameter `h3` is parsed but has no effect in this version".to_string(),
            2
        )]
    );
```

语料库里的 `trust-tunnel` 行写了 `h3=true`，多一条 `W0029`：

`crates/rurge-config/tests/snapshots/corpus__corpus__kitchen-sink.snap`——把

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
```

换成

```text
  - "warning[W0029] valid/kitchen-sink.conf:53: policy parameter `tfo` is parsed but has no effect in this version"
  - "warning[W0029] valid/kitchen-sink.conf:67: policy parameter `h3` is parsed but has no effect in this version"
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-config --lib`
Expected: FAIL——两个 spec、`ProtoSpec` 的两个变体与 `spec::h2` 由 Step 3 引入，编译不过：

```text
error[E0422]: cannot find struct, variant or union type `H2ConnectSpec` in this scope
error[E0422]: cannot find struct, variant or union type `TrustTunnelSpec` in this scope
error[E0599]: no variant or associated item named `H2Connect` found for enum `spec::ProtoSpec` in the current scope
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `h2`
error[E0599]: no variant or associated item named `TrustTunnel` found for enum `spec::ProtoSpec` in the current scope
error[E0433]: failed to resolve: use of unresolved module or unlinked crate `h2`
Some errors have detailed explanations: E0422, E0433, E0599.
For more information about an error, try `rustc --explain E0422`.
error: could not compile `rurge-config` (lib test) due to 6 previous errors
exit 101
```

- [ ] **Step 3: 实现**

新建 `crates/rurge-config/src/spec/h2.rs`：

```rust
//! `h2-connect` and `trust-tunnel` policy parameters (manual: Policies ›
//! HTTP and HTTP/2, Policies › Trust Tunnel; phase 2 M6 design 5.1). Both
//! carry each connection as a CONNECT stream over a TLS + HTTP/2 connection.

use super::http::HeaderTemplate;
use super::reader::ParamReader;
use super::secret::Secret;
use super::tls::{TlsOpts, read_tls};
use super::{read_credentials, read_headers};
use crate::diagnostic::codes;
use crate::keystore::KeystoreItem;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct H2ConnectSpec {
    /// HTTP/2 CONNECT always runs over TLS; `alpn` is always `h2`.
    pub tls: TlsOpts,
    pub username: Option<Secret<String>>,
    pub password: Option<Secret<String>>,
    pub headers: Vec<HeaderTemplate>,
    /// `max-streams`: CONNECT streams one HTTP/2 connection carries at once.
    pub max_streams: u32,
    /// `udp-relay`: UDP as CONNECT-UDP (RFC 9298) over extended CONNECT.
    pub udp_relay: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustTunnelSpec {
    /// Trust Tunnel always runs over TLS; `alpn` is always `h2`.
    pub tls: TlsOpts,
    pub username: Secret<String>,
    pub password: Secret<String>,
    pub headers: Vec<HeaderTemplate>,
    /// `max-streams`: CONNECT streams one HTTP/2 connection carries at once.
    pub max_streams: u32,
}

/// What a `trust-tunnel` line says: the spec, and whether it asks for
/// `h3=true`, which is parsed and reported (`W0029`) but connects over
/// HTTP/2 until the QUIC family lands (phase 2 M6 design 5.1).
pub struct TrustTunnelRead {
    pub spec: TrustTunnelSpec,
    pub h3: bool,
}

/// `max-streams` when the line has none (manual).
pub const DEFAULT_MAX_STREAMS: u32 = 3;

/// Connection-specific header fields HTTP/2 forbids (RFC 9113 8.2.2); the
/// `h2` crate refuses a request that carries any of them.
const CONNECTION_SPECIFIC: [&str; 6] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "te",
];

/// The TLS options with ALPN pinned to `h2`: an `alpn` that says anything
/// else is `W0028`.
fn read_h2_tls(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TlsOpts {
    let mut tls = read_tls(r, keystore);
    if !tls.alpn.is_empty() && tls.alpn != ["h2"] {
        let kind = r.policy().kind.keyword();
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("`alpn` is always `h2` on `{kind}` policies; ignored"),
        );
    }
    tls.alpn = vec!["h2".to_string()];
    tls
}

/// `headers`, without the fields HTTP/2 forbids (`W0028` each).
fn read_h2_headers(r: &mut ParamReader<'_>) -> Vec<HeaderTemplate> {
    let mut headers = read_headers(r);
    headers.retain(|header| {
        let name = header.name.to_ascii_lowercase();
        if !CONNECTION_SPECIFIC.contains(&name.as_str()) {
            return true;
        }
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            format!("header `{name}` in `headers` is not allowed in HTTP/2; ignored"),
        );
        false
    });
    headers
}

/// A named credential that must be there; its value is never quoted.
fn required(r: &mut ParamReader<'_>, key: &str) -> Secret<String> {
    let value = r.str(key).unwrap_or_default();
    if value.is_empty() {
        r.error(
            codes::E_INVALID_POLICY_PARAM,
            format!("`{key}` is required"),
        );
    }
    Secret::from(value)
}

fn read_max_streams(r: &mut ParamReader<'_>) -> u32 {
    let Some(v) = r.str("max-streams") else {
        return DEFAULT_MAX_STREAMS;
    };
    match v.trim().parse::<u32>() {
        Ok(n) if n >= 1 => n,
        _ => {
            r.invalid("max-streams", v, "an integer, at least 1");
            DEFAULT_MAX_STREAMS
        }
    }
}

/// Everything `h2-connect`-specific on the line. After an error was reported
/// the returned value is meaningless: the caller checks `r.has_errors()`.
///
/// The manual's syntax line has no positional credentials but its text says
/// they "may be given positionally after the port": both forms are read, the
/// named one winning, as on `http` / `https`.
pub fn read_h2_connect(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> H2ConnectSpec {
    let tls = read_h2_tls(r, keystore);
    let (username, password) = read_credentials(r);
    let headers = read_h2_headers(r);
    let max_streams = read_max_streams(r);
    let udp_relay = r.bool("udp-relay").unwrap_or(false);
    H2ConnectSpec {
        tls,
        username,
        password,
        headers,
        max_streams,
        udp_relay,
    }
}

/// Everything `trust-tunnel`-specific on the line. After an error was
/// reported the returned value is meaningless: the caller checks
/// `r.has_errors()`.
///
/// The credentials are named-only and required, as the manual writes them,
/// and never quoted in a diagnostic: a positional value stays unread and is
/// reported as an extra positional value.
pub fn read_trust_tunnel(r: &mut ParamReader<'_>, keystore: &[KeystoreItem]) -> TrustTunnelRead {
    let tls = read_h2_tls(r, keystore);
    let username = required(r, "username");
    let password = required(r, "password");
    let headers = read_h2_headers(r);
    let max_streams = read_max_streams(r);
    let h3 = r.bool("h3").unwrap_or(false);
    if r.has("udp-relay") {
        r.touch("udp-relay");
        r.warn(
            codes::W_PARAM_NOT_APPLICABLE,
            "`udp-relay` does not apply to `trust-tunnel` policies (no UDP); ignored".to_string(),
        );
    }
    TrustTunnelRead {
        spec: TrustTunnelSpec {
            tls,
            username,
            password,
            headers,
            max_streams,
        },
        h3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::{Diagnostic, codes};
    use crate::policy::parse_policy;
    use crate::span::Span;
    use crate::spec::{HeaderPart, Sni};
    use std::path::Path;
    use std::sync::Arc;

    fn reader_for<T>(
        def: &str,
        read: impl Fn(&mut ParamReader<'_>) -> T,
    ) -> (T, bool, Vec<Diagnostic>) {
        let p = parse_policy("P", def, &Span::new(Arc::from(Path::new("p.conf")), 1)).unwrap();
        let mut r = ParamReader::new(&p);
        let got = read(&mut r);
        let failed = r.has_errors();
        (got, failed, r.finish())
    }

    fn h2(def: &str) -> (H2ConnectSpec, bool, Vec<Diagnostic>) {
        reader_for(def, |r| read_h2_connect(r, &[]))
    }

    fn tt(def: &str) -> (TrustTunnelRead, bool, Vec<Diagnostic>) {
        reader_for(def, |r| read_trust_tunnel(r, &[]))
    }

    fn messages(diags: &[Diagnostic]) -> Vec<(&str, &str)> {
        diags.iter().map(|d| (d.code, d.message.as_str())).collect()
    }

    fn exposed(secret: &Option<Secret<String>>) -> Option<&str> {
        secret.as_ref().map(|s| s.expose().as_str())
    }

    #[test]
    fn the_manuals_h2_connect_example_and_the_defaults() {
        let (spec, failed, diags) = h2("h2-connect, 1.2.3.4, 443, max-streams=5");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.max_streams, 5);
        assert!(!spec.udp_relay);
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (None, None)
        );
        assert!(spec.headers.is_empty());
        assert_eq!(
            spec.tls,
            TlsOpts {
                alpn: vec!["h2".into()],
                ..TlsOpts::default()
            }
        );
        let (spec, failed, diags) = h2("h2-connect, example.com, 443");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.max_streams, DEFAULT_MAX_STREAMS);
    }

    /// Named credentials as the manual's example writes them, positional
    /// ones as its text allows; named win (as on `http`).
    #[test]
    fn h2_connect_credentials_named_or_positional() {
        let (spec, failed, diags) =
            h2("h2-connect, example.com, 443, username=user, password=pass");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (Some("user"), Some("pass"))
        );
        let (spec, failed, diags) = h2("h2-connect, example.com, 443, posuser, pospass");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (Some("posuser"), Some("pospass"))
        );
        let (spec, _, diags) = h2("h2-connect, example.com, 443, posuser, pospass, password=named");
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(
            (exposed(&spec.username), exposed(&spec.password)),
            (Some("posuser"), Some("named"))
        );
    }

    #[test]
    fn h2_connect_with_every_parameter() {
        let (spec, failed, diags) = h2(
            "h2-connect, example.com, 443, headers=X-Padding:<random-string(16-32)>;X-Client:rurge, max-streams=\"8\", udp-relay=true, sni=edge.test, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(!failed, "{diags:?}");
        assert_eq!(
            messages(&diags),
            [(
                codes::W_INVALID_VALUE,
                "policy `P`: `skip-cert-verify` is ignored because `server-cert-fingerprint-sha256` is set"
            )]
        );
        assert_eq!(spec.max_streams, 8);
        assert!(spec.udp_relay);
        assert_eq!(spec.headers.len(), 2);
        assert_eq!(spec.headers[0].name, "X-Padding");
        assert_eq!(
            spec.headers[0].value,
            [HeaderPart::Random { min: 16, max: 32 }]
        );
        assert_eq!(spec.tls.sni, Sni::Name("edge.test".into()));
        assert!(spec.tls.skip_cert_verify && spec.tls.fingerprint_sha256.is_some());
    }

    #[test]
    fn max_streams_is_an_integer_of_at_least_one() {
        for bad in ["0", "-1", "two", "1.5", "4294967296", ""] {
            let (_, failed, diags) = h2(&format!("h2-connect, h.test, 443, max-streams={bad}"));
            assert!(failed, "{bad}");
            assert_eq!(
                messages(&diags),
                [(
                    codes::E_INVALID_POLICY_PARAM,
                    format!(
                        "policy `P`: invalid value `{bad}` for `max-streams` (expected an integer, at least 1)"
                    )
                    .as_str()
                )]
            );
            let (got, failed, _) = tt(&format!(
                "trust-tunnel, h.test, 443, username=u, password=p, max-streams={bad}"
            ));
            assert!(failed, "{bad}");
            assert_eq!(got.spec.max_streams, DEFAULT_MAX_STREAMS);
        }
        let (spec, failed, _) = h2("h2-connect, h.test, 443, max-streams=1");
        assert!(!failed);
        assert_eq!(spec.max_streams, 1);
    }

    #[test]
    fn the_manuals_trust_tunnel_example() {
        let (got, failed, diags) =
            tt("trust-tunnel, 192.168.20.62, 443, username=test, password=test");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!got.h3);
        let spec = got.spec;
        assert_eq!(
            (
                spec.username.expose().as_str(),
                spec.password.expose().as_str()
            ),
            ("test", "test")
        );
        assert_eq!(spec.max_streams, DEFAULT_MAX_STREAMS);
        assert!(spec.headers.is_empty());
        assert_eq!(spec.tls.alpn, ["h2"]);
        let (got, failed, diags) = tt(
            "trust-tunnel, tt.test, 443, username=u, password=p, headers=X-Padding:<random-string(16-32)>, max-streams=5, sni=tt.test, client-cert=nope",
        );
        assert!(failed, "an unknown keystore item is an error");
        assert_eq!(diags[0].code, codes::E_KEYSTORE_REF);
        assert_eq!(got.spec.max_streams, 5);
        assert_eq!(got.spec.headers.len(), 1);
    }

    #[test]
    fn trust_tunnel_credentials_are_required_named_and_never_quoted() {
        let (_, failed, diags) = tt("trust-tunnel, h.test, 443");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `username` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
            ]
        );
        let (_, failed, diags) = tt("trust-tunnel, h.test, 443, s3cretUser, hunter2, password=");
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `username` is required"
                ),
                (
                    codes::E_INVALID_POLICY_PARAM,
                    "policy `P`: `password` is required"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #1 ignored"
                ),
                (
                    codes::W_UNKNOWN_KEY,
                    "policy `P`: unexpected positional value #2 ignored"
                ),
            ]
        );
        assert!(
            diags
                .iter()
                .all(|d| !d.message.contains("s3cretUser") && !d.message.contains("hunter2"))
        );
    }

    /// `h3=true` is read (the loader says once that it has no effect yet)
    /// and the policy connects over HTTP/2; `udp-relay` has no place here.
    #[test]
    fn trust_tunnel_h3_and_udp_relay() {
        let (got, failed, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, h3=true");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(got.h3);
        let (got, failed, diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, h3=false");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert!(!got.h3);
        let (_, failed, _) = tt("trust-tunnel, h.test, 443, username=u, password=p, h3=maybe");
        assert!(failed);
        let (_, failed, diags) =
            tt("trust-tunnel, h.test, 443, username=u, password=p, udp-relay=true");
        assert!(!failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `udp-relay` does not apply to `trust-tunnel` policies (no UDP); ignored"
            )]
        );
    }

    /// ALPN is `h2` whatever the line says: `alpn=h2` is welcome, anything
    /// else is said and ignored.
    #[test]
    fn alpn_is_always_h2() {
        let (spec, failed, diags) = h2("h2-connect, h.test, 443, alpn=h2");
        assert!(!failed && diags.is_empty(), "{diags:?}");
        assert_eq!(spec.tls.alpn, ["h2"]);
        let (spec, failed, diags) = h2("h2-connect, h.test, 443, alpn=http/1.1,h2");
        assert!(!failed);
        assert_eq!(spec.tls.alpn, ["h2"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h2` on `h2-connect` policies; ignored"
            )]
        );
        let (got, _, diags) = tt("trust-tunnel, h.test, 443, username=u, password=p, alpn=h3");
        assert_eq!(got.spec.tls.alpn, ["h2"]);
        assert_eq!(
            messages(&diags),
            [(
                codes::W_PARAM_NOT_APPLICABLE,
                "policy `P`: `alpn` is always `h2` on `trust-tunnel` policies; ignored"
            )]
        );
    }

    #[test]
    fn connection_specific_headers_are_dropped() {
        let (spec, failed, diags) = h2(
            "h2-connect, h.test, 443, headers=Connection:close;X-A:1;Transfer-Encoding:chunked;TE:trailers",
        );
        assert!(!failed);
        assert_eq!(
            spec.headers
                .iter()
                .map(|h| h.name.as_str())
                .collect::<Vec<_>>(),
            ["X-A"]
        );
        assert_eq!(
            messages(&diags),
            [
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `connection` in `headers` is not allowed in HTTP/2; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `transfer-encoding` in `headers` is not allowed in HTTP/2; ignored"
                ),
                (
                    codes::W_PARAM_NOT_APPLICABLE,
                    "policy `P`: header `te` in `headers` is not allowed in HTTP/2; ignored"
                ),
            ]
        );
        // a malformed list is an error, its entry named by position
        let (_, failed, diags) = tt(
            "trust-tunnel, h.test, 443, username=u, password=p, headers=X-A:1;Authorization Bearer s3cret",
        );
        assert!(failed);
        assert_eq!(
            messages(&diags),
            [(
                codes::E_INVALID_POLICY_PARAM,
                "policy `P`: invalid `headers`: header #2 has no `:`"
            )]
        );
    }

    #[test]
    fn the_specs_do_not_print_credentials() {
        let (spec, _, _) = h2("h2-connect, h.test, 443, username=s3cretUser, password=hunter2");
        let (got, _, _) = tt("trust-tunnel, h.test, 443, username=s3cretUser, password=hunter2");
        for printed in [format!("{spec:?}"), format!("{:?}", got.spec)] {
            assert!(printed.contains("Secret(***)"), "{printed}");
            assert!(
                !printed.contains("s3cretUser") && !printed.contains("hunter2"),
                "{printed}"
            );
        }
    }
}
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
pub mod group;
```

换成

```rust
pub mod group;
pub mod h2;
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
};
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
```

换成

```rust
};
pub use h2::{H2ConnectSpec, TrustTunnelSpec};
pub use http::{HeaderPart, HeaderTemplate, HttpSpec};
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    Snell(SnellSpec),
```

换成

```rust
    Snell(SnellSpec),
    H2Connect(H2ConnectSpec),
    TrustTunnel(TrustTunnelSpec),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            ProtoSpec::AnyTls(anytls) => Some(&anytls.tls),
```

换成

```rust
            ProtoSpec::AnyTls(anytls) => Some(&anytls.tls),
            ProtoSpec::H2Connect(h2) => Some(&h2.tls),
            ProtoSpec::TrustTunnel(tt) => Some(&tt.tls),
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    (username, password)
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
```

换成

```rust
    (username, password)
}

/// `headers`: a malformed list is `E0018`, its entry named by position.
fn read_headers(r: &mut ParamReader<'_>) -> Vec<HeaderTemplate> {
    let Some(v) = r.str("headers") else {
        return Vec::new();
    };
    match HeaderTemplate::parse_list(v) {
        Ok(list) => list,
        Err(why) => {
            r.error(
                codes::E_INVALID_POLICY_PARAM,
                format!("invalid `headers`: {why}"),
            );
            Vec::new()
        }
    }
}

fn check_underlying(r: &mut ParamReader<'_>, common: &mut CommonOpts, env: &SpecEnv<'_>) {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            let mut headers = Vec::new();
            if let Some(v) = r.str("headers") {
                match HeaderTemplate::parse_list(v) {
                    Ok(list) => headers = list,
                    Err(why) => r.error(
                        codes::E_INVALID_POLICY_PARAM,
                        format!("invalid `headers`: {why}"),
                    ),
                }
            }
```

换成

```rust
            let headers = read_headers(&mut r);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            (common, ProtoSpec::AnyTls(anytls))
        }
        PolicyKind::Ssh => {
```

换成

```rust
            (common, ProtoSpec::AnyTls(anytls))
        }
        PolicyKind::H2Connect => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let h2 = h2::read_h2_connect(&mut r, env.keystore);
            (common, ProtoSpec::H2Connect(h2))
        }
        PolicyKind::TrustTunnel => {
            let common = read_common(&mut r, Applies::Proxy, &mut notes);
            let read = h2::read_trust_tunnel(&mut r, env.keystore);
            // connects over HTTP/2 until the QUIC family (phase 2 M6 design 5.1)
            if read.h3 {
                notes.inert.push("h3");
            }
            (common, ProtoSpec::TrustTunnel(read.spec))
        }
        PolicyKind::Ssh => {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

换成

```rust
    // the engine builds `h2-connect` and `trust-tunnel` from M6c task 6 on:
    // until then a valid line is checked in full but has no spec
    let built = !matches!(policy.kind, PolicyKind::H2Connect | PolicyKind::TrustTunnel);
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            )?),
            // nothing starts here: the program starts on the first dial
```

换成

```rust
            )?),
            // the loader makes no spec of these lines before M6c task 6
            ProtoSpec::H2Connect(_) | ProtoSpec::TrustTunnel(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `{}` is not implemented yet",
                    spec.name,
                    spec.kind.keyword()
                )));
            }
            // nothing starts here: the program starts on the first dial
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-config` → 通过（`spec::h2` 的用例、`h2_connect_and_trust_tunnel_lines_are_checked_but_have_no_spec_yet`、脱敏用例、语料库快照）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-config crates/rurge-engine/src/outbounds.rs
git commit -m "feat(config): h2-connect 与 trust-tunnel 的配置（ALPN 固定 h2、HTTP/2 禁止的头在配置层去掉）"
```

### Task 2: HTTP/2 会话池

`rurge_proto::h2pool`（P4、P5）：每个出站一个 HTTP/2 连接池与 `H2Stream`。本任务还没有出站用它，`pub(crate) mod h2pool` 暂时带 `#[allow(dead_code)]`（Task 3 去掉）。本任务的用例（进程内的 `h2` 服务端）都在新模块里，与实现一同写出，没有单独的失败步骤。

**Files:**
- Create: `crates/rurge-proto/src/h2pool/mod.rs`、`stream.rs`（自带用例）
- Modify: `Cargo.toml`（工作区依赖 `h2 = "0.4"`）、`crates/rurge-proto/Cargo.toml`、`src/lib.rs`

**Interfaces:**
- Produces: `pub(crate) struct H2Pool`（`new(label, max_streams, dial: Arc<dyn Dial>)`、`open(req: http::Request<()>, opts: &ConnectOpts) -> Result<http::Response<H2Stream>, OutboundError>`、扩展 CONNECT 的门）；`pub(crate) trait Dial`；`pub(crate) struct H2Stream`（`new(label, send, recv)`，`AsyncRead` / `AsyncWrite`，服务端一侧也可用）

- [ ] **Step 1: 依赖与新模块**

`h2` 0.4.19 已经经 `hyper` 在锁文件里：

`Cargo.toml`——把

```toml
http = "1"
```

换成

```toml
http = "1"
h2 = "0.4"
```

`crates/rurge-proto/Cargo.toml`——把

```toml
http.workspace = true
```

换成

```toml
http.workspace = true
h2.workspace = true
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod external;
```

换成

```rust
pub mod external;
// the `h2-connect` outbound (M6c task 3) is its first user
#[allow(dead_code)]
pub(crate) mod h2pool;
```

新建 `crates/rurge-proto/src/h2pool/mod.rs`：

```rust
//! The HTTP/2 connections of one outbound, shared by its streams (phase 2
//! M6 design 5.2): `h2-connect` and `trust-tunnel` send each request (a
//! CONNECT) as a stream of a pooled connection.
//!
//! - A request goes to the oldest connection that carries fewer than
//!   `max-streams` streams (and fewer than the server's own limit) and is
//!   not draining. `h2` would queue a stream past the server's limit and
//!   send nothing until a slot frees, so the pool counts the open streams
//!   itself; a stream's `Lease` holds its place.
//! - With no room, a new connection is dialed. One dial at a time: requests
//!   that come in meanwhile wait for it and then look again, and when it
//!   fails they share its failure instead of dialing the same thing again.
//! - A connection that received GOAWAY, failed, or ended gets no new
//!   streams; the ones it carries run to their end.
//! - A connection without streams for `IDLE_TIMEOUT` is closed (looked at
//!   on every request and by a reaper).
//!
//! The pool does not know the transport: the outbound's `Dial` opens it
//! (TCP, Shadow TLS, TLS with ALPN `h2`) and checks that `h2` was
//! negotiated.

mod stream;

pub(crate) use stream::H2Stream;

use crate::OutboundError;
use crate::task::AbortOnDrop;
use bytes::Bytes;
use h2::client::{self, SendRequest};
use h2::ext::Protocol;
use h2::{Ping, PingPong};
use http::{Request, Response};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts};
use std::io;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const REAP_EVERY: Duration = Duration::from_secs(30);
/// What a stream may receive before its reader catches up
/// (SETTINGS_INITIAL_WINDOW_SIZE). The TrustTunnel document's 128 KiB
/// would cap a tunnel at 1.3 MB/s over a 100 ms round trip.
const STREAM_WINDOW: u32 = 1 << 20;
/// What the connection's streams together may receive: the default
/// `max-streams` at a full window each, and some.
const CONNECTION_WINDOW: u32 = 4 << 20;
/// How long a connection the pool let go of may take to close cleanly
/// (GOAWAY, then the transport's shutdown).
const CLOSE_GRACE: Duration = Duration::from_secs(5);

/// Opens the transport of a new connection. No `Debug` for implementers
/// that hold credentials.
pub(crate) trait Dial: Send + Sync {
    fn dial<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
}

type Conns = Mutex<Vec<Arc<Conn>>>;

/// No `Debug`: the dialer may hold credentials.
pub(crate) struct H2Pool {
    /// The protocol the error texts start with (`h2-connect`).
    label: &'static str,
    max_streams: usize,
    dial: Arc<dyn Dial>,
    /// The connections that take new streams, oldest first.
    conns: Arc<Conns>,
    /// One dial at a time.
    dialing: tokio::sync::Mutex<()>,
    /// Bumped after every dial, success or failure: tells a request that
    /// waited for `dialing` whether a dial finished meanwhile.
    attempts: AtomicU64,
    /// The latest dial's failure (`None` after a success); read and written
    /// with `dialing` held.
    failure: Mutex<Option<OutboundError>>,
    /// Started by the first request: building an outbound (a dry build
    /// included) leaves no task behind.
    reaper: OnceLock<AbortOnDrop>,
}

impl H2Pool {
    /// `max_streams` is the policy's `max-streams` (at least 1).
    pub(crate) fn new(label: &'static str, max_streams: u32, dial: Arc<dyn Dial>) -> H2Pool {
        H2Pool {
            label,
            max_streams: max_streams as usize,
            dial,
            conns: Arc::default(),
            dialing: tokio::sync::Mutex::new(()),
            attempts: AtomicU64::new(0),
            failure: Mutex::new(None),
            reaper: OnceLock::new(),
        }
    }

    /// Sends `request` on a stream of a pooled connection and waits for the
    /// response head; the body is the stream. A request carrying
    /// `h2::ext::Protocol` (extended CONNECT, RFC 8441) waits until the
    /// server's SETTINGS are known and fails unless they enabled it.
    ///
    /// Any status is returned: what a non-2xx one means is the caller's to
    /// say. Dropping the stream resets it.
    pub(crate) async fn open(
        &self,
        request: Request<()>,
        opts: &ConnectOpts,
    ) -> Result<Response<H2Stream>, OutboundError> {
        self.reaper.get_or_init(|| spawn_reaper(&self.conns));
        let lease = self.lease(opts).await?;
        let conn = lease.0.clone();
        if request.extensions().get::<Protocol>().is_some() {
            conn.settled().await;
            if !conn.send.is_extended_connect_protocol_enabled() {
                return Err(OutboundError::Proxy(format!(
                    "{}: the server does not support extended CONNECT",
                    self.label
                )));
            }
        }
        // a fresh handle: another request's queued stream cannot hold it up
        let (response, send) = conn
            .send
            .clone()
            .send_request(request, false)
            .map_err(|e| self.error(&e))?;
        let response = response.await.map_err(|e| self.error(&e))?;
        let (head, recv) = response.into_parts();
        let stream = H2Stream::new(self.label, send, recv).leased(lease);
        Ok(Response::from_parts(head, stream))
    }

    fn error(&self, e: &h2::Error) -> OutboundError {
        OutboundError::Proxy(format!("{}: {}", self.label, describe(e)))
    }

    /// A place on a connection with room, dialing one when there is none.
    async fn lease(&self, opts: &ConnectOpts) -> Result<Lease, OutboundError> {
        if let Some(lease) = self.take() {
            return Ok(lease);
        }
        // read before contending for the lock: tells us whether a dial
        // finished while we waited for it
        let attempt = self.attempts.load(Ordering::SeqCst);
        let _dialing = self.dialing.lock().await;
        if let Some(lease) = self.take() {
            return Ok(lease);
        }
        if self.attempts.load(Ordering::SeqCst) != attempt
            && let Some(failure) = self.failure.lock().expect("failure").as_ref()
        {
            return Err(copy_error(failure));
        }
        let result = self.connect(opts).await;
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let conn = match result {
            Ok(conn) => Arc::new(conn),
            Err(e) => {
                *self.failure.lock().expect("failure") = Some(copy_error(&e));
                return Err(e);
            }
        };
        *self.failure.lock().expect("failure") = None;
        let lease = conn.lease(self.max_streams);
        self.conns.lock().expect("conns").push(conn);
        lease.ok_or_else(|| {
            OutboundError::Proxy(format!("{}: the server accepts no streams", self.label))
        })
    }

    /// The oldest connection with room; the ones that are draining or have
    /// idled too long are let go on the way.
    fn take(&self) -> Option<Lease> {
        let mut conns = self.conns.lock().expect("conns");
        prune(&mut conns, Instant::now());
        conns.iter().find_map(|c| c.lease(self.max_streams))
    }

    async fn connect(&self, opts: &ConnectOpts) -> Result<Conn, OutboundError> {
        let io = self.dial.dial(opts).await?;
        let mut builder = client::Builder::new();
        builder
            .initial_window_size(STREAM_WINDOW)
            .initial_connection_window_size(CONNECTION_WINDOW)
            .enable_push(false);
        // writes the preface; the server's SETTINGS come later
        let (send, mut connection) = builder.handshake::<_, Bytes>(io).await.map_err(|e| {
            OutboundError::Proxy(format!(
                "{}: the HTTP/2 handshake failed: {}",
                self.label,
                describe(&e)
            ))
        })?;
        let ping = connection.ping_pong();
        let (settled_tx, settled) = watch::channel(false);
        let (closing, closed) = oneshot::channel();
        let ended = Arc::new(AtomicBool::new(false));
        tokio::spawn(drive(
            self.label,
            connection,
            ping,
            settled_tx,
            closed,
            ended.clone(),
        ));
        Ok(Conn {
            send,
            settled,
            ended,
            usage: Mutex::new(Usage {
                open: 0,
                idle_since: Instant::now(),
            }),
            _closing: closing,
        })
    }

    #[cfg(test)]
    fn connections(&self) -> usize {
        let mut conns = self.conns.lock().expect("conns");
        prune(&mut conns, Instant::now());
        conns.len()
    }
}

/// One pooled connection. Its streams' leases keep it alive after the pool
/// let go of it; when the last one goes, so does the connection.
struct Conn {
    send: SendRequest<Bytes>,
    /// Turns true once the server's SETTINGS are known (or never, when the
    /// connection ends first: the sender is dropped).
    settled: watch::Receiver<bool>,
    /// The connection's task has finished.
    ended: Arc<AtomicBool>,
    usage: Mutex<Usage>,
    /// Dropped with the connection: tells its task to close it.
    _closing: oneshot::Sender<()>,
}

struct Usage {
    open: usize,
    /// When `open` last fell to 0.
    idle_since: Instant,
}

impl Conn {
    fn lease(self: &Arc<Self>, max_streams: usize) -> Option<Lease> {
        // before the server's SETTINGS, its limit is unbounded
        let limit = max_streams.min(self.send.current_max_send_streams());
        let mut usage = self.usage.lock().expect("usage");
        if usage.open >= limit {
            return None;
        }
        usage.open += 1;
        Some(Lease(self.clone()))
    }

    /// GOAWAY received, an error, or the task ended: `h2` refuses new
    /// streams. A fresh handle is never pending, so the look never waits.
    fn is_draining(&self) -> bool {
        let mut cx = Context::from_waker(Waker::noop());
        self.ended.load(Ordering::SeqCst)
            || matches!(self.send.clone().poll_ready(&mut cx), Poll::Ready(Err(_)))
    }

    fn is_expired(&self, now: Instant) -> bool {
        let usage = self.usage.lock().expect("usage");
        usage.open == 0 && now.duration_since(usage.idle_since) >= IDLE_TIMEOUT
    }

    /// The server's SETTINGS are the first frame it sends (RFC 9113 3.4):
    /// the answer to our PING comes after them.
    async fn settled(&self) {
        let mut settled = self.settled.clone();
        let _ = settled.wait_for(|known| *known).await;
    }
}

/// A stream's place on its connection.
struct Lease(Arc<Conn>);

impl Drop for Lease {
    fn drop(&mut self) {
        let mut usage = self.0.usage.lock().expect("usage");
        usage.open -= 1;
        if usage.open == 0 {
            usage.idle_since = Instant::now();
        }
    }
}

fn prune(conns: &mut Vec<Arc<Conn>>, now: Instant) {
    conns.retain(|c| !c.is_draining() && !c.is_expired(now));
}

/// Runs a connection until it ends, or until the pool and its streams let
/// go of it and it has had `CLOSE_GRACE` to close. Meanwhile it learns the
/// server's SETTINGS with a PING round trip (`h2` has no way to wait for
/// them).
async fn drive<T>(
    label: &'static str,
    connection: client::Connection<T, Bytes>,
    ping: Option<PingPong>,
    settled: watch::Sender<bool>,
    closed: oneshot::Receiver<()>,
    ended: Arc<AtomicBool>,
) where
    T: AsyncRead + AsyncWrite + Unpin,
{
    let mut connection = pin!(connection);
    let settle = async {
        if let Some(mut ping) = ping {
            // failing, the connection is failing too: nothing more to learn
            let _ = ping.ping(Ping::opaque()).await;
        }
        settled.send_replace(true);
        std::future::pending::<()>().await
    };
    let finished = tokio::select! {
        result = &mut connection => Some(result),
        // the sender is dropped, never used
        _ = closed => None,
        () = settle => unreachable!("settling never ends"),
    };
    let result = match finished {
        Some(result) => result,
        // no handle and no stream left: `h2` sends GOAWAY and closes
        None => tokio::time::timeout(CLOSE_GRACE, connection)
            .await
            .unwrap_or(Ok(())),
    };
    ended.store(true, Ordering::SeqCst);
    if let Err(e) = result {
        tracing::debug!("{label}: HTTP/2 connection ended: {}", describe(&e));
    }
}

/// What went wrong, without the GOAWAY's debug data.
fn describe(e: &h2::Error) -> String {
    match (e.reason(), e.get_io()) {
        (Some(reason), _) if e.is_go_away() && e.is_remote() => {
            format!("the server closed the HTTP/2 connection ({reason:?})")
        }
        (Some(reason), _) if e.is_reset() && e.is_remote() => {
            format!("the server reset the stream ({reason:?})")
        }
        (Some(reason), _) => format!("HTTP/2 error ({reason:?})"),
        (None, Some(io)) => format!("the HTTP/2 connection failed: {io}"),
        (None, None) => format!("HTTP/2 error: {e}"),
    }
}

/// The latest dial's failure for the requests that waited for it.
fn copy_error(e: &OutboundError) -> OutboundError {
    match e {
        OutboundError::Reject(kind) => OutboundError::Reject(*kind),
        OutboundError::Unsupported(m) => OutboundError::Unsupported(m.clone()),
        OutboundError::Dns(m) => OutboundError::Dns(m.clone()),
        OutboundError::Io(e) => OutboundError::Io(io::Error::new(e.kind(), e.to_string())),
        OutboundError::Timeout => OutboundError::Timeout,
        OutboundError::Proxy(m) => OutboundError::Proxy(m.clone()),
        OutboundError::Tls(m) => OutboundError::Tls(m.clone()),
        OutboundError::Unavailable(m) => OutboundError::Unavailable(m.clone()),
    }
}

/// Lets go of idle connections until the pool is gone. Holds it weakly: the
/// pool dies with its outbound.
fn spawn_reaper(conns: &Arc<Conns>) -> AbortOnDrop {
    let conns: Weak<Conns> = Arc::downgrade(conns);
    AbortOnDrop(tokio::spawn(async move {
        let mut tick = tokio::time::interval(REAP_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let Some(conns) = conns.upgrade() else {
                return;
            };
            prune(&mut conns.lock().expect("conns"), Instant::now());
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use h2::server::SendResponse;
    use h2::{Reason, RecvStream};
    use http::{Method, StatusCode, Version};
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::Notify;

    /// What the in-process server advertises.
    #[derive(Clone, Copy, Default)]
    struct Server {
        extended: bool,
        /// SETTINGS_INITIAL_WINDOW_SIZE: what each of our streams may send
        /// before a WINDOW_UPDATE.
        window: Option<u32>,
    }

    /// Dials an in-process `h2` server over a pipe. By the host of the
    /// CONNECT, it echoes (half-closing after us), refuses (`deny…`, 407),
    /// or resets the stream after our first bytes (`reset…`).
    #[derive(Default)]
    struct TestDial {
        server: Server,
        /// Fails every dial, after a yield.
        fail: bool,
        dials: AtomicUsize,
        /// One per dial: makes that connection's server send GOAWAY.
        goaway: Mutex<Vec<Arc<Notify>>>,
        /// Server connections that have ended.
        ended: Arc<AtomicUsize>,
    }

    impl Dial for TestDial {
        fn dial<'a>(
            &'a self,
            _opts: &'a ConnectOpts,
        ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
            Box::pin(async move {
                self.dials.fetch_add(1, Ordering::SeqCst);
                if self.fail {
                    tokio::task::yield_now().await;
                    return Err(OutboundError::Proxy("test: connection refused".into()));
                }
                let (near, far) = tokio::io::duplex(64 * 1024);
                let goaway = Arc::new(Notify::new());
                self.goaway.lock().unwrap().push(goaway.clone());
                tokio::spawn(serve(far, self.server, goaway, self.ended.clone()));
                Ok(Box::new(near) as BoxedStream)
            })
        }
    }

    async fn serve(io: DuplexStream, server: Server, goaway: Arc<Notify>, ended: Arc<AtomicUsize>) {
        let mut builder = h2::server::Builder::new();
        if let Some(window) = server.window {
            builder.initial_window_size(window);
        }
        if server.extended {
            builder.enable_connect_protocol();
        }
        if let Ok(mut conn) = builder.handshake::<_, Bytes>(io).await {
            loop {
                tokio::select! {
                    next = conn.accept() => match next {
                        Some(Ok((request, respond))) => {
                            tokio::spawn(answer(request, respond));
                        }
                        _ => break,
                    },
                    () = goaway.notified() => conn.graceful_shutdown(),
                }
            }
        }
        ended.fetch_add(1, Ordering::SeqCst);
    }

    async fn answer(request: Request<RecvStream>, mut respond: SendResponse<Bytes>) {
        let host = request.uri().host().unwrap_or_default().to_string();
        if host.starts_with("deny") {
            let refusal = Response::builder().status(407).body(()).unwrap();
            let _ = respond.send_response(refusal, true);
            return;
        }
        let Ok(mut send) = respond.send_response(Response::new(()), false) else {
            return;
        };
        let mut recv = request.into_body();
        if host.starts_with("reset") {
            let _ = recv.data().await;
            send.send_reset(Reason::CONNECT_ERROR);
            return;
        }
        let stream = H2Stream::new("server", send, recv);
        let (mut r, mut w) = tokio::io::split(stream);
        let _ = tokio::io::copy(&mut r, &mut w).await;
        let _ = w.shutdown().await;
    }

    fn pool(dial: &Arc<TestDial>, max_streams: u32) -> H2Pool {
        H2Pool::new("test", max_streams, dial.clone())
    }

    fn connect(authority: &str) -> Request<()> {
        Request::builder()
            .method(Method::CONNECT)
            .uri(authority)
            .version(Version::HTTP_2)
            .body(())
            .unwrap()
    }

    async fn tunnel(pool: &H2Pool, authority: &str) -> H2Stream {
        let response = pool
            .open(connect(authority), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        response.into_body()
    }

    async fn round_trip(stream: &mut H2Stream, data: &[u8]) {
        stream.write_all(data).await.unwrap();
        let mut back = vec![0; data.len()];
        stream.read_exact(&mut back).await.unwrap();
        assert_eq!(back, data);
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_stream_carries_data_both_ways_under_flow_control() {
        // the server lets each stream send 16 KiB at a time; we get more
        // than two of our windows back, so reads must release capacity
        let dial = Arc::new(TestDial {
            server: Server {
                window: Some(16 * 1024),
                ..Server::default()
            },
            ..TestDial::default()
        });
        let pool = pool(&dial, 3);
        let stream = tunnel(&pool, "echo.test:7").await;
        let data: Vec<u8> = (0..(STREAM_WINDOW as usize * 2 + 12_345))
            .map(|i| (i % 251) as u8)
            .collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        // a window never handed back stalls the echo: bounded
        tokio::time::timeout(Duration::from_secs(30), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    #[tokio::test]
    async fn shutdown_is_a_half_close() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let mut stream = tunnel(&pool, "echo.test:7").await;
        stream.write_all(b"hello").await.unwrap();
        stream.shutdown().await.unwrap();
        // the server saw our END_STREAM, echoed, and ended its side; we
        // could still read all of it
        let mut back = Vec::new();
        stream.read_to_end(&mut back).await.unwrap();
        assert_eq!(back, b"hello");
        let e = stream.write_all(b"more").await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::BrokenPipe);
    }

    #[tokio::test]
    async fn a_full_connection_makes_another_and_freed_places_are_reused() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 2);
        let (mut a, mut b, mut c) = tokio::join!(
            tunnel(&pool, "echo.test:1"),
            tunnel(&pool, "echo.test:2"),
            tunnel(&pool, "echo.test:3"),
        );
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
        assert_eq!(pool.connections(), 2);
        for stream in [&mut a, &mut b, &mut c] {
            round_trip(stream, b"ping").await;
        }
        drop((a, b, c));
        // the places are free again: no new dial
        let (mut d, mut e) =
            tokio::join!(tunnel(&pool, "echo.test:4"), tunnel(&pool, "echo.test:5"));
        round_trip(&mut d, b"again").await;
        round_trip(&mut e, b"again").await;
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_refusal_comes_back_and_frees_its_place() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 1);
        let response = pool
            .open(connect("deny.test:7"), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PROXY_AUTHENTICATION_REQUIRED);
        drop(response);
        let mut stream = tunnel(&pool, "echo.test:7").await;
        round_trip(&mut stream, b"ok").await;
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn after_goaway_new_streams_go_elsewhere_and_the_old_one_finishes() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let mut first = tunnel(&pool, "echo.test:1").await;
        let goaway = dial.goaway.lock().unwrap()[0].clone();
        goaway.notify_one();
        eventually(|| pool.connections() == 0).await;
        let mut second = tunnel(&pool, "echo.test:2").await;
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
        // the stream that was open before the GOAWAY carries on to its end
        round_trip(&mut first, b"still here").await;
        first.shutdown().await.unwrap();
        let mut rest = Vec::new();
        first.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
        drop(first);
        eventually(|| dial.ended.load(Ordering::SeqCst) == 1).await;
        round_trip(&mut second, b"fine").await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_idle_for_a_minute_is_closed() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        drop(tunnel(&pool, "echo.test:1").await);
        tokio::time::advance(IDLE_TIMEOUT - Duration::from_secs(1)).await;
        drop(tunnel(&pool, "echo.test:2").await);
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1, "still fresh");
        tokio::time::advance(IDLE_TIMEOUT).await;
        assert_eq!(pool.connections(), 0);
        // let go of, the connection closes: the server sees it end
        eventually(|| dial.ended.load(Ordering::SeqCst) == 1).await;
        drop(tunnel(&pool, "echo.test:3").await);
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn extended_connect_follows_the_servers_settings() {
        let udp = || {
            let mut request = Request::builder()
                .method(Method::CONNECT)
                .uri("https://proxy.test:443/.well-known/masque/udp/192.0.2.6/443/")
                .version(Version::HTTP_2)
                .header("capsule-protocol", "?1")
                .body(())
                .unwrap();
            request
                .extensions_mut()
                .insert(Protocol::from_static("connect-udp"));
            request
        };
        let dial = Arc::new(TestDial {
            server: Server {
                extended: true,
                ..Server::default()
            },
            ..TestDial::default()
        });
        let response = pool(&dial, 3)
            .open(udp(), &ConnectOpts::default())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body();
        round_trip(&mut stream, b"capsules").await;

        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let Err(e) = pool.open(udp(), &ConnectOpts::default()).await else {
            panic!("extended CONNECT without the server's consent");
        };
        assert_eq!(
            e.to_string(),
            "test: the server does not support extended CONNECT"
        );
        // a plain CONNECT on the same connection is fine
        drop(tunnel(&pool, "echo.test:7").await);
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_reset_from_the_server_is_a_read_error() {
        let dial = Arc::new(TestDial::default());
        let pool = pool(&dial, 3);
        let mut stream = tunnel(&pool, "reset.test:7").await;
        stream.write_all(b"x").await.unwrap();
        let mut buf = [0u8; 8];
        let e = stream.read(&mut buf).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(
            e.to_string(),
            "test: the server reset the stream (CONNECT_ERROR)"
        );
    }

    #[tokio::test]
    async fn a_failed_dial_is_shared_with_the_requests_that_waited_for_it() {
        let dial = Arc::new(TestDial {
            fail: true,
            ..TestDial::default()
        });
        let pool = pool(&dial, 3);
        let opts = ConnectOpts::default();
        let (a, b, c) = tokio::join!(
            pool.open(connect("echo.test:1"), &opts),
            pool.open(connect("echo.test:2"), &opts),
            pool.open(connect("echo.test:3"), &opts),
        );
        for result in [a, b, c] {
            let Err(e) = result else {
                panic!("a failed dial gave a stream");
            };
            assert_eq!(e.to_string(), "test: connection refused");
        }
        assert_eq!(dial.dials.load(Ordering::SeqCst), 1);
        // a later request tries again
        assert!(pool.open(connect("echo.test:4"), &opts).await.is_err());
        assert_eq!(dial.dials.load(Ordering::SeqCst), 2);
    }
}
```

新建 `crates/rurge-proto/src/h2pool/stream.rs`：

```rust
//! One HTTP/2 stream as a byte stream (phase 2 M6 design 5.2): the tunnel
//! of a CONNECT, or (in the test fakes) the server's side of one.
//!
//! - Writes take the stream's send capacity first: at most what the peer's
//!   flow-control windows allow is handed to `h2`, and a write without
//!   capacity waits for it. `h2` itself would buffer without bound.
//! - Reads hand back the peer's capacity for exactly what they consume, so
//!   at most a window's worth of the peer's data waits unread.
//! - Shutdown sends END_STREAM: the half-close the peer sees as a FIN
//!   (RFC 9113 8.5); reading goes on.
//! - A stream dropped before both directions ended is reset by `h2`
//!   (RST_STREAM CANCEL).

use super::{Lease, describe};
use bytes::Bytes;
use h2::{RecvStream, SendStream};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The most capacity one write asks for.
const MAX_RESERVE: usize = 64 * 1024;

/// No `Debug`: nothing in it is worth printing.
pub(crate) struct H2Stream {
    /// The protocol the error texts start with (`h2-connect`).
    label: &'static str,
    send: SendStream<Bytes>,
    recv: RecvStream,
    /// The rest of the latest DATA frame.
    unread: Bytes,
    read_end: bool,
    write_end: bool,
    /// The pooled connection's count of open streams, while the stream lives.
    _lease: Option<Lease>,
}

impl H2Stream {
    /// Also the server's side of a stream (the test fakes).
    pub(crate) fn new(label: &'static str, send: SendStream<Bytes>, recv: RecvStream) -> H2Stream {
        H2Stream {
            label,
            send,
            recv,
            unread: Bytes::new(),
            read_end: false,
            write_end: false,
            _lease: None,
        }
    }

    pub(super) fn leased(mut self, lease: Lease) -> H2Stream {
        self._lease = Some(lease);
        self
    }

    /// Why sending stopped: the peer's reset, when it sent one.
    fn send_closed(&mut self, cx: &mut Context<'_>) -> io::Error {
        match self.send.poll_reset(cx) {
            Poll::Ready(Ok(reason)) => io::Error::new(
                io::ErrorKind::ConnectionReset,
                format!("{}: the server reset the stream ({reason:?})", self.label),
            ),
            Poll::Ready(Err(e)) => io_error(self.label, &e),
            Poll::Pending => io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("{}: the stream is closed", self.label),
            ),
        }
    }
}

/// An `h2` failure as an I/O error that says what happened, never the
/// GOAWAY's debug data.
pub(super) fn io_error(label: &str, e: &h2::Error) -> io::Error {
    let kind = if e.is_reset() {
        io::ErrorKind::ConnectionReset
    } else if e.is_go_away() {
        io::ErrorKind::ConnectionAborted
    } else {
        e.get_io().map_or(io::ErrorKind::Other, io::Error::kind)
    };
    io::Error::new(kind, format!("{label}: {}", describe(e)))
}

impl AsyncRead for H2Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        // an empty DATA frame is not the end: only END_STREAM is
        while this.unread.is_empty() {
            if this.read_end {
                return Poll::Ready(Ok(()));
            }
            match ready!(this.recv.poll_data(cx)) {
                None => this.read_end = true,
                Some(Err(e)) => return Poll::Ready(Err(io_error(this.label, &e))),
                Some(Ok(data)) => this.unread = data,
            }
        }
        let n = this.unread.len().min(buf.remaining());
        buf.put_slice(&this.unread.split_to(n));
        // fails only on a stream that is gone, which the next read reports
        let _ = this.recv.flow_control().release_capacity(n);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for H2Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.write_end {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("{}: write after shutdown", this.label),
            )));
        }
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            let capacity = this.send.capacity();
            if capacity > 0 {
                let n = capacity.min(buf.len());
                this.send
                    .send_data(Bytes::copy_from_slice(&buf[..n]), false)
                    .map_err(|e| io_error(this.label, &e))?;
                // what is left goes back to the connection: a stream that
                // stops writing holds none of the window its siblings share
                this.send.reserve_capacity(0);
                return Poll::Ready(Ok(n));
            }
            // the total wanted, not an increment; `poll_capacity` reports
            // only a change, so the capacity is read again above
            this.send.reserve_capacity(buf.len().min(MAX_RESERVE));
            match ready!(this.send.poll_capacity(cx)) {
                Some(Ok(_)) => {}
                Some(Err(e)) => return Poll::Ready(Err(io_error(this.label, &e))),
                None => return Poll::Ready(Err(this.send_closed(cx))),
            }
        }
    }

    /// The connection's task writes what `h2` holds.
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if !this.write_end {
            this.write_end = true;
            this.send
                .send_data(Bytes::new(), true)
                .map_err(|e| io_error(this.label, &e))?;
        }
        Poll::Ready(Ok(()))
    }
}
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-proto --lib h2pool`
Expected: 通过（9 条：流的往返与大数据（小窗口下流控生效）、半关闭、`max_streams=2` 时 3 条并发流用 2 条连接、流释放后复用连接、GOAWAY 后新请求去新连接而已有的流走完、空闲连接被回收、扩展 CONNECT 的标志跟随服务端的 SETTINGS、服务端的 RST 是读错误）。

要点：
- 连接的任务归池里的连接所有，连接被丢弃时任务一并结束；池放掉连接时用 `h2` 自己的 GOAWAY 体面关闭，最多 5 秒。
- 一个请求在挑中连接之后、发出之前连接死掉，不重试（错误交给调用方）。

- [ ] **Step 3: 门禁与提交**

跑门禁。

```bash
git add Cargo.toml Cargo.lock crates/rurge-proto
git commit -m "feat(proto): HTTP/2 会话池——自己数流、单飞拨号、等 SETTINGS、GOAWAY 后不再分配、空闲关闭；H2Stream"
```

### Task 3: `h2-connect` 的 TCP 与 `FakeH2Proxy`

`H2ConnectOutbound` 的 TCP（P7）：经 `Stack`（connect → shadow-tls → TLS，ALPN `h2`）交给 `H2Pool`。为报出协商到的 ALPN，TLS 层与 `Stack` 各加一个返回协商结果的入口；`http::render` / `merge` 改为 `pub(crate)` 供复用。`FakeH2Proxy` 独立实现（`h2::server`，共用测试 TLS 夹具，可设 ALPN）。

**Files:**
- Create: `crates/rurge-proto/src/h2connect.rs`（出站与用例）、`crates/rurge-proto/src/testing/h2proxy.rs`
- Modify: `crates/rurge-proto/src/h2pool/mod.rs`（`StackDial`）、`src/http.rs`、`src/lib.rs`、`src/transport/tls.rs`（`wrap_negotiated`）、`src/transport/stack.rs`（`open_negotiated`）、`src/testing/mod.rs`、`src/testing/tls.rs`（`acceptor_with_alpn`）

**Interfaces:**
- Consumes: Task 1 的 `H2ConnectSpec`；Task 2 的 `H2Pool` / `H2Stream`；既有的 `Stack`、`http::render` / `merge`。
- Produces: `pub struct H2ConnectOutbound`：`new(name, server: Target, spec: &H2ConnectSpec, shadow_tls: Option<&ShadowTlsOpts>, keystore, roots, connector) -> Result<_, BuildError>`（参数顺序同 trojan）；`h2pool::StackDial`；`connect_request`、`basic_authorization`（Task 4 复用）；测试设施 `FakeH2Proxy` 与 `H2ProxyScript`

- [ ] **Step 1: 先写假服务端与出站（连同用例）**

`crates/rurge-proto/src/testing/tls.rs`——把

```rust
    pub fn acceptor(&self, require_client_cert: bool) -> TlsAcceptor {
```

换成

```rust
    pub fn acceptor(&self, require_client_cert: bool) -> TlsAcceptor {
        self.acceptor_with_alpn(require_client_cert, &[b"h2", b"http/1.1"])
    }

    /// `acceptor`, offering these ALPN protocols (in the server's order of
    /// preference) instead of `h2` and `http/1.1`.
    pub fn acceptor_with_alpn(&self, require_client_cert: bool, alpn: &[&[u8]]) -> TlsAcceptor {
```

`crates/rurge-proto/src/testing/tls.rs`——把

```rust
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
```

换成

```rust
        config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
```

新建 `crates/rurge-proto/src/testing/h2proxy.rs`：

```rust
//! A scriptable HTTP/2 CONNECT proxy (phase 2 M6 design 5.5): TLS with ALPN
//! `h2`, `h2::server`, Basic authentication, and a relay to the target for
//! every CONNECT stream. It never resolves a name.

use super::{AbortOnDrop, TlsFixture};
use crate::h2pool::H2Stream;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use h2::RecvStream;
use h2::server::SendResponse;
use http::{Method, Request, Response, StatusCode};
use rurge_net::connector::BoxedStream;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Debug, Default)]
pub struct H2ProxyScript {
    /// Require Basic proxy credentials: any of these (user, password).
    pub users: Vec<(String, String)>,
    /// Answer every request with this status instead of serving it.
    pub refuse: Option<u16>,
    /// Wait this long before answering a request.
    pub delay: Duration,
    /// SETTINGS_MAX_CONCURRENT_STREAMS.
    pub max_concurrent_streams: Option<u32>,
    /// Send GOAWAY once a connection has accepted this many streams; those
    /// run to their end.
    pub goaway_after: Option<usize>,
    /// Advertise extended CONNECT (SETTINGS_ENABLE_CONNECT_PROTOCOL = 1).
    pub extended_connect: bool,
    /// Take no part in ALPN, like a TLS server that knows nothing of HTTP/2
    /// (a rustls server with protocols of its own would rather fail the
    /// handshake when none is the client's).
    pub no_alpn: bool,
    /// Complete the TLS handshake only with a client certificate signed by
    /// the fixture's CA.
    pub require_client_cert: bool,
    /// Tunnel here whatever the client asked for (needed for a domain target).
    pub connect_to: Option<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedH2Request {
    /// The connection that carried the stream, 0 for the first accepted.
    pub connection: usize,
    pub method: String,
    /// `:authority`.
    pub authority: String,
    /// `:protocol` (extended CONNECT).
    pub protocol: Option<String>,
    /// `:path`; empty for a plain CONNECT.
    pub path: String,
    /// In arrival order; HTTP/2 names are lowercase.
    pub headers: Vec<(String, String)>,
}

impl RecordedH2Request {
    /// The first header called `name` (lowercase).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

pub struct FakeH2Proxy {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedH2Request>>>,
    connections: Arc<AtomicUsize>,
    _task: AbortOnDrop,
}

struct Shared {
    script: H2ProxyScript,
    requests: Arc<Mutex<Vec<RecordedH2Request>>>,
}

fn record(request: &Request<RecvStream>, connection: usize) -> RecordedH2Request {
    let uri = request.uri();
    RecordedH2Request {
        connection,
        method: request.method().to_string(),
        authority: uri.authority().map(|a| a.to_string()).unwrap_or_default(),
        protocol: request
            .extensions()
            .get::<h2::ext::Protocol>()
            .map(|p| p.as_str().to_string()),
        path: uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default(),
        headers: request
            .headers()
            .iter()
            .map(|(n, v)| {
                (
                    n.as_str().to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
    }
}

fn authorized(users: &[(String, String)], request: &RecordedH2Request) -> bool {
    users.is_empty()
        || users.iter().any(|(user, password)| {
            let expected = format!("Basic {}", STANDARD.encode(format!("{user}:{password}")));
            request.header("proxy-authorization") == Some(expected.as_str())
        })
}

/// A status and no tunnel.
fn refuse(mut respond: SendResponse<Bytes>, status: StatusCode) {
    let mut response = Response::builder().status(status);
    if status == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        response = response.header("proxy-authenticate", "Basic realm=\"fake\"");
    }
    let _ = respond.send_response(response.body(()).expect("a response"), true);
}

async fn answer(
    request: Request<RecvStream>,
    respond: SendResponse<Bytes>,
    connection: usize,
    shared: Arc<Shared>,
) {
    let seen = record(&request, connection);
    shared.requests.lock().expect("requests").push(seen.clone());
    tokio::time::sleep(shared.script.delay).await;
    if !authorized(&shared.script.users, &seen) {
        return refuse(respond, StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    }
    if let Some(code) = shared.script.refuse {
        return refuse(respond, StatusCode::from_u16(code).expect("a status"));
    }
    // no extended CONNECT protocol is served
    if request.method() != Method::CONNECT || seen.protocol.is_some() {
        return refuse(respond, StatusCode::NOT_IMPLEMENTED);
    }
    let target = match shared.script.connect_to {
        Some(addr) => Some(addr),
        // never resolves: a name without `connect_to` is a dead end
        None => seen.authority.parse::<SocketAddr>().ok(),
    };
    let upstream = match target {
        Some(addr) => TcpStream::connect(addr).await.ok(),
        None => None,
    };
    let Some(mut upstream) = upstream else {
        return refuse(respond, StatusCode::BAD_GATEWAY);
    };
    tunnel(request, respond, &mut upstream).await;
}

/// 200, then the stream's bytes to and from `upstream`, half-closes
/// included.
async fn tunnel(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    upstream: &mut TcpStream,
) {
    let Ok(send) = respond.send_response(Response::new(()), false) else {
        return;
    };
    let mut stream = H2Stream::new("fake-h2", send, request.into_body());
    let _ = tokio::io::copy_bidirectional(&mut stream, upstream).await;
}

async fn serve(stream: BoxedStream, connection: usize, shared: Arc<Shared>) {
    let mut builder = h2::server::Builder::new();
    if let Some(n) = shared.script.max_concurrent_streams {
        builder.max_concurrent_streams(n);
    }
    if shared.script.extended_connect {
        builder.enable_connect_protocol();
    }
    let Ok(mut conn) = builder.handshake::<_, Bytes>(stream).await else {
        return;
    };
    let mut accepted = 0;
    // `accept` also drives the connection: polled until it ends
    while let Some(Ok((request, respond))) = conn.accept().await {
        accepted += 1;
        tokio::spawn(answer(request, respond, connection, shared.clone()));
        if shared.script.goaway_after == Some(accepted) {
            conn.graceful_shutdown();
        }
    }
}

impl FakeH2Proxy {
    pub async fn spawn(script: H2ProxyScript, fixture: Arc<TlsFixture>) -> FakeH2Proxy {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let alpn: &[&[u8]] = if script.no_alpn { &[] } else { &[b"h2"] };
        let acceptor = fixture.acceptor_with_alpn(script.require_client_cert, alpn);
        let requests: Arc<Mutex<Vec<RecordedH2Request>>> = Arc::default();
        let connections = Arc::new(AtomicUsize::new(0));
        let shared = Arc::new(Shared {
            script,
            requests: requests.clone(),
        });
        let count = connections.clone();
        let task = tokio::spawn(async move {
            while let Ok((tcp, _)) = listener.accept().await {
                let connection = count.fetch_add(1, Ordering::SeqCst);
                let (shared, fixture, acceptor) =
                    (shared.clone(), fixture.clone(), acceptor.clone());
                tokio::spawn(async move {
                    let Ok(stream) = fixture.accept(&acceptor, tcp).await else {
                        return;
                    };
                    serve(stream, connection, shared).await;
                });
            }
        });
        FakeH2Proxy {
            addr,
            requests,
            connections,
            _task: AbortOnDrop(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Every request, in arrival order.
    pub fn requests(&self) -> Vec<RecordedH2Request> {
        self.requests.lock().expect("requests").clone()
    }

    /// TCP connections accepted so far (before TLS).
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
mod anytls;
```

换成

```rust
mod anytls;
mod h2proxy;
```

`crates/rurge-proto/src/testing/mod.rs`——把

```rust
pub use anytls::{AnyTlsScript, FakeAnyTls, RecordedStream};
```

换成

```rust
pub use anytls::{AnyTlsScript, FakeAnyTls, RecordedStream};
pub use h2proxy::{FakeH2Proxy, H2ProxyScript, RecordedH2Request};
```

出站与它的用例（引用的 `StackDial` 与 `http::render` / `merge` 的可见性在 Step 3 改）：

新建 `crates/rurge-proto/src/h2connect.rs`：

```rust
//! `h2-connect` outbound (manual: Policies › HTTP and HTTP/2; phase 2 M6
//! design 5.3): every connection is a CONNECT stream on a pooled TLS +
//! HTTP/2 connection (`h2pool`), `max-streams` of them per connection.
//!
//! - The request is `:method CONNECT` and `:authority host:port` (an IPv6
//!   literal in brackets, an IDN as its A-labels), then
//!   `proxy-authorization: Basic …` when the policy has credentials, then
//!   the configured `headers`, rendered anew for every request. A
//!   configured header replaces one of ours with the same name, as on
//!   `http` / `https`: `headers=Proxy-Authorization:…` wins over the
//!   credentials. No `user-agent` unless `headers` adds one.
//! - A 2xx answer turns the stream into the tunnel; any other status is
//!   the proxy's refusal.

use crate::build::{shadow_tls_client, tls_client};
use crate::h2pool::{H2Pool, StackDial};
use crate::http::{merge, render, wire_host};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use http::header::{HeaderName, HeaderValue};
use http::{Method, Request, StatusCode, Version};
use rurge_config::KeystoreItem;
use rurge_config::spec::{H2ConnectSpec, HeaderTemplate, ShadowTlsOpts};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;

const LABEL: &str = "h2-connect";

/// No `Debug`: it holds the credentials.
pub struct H2ConnectOutbound {
    name: String,
    pool: H2Pool,
    /// The `proxy-authorization` value, ready to send.
    authorization: Option<String>,
    headers: Vec<HeaderTemplate>,
}

/// `Basic base64(user:password)` (RFC 9110 11.7.2, RFC 7617).
pub(crate) fn basic_authorization(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// The CONNECT request for `target`: `ours` (lowercase names), replaced
/// field by field by the rendered `templates`. Fails, before anything is
/// dialed, for a target whose name cannot be sent (the alphabet of
/// `http::wire_host`).
pub(crate) fn connect_request(
    label: &str,
    target: &Target,
    ours: Vec<(String, String)>,
    templates: &[HeaderTemplate],
) -> Result<Request<()>, OutboundError> {
    let Some(host) = wire_host(target) else {
        return Err(OutboundError::Proxy(format!(
            "{label}: the host name cannot be sent to the server"
        )));
    };
    let mut fields = ours;
    merge(&mut fields, render(templates));
    let mut request = Request::builder()
        .method(Method::CONNECT)
        .uri(format!("{host}:{}", target.port))
        .version(Version::HTTP_2);
    for (name, value) in fields {
        // `HeaderName` lowercases, as HTTP/2 wants; valid templates always
        // convert (`HeaderTemplate::is_valid`, checked at build time)
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) else {
            return Err(OutboundError::Proxy(format!(
                "{label}: a custom header is not valid"
            )));
        };
        request = request.header(name, value);
    }
    request
        .body(())
        .map_err(|_| OutboundError::Proxy(format!("{label}: the CONNECT request is not valid")))
}

impl H2ConnectOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &H2ConnectSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<H2ConnectOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        // the spec pins `alpn` to `h2`; `h2` is also what an empty one offers
        let tls = tls_client(Some(&spec.tls), &server.host, &["h2"], keystore, roots)?;
        let authorization = spec.username.as_ref().map(|user| {
            let password = spec.password.as_ref().map_or("", |p| p.expose().as_str());
            basic_authorization(user.expose(), password)
        });
        let dial = StackDial {
            label: LABEL,
            stack: Stack::new(connector, server, shadow_tls, tls, None),
        };
        Ok(H2ConnectOutbound {
            name: name.to_string(),
            pool: H2Pool::new(LABEL, spec.max_streams.max(1), Arc::new(dial)),
            authorization,
            headers: spec.headers.clone(),
        })
    }

    async fn tunnel(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let ours = self
            .authorization
            .iter()
            .map(|value| ("proxy-authorization".to_string(), value.clone()))
            .collect();
        // never dial for a target whose name cannot be sent
        let request = connect_request(LABEL, target, ours, &self.headers)?;
        let response = self.pool.open(request, opts).await?;
        match response.status() {
            status if status.is_success() => Ok(Box::new(response.into_body()) as BoxedStream),
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => Err(OutboundError::Proxy(format!(
                "{LABEL}: proxy authentication required"
            ))),
            status => Err(OutboundError::Proxy(format!(
                "{LABEL}: the proxy answered {}",
                status.as_u16()
            ))),
        }
    }
}

impl Outbound for H2ConnectOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), TLS, the
            // HTTP/2 handshake and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeH2Proxy, H2ProxyScript, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::h2::read_h2_connect;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::{HeaderPart, ParamReader};
    use rurge_config::{HostName, KeystoreType, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::Instant;

    /// The outbound for `definition` (an `h2-connect, host, port, ...` line).
    fn outbound_with(
        definition: &str,
        fixture: &TlsFixture,
        keystore: &[KeystoreItem],
    ) -> H2ConnectOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("H", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_h2_connect(&mut r, keystore);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        H2ConnectOutbound::new(
            "H",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            keystore,
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `h2-connect, 127.0.0.1, <the fake's port><extra>`.
    fn outbound(fake: &FakeH2Proxy, extra: &str, fixture: &TlsFixture) -> H2ConnectOutbound {
        outbound_with(
            &format!("h2-connect, 127.0.0.1, {}{extra}", fake.addr().port()),
            fixture,
            &[],
        )
    }

    async fn fake(script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeH2Proxy::spawn(script, fixture.clone()).await;
        (fixture, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    async fn refusal(out: &H2ConnectOutbound, to: SocketAddr) -> String {
        let Err(e) = out.connect_tcp(&target(to), &ConnectOpts::default()).await else {
            panic!("the proxy let us through");
        };
        assert!(matches!(e, OutboundError::Proxy(_)), "{e}");
        e.to_string()
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_tunnel_echoes_over_tls_with_alpn_h2() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.name(), "H");
        assert!(out.http_forward().is_none());
        assert!(matches!(out.udp(), crate::UdpSupport::Unsupported));
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"through the stream").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "CONNECT");
        assert_eq!(seen[0].authority, echo.to_string());
        assert_eq!(
            (seen[0].protocol.as_deref(), seen[0].path.as_str()),
            (None, "")
        );
        assert!(seen[0].headers.is_empty(), "{:?}", seen[0].headers);
        let tls = fixture.seen_at_least(1).await;
        assert_eq!(tls[0].alpn.as_deref(), Some("h2"));
        assert_eq!(tls[0].sni, None, "an IP literal: no SNI");
    }

    #[tokio::test]
    async fn a_large_payload_crosses_both_ways() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        let stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        // several of both sides' stream windows (1 MiB each)
        let data: Vec<u8> = (0..(5 << 20) + 777).map(|i| (i % 253) as u8).collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    #[tokio::test]
    async fn shutdown_reaches_the_target_as_a_half_close() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"last words").await.unwrap();
        stream.shutdown().await.unwrap();
        // the target saw the FIN, echoed what it had and closed: we read all
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut back))
            .await
            .expect("the target closed")
            .unwrap();
        assert_eq!(back, b"last words");
    }

    #[tokio::test]
    async fn credentials_go_as_basic_and_a_refusal_says_so() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            users: vec![("u".into(), "p:w".into())],
            ..H2ProxyScript::default()
        })
        .await;
        let mut stream = outbound(&fake, ", u, p:w", &fixture)
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"let in").await;
        // base64("u:p:w")
        assert_eq!(
            fake.requests()[0].header("proxy-authorization"),
            Some("Basic dTpwOnc=")
        );
        for extra in ["", ", username=u, password=wrong"] {
            let out = outbound(&fake, extra, &fixture);
            assert_eq!(
                refusal(&out, echo).await,
                "h2-connect: proxy authentication required"
            );
        }
        assert!(fake.requests()[1].header("proxy-authorization").is_none());
    }

    #[tokio::test]
    async fn any_other_status_is_quoted_by_its_number() {
        let echo = echo_server().await;
        for code in [403, 502, 503] {
            let (fixture, fake) = fake(H2ProxyScript {
                refuse: Some(code),
                ..H2ProxyScript::default()
            })
            .await;
            let out = outbound(&fake, "", &fixture);
            assert_eq!(
                refusal(&out, echo).await,
                format!("h2-connect: the proxy answered {code}")
            );
        }
    }

    #[tokio::test]
    async fn headers_are_rendered_per_request_and_replace_ours() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(
            &fake,
            ", user, pass, headers=X-Pad:p<random-string(12)>q<random-string(2-6)>;Proxy-Authorization:Bearer abc",
            &fixture,
        );
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            round_trip(&mut stream, b"x").await;
        }
        let seen = fake.requests();
        assert_eq!(fake.connections(), 1, "both on one connection");
        for request in &seen {
            let auth: Vec<&str> = request
                .headers
                .iter()
                .filter(|(n, _)| n == "proxy-authorization")
                .map(|(_, v)| v.as_str())
                .collect();
            assert_eq!(auth, ["Bearer abc"], "the configured header wins");
            assert!(request.header("user-agent").is_none());
            let pad = request.header("x-pad").expect("lowercased by HTTP/2");
            let inner = pad.strip_prefix('p').unwrap();
            let (first, second) = inner.split_at(12);
            let second = second.strip_prefix('q').unwrap();
            assert!((2..=6).contains(&second.len()), "{pad}");
            assert!(
                first
                    .chars()
                    .chain(second.chars())
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{pad}"
            );
        }
        assert_ne!(
            seen[0].header("x-pad"),
            seen[1].header("x-pad"),
            "drawn anew for every request, not every connection"
        );
    }

    #[tokio::test]
    async fn the_authority_brackets_ipv6_and_carries_a_labels() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            connect_to: Some(echo),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        for (host, authority) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = out
                .connect_tcp(&Target::new(host, 443), &ConnectOpts::default())
                .await
                .unwrap();
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.requests().last().unwrap().authority, authority);
        }
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        for name in ["x@blocked.test", "a b.test", "a.test\r\nx: 1", ""] {
            let Err(e) = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name.to_string()), 443),
                    &ConnectOpts::default(),
                )
                .await
            else {
                panic!("{name:?} was sent");
            };
            assert_eq!(
                e.to_string(),
                "h2-connect: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn max_streams_bounds_each_connection() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", max-streams=2", &fixture);
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b, c) = tokio::join!(
            out.connect_tcp(&to, &opts),
            out.connect_tcp(&to, &opts),
            out.connect_tcp(&to, &opts),
        );
        let mut streams = [a.unwrap(), b.unwrap(), c.unwrap()];
        for stream in &mut streams {
            round_trip(stream, b"three at once").await;
        }
        assert_eq!(fake.connections(), 2);
        let mut carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        carried.sort_unstable();
        assert_eq!(carried, [0, 0, 1]);
    }

    #[tokio::test]
    async fn the_servers_own_stream_limit_is_kept_too() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            max_concurrent_streams: Some(1),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        // one after the other: the first answer brought the server's SETTINGS
        let mut first = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        let mut second = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut first, b"one").await;
        round_trip(&mut second, b"two").await;
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn after_goaway_the_next_tunnel_takes_a_new_connection() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            goaway_after: Some(1),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        let mut first = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        eventually(|| out.pool.connections() == 0).await;
        let mut second = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        // the stream from before the GOAWAY runs on
        round_trip(&mut first, b"old").await;
        round_trip(&mut second, b"new").await;
        assert_eq!(fake.connections(), 2);
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
    }

    #[tokio::test]
    async fn a_server_without_h2_is_refused_before_http2() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            no_alpn: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, echo).await,
            "h2-connect: the server does not speak HTTP/2"
        );
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn a_client_certificate_comes_from_the_keystore() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            require_client_cert: true,
            ..H2ProxyScript::default()
        })
        .await;
        let keystore = [KeystoreItem {
            name: "mtls".into(),
            kind: KeystoreType::P12,
            base64: fixture.client_p12_base64("rurge"),
            password: Some("pw".into()),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        }];
        let out = outbound_with(
            &format!(
                "h2-connect, 127.0.0.1, {}, client-cert=mtls",
                fake.addr().port()
            ),
            &fixture,
            &keystore,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"mutual").await;
        assert!(fixture.seen_at_least(1).await[0].client_cert);
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            delay: Duration::from_secs(30),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        let started = Instant::now();
        let Err(e) = out
            .connect_tcp(
                &target(echo),
                &ConnectOpts {
                    timeout: Duration::from_millis(300),
                },
            )
            .await
        else {
            panic!("the proxy never answered");
        };
        assert!(matches!(e, OutboundError::Timeout), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(fake.requests().len(), 1, "the CONNECT was sent");
    }

    #[test]
    fn invalid_header_templates_are_refused_at_build_time() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let mut spec = H2ConnectSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: 3,
            udp_relay: false,
        };
        spec.headers.push(HeaderTemplate {
            name: "X-Evil".into(),
            value: vec![HeaderPart::Literal("a\r\nb: 1".into())],
        });
        let err = H2ConnectOutbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            None,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "custom header #1 is not valid");
    }

    #[test]
    fn basic_authorization_is_base64_of_user_colon_password() {
        // RFC 7617 2
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }
}
```

`crates/rurge-proto/src/lib.rs`——把

```rust
// the `h2-connect` outbound (M6c task 3) is its first user
#[allow(dead_code)]
pub(crate) mod h2pool;
```

换成

```rust
pub mod h2connect;
mod h2pool;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib h2connect`
Expected: FAIL——`StackDial` 与几处可见性由 Step 3 引入，编译不过：

```text
error[E0432]: unresolved import `crate::h2pool::StackDial`
error[E0603]: function `merge` is private
error[E0603]: function `render` is private
error[E0624]: method `connections` is private
Some errors have detailed explanations: E0432, E0603, E0624.
For more information about an error, try `rustc --explain E0432`.
error: could not compile `rurge-proto` (lib test) due to 4 previous errors
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/transport/tls.rs`——把

```rust
        let tls = self.connector.connect(self.name.clone(), stream).await?;
        Ok(Box::new(tls))
```

换成

```rust
        self.wrap_negotiated(stream)
            .await
            .map(|(stream, _alpn)| stream)
    }

    /// `wrap`, and the protocol ALPN settled on (`None`: the server chose none).
    pub async fn wrap_negotiated(
        &self,
        stream: BoxedStream,
    ) -> io::Result<(BoxedStream, Option<Vec<u8>>)> {
        let tls = self.connector.connect(self.name.clone(), stream).await?;
        let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        Ok((Box::new(tls), alpn))
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
    pub async fn open(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
```

换成

```rust
    pub async fn open(&self, opts: &ConnectOpts) -> Result<BoxedStream, OutboundError> {
        self.open_negotiated(opts)
            .await
            .map(|(stream, _alpn)| stream)
    }

    /// `open`, and the protocol the TLS layer's ALPN settled on (`None`
    /// without a TLS layer, or when the server chose none).
    pub async fn open_negotiated(
        &self,
        opts: &ConnectOpts,
    ) -> Result<(BoxedStream, Option<Vec<u8>>), OutboundError> {
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
        if let Some(tls) = &self.tls {
            stream = tls.wrap(stream).await.map_err(OutboundError::tls)?;
```

换成

```rust
        let mut alpn = None;
        if let Some(tls) = &self.tls {
            let wrapped = tls
                .wrap_negotiated(stream)
                .await
                .map_err(OutboundError::tls)?;
            (stream, alpn) = wrapped;
```

`crates/rurge-proto/src/transport/stack.rs`——把

```rust
        Ok(stream)
```

换成

```rust
        Ok((stream, alpn))
```

`crates/rurge-proto/src/http.rs`——把

```rust
fn render(templates: &[HeaderTemplate]) -> Vec<(String, String)> {
```

换成

```rust
/// The headers of one request: every `<random-string(..)>` drawn anew.
pub(crate) fn render(templates: &[HeaderTemplate]) -> Vec<(String, String)> {
```

`crates/rurge-proto/src/http.rs`——把

```rust
fn merge(base: &mut Vec<(String, String)>, custom: Vec<(String, String)>) {
```

换成

```rust
pub(crate) fn merge(base: &mut Vec<(String, String)>, custom: Vec<(String, String)>) {
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
//! (TCP, Shadow TLS, TLS with ALPN `h2`) and checks that `h2` was
//! negotiated.
```

换成

```rust
//! (`StackDial`: TCP, Shadow TLS, TLS with ALPN `h2`) and checks that `h2`
//! was negotiated.
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
use crate::task::AbortOnDrop;
```

换成

```rust
use crate::task::AbortOnDrop;
use crate::transport::Stack;
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
```

换成

```rust
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>>;
}

/// The transport of the HTTP/2 policies: their `Stack` (TCP, Shadow TLS,
/// TLS offering ALPN `h2`), refused unless the server chose `h2`.
pub(crate) struct StackDial {
    /// The protocol the error texts start with (`h2-connect`).
    pub(crate) label: &'static str,
    pub(crate) stack: Stack,
}

impl Dial for StackDial {
    fn dial<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            let (stream, alpn) = self.stack.open_negotiated(opts).await?;
            if alpn.as_deref() != Some(b"h2".as_slice()) {
                return Err(OutboundError::Proxy(format!(
                    "{}: the server does not speak HTTP/2",
                    self.label
                )));
            }
            Ok(stream)
        })
    }
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
    #[cfg(test)]
    fn connections(&self) -> usize {
```

换成

```rust
    /// The connections that take new streams.
    #[cfg(test)]
    pub(crate) fn connections(&self) -> usize {
```

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib -- h2connect h2pool` → 通过（新增 16 条：回显、大负载、半关闭、Basic 认证与 407、其它状态码、`headers` 与 `<random-string>` 每个请求不同、`max-streams` 下的连接数、GOAWAY 之后去新连接、服务端不参与 ALPN、IPv6 与 IDN 的 authority、客户端证书、时限）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto
git commit -m "feat(proto): h2-connect 的 TCP——HTTP/2 CONNECT、Basic 认证、headers、max-streams；FakeH2Proxy"
```

### Task 4: `trust-tunnel` 与 SETTINGS 之前只放一条流

`TrustTunnelOutbound`（P8），复用 Task 3 的 `StackDial` / `connect_request` / `basic_authorization` / `H2Pool`；连同池的修正：新连接在收到服务端 SETTINGS 之前只放一条流（P6），Task 3 里服务端上限的用例改回并发写法。`FakeH2Proxy` 加"要求 user-agent"选项（缺了回 400）。

**Files:**
- Create: `crates/rurge-proto/src/trust_tunnel.rs`（出站与用例）
- Modify: `crates/rurge-proto/src/h2pool/mod.rs`（与用例）、`src/h2connect.rs`（用例）、`src/lib.rs`、`src/testing/h2proxy.rs`

**Interfaces:**
- Consumes: Task 1 的 `TrustTunnelSpec`；Task 2 / 3 的池与请求构造。
- Produces: `pub struct TrustTunnelOutbound`（`new` 的参数顺序同 `H2ConnectOutbound::new`）；`pub const USER_AGENT: &str = "rurge"`

- [ ] **Step 1: 先写用例（连同假服务端的选项）**

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
    pub users: Vec<(String, String)>,
```

换成

```rust
    pub users: Vec<(String, String)>,
    /// Answer a request without a `user-agent` 400, as a server that
    /// follows the TrustTunnel document to the letter would.
    pub require_user_agent: bool,
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
        return refuse(respond, StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    }
    if let Some(code) = shared.script.refuse {
```

换成

```rust
        return refuse(respond, StatusCode::PROXY_AUTHENTICATION_REQUIRED);
    }
    if shared.script.require_user_agent && seen.header("user-agent").is_none() {
        return refuse(respond, StatusCode::BAD_REQUEST);
    }
    if let Some(code) = shared.script.refuse {
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
    };
    let Some(mut upstream) = upstream else {
```

换成

```rust
    };
    // an unreachable target is 502, as the TrustTunnel endpoint answers
    let Some(mut upstream) = upstream else {
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
        window: Option<u32>,
```

换成

```rust
        window: Option<u32>,
        /// SETTINGS_MAX_CONCURRENT_STREAMS.
        streams: Option<u32>,
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
            builder.initial_window_size(window);
```

换成

```rust
            builder.initial_window_size(window);
        }
        if let Some(streams) = server.streams {
            builder.max_concurrent_streams(streams);
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
    #[tokio::test]
    async fn a_refusal_comes_back_and_frees_its_place() {
```

换成

```rust
    #[tokio::test]
    async fn a_fresh_connection_carries_one_stream_until_the_servers_limit_is_known() {
        // the server takes one stream at a time, fewer than `max-streams`:
        // requests that come with the first dial must not queue behind it
        let dial = Arc::new(TestDial {
            server: Server {
                streams: Some(1),
                ..Server::default()
            },
            ..TestDial::default()
        });
        let pool = pool(&dial, 3);
        let (mut a, mut b, mut c) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                tunnel(&pool, "echo.test:1"),
                tunnel(&pool, "echo.test:2"),
                tunnel(&pool, "echo.test:3"),
            )
        })
        .await
        .expect("a stream waited behind the server's limit");
        for stream in [&mut a, &mut b, &mut c] {
            round_trip(stream, b"one each").await;
        }
        assert_eq!(dial.dials.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn a_refusal_comes_back_and_frees_its_place() {
```

`crates/rurge-proto/src/h2connect.rs`——把

```rust
        // one after the other: the first answer brought the server's SETTINGS
        let mut first = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        let mut second = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
```

换成

```rust
        // all at once: none may queue behind the server's limit, unknown
        // until its SETTINGS arrive
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(out.connect_tcp(&to, &opts), out.connect_tcp(&to, &opts))
        })
        .await
        .expect("a tunnel waited behind the server's limit");
        let (mut first, mut second) = (a.unwrap(), b.unwrap());
```

出站与它的用例：

新建 `crates/rurge-proto/src/trust_tunnel.rs`：

```rust
//! `trust-tunnel` outbound over HTTP/2 (manual: Policies › Trust Tunnel;
//! TrustTunnel protocol 5.1; phase 2 M6 design 5.4): every connection is a
//! CONNECT stream on a pooled TLS + HTTP/2 connection, as on `h2-connect`.
//!
//! - The request is `:method CONNECT`, `:authority host:port`,
//!   `proxy-authorization: Basic …` and `user-agent`, then the configured
//!   `headers`, rendered anew for every request; a configured header
//!   replaces one of ours with the same name.
//! - The document marks `user-agent` required (as `<platform> <app_name>`);
//!   the reference endpoint only records it. Unless `headers` sets one, the
//!   fixed `USER_AGENT` goes: the same for every platform and version.
//! - 200 turns the stream into the tunnel; 407 is a refused login; the
//!   endpoint answers 502 for a target it could not reach.
//! - TCP only: no UDP (`_udp2`), ICMP (`_icmp`) or health check (`_check`);
//!   a policy test is a URL test.

use crate::build::{shadow_tls_client, tls_client};
use crate::h2connect::{basic_authorization, connect_request};
use crate::h2pool::{H2Pool, StackDial};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError};
use http::StatusCode;
use rurge_config::KeystoreItem;
use rurge_config::spec::{HeaderTemplate, ShadowTlsOpts, TrustTunnelSpec};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;

const LABEL: &str = "trust-tunnel";

/// The `user-agent` when `headers` has none: names the client, nothing
/// about the platform or the version.
pub const USER_AGENT: &str = "rurge";

/// No `Debug`: it holds the credentials.
pub struct TrustTunnelOutbound {
    name: String,
    pool: H2Pool,
    /// The `proxy-authorization` value, ready to send.
    authorization: String,
    headers: Vec<HeaderTemplate>,
}

impl TrustTunnelOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &TrustTunnelSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<TrustTunnelOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        // the SNI is the server's name or `sni=`: the endpoint picks its
        // host by the exact SNI. The spec pins `alpn` to `h2`.
        let tls = tls_client(Some(&spec.tls), &server.host, &["h2"], keystore, roots)?;
        let dial = StackDial {
            label: LABEL,
            stack: Stack::new(connector, server, shadow_tls, tls, None),
        };
        Ok(TrustTunnelOutbound {
            name: name.to_string(),
            pool: H2Pool::new(LABEL, spec.max_streams.max(1), Arc::new(dial)),
            authorization: basic_authorization(spec.username.expose(), spec.password.expose()),
            headers: spec.headers.clone(),
        })
    }

    async fn tunnel(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        let ours = vec![
            (
                "proxy-authorization".to_string(),
                self.authorization.clone(),
            ),
            ("user-agent".to_string(), USER_AGENT.to_string()),
        ];
        // never dial for a target whose name cannot be sent
        let request = connect_request(LABEL, target, ours, &self.headers)?;
        let response = self.pool.open(request, opts).await?;
        match response.status() {
            status if status.is_success() => Ok(Box::new(response.into_body()) as BoxedStream),
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => Err(OutboundError::Proxy(format!(
                "{LABEL}: authentication failed"
            ))),
            status => Err(OutboundError::Proxy(format!(
                "{LABEL}: the server answered {}",
                status.as_u16()
            ))),
        }
    }
}

impl Outbound for TrustTunnelOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    // `udp()` stays `Unsupported`: the engine applies
    // `udp-policy-not-supported-behaviour`

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), TLS, the
            // HTTP/2 handshake and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UdpSupport;
    use crate::testing::{FakeH2Proxy, H2ProxyScript, TlsFixture, echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::ParamReader;
    use rurge_config::spec::h2::read_trust_tunnel;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::{HostName, Span};
    use rurge_net::connector::{DirectConnector, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// The outbound for `definition` (a `trust-tunnel, host, port, ...` line).
    fn outbound_with(definition: &str, fixture: &TlsFixture) -> TrustTunnelOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("T", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_trust_tunnel(&mut r, &[]).spec;
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        TrustTunnelOutbound::new(
            "T",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `trust-tunnel, 127.0.0.1, <the fake's port>, username=u,
    /// password=p<extra>`.
    fn outbound(fake: &FakeH2Proxy, extra: &str, fixture: &TlsFixture) -> TrustTunnelOutbound {
        outbound_with(
            &format!(
                "trust-tunnel, 127.0.0.1, {}, username=u, password=p{extra}",
                fake.addr().port()
            ),
            fixture,
        )
    }

    /// A fake that lets `u` / `p` in, with a certificate for `names`.
    async fn proxy(names: &[&str], script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        let fixture = TlsFixture::new(names);
        let script = H2ProxyScript {
            users: vec![("u".into(), "p".into())],
            ..script
        };
        let fake = FakeH2Proxy::spawn(script, fixture.clone()).await;
        (fixture, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    async fn refusal(out: &TrustTunnelOutbound, to: SocketAddr) -> String {
        let Err(e) = out.connect_tcp(&target(to), &ConnectOpts::default()).await else {
            panic!("the server let us through");
        };
        assert!(matches!(e, OutboundError::Proxy(_)), "{e}");
        e.to_string()
    }

    #[tokio::test]
    async fn a_tunnel_echoes_with_basic_credentials_and_a_user_agent() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(
            &["127.0.0.1"],
            H2ProxyScript {
                require_user_agent: true,
                ..H2ProxyScript::default()
            },
        )
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.name(), "T");
        assert!(out.http_forward().is_none());
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"through the tunnel").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].method.as_str(), seen[0].authority.as_str()),
            ("CONNECT", echo.to_string().as_str())
        );
        // base64("u:p")
        assert_eq!(seen[0].header("proxy-authorization"), Some("Basic dTpw"));
        assert_eq!(seen[0].header("user-agent"), Some(USER_AGENT));
        assert_eq!(
            fixture.seen_at_least(1).await[0].alpn.as_deref(),
            Some("h2")
        );
    }

    #[tokio::test]
    async fn headers_may_set_the_user_agent_and_add_fields() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(&["127.0.0.1"], H2ProxyScript::default()).await;
        let out = outbound(
            &fake,
            ", headers=User-Agent:Linux client<random-string(4)>;X-Note:n",
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"x").await;
        let seen = &fake.requests()[0];
        let agents: Vec<&str> = seen
            .headers
            .iter()
            .filter(|(n, _)| n == "user-agent")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(agents.len(), 1, "one user-agent: {agents:?}");
        let rest = agents[0].strip_prefix("Linux client").unwrap();
        assert_eq!(rest.len(), 4, "{}", agents[0]);
        assert_eq!(seen.header("x-note"), Some("n"));
        assert_eq!(seen.header("proxy-authorization"), Some("Basic dTpw"));
    }

    #[tokio::test]
    async fn a_refused_login_says_authentication_failed() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(&["127.0.0.1"], H2ProxyScript::default()).await;
        let out = outbound_with(
            &format!(
                "trust-tunnel, 127.0.0.1, {}, username=u, password=wrong",
                fake.addr().port()
            ),
            &fixture,
        );
        assert_eq!(
            refusal(&out, echo).await,
            "trust-tunnel: authentication failed"
        );
    }

    #[tokio::test]
    async fn an_unreachable_target_and_other_statuses_are_quoted_by_number() {
        // a port nothing listens on: the server cannot connect, 502
        let closed = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let (fixture, fake) = proxy(&["127.0.0.1"], H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, closed).await,
            "trust-tunnel: the server answered 502"
        );
        let (fixture, fake) = proxy(
            &["127.0.0.1"],
            H2ProxyScript {
                refuse: Some(403),
                ..H2ProxyScript::default()
            },
        )
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, closed).await,
            "trust-tunnel: the server answered 403"
        );
    }

    #[tokio::test]
    async fn the_sni_is_the_server_name_or_the_sni_parameter() {
        let echo = echo_server().await;
        // the server named by its host name
        let (fixture, fake) = proxy(&["localhost"], H2ProxyScript::default()).await;
        let out = outbound_with(
            &format!(
                "trust-tunnel, localhost, {}, username=u, password=p",
                fake.addr().port()
            ),
            &fixture,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"by name").await;
        assert_eq!(
            fixture.seen_at_least(1).await[0].sni.as_deref(),
            Some("localhost")
        );
        // by address, with the endpoint's host name in `sni=`
        let (fixture, fake) = proxy(&["tt.test"], H2ProxyScript::default()).await;
        let out = outbound(&fake, ", sni=tt.test", &fixture);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"by sni").await;
        assert_eq!(
            fixture.seen_at_least(1).await[0].sni.as_deref(),
            Some("tt.test")
        );
    }

    #[tokio::test]
    async fn concurrent_tunnels_do_not_wait_behind_the_servers_stream_limit() {
        let echo = echo_server().await;
        let (fixture, fake) = proxy(
            &["127.0.0.1"],
            H2ProxyScript {
                max_concurrent_streams: Some(1),
                ..H2ProxyScript::default()
            },
        )
        .await;
        // `max-streams` 3 by default, but the server takes one at a time
        let out = outbound(&fake, "", &fixture);
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b, c) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(
                out.connect_tcp(&to, &opts),
                out.connect_tcp(&to, &opts),
                out.connect_tcp(&to, &opts),
            )
        })
        .await
        .expect("a tunnel waited behind the server's limit");
        for mut stream in [a.unwrap(), b.unwrap(), c.unwrap()] {
            round_trip(&mut stream, b"one each").await;
        }
        assert_eq!(fake.connections(), 3);
    }
}
```

`crates/rurge-proto/src/lib.rs`——把

```rust
pub mod trojan;
```

换成

```rust
pub mod trojan;
pub mod trust_tunnel;
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-proto --lib -- trust_tunnel h2pool`
Expected: FAIL——池还没有 SETTINGS 之前的限制，几个请求同时到来时卡到超时：

```text
test trust_tunnel::tests::concurrent_tunnels_do_not_wait_behind_the_servers_stream_limit ... FAILED
test h2pool::tests::a_fresh_connection_carries_one_stream_until_the_servers_limit_is_known ... FAILED
thread 'trust_tunnel::tests::concurrent_tunnels_do_not_wait_behind_the_servers_stream_limit' panicked at crates\rurge-proto\src\trust_tunnel.rs:380:42:
thread 'h2pool::tests::a_fresh_connection_carries_one_stream_until_the_servers_limit_is_known' panicked at crates\rurge-proto\src\h2pool\mod.rs:654:10:
test result: FAILED. 14 passed; 2 failed; 0 ignored; 0 measured; 331 filtered out; finished in 5.01s
error: test failed, to rerun pass `-p rurge-proto --lib`
exit 101
```

- [ ] **Step 3: 实现**

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
//!   itself; a stream's `Lease` holds its place.
```

换成

```rust
//!   itself; a stream's `Lease` holds its place.
//! - Until a connection has the server's SETTINGS (a PING round trip after
//!   the handshake), its limit is unknown and it carries one stream: the
//!   first request goes out at once, the ones that would join it wait for
//!   the SETTINGS and then look again (a second connection when the
//!   server's limit is the lower one). The caller's own timeout bounds
//!   the wait.
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
        if let Some(lease) = self.take() {
            return Ok(lease);
        }
        // read before contending for the lock: tells us whether a dial
        // finished while we waited for it
        let attempt = self.attempts.load(Ordering::SeqCst);
        let _dialing = self.dialing.lock().await;
        if let Some(lease) = self.take() {
            return Ok(lease);
        }
        if self.attempts.load(Ordering::SeqCst) != attempt
            && let Some(failure) = self.failure.lock().expect("failure").as_ref()
        {
            return Err(copy_error(failure));
        }
        let result = self.connect(opts).await;
        self.attempts.fetch_add(1, Ordering::SeqCst);
        let conn = match result {
            Ok(conn) => Arc::new(conn),
            Err(e) => {
                *self.failure.lock().expect("failure") = Some(copy_error(&e));
                return Err(e);
            }
        };
        *self.failure.lock().expect("failure") = None;
        let lease = conn.lease(self.max_streams);
        self.conns.lock().expect("conns").push(conn);
        lease.ok_or_else(|| {
            OutboundError::Proxy(format!("{}: the server accepts no streams", self.label))
        })
```

换成

```rust
        loop {
            match self.take() {
                Pick::Lease(lease) => return Ok(lease),
                Pick::Unsettled(conn) => {
                    conn.settled().await;
                    continue;
                }
                Pick::Nothing => {}
            }
            // read before contending for the lock: tells us whether a dial
            // finished while we waited for it
            let attempt = self.attempts.load(Ordering::SeqCst);
            let dialing = self.dialing.lock().await;
            match self.take() {
                Pick::Lease(lease) => return Ok(lease),
                Pick::Unsettled(conn) => {
                    drop(dialing);
                    conn.settled().await;
                    continue;
                }
                Pick::Nothing => {}
            }
            if self.attempts.load(Ordering::SeqCst) != attempt
                && let Some(failure) = self.failure.lock().expect("failure").as_ref()
            {
                return Err(copy_error(failure));
            }
            let result = self.connect(opts).await;
            self.attempts.fetch_add(1, Ordering::SeqCst);
            let conn = match result {
                Ok(conn) => Arc::new(conn),
                Err(e) => {
                    *self.failure.lock().expect("failure") = Some(copy_error(&e));
                    return Err(e);
                }
            };
            *self.failure.lock().expect("failure") = None;
            let lease = conn.lease(self.max_streams);
            self.conns.lock().expect("conns").push(conn);
            return lease.ok_or_else(|| {
                OutboundError::Proxy(format!("{}: the server accepts no streams", self.label))
            });
        }
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
    /// idled too long are let go on the way.
    fn take(&self) -> Option<Lease> {
        let mut conns = self.conns.lock().expect("conns");
        prune(&mut conns, Instant::now());
        conns.iter().find_map(|c| c.lease(self.max_streams))
```

换成

```rust
    /// idled too long are let go on the way. Without one, a connection that
    /// may turn out to have room once its SETTINGS are known.
    fn take(&self) -> Pick {
        let mut conns = self.conns.lock().expect("conns");
        prune(&mut conns, Instant::now());
        let mut unsettled = None;
        for conn in conns.iter() {
            if let Some(lease) = conn.lease(self.max_streams) {
                return Pick::Lease(lease);
            }
            if unsettled.is_none() && !conn.is_settled() {
                unsettled = Some(conn.clone());
            }
        }
        unsettled.map_or(Pick::Nothing, Pick::Unsettled)
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
        conns.len()
    }
}

/// One pooled connection. Its streams' leases keep it alive after the pool
```

换成

```rust
        conns.len()
    }
}

/// What `H2Pool::take` found.
enum Pick {
    Lease(Lease),
    /// No room now, but this connection's limit is not known yet.
    Unsettled(Arc<Conn>),
    Nothing,
}

/// One pooled connection. Its streams' leases keep it alive after the pool
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
        // before the server's SETTINGS, its limit is unbounded
        let limit = max_streams.min(self.send.current_max_send_streams());
```

换成

```rust
        // before the server's SETTINGS, `h2` takes its limit as unbounded
        // and would queue the streams past the real one: one stream until
        // they are known
        let limit = if self.is_settled() {
            max_streams.min(self.send.current_max_send_streams())
        } else {
            1
        };
```

`crates/rurge-proto/src/h2pool/mod.rs`——把

```rust
        usage.open == 0 && now.duration_since(usage.idle_since) >= IDLE_TIMEOUT
```

换成

```rust
        usage.open == 0 && now.duration_since(usage.idle_since) >= IDLE_TIMEOUT
    }

    fn is_settled(&self) -> bool {
        *self.settled.borrow()
```

要点：
- 一条连接在 SETTINGS 到达前满员时，`take()` 返回"还未定"；请求在拨号锁之外等这条连接的 SETTINGS（受它自己的 `opts.timeout` 约束），再重新挑；连接先死掉时等待者被唤醒、连接被剔除、照常另拨。
- endpoint 也可能被配置成认证失败时回 405 / 404 / 403；那时用户看到的是 `trust-tunnel: the server answered <状态码>`，只有 407 算作认证失败。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-proto --lib -- trust_tunnel h2pool h2connect` → 通过（`trust_tunnel` 6 条：带凭据与 user-agent 的回显、`headers` 替换 user-agent 并加字段、认证失败、目标连不上与其它状态码、SNI、低于 `max-streams` 的服务端上限下的并发隧道；`h2pool` 新增 1 条；`h2connect` 的服务端上限用例为并发写法）。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates/rurge-proto
git commit -m "feat(proto): trust-tunnel（HTTP/2 模式）；会话池在服务端 SETTINGS 之前每条连接只放一条流"
```

### Task 5: CONNECT-UDP

`h2-connect` 的 `udp-relay`（P9）：`h2connect.rs` 改成目录——`mod.rs`（出站，状态移进与载体共用的 `Arc<Inner>`）、`capsule.rs`（varint 与 capsule 编解码）、`udp.rs`（载体 `H2Udp`：每个目标一条 extended CONNECT 流）。`FakeH2Proxy` 加 CONNECT-UDP 的服务端一侧（独立的编解码，回包的长度用非最短 varint；`udp_extra_capsules` 在每个回包前先发一个不认识的 capsule 和一个 context id 为 2 的数据报）。

新模块的用例（编解码的已知答案、路径编码、经 `FakeH2Proxy` 的往返等）都在新文件里，与实现一同写出，没有单独的失败步骤。

**Files:**
- Delete: `crates/rurge-proto/src/h2connect.rs`
- Create: `crates/rurge-proto/src/h2connect/mod.rs`、`capsule.rs`、`udp.rs`（均自带用例）
- Modify: `crates/rurge-proto/src/testing/h2proxy.rs`

**Interfaces:**
- Consumes: Task 2 的扩展 CONNECT 门；Task 3 / 4 的请求构造与池；M5b 的每目标载体做法（`vmess::udp`）。
- Produces: `H2ConnectOutbound::udp()` / `open_udp`；`h2connect::capsule`（varint、`DATAGRAM`、读写一个 capsule）

- [ ] **Step 1: 假服务端的 CONNECT-UDP**

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
//! every CONNECT stream. It never resolves a name.
```

换成

```rust
//! every CONNECT stream; with extended CONNECT, CONNECT-UDP (RFC 9298) too,
//! a UDP socket per stream. It never resolves a name.
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
use std::net::SocketAddr;
```

换成

```rust
use std::net::{IpAddr, SocketAddr};
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
use tokio::net::{TcpListener, TcpStream};
```

换成

```rust
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
    /// Advertise extended CONNECT (SETTINGS_ENABLE_CONNECT_PROTOCOL = 1).
    pub extended_connect: bool,
```

换成

```rust
    /// Advertise extended CONNECT (SETTINGS_ENABLE_CONNECT_PROTOCOL = 1),
    /// which CONNECT-UDP needs.
    pub extended_connect: bool,
    /// On CONNECT-UDP, put a capsule of an unknown type and a datagram with
    /// context id 2 before every datagram sent back, their varints longer
    /// than needed.
    pub udp_extra_capsules: bool,
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
    /// Tunnel here whatever the client asked for (needed for a domain target).
```

换成

```rust
    /// Tunnel or relay here whatever the client asked for (needed for a
    /// domain target).
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
    // no extended CONNECT protocol is served
```

换成

```rust
    if request.method() == Method::CONNECT && seen.protocol.as_deref() == Some("connect-udp") {
        return connect_udp(request, respond, &seen, &shared.script).await;
    }
    // no other extended CONNECT protocol is served
```

`crates/rurge-proto/src/testing/h2proxy.rs`——把

```rust
    let _ = tokio::io::copy_bidirectional(&mut stream, upstream).await;
```

换成

```rust
    let _ = tokio::io::copy_bidirectional(&mut stream, upstream).await;
}

/// `/.well-known/masque/udp/{host}/{port}/` (RFC 9298 2): the host
/// percent-decoded, and the port.
fn masque_target(path: &str) -> Option<(String, u16)> {
    let rest = path
        .strip_prefix("/.well-known/masque/udp/")?
        .strip_suffix('/')?;
    let (host, port) = rest.split_once('/')?;
    let mut decoded = Vec::new();
    let mut bytes = host.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            decoded.push(b);
        }
    }
    Some((String::from_utf8(decoded).ok()?, port.parse().ok()?))
}

/// `value` as a QUIC varint of `len` bytes (1, 2, 4 or 8), minimal or not.
fn put_varint(out: &mut Vec<u8>, value: u64, len: usize) {
    let tag = (len.trailing_zeros() as u64) << (len * 8 - 2);
    out.extend_from_slice(&(value | tag).to_be_bytes()[8 - len..]);
}

/// The first capsule in `buf` (type, value), taken out of it; `None` while
/// it is incomplete.
fn take_capsule(buf: &mut Vec<u8>) -> Option<(u64, Vec<u8>)> {
    fn varint(bytes: &[u8]) -> Option<(u64, usize)> {
        let len = 1usize << (bytes.first()? >> 6);
        let mut value = u64::from(bytes[0] & 0x3f);
        for &b in bytes.get(1..len)? {
            value = value << 8 | u64::from(b);
        }
        Some((value, len))
    }
    let (kind, a) = varint(buf)?;
    let (len, b) = varint(&buf[a..])?;
    let end = a + b + usize::try_from(len).ok()?;
    let value = buf.get(a + b..end)?.to_vec();
    buf.drain(..end);
    Some((kind, value))
}

/// A CONNECT-UDP request: 200 with `capsule-protocol: ?1`, then a UDP
/// socket relaying the stream's DATAGRAM capsules (context id 0) to the
/// target in the path and the target's datagrams back.
async fn connect_udp(
    request: Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
    seen: &RecordedH2Request,
    script: &H2ProxyScript,
) {
    let target = match (script.connect_to, masque_target(&seen.path)) {
        (Some(addr), Some(_)) => Some(addr),
        (None, Some((host, port))) => host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, port)),
        (_, None) => None,
    };
    let Some(target) = target else {
        return refuse(respond, StatusCode::BAD_REQUEST);
    };
    let local: SocketAddr = if target.is_ipv4() {
        "127.0.0.1:0".parse().expect("an address")
    } else {
        "[::1]:0".parse().expect("an address")
    };
    let Ok(socket) = UdpSocket::bind(local).await else {
        return refuse(respond, StatusCode::BAD_GATEWAY);
    };
    let response = Response::builder()
        .header("capsule-protocol", "?1")
        .body(())
        .expect("a response");
    let Ok(send) = respond.send_response(response, false) else {
        return;
    };
    let stream = H2Stream::new("fake-h2", send, request.into_body());
    let (mut reader, mut writer) = tokio::io::split(stream);
    let extra = script.udp_extra_capsules;
    let socket = Arc::new(socket);
    let outbound = socket.clone();
    let up = async move {
        let (mut buf, mut chunk) = (Vec::new(), vec![0u8; 65536]);
        while let Ok(n @ 1..) = reader.read(&mut chunk).await {
            buf.extend_from_slice(&chunk[..n]);
            while let Some((kind, value)) = take_capsule(&mut buf) {
                // a DATAGRAM capsule with context id 0 (one byte)
                if kind == 0 && value.first() == Some(&0) {
                    let _ = outbound.send_to(&value[1..], target).await;
                }
            }
        }
    };
    let down = async move {
        let mut datagram = vec![0u8; 65536];
        while let Ok((n, from)) = socket.recv_from(&mut datagram).await {
            if from != target {
                continue;
            }
            let mut out = Vec::new();
            if extra {
                // type 0x2a2a in eight bytes, three bytes of value
                put_varint(&mut out, 0x2a2a, 8);
                put_varint(&mut out, 3, 4);
                out.extend_from_slice(b"???");
                // context id 2 in two bytes
                put_varint(&mut out, 0, 2);
                put_varint(&mut out, n as u64 + 2, 8);
                put_varint(&mut out, 2, 2);
                out.extend_from_slice(&datagram[..n]);
            }
            put_varint(&mut out, 0, 1);
            put_varint(&mut out, n as u64 + 1, 4);
            put_varint(&mut out, 0, 1);
            out.extend_from_slice(&datagram[..n]);
            if writer.write_all(&out).await.is_err() {
                return;
            }
        }
    };
    // the socket lives as long as the stream (RFC 9298 3.1)
    tokio::select! {
        _ = up => {}
        _ = down => {}
    }
```

- [ ] **Step 2: 出站改成目录**

删除 `crates/rurge-proto/src/h2connect.rs`（它的内容移到了新目录下的文件里）。

新建 `crates/rurge-proto/src/h2connect/mod.rs`：

```rust
//! `h2-connect` outbound (manual: Policies › HTTP and HTTP/2; phase 2 M6
//! design 5.3): every connection is a CONNECT stream on a pooled TLS +
//! HTTP/2 connection (`h2pool`), `max-streams` of them per connection.
//!
//! - The request is `:method CONNECT` and `:authority host:port` (an IPv6
//!   literal in brackets, an IDN as its A-labels), then
//!   `proxy-authorization: Basic …` when the policy has credentials, then
//!   the configured `headers`, rendered anew for every request. A
//!   configured header replaces one of ours with the same name, as on
//!   `http` / `https`: `headers=Proxy-Authorization:…` wins over the
//!   credentials. No `user-agent` unless `headers` adds one.
//! - A 2xx answer turns the stream into the tunnel; any other status is
//!   the proxy's refusal.
//! - With `udp-relay=true`, UDP goes as CONNECT-UDP (RFC 9298) over
//!   extended CONNECT (RFC 8441), one stream per target (`udp`).

mod capsule;
mod udp;

use crate::build::{shadow_tls_client, tls_client};
use crate::h2pool::{H2Pool, H2Stream, StackDial};
use crate::http::{merge, render, wire_host};
use crate::transport::Stack;
use crate::{BuildError, Outbound, OutboundError, UdpSupport};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use h2::ext::Protocol;
use http::header::{HeaderName, HeaderValue};
use http::{Method, Request, Response, StatusCode, Version};
use rurge_config::KeystoreItem;
use rurge_config::spec::{H2ConnectSpec, HeaderTemplate, ShadowTlsOpts};
use rurge_net::BoxFuture;
use rurge_net::connector::{BoxedPacketSocket, BoxedStream, ConnectOpts, Connector, Target};
use rustls::RootCertStore;
use std::sync::Arc;
use udp::H2Udp;

const LABEL: &str = "h2-connect";

/// No `Debug`: it holds the credentials.
pub struct H2ConnectOutbound {
    name: String,
    /// Shared with the UDP carriers.
    inner: Arc<Inner>,
}

struct Inner {
    pool: H2Pool,
    /// The `proxy-authorization` value, ready to send.
    authorization: Option<String>,
    headers: Vec<HeaderTemplate>,
    /// `udp-relay=true`: the server's own `:authority` for CONNECT-UDP.
    udp_authority: Option<String>,
}

/// `Basic base64(user:password)` (RFC 9110 11.7.2, RFC 7617).
pub(crate) fn basic_authorization(user: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{user}:{password}")))
}

/// The CONNECT request for `target`: `ours` (lowercase names), replaced
/// field by field by the rendered `templates`. Fails, before anything is
/// dialed, for a target whose name cannot be sent (the alphabet of
/// `http::wire_host`).
pub(crate) fn connect_request(
    label: &str,
    target: &Target,
    ours: Vec<(String, String)>,
    templates: &[HeaderTemplate],
) -> Result<Request<()>, OutboundError> {
    let Some(host) = wire_host(target) else {
        return Err(unsendable(label));
    };
    request(label, format!("{host}:{}", target.port), ours, templates)
}

fn unsendable(label: &str) -> OutboundError {
    OutboundError::Proxy(format!(
        "{label}: the host name cannot be sent to the server"
    ))
}

/// A CONNECT request for `uri` carrying `ours`, replaced field by field by
/// the rendered `templates`.
fn request(
    label: &str,
    uri: String,
    ours: Vec<(String, String)>,
    templates: &[HeaderTemplate],
) -> Result<Request<()>, OutboundError> {
    let mut fields = ours;
    merge(&mut fields, render(templates));
    let mut request = Request::builder()
        .method(Method::CONNECT)
        .uri(uri)
        .version(Version::HTTP_2);
    for (name, value) in fields {
        // `HeaderName` lowercases, as HTTP/2 wants; valid templates always
        // convert (`HeaderTemplate::is_valid`, checked at build time)
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) else {
            return Err(OutboundError::Proxy(format!(
                "{label}: a custom header is not valid"
            )));
        };
        request = request.header(name, value);
    }
    request
        .body(())
        .map_err(|_| OutboundError::Proxy(format!("{label}: the CONNECT request is not valid")))
}

impl H2ConnectOutbound {
    pub fn new(
        name: &str,
        server: Target,
        spec: &H2ConnectSpec,
        shadow_tls: Option<&ShadowTlsOpts>,
        keystore: &[KeystoreItem],
        roots: Arc<RootCertStore>,
        connector: Arc<dyn Connector>,
    ) -> Result<H2ConnectOutbound, BuildError> {
        // error texts carry no policy name: the registry's `build_one` and
        // the dry build both prefix it. A spec made by hand may carry
        // headers `rurge-config` never let through: never echo them.
        if let Some(n) = spec.headers.iter().position(|t| !t.is_valid()) {
            return Err(BuildError::new(format!(
                "custom header #{} is not valid",
                n + 1
            )));
        }
        // CONNECT-UDP names the server itself (RFC 9298 3.4)
        let udp_authority = match spec.udp_relay {
            false => None,
            true => match wire_host(&server) {
                Some(host) => Some(format!("{host}:{}", server.port)),
                None => {
                    return Err(BuildError::new(
                        "`udp-relay`: the server name cannot be sent in a request".to_string(),
                    ));
                }
            },
        };
        let shadow_tls =
            shadow_tls_client(shadow_tls, Some(&spec.tls), &server.host, roots.clone())?;
        // the spec pins `alpn` to `h2`; `h2` is also what an empty one offers
        let tls = tls_client(Some(&spec.tls), &server.host, &["h2"], keystore, roots)?;
        let authorization = spec.username.as_ref().map(|user| {
            let password = spec.password.as_ref().map_or("", |p| p.expose().as_str());
            basic_authorization(user.expose(), password)
        });
        let dial = StackDial {
            label: LABEL,
            stack: Stack::new(connector, server, shadow_tls, tls, None),
        };
        Ok(H2ConnectOutbound {
            name: name.to_string(),
            inner: Arc::new(Inner {
                pool: H2Pool::new(LABEL, spec.max_streams.max(1), Arc::new(dial)),
                authorization,
                headers: spec.headers.clone(),
                udp_authority,
            }),
        })
    }
}

impl Inner {
    /// Our fields of every request: the credentials, if any.
    fn ours(&self) -> Vec<(String, String)> {
        self.authorization
            .iter()
            .map(|value| ("proxy-authorization".to_string(), value.clone()))
            .collect()
    }

    /// The stream of a 2xx `response`; any other status is the refusal.
    fn established(response: Response<H2Stream>) -> Result<H2Stream, OutboundError> {
        match response.status() {
            status if status.is_success() => Ok(response.into_body()),
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => Err(OutboundError::Proxy(format!(
                "{LABEL}: proxy authentication required"
            ))),
            status => Err(OutboundError::Proxy(format!(
                "{LABEL}: the proxy answered {}",
                status.as_u16()
            ))),
        }
    }

    async fn tunnel(
        &self,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<BoxedStream, OutboundError> {
        // never dial for a target whose name cannot be sent
        let request = connect_request(LABEL, target, self.ours(), &self.headers)?;
        let response = self.pool.open(request, opts).await?;
        Ok(Box::new(Self::established(response)?) as BoxedStream)
    }

    /// A CONNECT-UDP stream to `target` (RFC 9298 3.4): `:protocol
    /// connect-udp`, the server's `:authority`, the default template's
    /// `:path` and `capsule-protocol: ?1` (RFC 9297 3.4), then the same
    /// fields as a CONNECT. The pool refuses it on a connection whose server
    /// did not enable extended CONNECT.
    async fn udp_stream(
        &self,
        authority: &str,
        target: &Target,
        opts: &ConnectOpts,
    ) -> Result<H2Stream, OutboundError> {
        let Some(path) = udp::masque_path(target) else {
            return Err(unsendable(LABEL));
        };
        let mut ours = self.ours();
        ours.push(("capsule-protocol".to_string(), "?1".to_string()));
        let mut request = request(
            LABEL,
            format!("https://{authority}{path}"),
            ours,
            &self.headers,
        )?;
        request
            .extensions_mut()
            .insert(Protocol::from_static("connect-udp"));
        let response = self.pool.open(request, opts).await?;
        Self::established(response)
    }
}

impl Outbound for H2ConnectOutbound {
    fn name(&self) -> &str {
        &self.name
    }

    fn connect_tcp<'a>(
        &'a self,
        target: &'a Target,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedStream, OutboundError>> {
        Box::pin(async move {
            // one budget for the connection (when one is dialed), TLS, the
            // HTTP/2 handshake and the CONNECT exchange
            match tokio::time::timeout(opts.timeout, self.inner.tunnel(target, opts)).await {
                Ok(result) => result,
                Err(_) => Err(OutboundError::Timeout),
            }
        })
    }

    fn udp(&self) -> UdpSupport {
        if self.inner.udp_authority.is_some() {
            UdpSupport::Native
        } else {
            UdpSupport::Unsupported
        }
    }

    /// Nothing is opened yet: each target gets its stream with its first
    /// datagram.
    fn open_udp<'a>(
        &'a self,
        opts: &'a ConnectOpts,
    ) -> BoxFuture<'a, Result<BoxedPacketSocket, OutboundError>> {
        let carrier = match &self.inner.udp_authority {
            Some(authority) => {
                let udp = H2Udp::new(
                    self.inner.clone(),
                    authority.clone(),
                    opts.clone(),
                    udp::IDLE,
                );
                Ok(Box::new(udp) as BoxedPacketSocket)
            }
            None => Err(OutboundError::Unsupported(
                "UDP without `udp-relay=true`".to_string(),
            )),
        };
        Box::pin(std::future::ready(carrier))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeH2Proxy, H2ProxyScript, TlsFixture, echo_server, udp_echo_server};
    use rurge_config::policy::parse_policy;
    use rurge_config::spec::h2::read_h2_connect;
    use rurge_config::spec::shadow_tls::read_shadow_tls;
    use rurge_config::spec::{HeaderPart, ParamReader};
    use rurge_config::{HostName, KeystoreType, Span};
    use rurge_net::connector::{DirectConnector, PacketSocket, SystemResolve};
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::time::Instant;

    /// The outbound for `definition` (an `h2-connect, host, port, ...` line).
    fn outbound_with(
        definition: &str,
        fixture: &TlsFixture,
        keystore: &[KeystoreItem],
    ) -> H2ConnectOutbound {
        let span = Span::new(Arc::from(Path::new("p.conf")), 1);
        let policy = parse_policy("H", definition, &span).unwrap();
        let mut r = ParamReader::new(&policy);
        let spec = read_h2_connect(&mut r, keystore);
        let shadow_tls = read_shadow_tls(&mut r);
        assert!(!r.has_errors(), "{:?}", r.finish());
        H2ConnectOutbound::new(
            "H",
            Target::new(policy.server.clone().unwrap(), policy.port.unwrap()),
            &spec,
            shadow_tls.as_ref(),
            keystore,
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .unwrap()
    }

    /// `h2-connect, 127.0.0.1, <the fake's port><extra>`.
    fn outbound(fake: &FakeH2Proxy, extra: &str, fixture: &TlsFixture) -> H2ConnectOutbound {
        outbound_with(
            &format!("h2-connect, 127.0.0.1, {}{extra}", fake.addr().port()),
            fixture,
            &[],
        )
    }

    async fn fake(script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let fake = FakeH2Proxy::spawn(script, fixture.clone()).await;
        (fixture, fake)
    }

    fn target(addr: SocketAddr) -> Target {
        Target::new(HostName::Ip(addr.ip()), addr.port())
    }

    async fn round_trip(stream: &mut BoxedStream, payload: &[u8]) {
        stream.write_all(payload).await.unwrap();
        let mut back = vec![0u8; payload.len()];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut back))
            .await
            .expect("the echo arrives")
            .unwrap();
        assert_eq!(back, payload);
    }

    async fn refusal(out: &H2ConnectOutbound, to: SocketAddr) -> String {
        let Err(e) = out.connect_tcp(&target(to), &ConnectOpts::default()).await else {
            panic!("the proxy let us through");
        };
        assert!(matches!(e, OutboundError::Proxy(_)), "{e}");
        e.to_string()
    }

    /// Polls `check` until it holds, for at most five seconds.
    async fn eventually(mut check: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !check() {
            assert!(Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_tunnel_echoes_over_tls_with_alpn_h2() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.name(), "H");
        assert!(out.http_forward().is_none());
        assert!(matches!(out.udp(), crate::UdpSupport::Unsupported));
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"through the stream").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "CONNECT");
        assert_eq!(seen[0].authority, echo.to_string());
        assert_eq!(
            (seen[0].protocol.as_deref(), seen[0].path.as_str()),
            (None, "")
        );
        assert!(seen[0].headers.is_empty(), "{:?}", seen[0].headers);
        let tls = fixture.seen_at_least(1).await;
        assert_eq!(tls[0].alpn.as_deref(), Some("h2"));
        assert_eq!(tls[0].sni, None, "an IP literal: no SNI");
    }

    #[tokio::test]
    async fn a_large_payload_crosses_both_ways() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        let stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        // several of both sides' stream windows (1 MiB each)
        let data: Vec<u8> = (0..(5 << 20) + 777).map(|i| (i % 253) as u8).collect();
        let (mut r, mut w) = tokio::io::split(stream);
        let sent = data.clone();
        let writer = tokio::spawn(async move {
            w.write_all(&sent).await.unwrap();
            w.shutdown().await.unwrap();
        });
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(30), r.read_to_end(&mut back))
            .await
            .expect("stalled")
            .unwrap();
        writer.await.unwrap();
        assert!(back == data, "the echo differs");
    }

    #[tokio::test]
    async fn shutdown_reaches_the_target_as_a_half_close() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        stream.write_all(b"last words").await.unwrap();
        stream.shutdown().await.unwrap();
        // the target saw the FIN, echoed what it had and closed: we read all
        let mut back = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut back))
            .await
            .expect("the target closed")
            .unwrap();
        assert_eq!(back, b"last words");
    }

    #[tokio::test]
    async fn credentials_go_as_basic_and_a_refusal_says_so() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            users: vec![("u".into(), "p:w".into())],
            ..H2ProxyScript::default()
        })
        .await;
        let mut stream = outbound(&fake, ", u, p:w", &fixture)
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"let in").await;
        // base64("u:p:w")
        assert_eq!(
            fake.requests()[0].header("proxy-authorization"),
            Some("Basic dTpwOnc=")
        );
        for extra in ["", ", username=u, password=wrong"] {
            let out = outbound(&fake, extra, &fixture);
            assert_eq!(
                refusal(&out, echo).await,
                "h2-connect: proxy authentication required"
            );
        }
        assert!(fake.requests()[1].header("proxy-authorization").is_none());
    }

    #[tokio::test]
    async fn any_other_status_is_quoted_by_its_number() {
        let echo = echo_server().await;
        for code in [403, 502, 503] {
            let (fixture, fake) = fake(H2ProxyScript {
                refuse: Some(code),
                ..H2ProxyScript::default()
            })
            .await;
            let out = outbound(&fake, "", &fixture);
            assert_eq!(
                refusal(&out, echo).await,
                format!("h2-connect: the proxy answered {code}")
            );
        }
    }

    #[tokio::test]
    async fn headers_are_rendered_per_request_and_replace_ours() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(
            &fake,
            ", user, pass, headers=X-Pad:p<random-string(12)>q<random-string(2-6)>;Proxy-Authorization:Bearer abc",
            &fixture,
        );
        for _ in 0..2 {
            let mut stream = out
                .connect_tcp(&target(echo), &ConnectOpts::default())
                .await
                .unwrap();
            round_trip(&mut stream, b"x").await;
        }
        let seen = fake.requests();
        assert_eq!(fake.connections(), 1, "both on one connection");
        for request in &seen {
            let auth: Vec<&str> = request
                .headers
                .iter()
                .filter(|(n, _)| n == "proxy-authorization")
                .map(|(_, v)| v.as_str())
                .collect();
            assert_eq!(auth, ["Bearer abc"], "the configured header wins");
            assert!(request.header("user-agent").is_none());
            let pad = request.header("x-pad").expect("lowercased by HTTP/2");
            let inner = pad.strip_prefix('p').unwrap();
            let (first, second) = inner.split_at(12);
            let second = second.strip_prefix('q').unwrap();
            assert!((2..=6).contains(&second.len()), "{pad}");
            assert!(
                first
                    .chars()
                    .chain(second.chars())
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{pad}"
            );
        }
        assert_ne!(
            seen[0].header("x-pad"),
            seen[1].header("x-pad"),
            "drawn anew for every request, not every connection"
        );
    }

    #[tokio::test]
    async fn the_authority_brackets_ipv6_and_carries_a_labels() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            connect_to: Some(echo),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        for (host, authority) in [
            (HostName::parse("2001:db8::1"), "[2001:db8::1]:443"),
            (
                HostName::Domain("bücher.example".into()),
                "xn--bcher-kva.example:443",
            ),
            (HostName::parse("remote.example"), "remote.example:443"),
        ] {
            let mut stream = out
                .connect_tcp(&Target::new(host, 443), &ConnectOpts::default())
                .await
                .unwrap();
            round_trip(&mut stream, b"x").await;
            assert_eq!(fake.requests().last().unwrap().authority, authority);
        }
    }

    #[tokio::test]
    async fn a_name_that_cannot_be_sent_never_dials() {
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        for name in ["x@blocked.test", "a b.test", "a.test\r\nx: 1", ""] {
            let Err(e) = out
                .connect_tcp(
                    &Target::new(HostName::Domain(name.to_string()), 443),
                    &ConnectOpts::default(),
                )
                .await
            else {
                panic!("{name:?} was sent");
            };
            assert_eq!(
                e.to_string(),
                "h2-connect: the host name cannot be sent to the server"
            );
        }
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn max_streams_bounds_each_connection() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", max-streams=2", &fixture);
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b, c) = tokio::join!(
            out.connect_tcp(&to, &opts),
            out.connect_tcp(&to, &opts),
            out.connect_tcp(&to, &opts),
        );
        let mut streams = [a.unwrap(), b.unwrap(), c.unwrap()];
        for stream in &mut streams {
            round_trip(stream, b"three at once").await;
        }
        assert_eq!(fake.connections(), 2);
        let mut carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        carried.sort_unstable();
        assert_eq!(carried, [0, 0, 1]);
    }

    #[tokio::test]
    async fn the_servers_own_stream_limit_is_kept_too() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            max_concurrent_streams: Some(1),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        // all at once: none may queue behind the server's limit, unknown
        // until its SETTINGS arrive
        let (to, opts) = (target(echo), ConnectOpts::default());
        let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(out.connect_tcp(&to, &opts), out.connect_tcp(&to, &opts))
        })
        .await
        .expect("a tunnel waited behind the server's limit");
        let (mut first, mut second) = (a.unwrap(), b.unwrap());
        round_trip(&mut first, b"one").await;
        round_trip(&mut second, b"two").await;
        assert_eq!(fake.connections(), 2);
    }

    #[tokio::test]
    async fn after_goaway_the_next_tunnel_takes_a_new_connection() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            goaway_after: Some(1),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        let mut first = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        eventually(|| out.inner.pool.connections() == 0).await;
        let mut second = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        // the stream from before the GOAWAY runs on
        round_trip(&mut first, b"old").await;
        round_trip(&mut second, b"new").await;
        assert_eq!(fake.connections(), 2);
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 1]);
    }

    #[tokio::test]
    async fn a_server_without_h2_is_refused_before_http2() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            no_alpn: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(
            refusal(&out, echo).await,
            "h2-connect: the server does not speak HTTP/2"
        );
        assert!(fake.requests().is_empty());
    }

    #[tokio::test]
    async fn a_client_certificate_comes_from_the_keystore() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            require_client_cert: true,
            ..H2ProxyScript::default()
        })
        .await;
        let keystore = [KeystoreItem {
            name: "mtls".into(),
            kind: KeystoreType::P12,
            base64: fixture.client_p12_base64("rurge"),
            password: Some("pw".into()),
            unknown: Vec::new(),
            span: Span::new(Arc::from(Path::new("p.conf")), 1),
        }];
        let out = outbound_with(
            &format!(
                "h2-connect, 127.0.0.1, {}, client-cert=mtls",
                fake.addr().port()
            ),
            &fixture,
            &keystore,
        );
        let mut stream = out
            .connect_tcp(&target(echo), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"mutual").await;
        assert!(fixture.seen_at_least(1).await[0].client_cert);
    }

    #[tokio::test]
    async fn a_silent_proxy_times_out() {
        let echo = echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript {
            delay: Duration::from_secs(30),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, "", &fixture);
        let started = Instant::now();
        let Err(e) = out
            .connect_tcp(
                &target(echo),
                &ConnectOpts {
                    timeout: Duration::from_millis(300),
                },
            )
            .await
        else {
            panic!("the proxy never answered");
        };
        assert!(matches!(e, OutboundError::Timeout), "{e}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(fake.requests().len(), 1, "the CONNECT was sent");
    }

    #[test]
    fn invalid_header_templates_are_refused_at_build_time() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let mut spec = H2ConnectSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: 3,
            udp_relay: false,
        };
        spec.headers.push(HeaderTemplate {
            name: "X-Evil".into(),
            value: vec![HeaderPart::Literal("a\r\nb: 1".into())],
        });
        let err = H2ConnectOutbound::new(
            "H",
            Target::new(HostName::parse("127.0.0.1"), 443),
            &spec,
            None,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(err.message, "custom header #1 is not valid");
    }

    #[test]
    fn basic_authorization_is_base64_of_user_colon_password() {
        // RFC 7617 2
        assert_eq!(
            basic_authorization("Aladdin", "open sesame"),
            "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
    }

    /// A fake that serves CONNECT-UDP.
    async fn udp_fake(script: H2ProxyScript) -> (Arc<TlsFixture>, FakeH2Proxy) {
        fake(H2ProxyScript {
            extended_connect: true,
            ..script
        })
        .await
    }

    async fn udp_answer(carrier: &dyn PacketSocket) -> (Vec<u8>, Target) {
        let mut buf = vec![0u8; 65536];
        let (n, from) = tokio::time::timeout(Duration::from_secs(10), carrier.recv_from(&mut buf))
            .await
            .expect("an answer within the bound")
            .unwrap();
        (buf[..n].to_vec(), from)
    }

    async fn udp_roundtrip(carrier: &dyn PacketSocket, to: &Target, payload: &[u8]) {
        carrier.send_to(payload, to).await.unwrap();
        assert_eq!(udp_answer(carrier).await, (payload.to_vec(), to.clone()));
    }

    fn masque(addr: SocketAddr) -> String {
        format!("/.well-known/masque/udp/{}/{}/", addr.ip(), addr.port())
    }

    /// RFC 9298 3.4: extended CONNECT to the server's own authority, the
    /// target in the path, `capsule-protocol: ?1`, our fields as on TCP.
    #[tokio::test]
    async fn a_datagram_crosses_a_connect_udp_stream() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            users: vec![("u".into(), "p".into())],
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", u, p, udp-relay=true, headers=X-Pad:x", &fixture);
        assert_eq!(out.udp(), UdpSupport::Native);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        assert_eq!(fake.connections(), 0, "nothing is opened before a datagram");
        udp_roundtrip(carrier.as_ref(), &target(echo), b"through a capsule").await;
        udp_roundtrip(carrier.as_ref(), &target(echo), b"").await;
        let seen = fake.requests();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].method, "CONNECT");
        assert_eq!(seen[0].protocol.as_deref(), Some("connect-udp"));
        assert_eq!(seen[0].authority, fake.addr().to_string());
        assert_eq!(seen[0].path, masque(echo));
        assert_eq!(seen[0].header("capsule-protocol"), Some("?1"));
        assert_eq!(seen[0].header("proxy-authorization"), Some("Basic dTpw"));
        assert_eq!(seen[0].header("x-pad"), Some("x"));
        // a refused CONNECT-UDP fails the datagram with the TCP texts
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier.send_to(b"x", &target(echo)).await.unwrap_err();
        assert_eq!(err.to_string(), "h2-connect: proxy authentication required");
    }

    /// One stream per target (M6-D8), the streams sharing the pooled
    /// connection; each answer is its stream's target's.
    #[tokio::test]
    async fn every_target_has_its_own_stream() {
        let (one, two) = (udp_echo_server().await, udp_echo_server().await);
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), &target(one), b"to one").await;
        udp_roundtrip(carrier.as_ref(), &target(two), b"to two").await;
        udp_roundtrip(carrier.as_ref(), &target(one), b"one again").await;
        let seen: Vec<(usize, String)> = fake
            .requests()
            .iter()
            .map(|r| (r.connection, r.path.clone()))
            .collect();
        assert_eq!(seen, [(0, masque(one)), (0, masque(two))]);
        assert_eq!(fake.connections(), 1);
    }

    /// Names as A-labels, IPv6 literals with `%3A` (RFC 9298 3).
    #[tokio::test]
    async fn names_and_ipv6_literals_go_in_the_path() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            connect_to: Some(echo),
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        for (host, path) in [
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/53/",
            ),
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/53/",
            ),
        ] {
            udp_roundtrip(carrier.as_ref(), &Target::new(host, 53), b"q").await;
            assert_eq!(fake.requests().last().unwrap().path, path);
        }
        let err = carrier
            .send_to(
                b"q",
                &Target::new(HostName::Domain("x@blocked.test".into()), 53),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "h2-connect: the host name cannot be sent to the server"
        );
        assert_eq!(fake.requests().len(), 2);
    }

    /// RFC 8441 3: no extended CONNECT before the server allows it.
    #[tokio::test]
    async fn a_server_without_extended_connect_carries_no_udp() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier.send_to(b"x", &target(echo)).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "h2-connect: the server does not support extended CONNECT"
        );
        assert!(fake.requests().is_empty());
        // the connection still carries TCP
        let tcp = echo_server().await;
        let mut stream = out
            .connect_tcp(&target(tcp), &ConnectOpts::default())
            .await
            .unwrap();
        round_trip(&mut stream, b"tcp is fine").await;
    }

    /// RFC 9297 3.2, RFC 9298 4: other capsule types and context ids are
    /// skipped; nothing of them reaches the engine.
    #[tokio::test]
    async fn other_capsules_are_skipped() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript {
            udp_extra_capsules: true,
            ..H2ProxyScript::default()
        })
        .await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        udp_roundtrip(carrier.as_ref(), &target(echo), b"first").await;
        udp_roundtrip(carrier.as_ref(), &target(echo), b"second").await;
    }

    /// RFC 9298 5: at most 65527 bytes with context id 0; a longer datagram
    /// is refused and nothing is opened.
    #[tokio::test]
    async fn a_datagram_too_long_is_refused() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = out.open_udp(&ConnectOpts::default()).await.unwrap();
        let err = carrier
            .send_to(&vec![0u8; 65528], &target(echo))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(
            err.to_string(),
            "h2-connect: a datagram longer than 65527 bytes"
        );
        assert_eq!(fake.connections(), 0);
    }

    #[tokio::test]
    async fn without_udp_relay_there_is_no_udp() {
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, "", &fixture);
        assert_eq!(out.udp(), UdpSupport::Unsupported);
        let Err(e) = out.open_udp(&ConnectOpts::default()).await else {
            panic!("a carrier without udp-relay");
        };
        assert_eq!(
            e.to_string(),
            "policy protocol not implemented: UDP without `udp-relay=true`"
        );
    }

    /// A stream idle past the bound closes and leaves the table; the
    /// target's next datagram opens a new one on the pooled connection.
    #[tokio::test]
    async fn an_idle_stream_closes_and_the_next_datagram_reopens() {
        let echo = udp_echo_server().await;
        let (fixture, fake) = udp_fake(H2ProxyScript::default()).await;
        let out = outbound(&fake, ", udp-relay=true", &fixture);
        let carrier = H2Udp::new(
            out.inner.clone(),
            fake.addr().to_string(),
            ConnectOpts::default(),
            Duration::from_millis(200),
        );
        udp_roundtrip(&carrier, &target(echo), b"first").await;
        assert_eq!(carrier.tracked(), 1);
        eventually(|| carrier.tracked() == 0).await;
        udp_roundtrip(&carrier, &target(echo), b"second").await;
        let carried: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
        assert_eq!(carried, [0, 0]);
    }

    #[test]
    fn udp_relay_needs_a_server_name_that_can_be_sent() {
        let fixture = TlsFixture::new(&["127.0.0.1"]);
        let spec = H2ConnectSpec {
            tls: rurge_config::spec::TlsOpts::default(),
            username: None,
            password: None,
            headers: Vec::new(),
            max_streams: 3,
            udp_relay: true,
        };
        let err = H2ConnectOutbound::new(
            "H",
            Target::new(HostName::Domain("a b.test".into()), 443),
            &spec,
            None,
            &[],
            fixture.roots(),
            Arc::new(DirectConnector::new(Arc::new(SystemResolve))),
        )
        .map(|_| ())
        .unwrap_err();
        assert_eq!(
            err.message,
            "`udp-relay`: the server name cannot be sent in a request"
        );
    }
}
```

新建 `crates/rurge-proto/src/h2connect/capsule.rs`：

```rust
//! Capsules (RFC 9297 3.2) as CONNECT-UDP carries them over HTTP/2 (RFC
//! 9298 5): `type (varint), length (varint), value`, back to back across
//! the stream's DATA frames. A DATAGRAM capsule (type 0) holds a context id
//! (a varint, 0 for a UDP payload) and the datagram. Integers are QUIC
//! varints (RFC 9000 16), which need not be minimal.

use std::io;
use tokio::io::{AsyncRead, AsyncReadExt};

/// The DATAGRAM capsule type (RFC 9297 3.5).
const DATAGRAM: u64 = 0;

/// The longest UDP payload with context id 0 (RFC 9298 5): no longer one
/// is sent, and a longer one received ends the stream.
pub(super) const MAX_PAYLOAD: usize = 65527;

/// The longest DATAGRAM capsule value read: an 8-byte context id and the
/// longest payload. Nothing longer is buffered.
const MAX_VALUE: u64 = 8 + MAX_PAYLOAD as u64;

/// `value` as a minimal varint. Values from 2^62 on cannot be written; the
/// callers write lengths of at most 64 KiB.
fn put_varint(out: &mut Vec<u8>, value: u64) {
    match value {
        0..=0x3f => out.push(value as u8),
        0x40..=0x3fff => out.extend_from_slice(&(value as u16 | 0x4000).to_be_bytes()),
        0x4000..=0x3fff_ffff => out.extend_from_slice(&(value as u32 | 0x8000_0000).to_be_bytes()),
        _ => {
            debug_assert!(value < 1 << 62, "a varint holds 62 bits");
            out.extend_from_slice(&(value | 0xc000_0000_0000_0000).to_be_bytes());
        }
    }
}

/// The varint at the start of `bytes` and its length; `None` when `bytes`
/// ends inside it.
fn parse_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let len = 1usize << (bytes.first()? >> 6);
    let bytes = bytes.get(..len)?;
    let value = bytes[1..]
        .iter()
        .fold(u64::from(bytes[0] & 0x3f), |v, &b| v << 8 | u64::from(b));
    Some((value, len))
}

fn malformed(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("h2-connect: a malformed capsule ({what})"),
    )
}

/// A varint from `r`; `None` at the end of the stream before its first
/// byte (`at_start`: the end is clean there).
async fn read_varint<R: AsyncRead + Unpin>(r: &mut R, at_start: bool) -> io::Result<Option<u64>> {
    let mut bytes = [0u8; 8];
    if r.read(&mut bytes[..1]).await? == 0 {
        return if at_start {
            Ok(None)
        } else {
            Err(malformed("truncated"))
        };
    }
    let len = 1usize << (bytes[0] >> 6);
    r.read_exact(&mut bytes[1..len])
        .await
        .map_err(|_| malformed("truncated"))?;
    Ok(parse_varint(&bytes[..len]).map(|(value, _)| value))
}

/// A DATAGRAM capsule with context id 0 carrying `payload` (at most
/// `MAX_PAYLOAD` bytes).
pub(super) fn datagram(payload: &[u8]) -> Vec<u8> {
    debug_assert!(payload.len() <= MAX_PAYLOAD);
    let mut out = Vec::with_capacity(payload.len() + 5);
    put_varint(&mut out, DATAGRAM);
    // the context id is one byte
    put_varint(&mut out, payload.len() as u64 + 1);
    put_varint(&mut out, 0);
    out.extend_from_slice(payload);
    out
}

/// The next UDP payload on the stream: capsules of other types and
/// datagrams with another context id are skipped (RFC 9297 3.2, RFC 9298
/// 4). `None` at a clean end, between two capsules. A capsule cut short,
/// a DATAGRAM capsule without a context id, or a payload longer than
/// `MAX_PAYLOAD` is an error (RFC 9297 3.3, RFC 9298 5): the stream is done.
pub(super) async fn read_datagram<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    loop {
        let Some(kind) = read_varint(r, true).await? else {
            return Ok(None);
        };
        let Some(len) = read_varint(r, false).await? else {
            return Err(malformed("truncated"));
        };
        if kind != DATAGRAM {
            // dropped without being buffered
            let skipped = tokio::io::copy(&mut (&mut *r).take(len), &mut tokio::io::sink()).await?;
            if skipped != len {
                return Err(malformed("truncated"));
            }
            continue;
        }
        if len > MAX_VALUE {
            return Err(malformed("a datagram too long"));
        }
        let mut value = vec![0u8; len as usize];
        r.read_exact(&mut value)
            .await
            .map_err(|_| malformed("truncated"))?;
        let Some((context, at)) = parse_varint(&value) else {
            return Err(malformed("no context id"));
        };
        if context != 0 {
            continue;
        }
        if value.len() - at > MAX_PAYLOAD {
            return Err(malformed("a datagram too long"));
        }
        value.drain(..at);
        return Ok(Some(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn read_all(mut bytes: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        let mut got = Vec::new();
        while let Some(payload) = read_datagram(&mut bytes).await? {
            got.push(payload);
        }
        Ok(got)
    }

    #[test]
    fn varints_are_written_minimal_and_read_in_every_length() {
        // RFC 9000 A.1
        for (value, wire) in [
            (37u64, &[0x25u8][..]),
            (15293, &[0x7b, 0xbd]),
            (494_878_333, &[0x9d, 0x7f, 0x3e, 0x7d]),
            (
                151_288_809_941_952_652,
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
            ),
        ] {
            let mut out = Vec::new();
            put_varint(&mut out, value);
            assert_eq!(out, wire, "{value}");
            assert_eq!(parse_varint(wire), Some((value, wire.len())));
        }
        // 37 in two bytes, as RFC 9000 A.1 shows: not minimal, still 37
        assert_eq!(parse_varint(&[0x40, 0x25]), Some((37, 2)));
        assert_eq!(parse_varint(&[0x80, 0, 0]), None, "cut short");
        assert_eq!(parse_varint(&[]), None);
    }

    #[test]
    fn a_datagram_capsule_is_type_length_context_payload() {
        assert_eq!(datagram(b"abc"), [0x00, 0x04, 0x00, b'a', b'b', b'c']);
        assert_eq!(datagram(b""), [0x00, 0x01, 0x00]);
        let long = datagram(&[7u8; 100]);
        assert_eq!(&long[..4], [0x00, 0x40, 101, 0x00]);
        assert_eq!(long.len(), 104);
        let longest = datagram(&vec![7u8; MAX_PAYLOAD]);
        assert_eq!(&longest[..6], [0x00, 0x80, 0x00, 0xff, 0xf8, 0x00]);
    }

    #[tokio::test]
    async fn datagrams_come_back_whole_and_in_order() {
        let mut wire = datagram(b"one");
        wire.extend(datagram(b""));
        wire.extend(datagram(&vec![9u8; MAX_PAYLOAD]));
        let got = read_all(&wire).await.unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!(
            (got[0].as_slice(), got[1].as_slice()),
            (&b"one"[..], &b""[..])
        );
        assert!(got[2] == vec![9u8; MAX_PAYLOAD]);
    }

    #[tokio::test]
    async fn non_minimal_varints_are_read() {
        // type 0 in 8 bytes, length 4 in 4 bytes, context id 0 in 2 bytes
        let wire = [
            0xc0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 4, 0x40, 0, b'h', b'i',
        ];
        assert_eq!(read_all(&wire).await.unwrap(), [b"hi".to_vec()]);
    }

    #[tokio::test]
    async fn other_capsule_types_and_context_ids_are_skipped() {
        let mut wire = Vec::new();
        // an unknown type (0x2a2a, two bytes), five bytes of value
        wire.extend([0x6a, 0x2a, 0x05, 1, 2, 3, 4, 5]);
        // context id 2: a client-allocated context nobody registered
        wire.extend([0x00, 0x03, 0x02, b'n', b'o']);
        // an unknown type with an empty value
        wire.extend([0x17, 0x00]);
        wire.extend(datagram(b"yes"));
        assert_eq!(read_all(&wire).await.unwrap(), [b"yes".to_vec()]);
    }

    #[tokio::test]
    async fn malformed_capsules_end_the_stream() {
        let too_long = {
            let mut wire = vec![0x00];
            put_varint(&mut wire, MAX_PAYLOAD as u64 + 2);
            wire.push(0x00);
            wire.extend(vec![0u8; MAX_PAYLOAD + 1]);
            wire
        };
        let over_the_bound = {
            let mut wire = vec![0x00];
            put_varint(&mut wire, MAX_VALUE + 1);
            wire
        };
        for (wire, what) in [
            (vec![0x00, 0x04, 0x00, b'a'], "truncated"),
            (vec![0x00], "truncated"),
            (vec![0x40], "truncated"),
            (vec![0x17, 0x09, 1, 2], "truncated"),
            (vec![0x00, 0x00], "no context id"),
            (vec![0x00, 0x01, 0x40], "no context id"),
            (too_long, "a datagram too long"),
            (over_the_bound, "a datagram too long"),
        ] {
            let err = read_all(&wire).await.unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidData, "{wire:?}");
            assert_eq!(
                err.to_string(),
                format!("h2-connect: a malformed capsule ({what})"),
                "{wire:?}"
            );
        }
    }
}
```

新建 `crates/rurge-proto/src/h2connect/udp.rs`：

```rust
//! `h2-connect` UDP (phase 2 M6 design 5.3, M6-D8): CONNECT-UDP (RFC 9298)
//! over HTTP/2, one stream per target, each datagram one DATAGRAM capsule.
//! A stream is bound to its target, so every datagram it brings back counts
//! as the target's (symmetric, as VMess UDP). A stream is opened with its
//! target's first datagram, on the outbound's pooled connections; one that
//! ends is opened again by the next datagram.

use super::Inner;
use super::capsule::{self, MAX_PAYLOAD};
use crate::OutboundError;
use crate::h2pool::H2Stream;
use crate::task::AbortOnDrop;
use rurge_config::HostName;
use rurge_net::BoxFuture;
use rurge_net::connector::{ConnectOpts, PacketSocket, Target};
use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, BufReader, WriteHalf};
use tokio::sync::{Mutex, OnceCell, mpsc};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Answers waiting for the engine; more are dropped, as a socket would.
const INBOX: usize = 64;

/// A target's stream closes after this long with neither a read nor a
/// write: the engine's flow idle (M5-D7), as VMess UDP. Its place on the
/// pooled connection is freed with it.
pub(super) const IDLE: Duration = Duration::from_secs(60);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// The default URI template's path for `target` (RFC 9298 2, 3): an IPv6
/// literal without brackets and with its colons percent-encoded, an IDN
/// as its A-labels. Names and IPv4 literals hold only unreserved
/// characters, so nothing else needs encoding. `None` for a name that
/// cannot be sent (the alphabet of `http::wire_host`).
pub(super) fn masque_path(target: &Target) -> Option<String> {
    let host = match &target.host {
        HostName::Ip(IpAddr::V6(v6)) => v6.to_string().replace(':', "%3A"),
        HostName::Ip(ip) => ip.to_string(),
        HostName::Domain(name) => crate::hostname::to_ascii(name)?,
    };
    Some(format!("/.well-known/masque/udp/{host}/{}/", target.port))
}

type Streams = std::sync::Mutex<HashMap<Target, Slot>>;

/// One target's stream.
struct Stream {
    id: u64,
    /// The last read or write; the idle clock.
    last: Arc<std::sync::Mutex<Instant>>,
    writer: Mutex<WriteHalf<H2Stream>>,
    /// Fires when the stream's reading ends: it is opened again.
    ended: CancellationToken,
    _reader: AbortOnDrop,
}

type Slot = Arc<OnceCell<Arc<Stream>>>;

pub(super) struct H2Udp {
    inner: Arc<Inner>,
    /// The server's own `:authority`.
    authority: String,
    opts: ConnectOpts,
    idle: Duration,
    streams: Arc<Streams>,
    answers: mpsc::Sender<(Vec<u8>, Target)>,
    inbox: Mutex<mpsc::Receiver<(Vec<u8>, Target)>>,
}

impl H2Udp {
    pub(super) fn new(
        inner: Arc<Inner>,
        authority: String,
        opts: ConnectOpts,
        idle: Duration,
    ) -> H2Udp {
        let (answers, inbox) = mpsc::channel(INBOX);
        H2Udp {
            inner,
            authority,
            opts,
            idle,
            streams: Arc::default(),
            answers,
            inbox: Mutex::new(inbox),
        }
    }

    /// `to`'s slot; a slot whose stream has ended is replaced.
    fn slot(&self, to: &Target) -> Slot {
        let mut streams = self.streams.lock().expect("streams");
        let slot = streams.entry(to.clone()).or_default();
        if slot.get().is_some_and(|s| s.ended.is_cancelled()) {
            *slot = Slot::default();
        }
        slot.clone()
    }

    #[cfg(test)]
    pub(super) fn tracked(&self) -> usize {
        self.streams.lock().expect("streams").len()
    }

    /// Forgets `slot` for `to`, unless another has taken its place.
    fn forget(&self, to: &Target, slot: &Slot) {
        let mut streams = self.streams.lock().expect("streams");
        if streams.get(to).is_some_and(|s| Arc::ptr_eq(s, slot)) {
            streams.remove(to);
        }
    }

    async fn open(&self, to: &Target) -> io::Result<Arc<Stream>> {
        // one budget for the connection (when one is dialed), TLS, the
        // HTTP/2 handshake and the CONNECT-UDP exchange
        let opened = tokio::time::timeout(
            self.opts.timeout,
            self.inner.udp_stream(&self.authority, to, &self.opts),
        )
        .await
        .unwrap_or(Err(OutboundError::Timeout));
        let stream = opened.map_err(|e| io::Error::other(e.to_string()))?;
        let (reader, writer) = tokio::io::split(stream);
        let ended = CancellationToken::new();
        let (answers, from, done) = (self.answers.clone(), to.clone(), ended.clone());
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let last = Arc::new(std::sync::Mutex::new(Instant::now()));
        let (clock, idle, streams) = (last.clone(), self.idle, Arc::downgrade(&self.streams));
        let task = tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let why = 'stream: loop {
                // a capsule is read whole: the idle clock must not cut one
                // in two, so the read lives on while the deadline moves
                let mut next = std::pin::pin!(capsule::read_datagram(&mut reader));
                let read = loop {
                    let deadline = *clock.lock().expect("clock") + idle;
                    tokio::select! {
                        read = &mut next => break read,
                        // a write may have moved the deadline: look again
                        _ = tokio::time::sleep_until(deadline) => {
                            if *clock.lock().expect("clock") + idle <= Instant::now() {
                                break 'stream "idle".to_string();
                            }
                        }
                    }
                };
                match read {
                    Ok(None) => break "the server ended it".to_string(),
                    // the text names the fault, never the payload
                    Err(e) => break format!("read error: {e}"),
                    Ok(Some(datagram)) => {
                        *clock.lock().expect("clock") = Instant::now();
                        // a full inbox drops the answer
                        let _ = answers.try_send((datagram, from.clone()));
                    }
                }
            };
            tracing::debug!("h2-connect: a UDP stream ended: {why}");
            done.cancel();
            // leave the table, unless a newer stream has taken the place
            if let Some(streams) = Weak::upgrade(&streams) {
                let mut streams = streams.lock().expect("streams");
                if streams
                    .get(&from)
                    .is_some_and(|s| s.get().is_some_and(|s| s.id == id))
                {
                    streams.remove(&from);
                }
            }
        });
        Ok(Arc::new(Stream {
            id,
            last,
            writer: Mutex::new(writer),
            ended,
            _reader: AbortOnDrop(task),
        }))
    }
}

impl PacketSocket for H2Udp {
    fn send_to<'a>(&'a self, buf: &'a [u8], to: &'a Target) -> BoxFuture<'a, io::Result<()>> {
        Box::pin(async move {
            // one datagram, one capsule (RFC 9298 5)
            if buf.len() > MAX_PAYLOAD {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("h2-connect: a datagram longer than {MAX_PAYLOAD} bytes"),
                ));
            }
            let (slot, stream) = loop {
                let slot = self.slot(to);
                let stream = match slot.get_or_try_init(|| self.open(to)).await {
                    Ok(stream) => stream.clone(),
                    Err(e) => {
                        self.forget(to, &slot);
                        return Err(e);
                    }
                };
                // A sender waiting on a cell whose opener failed opens
                // again, after that opener forgot the slot: the stream must
                // be reachable from the table or its answers are lost.
                let mut streams = self.streams.lock().expect("streams");
                // an ended stream is not revived: open again
                if stream.ended.is_cancelled() {
                    continue;
                }
                match streams.get(to) {
                    None => {
                        streams.insert(to.clone(), slot.clone());
                        break (slot, stream);
                    }
                    Some(s) if Arc::ptr_eq(s, &slot) => break (slot, stream),
                    // another slot took its place: use that one
                    Some(_) => {}
                }
            };
            *stream.last.lock().expect("clock") = Instant::now();
            let capsule = capsule::datagram(buf);
            let mut writer = stream.writer.lock().await;
            if let Err(e) = writer.write_all(&capsule).await {
                self.forget(to, &slot);
                return Err(e);
            }
            Ok(())
        })
    }

    /// An answer longer than `buf` is dropped: give it 64 KiB.
    fn recv_from<'a>(&'a self, buf: &'a mut [u8]) -> BoxFuture<'a, io::Result<(usize, Target)>> {
        Box::pin(async move {
            let mut inbox = self.inbox.lock().await;
            loop {
                // `self` holds a sender: the inbox never closes
                let Some((answer, from)) = inbox.recv().await else {
                    return Err(io::ErrorKind::BrokenPipe.into());
                };
                if let Some(space) = buf.get_mut(..answer.len()) {
                    space.copy_from_slice(&answer);
                    return Ok((answer.len(), from));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_carries_the_target_by_the_default_template() {
        for (host, path) in [
            (
                HostName::parse("192.0.2.6"),
                "/.well-known/masque/udp/192.0.2.6/443/",
            ),
            // RFC 9298 3's example
            (
                HostName::parse("2001:db8::42"),
                "/.well-known/masque/udp/2001%3Adb8%3A%3A42/443/",
            ),
            (
                HostName::parse("::ffff:192.0.2.6"),
                "/.well-known/masque/udp/%3A%3Affff%3A192.0.2.6/443/",
            ),
            (
                HostName::Domain("bücher.example".into()),
                "/.well-known/masque/udp/xn--bcher-kva.example/443/",
            ),
            (
                HostName::Domain("_srv.example.test".into()),
                "/.well-known/masque/udp/_srv.example.test/443/",
            ),
        ] {
            assert_eq!(
                masque_path(&Target::new(host.clone(), 443)).as_deref(),
                Some(path),
                "{host:?}"
            );
        }
        for name in ["x@blocked.test", "a/b.test", "a b.test", ""] {
            let target = Target::new(HostName::Domain(name.into()), 443);
            assert_eq!(masque_path(&target), None, "{name:?}");
        }
    }
}
```

要点：
- 读 capsule 时整个读完：空闲检查的 `select!` 保持同一个读 future，不会把一个 capsule 从中间截断。
- 错误文字 `h2-connect: a malformed capsule (<what>)`（`truncated` / `no context id` / `a datagram too long`），不带载荷。
- `udp-relay=true` 而服务器名发不出去时，建出站报 `` `udp-relay`: the server name cannot be sent in a request ``。

- [ ] **Step 3: 运行**

Run: `cargo test -p rurge-proto --lib h2connect`
Expected: 通过（新增 16 条：varint 与 capsule 的已知答案（含非最短 varint、跳过不认识的类型）、路径编码（IPv4 / IPv6 / 名字）、经 `FakeH2Proxy` 到回环 UDP 回显的往返、两个目标两条流、服务端没开 extended CONNECT 时的报错、不认识的 capsule 之后下一个数据报照常到达、太长的数据报、没有 `udp-relay`、每目标流空闲关闭后重开）。

- [ ] **Step 4: 门禁与提交**

跑门禁。

```bash
git add -A crates/rurge-proto
git commit -m "feat(proto): h2-connect 的 CONNECT-UDP——每个目标一条 extended CONNECT 流、DATAGRAM capsule"
```

### Task 6: 引擎装配与能力表

去掉 Task 1 的门，引擎工厂装上 `H2ConnectOutbound` / `TrustTunnelOutbound`（同 trojan 的分支：`server_of`、keystore、shadow-tls、根证书、直连或 `underlying-proxy` 的连接器），能力表加入两种协议。重载按指纹复用不需要改代码，用例证明参数没变时连 HTTP/2 连接也沿用。

**Files:**
- Create: `crates/rurge-engine/tests/outbounds_http2.rs`
- Modify: `crates/rurge-config/src/spec/mod.rs`（与用例）、`tests/policy_spec.rs`、`crates/rurge-engine/src/outbounds.rs`（与用例）、`tests/common/mod.rs`、`crates/rurge/src/capabilities.rs`、`crates/rurge/tests/cli.rs`

**Interfaces:**
- Consumes: Task 1 ～ 5 的全部。
- Produces: 工厂的两个分支；能力表的两种协议；`tests/common` 再导出 `FakeH2Proxy` / `H2ProxyScript`。

- [ ] **Step 1: 先写用例**

新建 `crates/rurge-engine/tests/outbounds_http2.rs`：

```rust
//! Sessions that leave through the HTTP/2 family (phase 2 M6 design 5):
//! `h2-connect` and `trust-tunnel` through the engine to the loopback
//! `FakeH2Proxy` — TCP tunnels, `max-streams`, refused credentials, UDP
//! over CONNECT-UDP (and where there is none), `underlying-proxy`, reloads.

mod common;
use common::*;
use rurge_config::session::Transport;
use rurge_engine::RequestRecord;

fn udp_records(h: &Harness) -> Vec<RequestRecord> {
    h.engine
        .request_log()
        .recent(4096)
        .into_iter()
        .filter(|r| r.transport == Transport::Udp)
        .collect()
}

/// A fake HTTP/2 proxy on loopback, and the parameter that makes rurge
/// trust it: the harness trusts the OS roots, so the leaf is pinned.
async fn h2_upstream(script: H2ProxyScript) -> (FakeH2Proxy, String) {
    let fixture = TlsFixture::new(&["127.0.0.1"]);
    let pin = format!("server-cert-fingerprint-sha256={}", pin_of(&fixture));
    (FakeH2Proxy::spawn(script, fixture).await, pin)
}

/// `P = <kind>, …` to `fake`, with `params` after the port.
fn line(kind: &str, fake: &FakeH2Proxy, params: &str) -> String {
    format!("P = {kind}, 127.0.0.1, {}, {params}", fake.addr().port())
}

async fn through(proxies: &str) -> Harness {
    harness(Profile {
        proxies,
        rules: "DOMAIN,target.test,P\nIP-CIDR,127.0.0.1/32,P,no-resolve",
        ..Profile::default()
    })
    .await
}

/// The first `n` sessions' records, oldest first, once they have finished.
async fn the_records(h: &Harness, n: usize) -> Vec<RequestRecord> {
    let log = h.engine.request_log();
    wait_until("the sessions to finish", || log.recent(10).len() >= n).await;
    let mut records = log.recent(10);
    records.reverse();
    records
}

/// One session to the echo through `h`: a round trip, then the client
/// goes and the session finishes.
async fn one_session(h: &Harness, n: usize, payload: &[u8]) -> RequestRecord {
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, payload).await;
    drop(tunnel);
    the_records(h, n).await[n - 1].clone()
}

/// Opens a session the proxy refuses: the client's CONNECT is answered
/// with 502 within the bound, and the record says why.
async fn a_failed_session(h: &Harness) -> RequestRecord {
    let mut s = TcpStream::connect(h.http()).await.unwrap();
    s.write_all(b"CONNECT target.test:7 HTTP/1.1\r\nHost: target.test:7\r\n\r\n")
        .await
        .unwrap();
    let mut answer = [0u8; 12];
    tokio::time::timeout(Duration::from_secs(10), s.read_exact(&mut answer))
        .await
        .expect("the CONNECT is answered within the bound")
        .unwrap();
    assert_eq!(
        &answer,
        b"HTTP/1.1 502",
        "{}",
        String::from_utf8_lossy(&answer)
    );
    let record = the_records(h, 1).await[0].clone();
    assert_eq!(record.status, RecordStatus::Failed, "{record:?}");
    record
}

/// `h2-connect` carries a tunnel to the echo: one CONNECT naming the
/// target, the name left to the proxy, Basic credentials sent.
#[tokio::test]
async fn a_connect_leaves_through_h2_connect() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("alice".into(), "s3cret".into())],
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line("h2-connect", &fake, &format!("alice, s3cret, {pin}"))).await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"through h2-connect").await;
    echo_through(&mut tunnel, &vec![0x5a; 300_000]).await;
    let seen = fake.requests();
    assert_eq!(
        (seen[0].method.as_str(), seen[0].authority.as_str()),
        ("CONNECT", "target.test:7"),
        "the proxy resolves the name"
    );
    assert!(h.dns.queries().is_empty(), "rurge never looked the name up");
    drop(tunnel);
    let record = the_records(&h, 1).await[0].clone();
    assert_eq!(record.policy, ["P"]);
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
}

/// Sessions open at the same time share a connection up to `max-streams`;
/// the next one takes another TLS connection.
#[tokio::test]
async fn max_streams_bounds_the_sessions_on_one_connection() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line("h2-connect", &fake, &format!("max-streams=2, {pin}"))).await;
    let mut tunnels = Vec::new();
    for n in 0..3u8 {
        let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
        echo_through(&mut tunnel, &[n; 16]).await;
        tunnels.push(tunnel);
    }
    // every tunnel is still open
    for tunnel in &mut tunnels {
        echo_through(tunnel, b"still here").await;
    }
    assert_eq!(fake.connections(), 2);
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0, 1]);
}

/// Credentials the proxy refuses fail the session with the text that says
/// so, and the password is nowhere in the record.
#[tokio::test]
async fn refused_credentials_fail_the_session() {
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("alice".into(), "right".into())],
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "h2-connect",
        &fake,
        &format!("alice, wr0ngPw, {pin}"),
    ))
    .await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("h2-connect: proxy authentication required")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
}

/// `trust-tunnel` carries a tunnel with its credentials and `user-agent`.
#[tokio::test]
async fn a_connect_leaves_through_trust_tunnel() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("u".into(), "s3cret".into())],
        require_user_agent: true,
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "trust-tunnel",
        &fake,
        &format!("username=u, password=s3cret, {pin}"),
    ))
    .await;
    let record = one_session(&h, 1, b"through trust-tunnel").await;
    assert_eq!(record.status, RecordStatus::Completed, "{record:?}");
    let seen = fake.requests();
    assert_eq!(seen[0].authority, "target.test:7");
    assert_eq!(seen[0].header("user-agent"), Some("rurge"));
}

#[tokio::test]
async fn a_refused_trust_tunnel_login_fails_the_session() {
    let (fake, pin) = h2_upstream(H2ProxyScript {
        users: vec![("u".into(), "right".into())],
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "trust-tunnel",
        &fake,
        &format!("username=u, password=wr0ngPw, {pin}"),
    ))
    .await;
    let record = a_failed_session(&h).await;
    assert_eq!(
        record.error.as_deref(),
        Some("trust-tunnel: authentication failed")
    );
    assert!(!format!("{record:?}").contains("wr0ngPw"));
}

/// `trust-tunnel` carries no UDP: `udp-policy-not-supported-behaviour`
/// (REJECT by default) decides, and nothing reaches the server.
#[tokio::test]
async fn trust_tunnel_carries_no_udp() {
    let (fake, pin) = h2_upstream(H2ProxyScript::default()).await;
    let h = through(&line(
        "trust-tunnel",
        &fake,
        &format!("username=u, password=p, {pin}"),
    ))
    .await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", 53, b"q").await;
    assert!(association.quiet_for(Duration::from_millis(300)).await);
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Rejected("REJECT".into()));
    assert_eq!(
        records[0].error.as_deref(),
        Some("policy does not support UDP")
    );
    assert_eq!(fake.connections(), 0);
}

/// Datagrams to two echoes through `h2-connect` over one association:
/// each target has its own CONNECT-UDP stream, both on one connection,
/// and every flow ends with the association.
#[tokio::test]
async fn udp_goes_through_h2_connect_as_connect_udp() {
    let (fake, pin) = h2_upstream(H2ProxyScript {
        extended_connect: true,
        ..H2ProxyScript::default()
    })
    .await;
    let h = through(&line(
        "h2-connect",
        &fake,
        &format!("udp-relay=true, {pin}"),
    ))
    .await;
    let ((one, _), (two, _)) = (udp_echo().await, udp_echo().await);
    let association = udp_associate(h.socks()).await;
    for (echo, payload) in [(one, &b"one"[..]), (two, b"two"), (one, b"again")] {
        association.send("127.0.0.1", echo.port(), payload).await;
        assert_eq!(association.recv().await, (echo, payload.to_vec()));
    }
    drop(association);
    wait_until("both flows to finish", || udp_records(&h).len() == 2).await;
    for r in udp_records(&h) {
        assert_eq!(r.policy, ["P"], "{r:?}");
        assert_eq!(r.status, RecordStatus::Completed, "{r:?}");
    }
    let streams: Vec<(Option<String>, String)> = fake
        .requests()
        .into_iter()
        .map(|r| (r.protocol, r.path))
        .collect();
    assert_eq!(
        streams,
        [one, two].map(|echo| (
            Some("connect-udp".to_string()),
            format!("/.well-known/masque/udp/127.0.0.1/{}/", echo.port())
        ))
    );
    assert_eq!(fake.connections(), 1);
}

/// A server without extended CONNECT carries no UDP: the flow fails with
/// the text that says so, and no request reaches the server.
#[tokio::test]
async fn udp_without_extended_connect_fails_the_flow() {
    let (fake, pin) = h2_upstream(H2ProxyScript::default()).await;
    let h = through(&line(
        "h2-connect",
        &fake,
        &format!("udp-relay=true, {pin}"),
    ))
    .await;
    let (echo, _) = udp_echo().await;
    let association = udp_associate(h.socks()).await;
    association.send("127.0.0.1", echo.port(), b"q").await;
    wait_until("the flow to finish", || !udp_records(&h).is_empty()).await;
    let records = udp_records(&h);
    assert_eq!(records[0].status, RecordStatus::Failed, "{:?}", records[0]);
    assert_eq!(
        records[0].error.as_deref(),
        Some("h2-connect: the server does not support extended CONNECT")
    );
    assert!(fake.requests().is_empty());
}

/// `h2-connect` over a SOCKS5 `underlying-proxy`: the entry is asked for
/// the HTTP/2 server, and the session goes through its tunnel.
#[tokio::test]
async fn h2_connect_goes_through_an_underlying_socks5_proxy() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let entry = FakeSocks5::spawn(Socks5Script::default()).await;
    let h = through(&format!(
        "Entry = socks5, 127.0.0.1, {}\n{}",
        entry.addr().port(),
        line(
            "h2-connect",
            &fake,
            &format!("underlying-proxy=Entry, {pin}")
        )
    ))
    .await;
    let mut tunnel = connect_via_http(h.http(), "target.test:7").await;
    echo_through(&mut tunnel, b"socks5 then h2").await;
    let first = entry.requests()[0].clone();
    assert_eq!(
        (first.command, first.host.as_str(), first.port),
        (1, "127.0.0.1", fake.addr().port())
    );
    assert_eq!(fake.requests()[0].authority, "target.test:7");
}

/// The outbound — and with it its HTTP/2 connection — is kept across a
/// reload that leaves the line alone, and rebuilt when its credentials,
/// `headers`, `max-streams` or `udp-relay` change (design 6).
#[tokio::test]
async fn a_reload_keeps_an_unchanged_policy_and_rebuilds_a_changed_one() {
    let echo = rurge_proto::testing::echo_server().await;
    let (fake, pin) = h2_upstream(H2ProxyScript {
        connect_to: Some(echo),
        ..H2ProxyScript::default()
    })
    .await;
    let proxies = |params: &str, extra: &str| {
        format!(
            "{}\n{extra}",
            line("h2-connect", &fake, &format!("{params}, {pin}"))
        )
    };
    let base = "alice, s3cret";
    let h = through(&proxies(base, "")).await;
    let reload = |params: &str, extra: &str| {
        let next = Profile {
            proxies: &proxies(params, extra),
            rules: "DOMAIN,target.test,P",
            ..Profile::default()
        }
        .text(h.dns.addr());
        let dir = h.dir.path().to_path_buf();
        let shared = h.engine.shared();
        async move { runtime(&dir, &next, shared).await }
    };
    one_session(&h, 1, b"before the reload").await;
    let before = outbound_now(&h, "P");
    h.engine
        .swap_runtime(reload(base, "Other = http, other.example, 8080").await);
    assert!(
        Arc::ptr_eq(&before, &outbound_now(&h, "P")),
        "an unrelated reload rebuilt P"
    );
    one_session(&h, 2, b"after the reload").await;
    assert_eq!(fake.connections(), 1, "the HTTP/2 connection was kept");
    let places: Vec<usize> = fake.requests().iter().map(|r| r.connection).collect();
    assert_eq!(places, [0, 0]);

    let mut previous = before;
    for changed in [
        "alice, 0ther",
        "alice, 0ther, headers=X-Id:1",
        "alice, 0ther, headers=X-Id:1, max-streams=8",
        "alice, 0ther, headers=X-Id:1, max-streams=8, udp-relay=true",
    ] {
        h.engine.swap_runtime(reload(changed, "").await);
        let now = outbound_now(&h, "P");
        assert!(!Arc::ptr_eq(&previous, &now), "kept after: {changed}");
        previous = now;
    }
}
```

`crates/rurge-engine/tests/common/mod.rs`——把

```rust
    AnyTlsScript, FakeAnyTls, FakeHttpProxy, FakeShadowsocks, FakeSnell, FakeSocks5, FakeTrojan,
    FakeVmess, HttpProxyScript, ShadowsocksScript, SnellScript, Socks5Script, TlsFixture,
    TrojanScript, VmessScript,
```

换成

```rust
    AnyTlsScript, FakeAnyTls, FakeH2Proxy, FakeHttpProxy, FakeShadowsocks, FakeSnell, FakeSocks5,
    FakeTrojan, FakeVmess, H2ProxyScript, HttpProxyScript, ShadowsocksScript, SnellScript,
    Socks5Script, TlsFixture, TrojanScript, VmessScript,
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
N5 = snell, proxy.test, 443, psk=pw, version=5, udp-port=8443, shadow-tls-password=st\n\
```

换成

```rust
N5 = snell, proxy.test, 443, psk=pw, version=5, udp-port=8443, shadow-tls-password=st\n\
H2 = h2-connect, proxy.test, 443, alice, s3cret, max-streams=8, udp-relay=true\n\
TT = trust-tunnel, proxy.test, 443, username=u, password=pw, shadow-tls-password=st\n\
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            ("N5", "N5"),
```

换成

```rust
            ("N5", "N5"),
            ("H2", "H2"),
            ("TT", "TT"),
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert!(dry_build(&cfg).is_empty());
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

换成

```rust
        assert!(dry_build(&cfg).is_empty());
    }

    /// Sound `h2-connect` / `trust-tunnel` lines pass the dry build, which
    /// opens no connection (no tokio runtime here); a client certificate
    /// that cannot be read is a load error, never quoted (phase 2 M6 design
    /// 5.1).
    #[test]
    fn the_http2_policies_pass_the_dry_build() {
        let cfg = config(
            "[Proxy]\nH2 = h2-connect, proxy.test, 443, alice, s3cret, headers=X-Id:<random-string(8)>, max-streams=8, udp-relay=true\n\
TT = trust-tunnel, proxy.test, 443, username=u, password=pw, sni=tt.test, shadow-tls-password=st, shadow-tls-version=3, shadow-tls-sni=site.test\n\
[Rule]\nFINAL,DIRECT\n",
        );
        assert!(dry_build(&cfg).is_empty());

        let cfg = config(
            "[Proxy]\nH2 = h2-connect, proxy.test, 443, client-cert=cert1\n\
TT = trust-tunnel, proxy.test, 443, username=u, password=s3same0pen, client-cert=cert1\n\
[Keystore]\ncert1 = type=p12, base64=QUJD, password=hunter2\n[Rule]\nFINAL,DIRECT\n",
        );
        let diags = dry_build(&cfg).sorted();
        let messages: Vec<String> = diags.iter().map(|d| d.message.clone()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("policy `H2` cannot be built: keystore item `cert1`"));
        assert!(messages[1].starts_with("policy `TT` cannot be built: keystore item `cert1`"));
        for m in &messages {
            assert!(!m.contains("hunter2") && !m.contains("s3same0pen"), "{m}");
        }
    }

    #[test]
    fn a_dry_build_of_an_anytls_policy_leaves_no_task_behind() {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
Plain = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true\n[Rule]\nFINAL,DIRECT\n",
```

换成

```rust
Plain = vmess, proxy.test, 80, username=0233d11c-15a4-47d3-ade3-48ffca0ce119, vmess-aead=true\n\
H2 = h2-connect, proxy.test, 443, skip-cert-verify=true\n\
TT = trust-tunnel, proxy.test, 443, username=u, password=pw, skip-cert-verify=true\n\
TP = trust-tunnel, proxy.test, 443, username=u, password=pw, skip-cert-verify=true, server-cert-fingerprint-sha256=0000000000000000000000000000000000000000000000000000000000000000\n\
[Rule]\nFINAL,DIRECT\n",
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
        assert_eq!(flagged, ["A", "V"]);
```

换成

```rust
        assert_eq!(flagged, ["A", "V", "H2", "TT"]);
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    /// `h2-connect` and `trust-tunnel` lines are read and checked in full;
    /// they have no spec until the engine builds them (M6c task 6).
    #[test]
    fn h2_connect_and_trust_tunnel_lines_are_checked_but_have_no_spec_yet() {
```

换成

```rust
    /// `h2-connect` and `trust-tunnel` lines are read and checked in full,
    /// and a sound one has a spec (phase 2 M6 design 5.1).
    #[test]
    fn h2_connect_and_trust_tunnel_lines_are_checked_and_have_a_spec() {
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
            assert!(o.spec.is_none() && o.not_implemented.is_none(), "{def}");
        }
```

换成

```rust
            assert_eq!(o.not_implemented, None, "{def}");
            let spec = o.spec.unwrap_or_else(|| panic!("{def}: no spec"));
            assert!(
                matches!(
                    spec.proto,
                    ProtoSpec::H2Connect(_) | ProtoSpec::TrustTunnel(_)
                ),
                "{def}"
            );
            let layered = def.contains("shadow-tls-password");
            assert_eq!(spec.shadow_tls.is_some(), layered, "{def}");
            assert_eq!(spec.common.underlying_proxy.is_some(), layered, "{def}");
        }
        // `h3=true` is inert: the policy still has a spec and uses HTTP/2
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
        assert_eq!(o.inert, ["h3"]);
```

换成

```rust
        assert_eq!(o.inert, ["h3"]);
        assert!(matches!(
            o.spec.map(|s| s.proto),
            Some(ProtoSpec::TrustTunnel(_))
        ));
```

`crates/rurge-config/src/spec/mod.rs`——把

```rust
                ),
            ]
        );
        // both always run over TLS: the client certificate is theirs
```

换成

```rust
                ),
            ]
        );
        assert!(o.spec.is_none());
        // both always run over TLS: the client certificate is theirs
```

`crates/rurge-config/tests/policy_spec.rs`——把

```rust
            2
        )]
    );
}

#[test]
```

换成

```rust
            2
        )]
    );
    assert!(loaded.config.spec("A").is_some() && loaded.config.spec("B").is_some());
}

#[test]
```

`crates/rurge/tests/cli.rs`——把

```rust
        .stdout(predicate::str::contains("s3cretPsk").not());
```

换成

```rust
        .stdout(predicate::str::contains("s3cretPsk").not());
}

const HTTP2: &str = "[General]\n[Proxy]\n\
H = h2-connect, proxy.test, 443, alice, s3cretPw, headers=X-Token:t0kenValue, max-streams=5, udp-relay=true\n\
T1 = trust-tunnel, proxy.test, 443, username=bob, password=s3cretPw, h3=true\n\
T2 = trust-tunnel, proxy.test, 443, username=bob, password=s3cretPw, h3=true\n\
Old = hysteria2, 1.2.3.4, 443, password=x\n[Rule]\nFINAL,DIRECT\n";
const HTTP2_NO_PASSWORD: &str = "[General]\n[Proxy]\n\
T = trust-tunnel, proxy.test, 443, username=bob\n[Rule]\nFINAL,DIRECT\n";

/// `rurge check` knows `h2-connect` and `trust-tunnel` (phase 2 M6 design
/// 5.1): neither is "not implemented"; `h3=true` is parsed but has no
/// effect, said once however many lines use it; a `trust-tunnel` without a
/// password is an error; credentials and headers are never printed.
#[test]
fn check_knows_http2() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "http2.conf", HTTP2))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8_lossy(&out);
    // `hysteria2` is still a later milestone; the HTTP/2 family is not
    assert_eq!(out.matches("W0007").count(), 1, "{out}");
    assert!(
        out.contains("`hysteria2`")
            && !out.contains("`h2-connect`")
            && !out.contains("`trust-tunnel`"),
        "{out}"
    );
    assert_eq!(out.matches("W0029").count(), 1, "{out}");
    assert!(
        out.contains("http2.conf:4")
            && out.contains("policy parameter `h3` is parsed but has no effect in this version"),
        "{out}"
    );
    assert!(
        !out.contains("s3cretPw") && !out.contains("alice") && !out.contains("t0kenValue"),
        "{out}"
    );

    Command::cargo_bin("rurge")
        .unwrap()
        .args(["check", "-c"])
        .arg(write(&dir, "bad.conf", HTTP2_NO_PASSWORD))
        .assert()
        .code(2)
        .stdout(predicate::str::contains("E0018"))
        .stdout(predicate::str::contains(
            "bad.conf:3: policy `T`: `password` is required",
        ))
        .stdout(predicate::str::contains("bob").not());
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p rurge-engine --test outbounds_http2`
Expected: FAIL——两种行还没有 spec、工厂还不会构建，经它们的会话都被拒绝：

```text
test max_streams_bounds_the_sessions_on_one_connection ... FAILED
test a_refused_trust_tunnel_login_fails_the_session ... FAILED
test a_connect_leaves_through_h2_connect ... FAILED
test refused_credentials_fail_the_session ... FAILED
test h2_connect_goes_through_an_underlying_socks5_proxy ... FAILED
test a_connect_leaves_through_trust_tunnel ... FAILED
test udp_without_extended_connect_fails_the_flow ... FAILED
test a_reload_keeps_an_unchanged_policy_and_rebuilds_a_changed_one ... FAILED
test trust_tunnel_carries_no_udp ... FAILED
test udp_goes_through_h2_connect_as_connect_udp ... FAILED
thread 'max_streams_bounds_the_sessions_on_one_connection' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'a_refused_trust_tunnel_login_fails_the_session' panicked at crates\rurge-engine\tests\outbounds_http2.rs:71:10:
called `Result::unwrap()` on an `Err` value: Custom { kind: UnexpectedEof, error: "early eof" }
thread 'a_connect_leaves_through_h2_connect' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'refused_credentials_fail_the_session' panicked at crates\rurge-engine\tests\outbounds_http2.rs:71:10:
called `Result::unwrap()` on an `Err` value: Custom { kind: UnexpectedEof, error: "early eof" }
thread 'h2_connect_goes_through_an_underlying_socks5_proxy' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'a_connect_leaves_through_trust_tunnel' panicked at crates\rurge-engine\tests\common\mod.rs:176:9:
thread 'udp_without_extended_connect_fails_the_flow' panicked at crates\rurge-engine\tests\outbounds_http2.rs:288:5:
assertion `left == right` failed: RequestRecord { id: 1, listener: Socks5, transport: Udp, src: 127.0.0.1:11093, dst: "127.0.0.1:52144", rule: Some("IP-CIDR,127.0.0.1/32,P,no-resolve"), policy: ["P", "!unsupported:h2-connect", "REJECT"], sni: None, protocol: Some(Udp), up: 0, down: 0, started_ms: 1790833208583, elapsed_ms: 0, connect_ms: None, first_byte_ms: None, status: Rejected("REJECT"), error: Some("policy protocol not implemented: h2-connect") }
```

- [ ] **Step 3: 实现**

`crates/rurge-config/src/spec/mod.rs`——把

```rust
    // the engine builds `h2-connect` and `trust-tunnel` from M6c task 6 on:
    // until then a valid line is checked in full but has no spec
    let built = !matches!(policy.kind, PolicyKind::H2Connect | PolicyKind::TrustTunnel);
    let spec = (!failed && not_implemented.is_none() && built).then(|| PolicySpec {
```

换成

```rust
    let spec = (!failed && not_implemented.is_none()).then(|| PolicySpec {
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessHook};
```

换成

```rust
use rurge_proto::external::{ExternalOutbound, NoProcessGroups, ProcessHook};
use rurge_proto::h2connect::H2ConnectOutbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
use rurge_proto::trojan::TrojanOutbound;
```

换成

```rust
use rurge_proto::trojan::TrojanOutbound;
use rurge_proto::trust_tunnel::TrustTunnelOutbound;
```

`crates/rurge-engine/src/outbounds.rs`——把

```rust
            // the loader makes no spec of these lines before M6c task 6
            ProtoSpec::H2Connect(_) | ProtoSpec::TrustTunnel(_) => {
                return Err(BuildError::new(format!(
                    "policy `{}`: `{}` is not implemented yet",
                    spec.name,
                    spec.kind.keyword()
                )));
            }
```

换成

```rust
            ProtoSpec::H2Connect(h2) => Arc::new(H2ConnectOutbound::new(
                &spec.name,
                server_of(spec)?,
                h2,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
            ProtoSpec::TrustTunnel(tt) => Arc::new(TrustTunnelOutbound::new(
                &spec.name,
                server_of(spec)?,
                tt,
                spec.shadow_tls.as_ref(),
                &self.keystore,
                self.roots.clone(),
                connector,
            )?),
```

`crates/rurge/src/capabilities.rs`——把

```rust
//! M6a), `snell` versions 4 and 5 (phase 2 M6b), `select` groups,
```

换成

```rust
//! M6a), `snell` versions 4 and 5 (phase 2 M6b), `h2-connect` and
//! `trust-tunnel` over HTTP/2 only (phase 2 M6c), `select` groups,
```

`crates/rurge/src/capabilities.rs`——把

```rust
            PolicyKind::Snell,
```

换成

```rust
            PolicyKind::Snell,
            PolicyKind::H2Connect,
            PolicyKind::TrustTunnel,
```

要点：出站失败时引擎对 CONNECT 回 `HTTP/1.1 502` 而不关闭客户端连接，所以失败用例读状态行，而不是等连接关闭。

- [ ] **Step 4: 运行，确认通过**

Run: `cargo test -p rurge-engine --test outbounds_http2` → 通过（10 条：`h2-connect` 的连接、`max-streams` 下的连接数、凭据被拒、`trust-tunnel` 的连接与登录被拒、`trust-tunnel` 不载 UDP、经 SOCKS5 UDP ASSOCIATE 的 CONNECT-UDP、服务端没开 extended CONNECT 时 UDP 流失败、经 `underlying-proxy`（socks5）的 TCP、重载时沿用与重建）。
Run: `cargo test -p rurge --test cli check_knows_http2` → 通过。

- [ ] **Step 5: 门禁与提交**

跑门禁。

```bash
git add crates
git commit -m "feat(engine): 装配 h2-connect 与 trust-tunnel、能力表翻转"
```

### Task 7: 互操作与文档

TrustTunnel endpoint v1.1.0 的夹具（P10：同时充当 `trust-tunnel` 与 `h2-connect` 普通 CONNECT 的参考服务端，只在 Linux / macOS CI 安装）；兼容性清单、手工验收、两份 README、`CLAUDE.md` 与总设计。

**Files:**
- Create: `tests/interop/src/trusttunnel.rs`（自带用例）、`tests/interop/tests/http2.rs`
- Modify: `tests/interop/src/lib.rs`、`tests/interop/README.md`、`.github/workflows/ci.yml`、`docs/surge-compatibility-matrix.md`、`docs/acceptance/phase2-manual.md`、`README.md`、`README_en.md`、`CLAUDE.md`、`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`

**Interfaces:**
- Consumes: 互操作夹具既有的 `Reference`、`roundtrip` / `roundtrip_big`、`outbound(profile, name, fixture)`、测试 TLS 夹具。
- Produces: `rurge_interop::trusttunnel`（`RURGE_TEST_TRUSTTUNNEL`；配置只监听 `127.0.0.1`）。

- [ ] **Step 1: 互操作夹具与用例**

`tests/interop/src/lib.rs`——把

```rust
pub mod sshd;
```

换成

```rust
pub mod sshd;
pub mod trusttunnel;
```

新建 `tests/interop/src/trusttunnel.rs`：

```rust
//! The TrustTunnel endpoint v1.1.0 as a child process on the loopback: the
//! reference for `trust-tunnel` and, through the same TCP mode (a standard
//! HTTP/2 CONNECT with Basic authentication), for `h2-connect`'s plain
//! CONNECT — sing-box 1.14.2's `http` inbound speaks HTTP/1 only. It is
//! built for Linux and macOS only: on Windows a missing binary is always a
//! skip, even with `RURGE_INTEROP_REQUIRED=1`. The same rules as for sing-box
//! apply: nothing is downloaded or installed here, and the rendered
//! configuration listens on 127.0.0.1 only.
//!
//! The endpoint picks its host by the exact SNI, so a client sends
//! `sni=<HOSTNAME>` and trusts a certificate for that name; it refuses
//! private and loopback targets unless `allow_private_network_connections`
//! is set, which the loopback echo needs.

use crate::{REQUIRED_ENV, Reference, TlsFiles, free_port};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BINARY_ENV: &str = "RURGE_TEST_TRUSTTUNNEL";

/// The endpoint's one host: the SNI a client must send and the name its
/// certificate is for.
pub const HOSTNAME: &str = "tt.test";

/// `RURGE_TEST_TRUSTTUNNEL`, else the first `trusttunnel_endpoint` on `PATH`.
pub fn locate() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(BINARY_ENV).filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("trusttunnel_endpoint"))
        .find(|candidate| candidate.is_file())
}

/// The binary — or `None` after saying why `test` is skipped. With
/// `RURGE_INTEROP_REQUIRED=1` (CI) a missing binary is a failure on Linux
/// and macOS, the platforms the endpoint is built for.
pub fn trusttunnel_or_skip(test: &str) -> Option<PathBuf> {
    if let Some(path) = locate() {
        return Some(path);
    }
    if cfg!(any(target_os = "linux", target_os = "macos"))
        && std::env::var(REQUIRED_ENV).as_deref() == Ok("1")
    {
        panic!("{REQUIRED_ENV}=1 but no trusttunnel_endpoint was found ({BINARY_ENV} or PATH)");
    }
    eprintln!(
        "skipping {test}: no trusttunnel_endpoint ({BINARY_ENV} or PATH; Linux and macOS only); see tests/interop/README.md"
    );
    None
}

/// The endpoint's three files.
#[derive(Debug, PartialEq, Eq)]
pub struct Rendered {
    /// `vpn.toml`: the listener, the protocols and the forwarding.
    pub vpn: String,
    /// `hosts.toml`: [`HOSTNAME`] with the certificate and key of `tls`.
    pub hosts: String,
    /// `credentials.toml`: one client.
    pub credentials: String,
}

/// A TOML basic string.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn path(p: &Path) -> String {
    quoted(&p.to_string_lossy())
}

/// The whole configuration: HTTP/2 only on `127.0.0.1:port`, IPv4 only,
/// direct forwarding that may reach the loopback, one client. The endpoint
/// resolves file paths against its working directory, so they must be
/// absolute.
pub fn render(
    port: u16,
    tls: &TlsFiles,
    credentials: &Path,
    username: &str,
    password: &str,
) -> Rendered {
    let vpn = format!(
        "listen_address = \"127.0.0.1:{port}\"\n\
         ipv6_available = false\n\
         allow_private_network_connections = true\n\
         credentials_file = {}\n\
         \n\
         [listen_protocols.http2]\n\
         \n\
         [forward_protocol]\n\
         direct = {{}}\n",
        path(credentials)
    );
    let hosts = format!(
        "[[main_hosts]]\n\
         hostname = {}\n\
         cert_chain_path = {}\n\
         private_key_path = {}\n",
        quoted(HOSTNAME),
        path(&tls.certificate),
        path(&tls.key)
    );
    let credentials = format!(
        "[[client]]\nusername = {}\npassword = {}\n",
        quoted(username),
        quoted(password)
    );
    Rendered {
        vpn,
        hosts,
        credentials,
    }
}

/// A running endpoint; killed and reaped on drop.
pub struct TrustTunnel(Reference);

impl TrustTunnel {
    /// Writes the configuration into `dir` (which must be absolute), starts
    /// `binary` there and waits until it accepts connections. `tls` is a
    /// certificate for [`HOSTNAME`].
    pub fn spawn(
        binary: &Path,
        dir: &Path,
        tls: &TlsFiles,
        username: &str,
        password: &str,
    ) -> TrustTunnel {
        let port = free_port();
        let (vpn, hosts, credentials) = (
            dir.join("vpn.toml"),
            dir.join("hosts.toml"),
            dir.join("credentials.toml"),
        );
        let rendered = render(port, tls, &credentials, username, password);
        for (file, text) in [
            (&vpn, &rendered.vpn),
            (&hosts, &rendered.hosts),
            (&credentials, &rendered.credentials),
        ] {
            std::fs::write(file, text).expect("write the config");
        }
        let mut command = Command::new(binary);
        command.arg(&vpn).arg(&hosts).current_dir(dir);
        TrustTunnel(Reference::start(
            "trusttunnel_endpoint",
            command,
            vec![port],
            dir.join("trusttunnel.log"),
        ))
    }

    /// The loopback port.
    pub fn port(&self) -> u16 {
        self.0.port(0)
    }

    pub fn log_text(&self) -> String {
        self.0.log_text()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> TlsFiles {
        TlsFiles {
            certificate: "/tmp/tt/leaf.pem".into(),
            key: "/tmp/tt/leaf.key".into(),
            client_ca: None,
        }
    }

    #[test]
    fn the_configuration_stays_on_the_loopback() {
        let rendered = render(
            4001,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "pw",
        );
        let text = format!("{}{}{}", rendered.vpn, rendered.hosts, rendered.credentials);
        for forbidden in ["0.0.0.0", "::", "socks5", "listen_protocols.quic", "http1"] {
            assert!(!text.contains(forbidden), "`{forbidden}` in {text}");
        }
        let listen: Vec<&str> = rendered
            .vpn
            .lines()
            .filter(|l| l.starts_with("listen_address"))
            .collect();
        assert_eq!(listen, ["listen_address = \"127.0.0.1:4001\""]);
        assert!(rendered.vpn.contains("[forward_protocol]\ndirect = {}\n"));
    }

    #[test]
    fn the_configuration_is_written_as_the_endpoint_reads_it() {
        let rendered = render(
            4001,
            &files(),
            Path::new("/tmp/tt/credentials.toml"),
            "u",
            "p\"w",
        );
        assert_eq!(
            rendered,
            Rendered {
                vpn: "listen_address = \"127.0.0.1:4001\"\n\
                      ipv6_available = false\n\
                      allow_private_network_connections = true\n\
                      credentials_file = \"/tmp/tt/credentials.toml\"\n\
                      \n\
                      [listen_protocols.http2]\n\
                      \n\
                      [forward_protocol]\n\
                      direct = {}\n"
                    .into(),
                hosts: "[[main_hosts]]\n\
                        hostname = \"tt.test\"\n\
                        cert_chain_path = \"/tmp/tt/leaf.pem\"\n\
                        private_key_path = \"/tmp/tt/leaf.key\"\n"
                    .into(),
                credentials: "[[client]]\nusername = \"u\"\npassword = \"p\\\"w\"\n".into(),
            }
        );
    }

    #[test]
    fn windows_paths_are_escaped() {
        assert_eq!(path(Path::new(r"C:\t\x.pem")), r#""C:\\t\\x.pem""#);
    }
}
```

新建 `tests/interop/tests/http2.rs`：

```rust
//! rurge's `trust-tunnel` and `h2-connect` outbounds against the TrustTunnel
//! endpoint v1.1.0 (Linux and macOS only), phase 2 M6 design 5.5: its TCP
//! mode is a standard HTTP/2 CONNECT with Basic authentication, so it serves
//! `h2-connect`'s plain CONNECT too (sing-box 1.14.2's `http` inbound speaks
//! HTTP/1 only). Over TCP only: no reference server for CONNECT-UDP. Every
//! target is a loopback IP literal; the endpoint picks its host by SNI, so
//! every line sends `sni=tt.test` and trusts the fixture's CA.

mod common;

use common::*;
use rurge_interop::trusttunnel::{HOSTNAME, TrustTunnel, trusttunnel_or_skip};
use std::time::Duration;

const USER: &str = "alice";
const PASSWORD: &str = "s3cret-tt";

/// The endpoint with a certificate for its host, and the fixture whose CA
/// signed it.
fn endpoint(bin: &Path, dir: &Path) -> (TrustTunnel, Arc<TlsFixture>) {
    let fixture = TlsFixture::new(&[HOSTNAME]);
    let server = TrustTunnel::spawn(bin, dir, &leaf_files(&fixture, dir), USER, PASSWORD);
    (server, fixture)
}

/// `name = <kind>, 127.0.0.1, <port>, <params>, sni=tt.test`.
fn line(name: &str, kind: &str, port: u16, params: &str) -> String {
    format!("{name} = {kind}, 127.0.0.1, {port}, {params}, sni={HOSTNAME}\n")
}

fn profile(lines: &[String]) -> String {
    format!("[Proxy]\n{}[Rule]\nFINAL,DIRECT\n", lines.concat())
}

/// `count` tunnels opened at the same time and all held open, each echoing
/// its own bytes: more than `max-streams` of them share the pool's
/// connections. Bounded like `roundtrip`.
async fn tunnels_at_once(out: &OutboundRef, echo: SocketAddr, count: u8) {
    let bound = Duration::from_secs(10);
    let mut opening = tokio::task::JoinSet::new();
    for n in 0..count {
        let out = out.clone();
        opening.spawn(async move {
            let stream = tokio::time::timeout(
                bound,
                out.connect_tcp(&target(echo), &ConnectOpts::default()),
            )
            .await
            .expect("the tunnel is established within the bound")
            .expect("the tunnel is established");
            (n, stream)
        });
    }
    let mut tunnels = Vec::new();
    while let Some(opened) = opening.join_next().await {
        tunnels.push(opened.expect("the opening task completes"));
    }
    // every tunnel is open at once; each carries its own bytes
    for (n, stream) in &mut tunnels {
        let payload = vec![*n; 4096];
        let mut back = vec![0u8; payload.len()];
        let exchange = async {
            stream.write_all(&payload).await.unwrap();
            stream.read_exact(&mut back).await.unwrap();
        };
        tokio::time::timeout(bound, exchange)
            .await
            .expect("the echo comes back within the bound");
        assert!(back == payload, "tunnel {n} carried another tunnel's bytes");
    }
}

#[tokio::test]
async fn trust_tunnel_against_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("trust_tunnel_against_the_endpoint") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[line(
        "TT",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password={PASSWORD}"),
    )]);
    let (out, echo) = (
        outbound(&profile, "TT", Some(&fixture)),
        echo_server().await,
    );
    roundtrip(&out, echo).await;
    roundtrip_big(&out, echo).await;
}

#[tokio::test]
async fn a_wrong_trust_tunnel_password_is_refused() {
    let Some(bin) = trusttunnel_or_skip("a_wrong_trust_tunnel_password_is_refused") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[line(
        "Wrong",
        "trust-tunnel",
        server.port(),
        &format!("username={USER}, password=nope"),
    )]);
    let out = outbound(&profile, "Wrong", Some(&fixture));
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        out.connect_tcp(&target(echo_server().await), &ConnectOpts::default()),
    )
    .await
    .expect("the endpoint answers within the bound");
    let Err(err) = refused else {
        panic!("a wrong password must not open a tunnel");
    };
    // the endpoint's `auth_failure_status_code` is 407 by default
    assert_eq!(err.to_string(), "trust-tunnel: authentication failed");
}

#[tokio::test]
async fn h2_connect_against_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("h2_connect_against_the_endpoint") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[
        // the credentials positionally after the port, and named
        line(
            "Positional",
            "h2-connect",
            server.port(),
            &format!("{USER}, {PASSWORD}"),
        ),
        line(
            "Named",
            "h2-connect",
            server.port(),
            &format!("username={USER}, password={PASSWORD}"),
        ),
    ]);
    let echo = echo_server().await;
    for name in ["Positional", "Named"] {
        let out = outbound(&profile, name, Some(&fixture));
        roundtrip(&out, echo).await;
        roundtrip_big(&out, echo).await;
    }
}

#[tokio::test]
async fn h2_connect_streams_share_connections_on_the_endpoint() {
    let Some(bin) = trusttunnel_or_skip("h2_connect_streams_share_connections_on_the_endpoint")
    else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let (server, fixture) = endpoint(&bin, dir.path());
    let profile = profile(&[line(
        "Shared",
        "h2-connect",
        server.port(),
        &format!("{USER}, {PASSWORD}, max-streams=3"),
    )]);
    let (out, echo) = (
        outbound(&profile, "Shared", Some(&fixture)),
        echo_server().await,
    );
    tunnels_at_once(&out, echo, 7).await;
    // the pool still serves new tunnels after they are gone
    roundtrip(&out, echo).await;
}
```

- [ ] **Step 2: 运行**

Run: `cargo test -p rurge-interop`
Expected: 本机没有 TrustTunnel endpoint 时，4 条用例各打印一行 `skipping …` 后通过，夹具的单元用例照常断言并通过。互操作由首次推送后的 Linux / macOS CI 证明。

- [ ] **Step 3: CI 与文档**

CI：只在 Linux 与 macOS 作业里按 SHA-256 安装 TrustTunnel v1.1.0（取自 GitHub 发布资产的 `digest` 字段），设 `RURGE_TEST_TRUSTTUNNEL`：

`.github/workflows/ci.yml`——把

```yaml
          echo "RURGE_TEST_SNELL_SERVER=$bin" >> "$GITHUB_ENV"
```

换成

```yaml
          echo "RURGE_TEST_SNELL_SERVER=$bin" >> "$GITHUB_ENV"
      - name: Install the TrustTunnel endpoint for the HTTP/2 interoperability tests
        if: runner.os != 'Windows'
        shell: bash
        run: |
          set -euo pipefail
          version=1.1.0
          case "$RUNNER_OS" in
            Linux) asset="trusttunnel-v$version-linux-x86_64.tar.gz";  sha=91c2ea3db7416a01b5258a4c047ec22890490bc55e1b194206031aa75144f0e7 ;;
            macOS) asset="trusttunnel-v$version-macos-universal.tar.gz"; sha=126a5688e922ce8f83d4de2d8a2659b3aab97820184d30f2edbecc0ec4a32ae5 ;;
            *) echo "unexpected runner OS: $RUNNER_OS"; exit 1 ;;
          esac
          cd "$RUNNER_TEMP"
          curl -fsSL --retry 3 --retry-all-errors -o "$asset" "https://github.com/TrustTunnel/TrustTunnel/releases/download/v$version/$asset"
          if command -v sha256sum >/dev/null 2>&1; then
            actual=$(sha256sum "$asset" | cut -d' ' -f1)
          else
            actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
          fi
          if [ "$actual" != "$sha" ]; then
            echo "TrustTunnel checksum mismatch: expected $sha, got $actual"
            exit 1
          fi
          mkdir -p trusttunnel
          tar -xzf "$asset" -C trusttunnel
          bin=$(find "$PWD/trusttunnel" -type f -name trusttunnel_endpoint | head -n 1)
          [ -n "$bin" ] || { echo "no trusttunnel_endpoint binary in $asset"; exit 1; }
          chmod +x "$bin"
          echo "RURGE_TEST_TRUSTTUNNEL=$bin" >> "$GITHUB_ENV"
```

`tests/interop/README.md`——把

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver` 与 Surge 官方的 `snell-server` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

换成

```markdown
`rurge-interop` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它把 [sing-box](https://sing-box.sagernet.org/)、[xray](https://github.com/XTLS/Xray-core)、[shadowsocks-rust](https://github.com/shadowsocks/shadowsocks-rust) 的 `ssserver`、Surge 官方的 `snell-server` 与 [TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 `trusttunnel_endpoint` 作为参照实现，以回环子进程的方式拉起来，驱动 rurge 的 `http` / `https` / `socks5` / `trojan` / `vmess` / `anytls` / `wireguard` / `ss` / `snell` / `h2-connect` / `trust-tunnel` 出站，以及包在 Shadow TLS 里的 `trojan`，去连它们，验证 rurge 与真实的第三方实现互通。xray 只用来跑 `vmess`：VMess 协议由 xray 所在的这一脉实现定义，sing-box 的实现是重写，手写的编解码需要两个独立参照互相印证（M2 设计 M2-D5）。
```

`tests/interop/README.md`——把

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust 与 snell-server：`cargo test -p rurge-interop` 会正常通过，sing-box 的十九个互操作用例、xray 的两个、shadowsocks-rust 的两个与 snell-server 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

换成

```markdown
本地默认不安装 sing-box、xray、shadowsocks-rust、snell-server 与 TrustTunnel endpoint：`cargo test -p rurge-interop` 会正常通过，sing-box 的十九个互操作用例、xray 的两个、shadowsocks-rust 的两个、snell-server 的四个与 TrustTunnel endpoint 的四个互操作用例各打印一行 `skipping …` 后直接返回（各夹具自身的单元测试——配置渲染、"配置绝不碰本机"的安全守卫、取空闲端口——照常运行并断言）。要在本机真正跑 sing-box 的用例，二选一：
```

`tests/interop/README.md`——把

```markdown
xray、shadowsocks-rust 与 snell-server 的本机运行方式同理，见下面「xray」「shadowsocks-rust」「snell-server」三节。
```

换成

```markdown
xray、shadowsocks-rust、snell-server 与 TrustTunnel endpoint 的本机运行方式同理，见下面「xray」「shadowsocks-rust」「snell-server」「TrustTunnel」四节。
```

`tests/interop/README.md`——把

```markdown
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版）。CI 会设置它，本机一般不需要。
```

换成

```markdown
- `RURGE_TEST_TRUSTTUNNEL`：TrustTunnel `trusttunnel_endpoint` 可执行文件的路径，优先于 `PATH` 查找。
- `RURGE_INTEROP_REQUIRED=1`：找不到二进制时让用例直接失败，而不是打印 `skipping …` 后跳过；对 sing-box、xray、shadowsocks-rust 与 sshd 各夹具都生效，对 snell-server 只在 Linux 上生效（它只有 Linux 版），对 TrustTunnel endpoint 只在 Linux 与 macOS 上生效（它没有 Windows 版）。CI 会设置它，本机一般不需要。
```

`tests/interop/README.md`——把

```markdown
- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的四个用例，阶段 2 / M6b）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。
```

换成

```markdown
- Snell（`tests/snell.rs` 里 `…_against_sing_box` 的四个用例，阶段 2 / M6b）：sing-box 的 `snell` 入站（1.14.0 起），`version: 5`、一个 `psk`、不配 `users`（服务端不看 client id）。sing-box 没有"version 4"的入站；`version: 5` 走 v4 / v5 共用的线上格式，所以同时接受 v4 客户端。覆盖 `version=5` 与 `version=4` 两种客户端的单块与跨多块（100 000 字节）TCP 往返；`reuse=true` 时一连四个请求（每个都两个方向干净结束，连接回池给下一个请求用）再加一个大负载；`obfs=http`（入站的 `obfs_mode: http`；含 `reuse=true` 与 UDP）；以及 UDP over TCP（命令 `0x06`）经同一个载体往返一个回环 UDP 回显两次。

**不覆盖 HTTP/2 的 `h2-connect`**：sing-box 1.14.2 的 `http` 入站只说 HTTP/1（TLS 握手之后按 HTTP/1.x 读请求，不设 ALPN、没有 h2 代码路径；HTTP/2 要到 1.15.0，目前仍是预发布），所以 `h2-connect` 的普通 CONNECT 改由 TrustTunnel endpoint 覆盖（见下面「TrustTunnel」一节）。
```

`tests/interop/README.md`——把

```markdown
渲染出的配置（`rurge_interop::snell_server::render`）是一个 `[snell-server]` 节：`listen = 127.0.0.1:<端口>`、`psk`、`ipv6 = false`，按需加 `obfs = http`；没有 `dns`、`egress-interface` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环）。启动命令是 `snell-server -c <配置文件>`，就绪与否看 TCP 端口能否连上。

## sshd
```

换成

```markdown
渲染出的配置（`rurge_interop::snell_server::render`）是一个 `[snell-server]` 节：`listen = 127.0.0.1:<端口>`、`psk`、`ipv6 = false`，按需加 `obfs = http`；没有 `dns`、`egress-interface` 这些键（夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环）。启动命令是 `snell-server -c <配置文件>`，就绪与否看 TCP 端口能否连上。

## TrustTunnel

[TrustTunnel](https://github.com/TrustTunnel/TrustTunnel) 的 endpoint 是 `trust-tunnel` 的参照实现；它的 TCP 模式就是标准的 HTTP/2 CONNECT（`:authority` 为 `host:port`，`proxy-authorization: Basic`，200 / 407 / 502），所以同时充当 `h2-connect` 普通 CONNECT 的参照服务端（sing-box 1.14.2 的 `http` 入站只说 HTTP/1，见上面「覆盖范围」末尾）。互操作测试固定 **v1.1.0**（2026-09-01 发布），只有 Linux 与 macOS 版。CI 下载并校验以下两个发布包（SHA-256 取自 GitHub 发布 API 每个资产的 `digest`）：

| 平台 | 资产 | SHA-256 |
| ---- | ---- | ------- |
| Linux (x86_64) | `trusttunnel-v1.1.0-linux-x86_64.tar.gz` | `91c2ea3db7416a01b5258a4c047ec22890490bc55e1b194206031aa75144f0e7` |
| macOS (universal) | `trusttunnel-v1.1.0-macos-universal.tar.gz` | `126a5688e922ce8f83d4de2d8a2659b3aab97820184d30f2edbecc0ec4a32ae5` |

发布包里是一个以资产名命名的目录，内有 `trusttunnel_endpoint`（另有 `setup_wizard`、签名文件与 `LICENSE`，不用）。

`tests/http2.rs` 驱动的四个用例覆盖：

- `trust-tunnel`：单块与跨多块（100 000 字节）的 TCP 往返；口令错误时 endpoint 回 407，出站报 `trust-tunnel: authentication failed`。
- `h2-connect`：凭据写在端口之后（位置参数）与写成 `username=` / `password=` 两种，各做单块与跨多块的 TCP 往返；`max-streams=3` 时同时打开七条隧道并全部保持（多于一条连接能承载的流，池要开新连接），每条各自回显自己的字节，之后再开一条新隧道。

**不覆盖 CONNECT-UDP**：TrustTunnel 的 UDP 走它自己的 `_udp2` 复用格式，不是 RFC 9298 的 CONNECT-UDP；目前找不到可用的稳定版参照服务端（sing-box 要到 1.15.0 才在 `http` 入站上提供，且它的发布版是否通告 extended CONNECT 未经核实）。`h2-connect` 的 CONNECT-UDP 由回环假服务端（`rurge_proto::testing::FakeH2Proxy`，开 `enable_connect_protocol`）与手工验收（`docs/acceptance/phase2-manual.md` 的 M6c 一节）覆盖。

endpoint 按 SNI 精确匹配它的主机（`main_hosts[].hostname`），所以夹具的证书签给 `tt.test`，rurge 的每一行都写 `sni=tt.test` 并信任夹具的 CA；ALPN 固定 `h2`（不发 ALPN 时 endpoint 按 HTTP/1 处理）。

CI 只在 Linux 与 macOS 上下载、校验并设置 `RURGE_TEST_TRUSTTUNNEL`；Windows 上这四个用例照常编译，运行时打印 `skipping …` 后返回（`RURGE_INTEROP_REQUIRED=1` 对它只在 Linux 与 macOS 上生效）。本机不安装它：`RURGE_TEST_TRUSTTUNNEL`（优先于 `PATH` 查找）没有指向可执行文件、`PATH` 上也找不到 `trusttunnel_endpoint` 时同样跳过；这个 crate 不会下载或安装它。

渲染出的配置（`rurge_interop::trusttunnel::render`）是三个 TOML 文件：`vpn.toml`（`listen_address = "127.0.0.1:<端口>"`、`ipv6_available = false`、只开 `[listen_protocols.http2]`、`[forward_protocol]` 为 `direct`，以及 `allow_private_network_connections = true`——默认值 `false` 会让 endpoint 拒绝连回环上的 echo）、`hosts.toml`（一个主机 `tt.test` 与证书、私钥的绝对路径）与 `credentials.toml`（一个用户）。夹具的单元用例 `the_configuration_stays_on_the_loopback` 断言只监听回环、只开 HTTP/2。启动命令是 `trusttunnel_endpoint vpn.toml hosts.toml`，就绪与否看 TCP 端口能否连上。

## sshd
```

`tests/interop/README.md`——把

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver` 与 `snell-server` 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust 或 snell-server 由项目所有者决定，没装就跳过。
```

换成

```markdown
- 夹具渲染出的 sing-box 配置只有 `log` / `inbounds` / `outbounds` 三个顶层键（WireGuard 的配置另有 `endpoints`）；每个入站只监听 `127.0.0.1`；唯一的出站是 `direct`。WireGuard 端点在用户态运行（`system: false`），它的 UDP 端口开在所有地址上（端点没有监听地址这一项；`rurge_interop::render_wireguard` 的单元测试 `the_wireguard_configuration_never_touches_the_machine` 断言其余各项）。xray 配置同样只有这三个顶层键，唯一的出站是 `freedom`。`ssserver` 的配置只有 `servers`，每个服务端只听 `127.0.0.1`（TCP 与同号的 UDP）。`snell-server` 的配置只有 `[snell-server]` 一节，只听 `127.0.0.1`。TrustTunnel endpoint 只听 `127.0.0.1`、只开 HTTP/2，转发只有 `direct`；`allow_private_network_connections = true` 只是为了让它连回环上的 echo。任何地方都不出现 `set_system_proxy`、`tun`、`auto_route` 这些键（`rurge_interop::render` 与 `rurge_interop::xray::render` 的单元测试 `the_configuration_never_touches_the_machine` 各自断言这一点）；`shadowtls` 入站的 `handshake.server` 恒为 `127.0.0.1`（夹具的单元用例断言）。
- 每个用例的连接目标都是回环 IP 字面量（`127.0.0.1` 上的 echo / 测试服务器；WireGuard 用例在隧道里连的是 sing-box 自己的隧道地址 `10.9.0.1`，由 sing-box 映射到它的 `127.0.0.1`），sing-box、xray、`ssserver`、`snell-server` 与 TrustTunnel endpoint 因此既不解析域名也不会访问公网。
- 这个 crate 本身不下载、不安装任何东西；本机是否装有 sing-box、xray、shadowsocks-rust、snell-server 或 TrustTunnel endpoint 由项目所有者决定，没装就跳过。
```

兼容性清单（4.2 的 `h2-connect` / `trust-tunnel` 行、4.4 的 `alpn` 与 `shadow-tls-password` 行、4.5 的 `udp-relay`、4.6 的参数行；`trust-tunnel` 因 `h3` 回落 HTTP/2 且没有 UDP 标 🟡）：

`docs/surge-compatibility-matrix.md`——把

```markdown
| `h2-connect` | HTTP/2 CONNECT 多路复用 | Mac 6.6+ | ✅ | 2 | |
```

换成

```markdown
| `h2-connect` | HTTP/2 CONNECT 多路复用 | Mac 6.6+ | ✅ | 2 | M6c（阶段 2）已实现 TCP 与 UDP。HTTP/2 会话池（两种协议共用）：每个策略一个池，池里是若干条 TLS + HTTP/2 连接（可叠 Shadow TLS 与 `underlying-proxy`）；开流时选最早的一条正在承载的流少于上限、没收到 GOAWAY 的连接，没有就新建一条（同时进来的请求共用一次握手，握手失败一并报给等着它的请求，不退避）；上限是 `max-streams`（缺省 3）与服务端 `SETTINGS_MAX_CONCURRENT_STREAMS` 中较小者，流数由 rurge 自己计（不靠 h2 的排队）；新连接在收到服务端的 SETTINGS 之前只承载一条流（连上后发一个 PING，回来即视为 SETTINGS 已知），同时进来的其它请求等它定下来再决定共用还是另开；没有流的连接空闲 60 秒关闭（每 30 秒检查一次）；收到 GOAWAY 或出错的连接不再分配新流，现有流走完；每条流的接收窗口 1 MiB、连接窗口 4 MiB（TrustTunnel 文档建议 131072，rurge 取更大的值，服务端都接受）。流即字节流：写按对方的流控窗口分段，读到数据即释放流控容量，本方向结束发 END_STREAM（真正的半关闭），丢弃未结束的流发 RST_STREAM(CANCEL)；整个建立过程（取连接、拨号、TLS 与 HTTP/2 握手、CONNECT 应答）受连接超时约束。**ALPN 固定 `h2`**：写了别的 `alpn` 时 `W0028` 并忽略；服务端不参与 ALPN 或选了别的协议时失败 `<协议>: the server does not speak HTTP/2`；服务端（如 rustls）坚持另一种 ALPN 时握手本身失败，看到的是 TLS 层的 `tls: received fatal alert: NoApplicationProtocol`。`headers` 里 HTTP/2 禁止的连接专用字段（`connection` `keep-alive` `proxy-connection` `transfer-encoding` `upgrade` `te`）加载时各 `W0028` 并去掉；`headers` 里的字段替换 rurge 自己生成的同名字段（`proxy-authorization`、`user-agent`，不区分大小写），每个字段只发一个；`<random-string>` 占位按每个 CONNECT 请求（每条流）各自生成。目标写进 `:authority`（`host:port`，IPv6 带方括号，名字转成 A-label、交给服务端解析；写不出的名字不拨号即失败 `<协议>: the host name cannot be sent to the server`）。错误文本与日志不含凭据与 `headers` 的值。**TCP**：`:method CONNECT`，有凭据时 `proxy-authorization: Basic`；不发 `user-agent`（除非 `headers` 写了）；2xx 后流即隧道，407 是 `h2-connect: proxy authentication required`，其它状态 `h2-connect: the proxy answered <状态码>`。凭据可写在端口之后（位置参数）或写成 `username=` / `password=`，两种都写时命名的优先（手册的 `h2-connect` 语法没列凭据，兼容性清单与 `http` 一致按支持处理）。**UDP（`udp-relay=true`，CONNECT-UDP，RFC 9298 / 9297）**：每个目标一条 extended CONNECT 流（`:protocol connect-udp`、`:path /.well-known/masque/udp/<主机>/<端口>/`，IPv6 不带方括号、`:` 写作 `%3A`，名字转成 A-label；`capsule-protocol: ?1`），因此是**对称型 NAT**（M6-D8，与 M5b 的 vmess 相同）；数据报装在 DATAGRAM capsule（类型 0、context id 0）里，其它类型的 capsule 与其它 context id 丢弃，截断或畸形的 capsule 结束那条流（`h2-connect: a malformed capsule (…)`），下一个发往该目标的包重开一条；超过 65527 字节的数据报不发送（`h2-connect: a datagram longer than 65527 bytes`），回包超长同样结束那条流；每个目标的流空闲 60 秒关闭。**服务端须通告 extended CONNECT**（`SETTINGS_ENABLE_CONNECT_PROTOCOL=1`）：打开 UDP 载体本身不拨号，没有通告时**错误出现在发往每个目标的第一个包上**（`h2-connect: the server does not support extended CONNECT`，什么也不发给服务端，该连接照常承载 TCP）。互操作参照：TCP 对 TrustTunnel endpoint（sing-box 1.14.2 的 `http` 入站只说 HTTP/1）；CONNECT-UDP 只有回环假服务端与手工验收。 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `trust-tunnel` | Trust Tunnel（AdGuard，HTTP/2 或 HTTP/3） | Mac 6.4.4+ | ✅ | 2 | |
```

换成

```markdown
| `trust-tunnel` | Trust Tunnel（AdGuard，HTTP/2 或 HTTP/3） | Mac 6.4.4+ | 🟡 | 2 | M6c（阶段 2）已实现 TCP（HTTP/2）。HTTP/2 会话池（两种协议共用）：每个策略一个池，池里是若干条 TLS + HTTP/2 连接（可叠 Shadow TLS 与 `underlying-proxy`）；开流时选最早的一条正在承载的流少于上限、没收到 GOAWAY 的连接，没有就新建一条（同时进来的请求共用一次握手，握手失败一并报给等着它的请求，不退避）；上限是 `max-streams`（缺省 3）与服务端 `SETTINGS_MAX_CONCURRENT_STREAMS` 中较小者，流数由 rurge 自己计（不靠 h2 的排队）；新连接在收到服务端的 SETTINGS 之前只承载一条流（连上后发一个 PING，回来即视为 SETTINGS 已知），同时进来的其它请求等它定下来再决定共用还是另开；没有流的连接空闲 60 秒关闭（每 30 秒检查一次）；收到 GOAWAY 或出错的连接不再分配新流，现有流走完；每条流的接收窗口 1 MiB、连接窗口 4 MiB（TrustTunnel 文档建议 131072，rurge 取更大的值，服务端都接受）。流即字节流：写按对方的流控窗口分段，读到数据即释放流控容量，本方向结束发 END_STREAM（真正的半关闭），丢弃未结束的流发 RST_STREAM(CANCEL)；整个建立过程（取连接、拨号、TLS 与 HTTP/2 握手、CONNECT 应答）受连接超时约束。**ALPN 固定 `h2`**：写了别的 `alpn` 时 `W0028` 并忽略；服务端不参与 ALPN 或选了别的协议时失败 `<协议>: the server does not speak HTTP/2`；服务端（如 rustls）坚持另一种 ALPN 时握手本身失败，看到的是 TLS 层的 `tls: received fatal alert: NoApplicationProtocol`。`headers` 里 HTTP/2 禁止的连接专用字段（`connection` `keep-alive` `proxy-connection` `transfer-encoding` `upgrade` `te`）加载时各 `W0028` 并去掉；`headers` 里的字段替换 rurge 自己生成的同名字段（`proxy-authorization`、`user-agent`，不区分大小写），每个字段只发一个；`<random-string>` 占位按每个 CONNECT 请求（每条流）各自生成。目标写进 `:authority`（`host:port`，IPv6 带方括号，名字转成 A-label、交给服务端解析；写不出的名字不拨号即失败 `<协议>: the host name cannot be sent to the server`）。错误文本与日志不含凭据与 `headers` 的值。请求是标准的 HTTP/2 CONNECT：`proxy-authorization: Basic`（`username` / `password` 只读命名写法、必填，缺了是 `E0018`，不引用取值）与 `user-agent`——rurge 发固定的 `rurge`（协议文档写作必填、格式为 `<平台> <应用名>`，TrustTunnel endpoint 的实现并不检查；Surge 发什么未核对），`headers` 写了 `User-Agent` 时用它的值。2xx 后流即隧道；**407 是 `trust-tunnel: authentication failed`**，其它状态（endpoint 连不上目标时的 502、endpoint 把认证失败的状态码配成 405 / 404 / 403 时也在此列）是 `trust-tunnel: the server answered <状态码>`。endpoint 按 SNI 精确选主机：服务器写成 IP 时要写 `sni=<主机名>`。**`h3=true` 解析并 `W0029`（每次加载一条），M7 之前仍按 HTTP/2 连**；写了 `udp-relay` 时 `W0028` 并忽略。**不载 UDP**：`udp()` 为不支持，由 `udp-policy-not-supported-behaviour` 处理；不实现 TrustTunnel 的 `_udp2`（UDP 复用）、`_icmp` 与 `_check` 伪主机；测速按 URL 测速。互操作参照：TrustTunnel endpoint v1.1.0（Linux / macOS）。 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

换成

```markdown
| 策略指向 rurge 尚未实现的协议类型 | Surge 原生支持全部协议 | 全部 | 🟡 | 1 / 2 | 加载时告警 `W0007`；运行时该策略按 `REJECT` 处理，会话日志 `error = policy protocol not implemented: <type>`；随阶段 2 各里程碑逐协议移除：M1 已移除 `http` `https` `socks5` `socks5-tls`；M2a 已移除 `trojan`；M2b 已移除 `vmess`（写了 `vmess-aead=true` 的行）与 `anytls`；M4a 已移除 `ssh`；M4b 已移除 `wireguard`；M4c 已移除 `external`；M6a 已移除 `ss`（流式旧方法除外）；M6b 已移除 `snell`（v4 / v5）；M6c 已移除 `h2-connect` 与 `trust-tunnel`（HTTP/2；`h3=true` 是 `W0029`，不是 `W0007`）。例外有三个：没写 `vmess-aead=true` 的 `vmess` 行仍按 `W0007` 处理，但走专门的诊断文本 `` `vmess` without `vmess-aead=true` uses the legacy handshake, which is not implemented yet ``（每次加载一条，不是每行一条）与专门的会话日志文本 `policy protocol not implemented: vmess (legacy handshake)`；流式旧方法的 `ss` 行同样如此，诊断文本 `` `ss` stream cipher `<method>` is not implemented yet; such policies behave as REJECT ``（每种方法每次加载一条），会话日志 `policy protocol not implemented: ss (<method>)`；`version` 为 1–3 或 6（含没写 `version` 的，Surge 的缺省是 1）的 `snell` 行也是，诊断文本 `` `snell` version <n> is not implemented; rurge supports versions 4 and 5 (`version` must match the server); such policies behave as REJECT ``（每个版本每次加载一条），会话日志 `policy protocol not implemented: snell v<n>`；三者都不是这里的通用 `<type>` 模板 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对） |
| `client-cert` | `[Keystore]` 条目名（p12） | ✅ | 2 | 双向 TLS；M1 已实现 |
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`，M4a 起还有 `ssh`）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
```

换成

```markdown
| `alpn` | 协议列表；TUIC / Hysteria 2 / MASQUE 默认 `h3` | ✅ | 2 | M1 已实现：`https` / `socks5-tls` 握手不带 ALPN，除非写了 `alpn`（未与真实 Surge 核对）；M6c：`h2-connect` / `trust-tunnel` 固定 `h2`，写了别的取值 `W0028` 并忽略 |
| `client-cert` | `[Keystore]` 条目名（p12） | ✅ | 2 | 双向 TLS；M1 已实现 |
| `shadow-tls-password` | 字符串；设置即启用 Shadow TLS | ✅ | 2 | M2c 已实现：所有 TCP 类出站（`http` `https` `socks5` `socks5-tls` `trojan` `vmess` `anytls`，M4a 起还有 `ssh`，M6c 起还有 `h2-connect` `trust-tunnel`）；顺序是 connect → shadow-tls → tls → ws → 协议。伪装握手照常校验证书（出站的根证书库），**不受本节其余六个 TLS 参数影响**（它们只作用于里层的真实 TLS）；伪装握手不带 ALPN。口令为空 `E0018`；没有口令时另外两个参数各报一条 `W0028` |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；HTTP/2 CONNECT 随 M6c | 2 |
```

换成

```markdown
| `udp-relay`（布尔；默认 false） | 适用 SOCKS5 / SOCKS5-TLS / Shadowsocks / External / HTTP/2 CONNECT（RFC 9298） | ✅ M5a：`socks5` / `socks5-tls` / `external` 已生效；M6a：Shadowsocks 已生效；M6c：HTTP/2 CONNECT（`h2-connect` 的 CONNECT-UDP，每个目标一条流，对称型；服务端须通告 extended CONNECT，见 4.2 `h2-connect` 一行）已生效；写在 `trust-tunnel` 行上是 `W0028` 并忽略 | 2 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `http` `https` `h2-connect` | `username` `password`（位置或命名） | ✅ | 2 | |
| `http` `https` | `always-use-connect`（默认 false） | ✅ | 2 | |
| `http` `https` `h2-connect` | `headers`（分号分隔；`<random-string(n)>` / `<random-string(min-max)>` 占位） | ✅ | 2 | 值里允许 HTAB，拒绝其它一切控制字符（`E0018`）；Surge 大概原样发送（未核对） |
| `h2-connect` | `max-streams`（默认 3） | ✅ | 2 | |
| `h2-connect` | `udp-relay`（CONNECT-UDP，RFC 9298） | ✅ | 2 | |
```

换成

```markdown
| `http` `https` `h2-connect` | `username` `password`（位置或命名） | ✅ | 2 | `h2-connect`：M6c 已实现；手册的 `h2-connect` 语法没列凭据，rurge 与 `http` 一样两种写法都收、命名的优先；以 `proxy-authorization: Basic` 发送 |
| `http` `https` | `always-use-connect`（默认 false） | ✅ | 2 | |
| `http` `https` `h2-connect` | `headers`（分号分隔；`<random-string(n)>` / `<random-string(min-max)>` 占位） | ✅ | 2 | 值里允许 HTAB，拒绝其它一切控制字符（`E0018`）；Surge 大概原样发送（未核对） |
| `h2-connect` | `max-streams`（默认 3） | ✅ | 2 | M6c 已实现：每条 HTTP/2 连接同时承载的流数上限（还受服务端 `SETTINGS_MAX_CONCURRENT_STREAMS` 约束；收到服务端 SETTINGS 之前一条连接只承载一条流），满了开新连接；须是至少为 1 的整数，否则 `E0018` |
| `h2-connect` | `udp-relay`（CONNECT-UDP，RFC 9298） | ✅ | 2 | M6c 已实现：每个目标一条 extended CONNECT 流（对称型 NAT）；服务端没有通告 extended CONNECT 时错误出现在发往每个目标的第一个包上；超过 65527 字节的数据报不发送；服务器名写不进请求时构建失败 |
```

`docs/surge-compatibility-matrix.md`——把

```markdown
| `trust-tunnel` | `username` `password` `headers` `max-streams`（默认 3）`h3`（默认 false） | ✅ | 2 | 无 UDP |
```

换成

```markdown
| `trust-tunnel` | `username` `password` `headers` `max-streams`（默认 3）`h3`（默认 false） | 🟡 | 2 | M6c 已实现（HTTP/2）。`username` / `password` 只读命名写法、必填（`E0018`，不引用取值；写在端口之后的位置值是 `W0001`）；`headers` 与 `max-streams` 同 `h2-connect`（`headers` 可覆盖 rurge 的 `user-agent: rurge` 与 `proxy-authorization`）；TLS 参数适用，`alpn` 固定 `h2`；**`h3=true` 解析并 `W0029`，M7 之前按 HTTP/2 连**；`udp-relay` 是 `W0028`；无 UDP |
```

`docs/acceptance/phase2-manual.md`——把

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里 `psk` 与 `obfs-host` 都是 `***`。

```

换成

```markdown
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里 `psk` 与 `obfs-host` 都是 `***`。

## M6c　HTTP/2 族

前置：同 M5a 一节的 SOCKS5 UDP 客户端；自己的 `h2-connect` 节点（HTTP/2 over TLS 的 CONNECT 代理，带用户名口令；记下服务端实现与版本，CONNECT-UDP 一项要求它支持 RFC 9298 并通告 extended CONNECT，如 sing-box 1.15 起的 `http` 入站）与自己的 TrustTunnel 节点（TrustTunnel endpoint，记下版本）。每份配置 `[Rule]` 里 `FINAL,<策略名>`，`rurge check -c <配置>` 零错误（没有 `W0007`）。

- [ ] `h2-connect` 的 TCP：策略写 `h2-connect, <服务器>, <端口>, <用户名>, <口令>`（凭据写在端口之后），`curl -x http://127.0.0.1:<http-listen 端口> https://example.com/ -I` 与 `curl --socks5-hostname 127.0.0.1:<socks5-listen 端口> https://example.com/ -I` 都返回 200，请求记录里策略链是该策略、`error` 为空；把凭据改成 `username=` / `password=` 的写法，重载后同样可用。服务端先说话的协议（经代理连一个 SMTP / SSH 主机）能看到对端的欢迎行。
- [ ] `max-streams`：缺省（3）时同时打开五六个下载（或用浏览器同时打开多个网站），服务端日志或抓包看到只有两条左右的 TLS 连接、每条上同时最多 3 条流；改成 `max-streams=1` 后每个同时进行的请求各占一条连接；全部结束、空闲 60 秒后连接被关掉。
- [ ] 认证失败：把口令改错一位，重载后访问 `https://example.com/`：请求失败，`h2-connect` 的会话记录 `error` 是 `h2-connect: proxy authentication required`，`trust-tunnel` 的是 `trust-tunnel: authentication failed`；**错误文本与日志都不含用户名、口令与 `headers` 的值**。
- [ ] `h2-connect` 的 UDP：加 `udp-relay=true`，经 SOCKS5 UDP 发 DNS 查询（如 Proxifier 代理 `nslookup example.com 8.8.8.8`）得到回答；轮流发往两个不同的 DNS 服务器都有回答，服务端日志（或抓包）看到每个目标一条 `connect-udp` 流（对称型，已知差异，见兼容性清单 `h2-connect` 一行）。服务端不支持 extended CONNECT 时（换一个只支持普通 CONNECT 的节点）UDP 流失败，会话记录写 `h2-connect: the server does not support extended CONNECT`，同一节点上的 TCP 照常可用。
- [ ] `trust-tunnel` 的 TCP：策略写 `trust-tunnel, <服务器>, <端口>, username=<用户名>, password=<口令>`（服务器写成 IP 时另加 `sni=<节点的主机名>`），`curl` 经 HTTP 与 SOCKS5 两个入口都返回 200；服务端日志看到的 `User-Agent` 是 `rurge`，加 `headers=User-Agent:<自定义>` 后变成自定义值。
- [ ] `trust-tunnel` 不载 UDP：经 SOCKS5 UDP 发 DNS 查询，`udp-policy-not-supported-behaviour` 缺省（`REJECT`）时查询没有回答，会话记录写 `policy does not support UDP`；改成 `DIRECT` 后查询直连得到回答。写 `h3=true` 时 `rurge check` 报一条 `W0029`，运行时仍按 HTTP/2 连通。
- [ ] 脱敏：`GET /v1/policies/detail?policy_name=<策略名>` 与 `GET /v1/profiles/current` 里两种策略的用户名、口令（含 `h2-connect` 写在端口之后的位置值）与 `headers` 都是 `***`。

```

`README.md`——把

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）进行中：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

换成

```markdown
> **阶段 1 功能齐备：M1 ～ M4b**（配置解析、规则引擎、规则集、GeoIP、外部资源管理、DNS 客户端、HTTP / SOCKS5 代理与 DIRECT / REJECT 分流，`rurge check` / `rule match` / `dns lookup` / `run`；M3b 新增请求记录与流量统计、SNI 记录、空闲超时、REJECT 自动升级、CONNECT 502、优雅退出、热重载（SIGHUP / `--watch`）、`--log-file`、`encrypted-dns-follow-outbound-mode`；M4a 新增 Surge 兼容 HTTP API（阶段 1 端点、`X-Key` 鉴权与封禁）、出站模式 / 全局策略持久化到 `state.json`、`rurge reload` / `stop` / `status`；M4b 新增系统代理（Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME / KDE，`--system-proxy` 与 `POST /v1/features/system_proxy`，退出与崩溃后自动恢复，重载后跟随监听地址变化）与 `rurge service install / uninstall [--dry-run]`（systemd / launchd / Windows 计划任务））——阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md)；Dashboard 在阶段 6。阶段 2（出站协议与策略组）进行中：M1（出站地基与 HTTP / SOCKS5 上游）已完成——`rurge run -c <conf>` 已能作为 HTTP / SOCKS5 代理按规则把连接经 DIRECT / REJECT 或 `http` / `https` / `socks5` / `socks5-tls` 上游转发（TCP，含 `underlying-proxy` 两级及以上链），`select` 组的选择可经 HTTP API 读取与切换（见 [docs/api/phase2.md](docs/api/phase2.md)）；M2a（TLS 族，Trojan 优先）已完成——`trojan` 策略（TCP，TLS 必有，可叠加 WebSocket 传输）已可用；M2b（VMess / AnyTLS）已完成——`vmess`（AEAD 握手，TCP，可叠加 TLS / WebSocket）与 `anytls`（TCP，会话复用）策略已可用，重载时按指纹复用出站，不影响正在使用的连接池；M2c（Shadow TLS）已完成——`shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` 在所有 TCP 类出站上生效（v2 与 v3，v3 在 stock rustls 上实现）；M3a（成员装配与订阅）已完成——`policy-path` 订阅（本地文件或 URL，Surge 格式；订阅更新只重建策略表，不打断无关的连接）、`include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`、组级 `underlying-proxy`（派生 `名字 (via 中继)`）、组环告警（`W0030`）与空组兜底（默认 DIRECT，`--empty-group-reject` 改为 REJECT）已可用；M3b（测速与自动组）已完成——`url-test` / `fallback` / `load-balance` 组按连通性测试（经各节点在同一条连接上两次 `HEAD`，HTTPS 测试 URL 在已建立的 TLS 连接上测第二次）自动选择，`interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` 与策略的 `test-url` / `test-timeout` 生效，自动组可经 `POST /v1/policy_groups/select` 临时覆盖，新增 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 三个端点；M3c（`smart`）已完成——`smart` 组按真实会话的首字节耗时与测速结果打分（失败罚分、`policy-priority` 因子、约 1 小时的站点记忆），选中的成员连不上时自动换下一个（最多再试 2 个），请求记录增加 `connectMs` / `firstByteMs`；M4a（SSH）已完成——`ssh` 策略（TCP；每个策略一条 SSH 会话、每个连接一个通道；口令或 `[Keystore]` 里的 OpenSSH 私钥登录；`server-fingerprint` 校验主机密钥；按 `idle-timeout` 断开空闲会话、30 秒一次保活）已可用；M4b（WireGuard）已完成——`wireguard` 策略（TCP；用户态隧道，不建虚拟网卡；多个 peer 按 `allowed-ips` 选路；`client-id`（WARP 的保留字节）；配了 `dns-server` 时目标域名经隧道查询，否则在本机解析；没有 `dns-server` 也没写 `test-url` 时以握手测速）已可用；M4c（external）已完成——`external` 策略（TCP；第一次用到时拉起外部程序、经它在本机端口上的 SOCKS5 转发；程序退出后下次用到时再拉起；输出写进数据目录；rurge 退出时连同它启动的进程一起停掉——Unix 用进程组，Windows 用 Job Object；三平台都支持，Surge 只在 Mac 上有）已可用，订阅导入的 `external` 一律跳过；M5a（UDP 地基）已完成——SOCKS5 监听支持 UDP ASSOCIATE，UDP 按规则分流到 DIRECT、REJECT 或 `socks5` / `socks5-tls` / `external`（`udp-relay=true`，含 `underlying-proxy` 链），全锥 NAT，每条 UDP 流一条请求记录，`block-quic` 与 `udp-policy-not-supported-behaviour` 生效；M5b（TLS 族的 UDP）已完成——`trojan`（UDP ASSOCIATE）、`anytls`（UDP over TCP v2）全锥，`vmess`（命令 2，每个目标一条连接）对称型；M5c（WireGuard 的 UDP 与其余）已完成——`wireguard` 的 UDP（全锥）与经 `underlying-proxy` 的隧道、`test-udp` / `proxy-test-udp`、`smart` 组计入 UDP、`dns-follow-interface`；M6（Shadowsocks / Snell / HTTP/2 族）已完成：M6a（Shadowsocks）已完成——`ss` 的 AEAD、`none` 与 SS 2022（含多用户身份头）方法、simple-obfs（`http` / `tls`），TCP 与 UDP（`udp-relay=true`、`udp-port`，全锥），流式旧方法在 M8 之前按 REJECT 处理；M6b（Snell）已完成——`snell` v4 / v5 的 TCP 与 UDP（UDP over TCP，全锥）、`reuse`（连接回池给下一个请求用）、simple-obfs `http`，v1–v3 与 v6 按 REJECT 处理（没写 `version` 即 v1）；M6c（HTTP/2 族）已完成——`h2-connect`（HTTP/2 CONNECT，多条隧道共用连接、`max-streams`；`udp-relay=true` 时经 CONNECT-UDP（RFC 9298）转发 UDP，每个目标一条流、对称型）与 `trust-tunnel`（HTTP/2，TCP；`h3=true` 在 M7 之前仍按 HTTP/2 连）；其余出站协议仍在阶段 2 后续里程碑，`subnet` 策略组在阶段 3。
```

`README.md`——把

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b） | 2     |
```

换成

```markdown
| 出站协议       | DIRECT / REJECT 系列 / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / 外部程序（DIRECT 与 REJECT 系四种已实现，M3a；HTTP / SOCKS5（含 `socks5-tls`）TCP 上游与 `underlying-proxy` 链已实现，阶段 2 / M1；Trojan（TCP；WebSocket 传输）已实现，阶段 2 / M2a；VMess（AEAD 握手，TCP；可叠加 TLS / WebSocket）与 AnyTLS（TCP，会话复用）已实现，阶段 2 / M2b；SSH（TCP，会话复用）已实现，阶段 2 / M4a；WireGuard（TCP，用户态隧道）已实现，阶段 2 / M4b；外部程序（TCP，三平台）已实现，阶段 2 / M4c；Shadowsocks（AEAD / 2022、obfs，TCP 与 UDP）已实现，阶段 2 / M6a；Snell v4 / v5（TCP 与 UDP、`reuse`、obfs `http`）已实现，阶段 2 / M6b；HTTP/2 CONNECT（`h2-connect`，TCP 与 CONNECT-UDP）与 Trust Tunnel（HTTP/2，TCP）已实现，阶段 2 / M6c） | 2     |
```

`README.md`——把

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成）
```

换成

```markdown
3. **阶段 2** 出站协议全集、策略组、策略订阅（进行中：M1 出站地基与 HTTP / SOCKS5 上游已完成；M2a（Trojan，TCP + WebSocket）已完成；M2b（VMess / AnyTLS）已完成；M2c（Shadow TLS）已完成；M3a（成员装配与订阅）已完成；M3b（测速与自动组）已完成；M3c（`smart`）已完成；M4a（SSH）已完成；M4b（WireGuard）已完成；M4c（external）已完成；M5a（UDP 地基）已完成；M5b（TLS 族的 UDP）已完成；M5c（WireGuard 的 UDP 与其余）已完成；M6a（Shadowsocks）已完成；M6b（Snell）已完成；M6c（HTTP/2 族）已完成，M6 至此完成）
```

`README_en.md`——把

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is in progress: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

换成

```markdown
> **Phase 1 feature-complete: M1 through M4b** (profile parsing, rule engine, rule sets, GeoIP, external resource management, DNS client, HTTP / SOCKS5 proxy with DIRECT / REJECT routing, `rurge check` / `rule match` / `dns lookup` / `run`; M3b added the request log and traffic stats, SNI recording, idle timeout, REJECT auto-escalation, a 502 for failed CONNECT dials, graceful shutdown, hot reload (SIGHUP / `--watch`), `--log-file`, and `encrypted-dns-follow-outbound-mode`; M4a added a Surge-compatible HTTP API (phase-1 endpoints, `X-Key` auth with banning), outbound mode / global policy persisted to `state.json`, and `rurge reload` / `stop` / `status`; M4b added the system proxy (Windows registry + a WinINet notification, macOS `networksetup`, Linux GNOME / KDE, `--system-proxy` and `POST /v1/features/system_proxy`, automatic recovery on exit or crash, and following listener changes across a reload) and `rurge service install / uninstall [--dry-run]` (systemd / launchd / a Windows scheduled task)) — the phase-1 acceptance criteria that need a real desktop environment (the three-platform system proxy, service install) have not been through manual acceptance yet; see [docs/acceptance/phase1-manual.md](docs/acceptance/phase1-manual.md) for the checklist. The dashboard is phase 6. Phase 2 (outbound protocols and policy groups) is in progress: M1 (outbound foundation and HTTP / SOCKS5 upstreams) is done — `rurge run -c <conf>` already serves as an HTTP / SOCKS5 proxy routing connections by rule through DIRECT / REJECT or `http` / `https` / `socks5` / `socks5-tls` upstreams (TCP, including two-or-more-hop `underlying-proxy` chains), and a `select` group's choice can be read and switched over the HTTP API (see [docs/api/phase2.md](docs/api/phase2.md)); M2a (the TLS family, Trojan first) is done — the `trojan` policy (TCP, TLS always on, optionally over WebSocket) is usable; M2b (VMess / AnyTLS) is done — the `vmess` (AEAD handshake, TCP, optionally over TLS / WebSocket) and `anytls` (TCP, session reuse) policies are usable, and a reload reuses outbounds by fingerprint without disturbing connection pools still in use; M2c (Shadow TLS) is done — `shadow-tls-password` / `shadow-tls-sni` / `shadow-tls-version` take effect on every TCP outbound (v2 and v3, the latter on stock rustls); M3a (member assembly and subscriptions) is done — `policy-path` subscriptions (a local file or a URL, in Surge format; an update rebuilds only the policy table, without disturbing unrelated connections), `include-all-proxies` / `include-other-group` / `policy-regex-filter` / `external-policy-name-prefix` / `external-policy-modifier`, the group-level `underlying-proxy` (derived `Name (via Relay)` members), group cycle warnings (`W0030`) and the empty-group fallback (DIRECT by default, REJECT with `--empty-group-reject`) are usable; M3b (connectivity tests and automatic groups) is done — `url-test` / `fallback` / `load-balance` groups choose by connectivity tests (two `HEAD`s over one connection through each member, the second one on the established TLS connection for an HTTPS test URL), `interval` / `tolerance` / `timeout` / `evaluate-before-use` / `persistent` and the policies' `test-url` / `test-timeout` take effect, an automatic group takes a temporary override through `POST /v1/policy_groups/select`, and `POST /v1/policies/test`, `GET /v1/policy_groups/test_results` and `POST /v1/policy_groups/test` are available; M3c (`smart`) is done — a `smart` group scores its members by the first-byte time of real sessions and by the tests (failure penalties, `policy-priority` factors, per-site memory for about an hour) and moves on to the next member when the one it picked does not connect (up to two more), and the request log gains `connectMs` / `firstByteMs`; M4a (SSH) is done — the `ssh` policy (TCP; one SSH session per policy and a channel per connection; login with a password or an OpenSSH private key from `[Keystore]`; host keys checked against `server-fingerprint`; idle sessions closed after `idle-timeout`, a keepalive every 30 seconds) is usable; M4b (WireGuard) is done — the `wireguard` policy (TCP; a user-space tunnel, no virtual interface; several peers chosen by `allowed-ips`; `client-id` (WARP's reserved bytes); destination names looked up through the tunnel when the section has a `dns-server`, on this machine otherwise; tested by a handshake when there is neither a `dns-server` nor a `test-url`) is usable; M4c (external) is done — the `external` policy (TCP; the program is started on first use and reached as a SOCKS5 proxy on its local port; a program that exited is started again on the next use; its output goes to the data directory; when rurge exits, the program is stopped together with whatever it started — a process group on Unix, a Job Object on Windows; all three platforms, where Surge has it on the Mac only) is usable, and subscriptions never bring `external` policies in; M5a (the UDP foundation) is done — the SOCKS5 listener takes UDP ASSOCIATE, and UDP is routed by rule to DIRECT, REJECT or `socks5` / `socks5-tls` / `external` (`udp-relay=true`, `underlying-proxy` chains included), full-cone NAT, one request record per UDP flow, and `block-quic` and `udp-policy-not-supported-behaviour` take effect; M5b (UDP over the TLS family) is done — `trojan` (UDP ASSOCIATE) and `anytls` (UDP over TCP v2) with full-cone NAT, `vmess` (command 2, one connection per target) symmetric; M5c (UDP over WireGuard and the rest) is done — UDP over `wireguard` (full cone) and tunnels over an `underlying-proxy`, `test-udp` / `proxy-test-udp`, UDP in `smart` groups, and `dns-follow-interface`; M6 (Shadowsocks / Snell / the HTTP/2 family) is done: M6a (Shadowsocks) is done — `ss` with the AEAD, `none` and SS 2022 (multi-user identity headers included) methods, simple-obfs (`http` / `tls`), TCP and UDP (`udp-relay=true`, `udp-port`, full cone), with the legacy stream ciphers behaving as REJECT until M8; M6b (Snell) is done — `snell` v4 / v5 over TCP and UDP (UDP over TCP, full cone), `reuse` (a connection goes back to a pool for the next request) and simple-obfs `http`, with v1–v3 and v6 behaving as REJECT (a line without `version` is v1); M6c (the HTTP/2 family) is done — `h2-connect` (HTTP/2 CONNECT, tunnels sharing connections up to `max-streams`; with `udp-relay=true`, UDP over CONNECT-UDP (RFC 9298), one stream per target, symmetric) and `trust-tunnel` (HTTP/2, TCP; `h3=true` still connects over HTTP/2 until M7); the remaining outbound protocols are later phase-2 milestones, and the `subnet` group comes in phase 3.
```

`README_en.md`——把

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b) | 2     |
```

换成

```markdown
| Outbound            | DIRECT / REJECT family / HTTP / SOCKS5 / Shadowsocks / Snell / VMess / Trojan / TUIC / Hysteria 2 / MASQUE / AnyTLS / Trust Tunnel / SSH / WireGuard / external program (DIRECT and the four REJECT flavours implemented, M3a; HTTP / SOCKS5 (including `socks5-tls`) TCP upstreams and `underlying-proxy` chains implemented, phase 2 / M1; Trojan (TCP; WebSocket transport) implemented, phase 2 / M2a; VMess (AEAD handshake, TCP; optionally TLS / WebSocket) and AnyTLS (TCP, session reuse) implemented, phase 2 / M2b; SSH (TCP, session reuse) implemented, phase 2 / M4a; WireGuard (TCP, user-space tunnel) implemented, phase 2 / M4b; external program (TCP, all three platforms) implemented, phase 2 / M4c; Shadowsocks (AEAD / 2022, obfs, TCP and UDP) implemented, phase 2 / M6a; Snell v4 / v5 (TCP and UDP, `reuse`, obfs `http`) implemented, phase 2 / M6b; HTTP/2 CONNECT (`h2-connect`, TCP and CONNECT-UDP) and Trust Tunnel (HTTP/2, TCP) implemented, phase 2 / M6c) | 2     |
```

`README_en.md`——把

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done)
```

换成

```markdown
3. **Phase 2** All outbound protocols, policy groups, subscriptions (in progress: M1, the outbound foundation and HTTP / SOCKS5 upstreams, is done; M2a, Trojan (TCP + WebSocket), is done; M2b, VMess / AnyTLS, is done; M2c, Shadow TLS, is done; M3a, member assembly and subscriptions, is done; M3b, connectivity tests and automatic groups, is done; M3c, `smart` groups, is done; M4a, SSH, is done; M4b, WireGuard, is done; M4c, external, is done; M5a, the UDP foundation, is done; M5b, UDP over the TLS family, is done; M5c, UDP over WireGuard and the rest, is done; M6a, Shadowsocks, is done; M6b, Snell, is done; M6c, the HTTP/2 family, is done, which completes M6)
```

`CLAUDE.md`——把

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 是这条连接所连的端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。
```

换成

```markdown
阶段 1 功能已齐备（M1 ～ M4b）；阶段 1 验收标准里需要真实桌面环境的项目（三平台系统代理、服务安装）的手工验收尚未进行，清单见 `docs/acceptance/phase1-manual.md`。M1、M2a、M2b、M3a、M3b、M4a：Cargo workspace、`rurge-config`（解析全部 Surge 语法为强类型 `Config` + 诊断）、`rurge check`、`rurge-net`（连接器 / 内部 HTTP 客户端 / 外部资源管理器）、`rurge-rules`（域名 / IP 索引、规则集、GeoIP / ASN、规则引擎）、`rurge rule match`（离线规则匹配开发命令）、`rurge-dns`（UDP / TCP / DoT / DoH 上游、并发查询与重试、缓存、`[Host]` 链、系统 hosts）、`rurge-platform::dns`、`rurge dns lookup`、`rurge-proto`（`Outbound` 抽象、DIRECT / REJECT）、`rurge-policy`（策略注册表）、`rurge-inbound`（HTTP / SOCKS5 监听）、`rurge-engine`（会话流水线）、`rurge run`（前台代理，DIRECT / REJECT 分流）；M3b 新增可中断带空闲超时的 relay（`--idle-timeout`）、优雅退出、请求记录与流量统计（`--request-log-size`）、SNI 记录（观测用）、REJECT 30 s/50 次自动升级 REJECT-DROP、CONNECT 连接失败 502、热重载（SIGHUP / `--watch`）、`--log-file` 按天滚动、`encrypted-dns-follow-outbound-mode`；M4a 新增 `rurge-api`（axum 服务，Surge 兼容 HTTP API 阶段 1 端点，`X-Key` 鉴权与失败封禁）、`StateStore`（`state.json` 异步原子写入）、引擎运行期出站模式 / 全局策略覆盖（持久化）、`Control` 命令通道、`rurge reload` / `stop` / `status`。M4b 新增 `rurge_platform::sysproxy`（`SystemProxy` trait；Windows 注册表 + WinINet 通知、macOS `networksetup`、Linux GNOME `gsettings` / KDE `kwriteconfig` + 其它桌面的环境变量提示三个后端）、`rurge_platform::service`（systemd / launchd / `schtasks` 的安装 / 卸载计划与执行器）、bin 侧 `SystemProxyManager`（快照 / 备份 / 应用 / 退出与崩溃恢复 / 重载跟随，经 `RURGE_SYSTEM_PROXY_BACKEND` 可切到测试用文件后端）、`Control::system_proxy_enabled` 与 `/v1/features/system_proxy`（读真实状态、失败 500）、`rurge run --system-proxy`、`rurge service install / uninstall [--dry-run]`、`rurge run` 的 per-data-dir 实例锁 `rurge.lock`（同一数据目录上的第二个实例退出 1）。阶段 2（出站协议与策略组）进行中：总设计与 M1 设计已写好；M1a（配置与出站库）已完成——`rurge-config::spec`（`PolicySpec`、`ParamReader`，诊断码 `E0018`–`E0022` / `W0028`–`W0029`）、`rurge_net::socket`（`SocketOpts`、`SocketHook`、按 `ip-version` 竞速的 `DirectConnector`）、`rurge_net::tls::root_store`、`rurge-platform::socket`（网卡绑定与 TOS）、`rurge-proto` 的 TLS 层（标准 / 指纹 / 不校验）、p12 解码、`http(s)` / `socks5(-tls)` 出站与 `rurge_proto::testing` 回环假上游；M1b（装配与控制面）已完成——`rurge-policy` 的 `OutboundFactory` / `RegistryCell` / `ChainConnector` / `SelectionTable`，`rurge-engine` 的 `EngineFactory` / `dry_build` / `load_checked` / `EngineShared` 与四个视图方法（`groups_view` / `policy_detail` / `group_selection` / `select_group`），明文 HTTP 的绝对 URI 转发，`use-local-host-item-for-proxy`，`rurge-dns` 的 `Resolver::host_lookup`，`rurge-api` 的四个策略 / 组端点（`GET /v1/policies/detail`、`GET /v1/policy_groups`、`GET`/`POST /v1/policy_groups/select`），bin 侧的 `PlatformSockets`（`SocketHook` 适配器）与能力表翻转（`http` `https` `socks5` `socks5-tls` 不再是 `W0007`），`tests/interop`（对 sing-box 的互操作测试）。M2（TLS 族）按三份计划推进：M2a（Trojan 优先）已完成——`rurge-config::spec` 的 `WsOpts` / `TrojanSpec`、`rurge-proto` 的传输阶梯 `transport::Stack`（connect → tls → ws）、WebSocket 字节流（`tokio-tungstenite`）、惰性请求头 `LazyHead`、trojan 出站与 `rurge_proto::testing` 的 `FakeWs` / `FakeTrojan`、能力表翻转 `trojan`、对 sing-box 的 trojan 互操作用例；M2b（VMess / AnyTLS）已完成——`rurge-config::spec` 的 `Secret<T>`（凭据字段的 `Debug` 恒为 `Secret(***)`）、`VmessSpec` / `AnyTlsSpec`；`rurge-proto` 的 `vmess`（AEAD 握手、分块流）与 `anytls`（会话层、padding、连接池）两个模块，及 `rurge_proto::testing` 的 `FakeVmess` / `FakeAnyTls` 两个假服务端；`rurge-engine` 的 `ResolverCell`（被复用的出站跟随新一代解析器）与 `publish_generation`（原 `publish_registry` 扩展）；`rurge-policy` 重载时按指纹复用出站（名字、参数、引用的 Keystore 条目与 `[General] ipv6` 都未变的策略沿用上一代的出站与连接池）；转发循环的两处修正（写完即 `flush`；任一方向以错误结束时另一方向随之结束）；能力表翻转 `vmess`（AEAD）与 `anytls`；对 sing-box 的 vmess / anytls 与对 xray（固定版本 v26.3.27，只测 vmess）的互操作用例。M2c（Shadow TLS）已完成——`rurge-config::spec` 的 `ShadowTlsOpts`（挂在 `PolicySpec` 上，三个参数的 `W0029` 退役）、`rurge_proto::transport::shadow_tls`（记录读取器、HMAC 链、帧化字节流、v3 在 stock rustls 上两遍构造 ClientHello、自己驱动的伪装握手与体面收尾）、`Stack` 的 shadow-tls 一层（connect → shadow-tls → tls → ws），`http` / `socks5` 出站迁到 `Stack`，`rurge_proto::testing` 的 `FakeShadowTls` / `Camouflage`，`EngineShared.roots`（根证书库随引擎存续），对 sing-box `shadowtls` 入站的互操作用例。M2（TLS 族）至此完成。M3（策略组、订阅与连通性测试）按三份计划推进：M3a（成员装配与订阅）已完成——`rurge-config::spec::GroupSpec`（组参数的读取与校验；`W0030` 组环告警取代 `E0009`；组级 `underlying-proxy` 成环是 `E0019`；脱敏名单加 `policy-path` / `external-policy-modifier`）、`rurge_config::policy::with_params`；`rurge-policy` 的 `subscription`（订阅文本 → 策略行，坏行只报行号）与 `assemble`（四类来源的成员装配、过滤 / 前缀 / 修饰、全局重名、导入行的中继成环检查、组级中继派生 `M (via R)`、组环）；注册表接受装配结果（导入 / 派生条目按指纹复用、构建失败只略去该条、环上的组 REJECT、空组按 `EmptyGroup` 兜底、视图数据 `Line` / `GroupInfo`）；`rurge-net` 资源管理器的日志标签（`get_labelled`，订阅 URL 不进日志）与离线读缓存 `cached`；`rurge-engine` 的订阅接入（构建时同步载入缓存）、订阅热重建任务（去抖 1 秒、代际锁、被替换的代不再发布）、拨号与视图改读 `EngineShared.cell`（`Runtime.policies` 去掉，`Engine::registry()`）、`EngineShared.empty_group`、`check_profile`；bin 的 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT=true` 与 `rurge check --data-dir`。M3b（测速与自动组）已完成——`[General] test-timeout` 区分未设置（`General::test_target`；策略的 `test-url` / `test-timeout` 生效，`W0029` 对它们退役）；`rurge-policy` 的 `probe`（经策略自己的出站在一条连接上两次 HEAD，HTTPS 在已建立的 TLS 连接上测第二次，根证书取 `OutboundFactory::roots`）、`testbook`（`TestBook`：结果按策略与定义保存、同一策略同时只测一次、最多 8 个并发、`TestObserver`、一次性的 `test_once`）与 `auto`（`url-test` / `fallback` / `load-balance` 三种选法、`SelectCtx`、`AutoGroups`：临时覆盖、`url-test` 保持的成员、每组上一轮测试的时间与测试请求）；注册表按测试结果选成员（只有拨号触发测试、`evaluate-before-use` 的 `Resolution.pending`、嵌套组的分数、`round_timeout`）、`resolve_relay`（空组当中继一律拒绝）、订阅行不能动用主配置的 `client-cert` 与策略（`W0023`）；`rurge-engine` 的测试调度任务、测试会话进请求记录（规则 `policy test`）、`evaluate-before-use` 的有界等待（`policy group evaluation failed`）、重载时保留未变组的覆盖、`Engine::snapshot`（运行时与注册表在代际锁下成对读取）与 DNS 会话在链底为 REJECT 时的旁路；`rurge-api` 的 `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` 与 `POST /v1/policy_groups/select` 对自动组的临时覆盖；能力表翻转 `url-test` / `fallback` / `load-balance`（`W0008` 只剩 `smart` 与 `subnet`）。M3c（`smart`）已完成——`rurge-policy::smart`（`SmartBook`：按策略的时间加权首字节分数与失败罚分、健康 / 未知 / 失败三种状态、站点记忆、使用计数；`rank` 给出选中的成员与重试列表；`sample` 给大组的常规轮抽样）、注册表的 `smart` 成员过滤与 `policy-priority` 因子、`Resolution.smart`（`SmartPick`）、`resolve_member`、`test_round`，以及每代预存的测试查表（`TestBook::outcome`，M3b 延后事项 #14）；`rurge-inbound` 的 `SessionHandle` 两个时刻（出站就绪、首字节）、可挂多个的结束钩子与首字节钩子、`mark_upstream_failed`；`rurge-engine` 的 `smart` 模块（拨号时按重试列表换成员，单次时限"剩余时间 ÷ 剩余尝试次数"；质量回报：首字节、3 秒无响应、上游先断）、请求记录的 `connectMs` / `firstByteMs`、`automatic()` 纳入 `smart`、每次发布注册表时 `SmartBook::retain`；能力表翻转 `smart`（`W0008` 只剩 `subnet`）。M4（WireGuard / SSH / external）按三份计划推进（M4a SSH → M4b WireGuard → M4c external）：M4a（SSH）已完成——`rurge-config::spec::SshSpec`（`server-fingerprint` 解析为 `HostKeyPin`、`idle-timeout`、`private-key` 的 `E0020`）与 `ProtoSpec::keystore_item`（重载的指纹含 SSH 私钥）；新 crate `rurge-proto-ssh`（russh 0.63.3，`ring` 后端）：Keystore 私钥解码（Ed25519 / ECDSA / RSA；带口令与 DSA 为 `E0022`）、主机密钥比对、`SshOutbound`（每个策略一条会话、单飞握手、认证失败等持久失败后按 60 秒起翻倍至 10 分钟退避、钉住的主机密钥算法优先协商、`direct-tcpip` 通道、会话断了重建一次、按通道计的空闲断开、30 秒 × 3 的保活、没配指纹的一次性告警、RSA 只用 SHA-2 签名）与 `testing::FakeSsh`；`rurge-engine` 的工厂分支；订阅行自己的 `private-key=` 进订阅安全门（`W0023`）；`tests/interop` 的临时 `sshd`（`RURGE_TEST_SSHD`，只在 Unix）；能力表翻转 `ssh`。M4b（WireGuard）已完成——`rurge-config` 的 `[WireGuard <name>]` 类型化（`WireGuardSection`，`E0023`，重名的节 `W0020`）与 `spec::WireGuardSpec`（spec 带着节的内容；`underlying-proxy` 为 `W0029` 且策略 REJECT；订阅行自己的 `section-name=` 进订阅安全门）；`rurge-net` 的 `Datagram`、`Connector::connect_udp` 与 `DirectConnector::connect_udp`（按 `ip-version`、`set_tos`、7 MiB 收发缓冲）；新 crate `rurge-proto-wireguard`（boringtun 0.7.1 的 sans-IO `Tunn` 加 smoltcp 0.12：`routes` 最长前缀选 peer、`wire` 的 `client-id`、`stack`（锁内只做内存操作，每次推进都 poll 到发完，Reno 拥塞控制）、`device`（每条隧道一个任务、批量收、握手 TOS 0x88、5 分钟重拨与 endpoint 跟随、发送时本机地址或路由失效或握手无回应时换新载体（网络变化后自愈）、网络变化入口、同一私钥与 peer 只留一条隧道，节与载体设置都相同的策略共用）、`stream`、`dns`（隧道内 DNS 与缓存；每个问题 2 秒内重发、先等隧道第一次握手）、`outbound`（`WireGuardOutbound`：没有 `dns-server` 时在本机解析、含 `[Host]`；原生握手测速）与 `testing::FakeWgPeer`）；`rurge-proto` 的 `Outbound::native_test`；`rurge-policy` 的 `TestMode`（URL 与原生两种测速，`wireguard` 另加 10 秒）；`rurge-engine` 的工厂分支、拨号时 `Unsupported` 带说明、DNS 会话防环扩展到 endpoint 写成域名的 `wireguard`；bin 不输出 boringtun 自己的日志；`tests/interop` 对 sing-box WireGuard 端点（带保留字节）的互操作用例；能力表翻转 `wireguard`。M4c（external）已完成——`rurge-config::spec::ExternalSpec`（`args` 是 `Secret`；两个 `external` 策略写了同一个 `local-port` 是 `E0018`；`args` 进内联参数的脱敏名单）、订阅导入的 `external` 行一律跳过（`W0023`）；`rurge-platform::process`（Unix 进程组与 `killpg`，Windows Job Object）；`rurge_proto::external`（`ExternalOutbound`：第一次用到时拉起、输出写 `<数据目录>/external/<策略名>.log` 且超过 1 MiB 时在拉起前轮转、子进程环境去掉代理变量并设 `NO_PROXY=*`、程序退出后下次用到时再拉起且两次拉起至少隔 2 秒、每 500 ms 连一次本机端口最多 6 次；`ProcessHook` / `ProcessGroup` 与 `NoProcessGroups`；程序自己退出时连同它启动的进程一起结束）；`rurge-engine` 的工厂分支、`EngineShared.processes` / `externals` 与 `Engine::stop_external_programs`；bin 的 `PlatformProcesses` 与退出流程最后停掉全部外部程序；只用于测试的工作区成员 `tests/external`（`rurge-external-tests`：极小的 SOCKS5 辅助程序 `socks-helper` 与真实拉起它的用例）；能力表翻转 `external`（`W0007` 不再因 `external` 出现）。M4 至此完成。M5（UDP 路径）按三份计划推进（M5a 地基 → M5b TLS 族 → M5c WireGuard 与其余）：M5a 已完成——`rurge_net::connector::PacketSocket`（按包收发、带地址的 UDP 载体）与 `Connector::open_udp`（`DirectConnector`：每个地址族一个未连接的 socket，忽略 Windows 的 ICMP 不可达报错）；`Outbound::udp()` / `open_udp()` 与 `UdpSupport`；DIRECT 与 `socks5` / `socks5-tls` / `external` 的 UDP（`udp-relay`，`W0029` 退役）；`ChainConnector::open_udp`（链式 UDP 载体）；`rurge-inbound` 的 SOCKS5 UDP ASSOCIATE（`UdpClient`、`UdpAdmission`、`Dialer::admit_udp` / `associate`）；`rurge-engine` 的 UDP 流水线（`udp` 模块：一条流 = 关联 + 目标、按"关联 × 出站"共用载体的全锥、60 秒 / DNS 10 秒回收、1024 流 / 4096 关联的上限）、请求记录与 API 的 `transport`、`block-quic`（`W0029` 退役）与 `udp-policy-not-supported-behaviour`、QUIC Initial 识别、`PROTOCOL` 规则按传输层匹配 `TCP` / `UDP`；`FakeSocks5` 与 `tests/external` 辅助程序的 UDP ASSOCIATE；对 sing-box `socks` 入站的 UDP 互操作用例。M5b（TLS 族的 UDP）已完成——`rurge-proto` 的 `stream_udp`（一条字节流上按包收发：请求头随第一个包发出、写那个包的目标；`trojan` 的 UDP ASSOCIATE 与 `anytls` 的 UDP over TCP v2 两种封装）、`trojan` / `anytls` 的 `open_udp`（全锥）、`vmess` 的命令 2（`vmess::udp`：每个目标一条 VMess 连接、随它的第一个包建立、每个数据报一个分块，对称型）；`FakeTrojan` / `FakeAnyTls` / `FakeVmess` 的 UDP 与 `rurge_proto::testing::udp_echo_server`；经引擎的端到端用例（`tests/udp_tls_family.rs`）；对 sing-box（三种）与 xray（vmess）的 UDP 互操作用例。M5c（WireGuard 的 UDP 与其余）已完成——`rurge-proto-wireguard` 的 `TunnelUdp`（隧道里每个地址族一个 UDP socket、第一次发往该族时绑定、全锥，目标名经隧道 DNS 或本机解析）与 `Stack::udp_bind` / `check`；`rurge_net::packet_datagram`（把 `PacketSocket` 变成一条到固定目标的 `Datagram`）与 `ChainConnector::connect_udp`，`wireguard` 的载体经 `underlying-proxy`（底层策略不载 UDP 时拨号失败，`W0029` 退役）；启动时 peer 连不上的告警每个策略的每个 peer 5 分钟至多一次（M4b 延后事项 #15）；`rurge_policy::udp_probe`（经策略的 UDP 向 `hostname@ipv4` 问一次 A 记录）、`Engine::test_udp` 与 `POST /v1/policies/test` 结果里的 `udp` 键（`test-udp` / `proxy-test-udp`，不保存、不参与组的选择）；`smart` 计入 UDP（载体打不开算失败、第一个回包算首字节、3 秒无回包只在 53 / 443 端口算失败、UDP 不换成员）；`dns-follow-interface`（`rurge_net::connector::Via` / `ResolveVia`、`Resolver::lookup_via`：策略自己的解析经它的 `interface` 问普通 DNS 服务器、答案另存，配了加密 DNS 时不跟随；没有 `interface` 时 `W0028`）；对 sing-box WireGuard 端点的 UDP 互操作。M5 至此完成。M6（Shadowsocks / Snell / HTTP/2 族）按三份计划推进（M6a Shadowsocks → M6b Snell → M6c HTTP/2 族）：M6a（Shadowsocks）已完成——`rurge-config::spec` 的 `SsSpec`（`SsMethod`：AEAD 五种、`none`、SS 2022 两种；2022 的 `password` 按冒号拆成逐层的 Base64 密钥、最后一段是用户密钥，不合法是 `E0018` 且不引用取值；`udp-relay`、`udp-port`）与 `ObfsOpts`（与 Snell 共用；`obfs-host` 缺省服务器主机名、`obfs-uri` 缺省 `/`）、`NotImplemented`（取代 `legacy_vmess` 标记：vmess 旧握手与 `ss` 流式旧方法都是 `W0007` + REJECT，流式方法每种每次加载一条，会话日志 `policy protocol not implemented: ss (<method>)`）、`obfs-host` 进内联参数的脱敏名单；`rurge_proto::transport::obfs`（simple-obfs 的 `http` / `tls`，自写模板，`Stack` 的一层：connect → shadow-tls → obfs → tls → ws）；`rurge_proto::shadowsocks`（`ShadowsocksOutbound`：AEAD 分块流与 `none`、SS 2022（BLAKE3 子密钥、请求头块与填充、应答的类型 / 时间戳 / 回显 salt 校验、SIP023 多用户身份头）、UDP（`udp-relay=true` 发往 `udp-port`，全锥；2022 的分离头、按服务端 session 的防重放窗口））；新依赖只有 `blake3`；`rurge_proto::testing` 的 `FakeShadowsocks`（AEAD、2022 含身份头、UDP、两种 obfs）与 `accept_obfs`；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_shadowsocks.rs`）；能力表翻转 `ss`（流式旧方法除外）；测试里"未实现的协议"的例子从 `ss` 换成 `hysteria2`；`tests/interop` 对 shadowsocks-rust `ssserver`（固定版本 v1.25.0，`RURGE_TEST_SSSERVER`）与 sing-box `shadowsocks` 入站的互操作用例。M6b（Snell）已完成——`rurge-config::spec` 的 `SnellSpec`（`SnellVersion` 只有 V4 / V5；`version` 1–6、缺省 1，1–3 与 6 是 `NotImplemented::SnellVersion`：`W0007` + REJECT，每个版本每次加载一条，会话日志 `policy protocol not implemented: snell v<n>`；`psk` 是 `Secret`；v4 / v5 的 `obfs` 只有 `http`；`mode` 只对 v6 校验）；`rurge_proto::snell`（`SnellOutbound`：`kdf`（Argon2id 取前 16 字节，在阻塞线程上算；工作区依赖 `argon2` 0.6，已在依赖树里）、`record`（`SnellStream`：每方向一个 salt、7 字节加密头、首帧 256–511 字节填充与负载密文交错、每帧至多 0x3FFF、空帧结束一个方向、复用时计数器延续）、`tunnel`（请求头随首段负载写出，`reuse=false` 发 Connect `0x01`、`reuse=true` 发 ConnectV2 `0x05`；应答 `00` / `02`；干净结束的连接在 `Drop` 时回池，没结束的在后台补完再回池；池里取出的失效连接换新连接重发一次）、`pool`（每个出站至多 8 条空闲连接、空闲 60 秒回收）、`udp`（命令 `0x06`：每个载体一条自己的连接、从不进池，每个数据报一帧，全锥；`udp-port` 是这条连接所连的端口））；`LazyHead` 改为泛型（`LazyHead<S = BoxedStream>`，加 `get_mut` / `into_inner`）；`rurge_proto::testing` 的 `FakeSnell`（`SnellScript`：TCP、ConnectV2 复用、UDP、obfs http、拒绝、每条连接的请求数上限）；`rurge-engine` 的工厂分支与经引擎的端到端用例（`tests/outbounds_snell.rs`）；能力表翻转 `snell`（v4 / v5）；`tests/interop` 对 sing-box `snell` 入站（`version: 5`，同时接受 v4 客户端；sing-box 固定版本从 1.14.1 升到 1.14.2）与 Surge 官方 snell-server v5.0.1（只有 Linux 版，`RURGE_TEST_SNELL_SERVER`，只在 Linux CI 上装）的互操作用例。M6c（HTTP/2 族）已完成——`rurge-config::spec::h2`（`H2ConnectSpec`：凭据写在端口之后或命名、命名的优先，`headers`、`max-streams` 缺省 3 且至少 1、`udp-relay`；`TrustTunnelSpec`：`username` / `password` 只读命名写法、必填（`E0018`）；两者 `alpn` 固定 `h2`（写了别的 `W0028`），`headers` 里 HTTP/2 禁止的连接专用字段各 `W0028` 并去掉；`trust-tunnel` 的 `h3=true` 是 `W0029`、M7 之前按 HTTP/2 连，`udp-relay` 是 `W0028`）；`rurge_proto::h2pool`（`H2Pool`：每个出站一个池，每条 TLS + HTTP/2 连接至多 `max-streams` 条流且不超过服务端的上限、流数自己计，新连接收到服务端 SETTINGS 之前只承载一条流，同时进来的请求共用一次拨号，没有流的连接空闲 60 秒关闭，收到 GOAWAY 的连接不再分配新流；`H2Stream`：按流控窗口分段写、读即释放容量、END_STREAM 半关闭；`StackDial`：ALPN 不是 `h2` 时 `<协议>: the server does not speak HTTP/2`）；`rurge_proto::h2connect`（`H2ConnectOutbound`：CONNECT + Basic，`headers` 替换同名的自生成字段、`<random-string>` 按请求生成；407 是 `h2-connect: proxy authentication required`；`capsule`（RFC 9297 的 capsule 与 varint）与 `udp`（`udp-relay=true` 时的 CONNECT-UDP：每个目标一条 extended CONNECT 流、对称型，masque 默认模板的路径，DATAGRAM capsule，其它 capsule 与 context id 丢弃，超过 65527 字节的数据报不发，服务端没通告 extended CONNECT 时发往每个目标的第一个包失败，空闲 60 秒关流））；`rurge_proto::trust_tunnel`（`TrustTunnelOutbound`：同一套 CONNECT，`user-agent: rurge`，407 是 `trust-tunnel: authentication failed`，不载 UDP）；`TlsClient::wrap_negotiated` / `Stack::open_negotiated`（取协商出的 ALPN）；`rurge_proto::testing` 的 `FakeH2Proxy`（`H2ProxyScript`：Basic 与 407、拒绝、服务端流数上限、GOAWAY、extended CONNECT 与 CONNECT-UDP、多余的 capsule、`user-agent` 必填、不参与 ALPN、双向 TLS）；`rurge-engine` 的两个工厂分支与经引擎的端到端用例（`tests/outbounds_http2.rs`）；能力表翻转 `h2-connect` 与 `trust-tunnel`；`tests/interop` 对 TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，`RURGE_TEST_TRUSTTUNNEL`；sing-box 1.14.2 的 `http` 入站只说 HTTP/1，所以 endpoint 同时充当 `h2-connect` 普通 CONNECT 的参照）的互操作用例；CONNECT-UDP 没有可用的参照服务端。M6 至此完成。
```

`CLAUDE.md`——把

```markdown
- `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`：阶段 2 / M6 细化设计（Shadowsocks / Snell / HTTP/2 族），细化总设计的 M6 里程碑、不一致处以它为准，并关闭总设计的开放问题 Q7。三份计划的拆分（M6a Shadowsocks → M6b Snell → M6c `h2-connect` / `trust-tunnel`）；已决事项 M6-D1 ～ D8（四种协议都做、Snell 只实现 v4 / v5 而 `version` 缺省为 1 的行按 v1 拒绝、GPL 参考实现只取协议事实、新依赖只有 `blake3`、各协议的互操作参考、sing-box 升到有 `snell` 入站的 1.14.x、v5 的动态帧大小只影响发送方、`h2-connect` 的 UDP 每个目标一条流）；各协议的配置、线上格式、错误与三层测试；第 7 节是需登记的差异，第 9 节 V1 ～ V10 是写各份计划时必须核对的事项，第 10 节是三份计划的任务草图，第 12 节是 M6a 计划期的订正，第 13 节是 M6a 实施期的订正，第 14 节是 M6b 计划期的订正，第 15 节是 M6b 实施期的订正。
- `docs/superpowers/plans/2026-09-30-phase2-m6a-shadowsocks-plan.md`：阶段 2 / M6a（Shadowsocks）实施计划（7 个任务）。开头「计划期决定」表记录核对参考实现、SIP022 / SIP023 原文与本仓库得出的结论和与设计文字不同的决定（加密只用 RustCrypto 且 `chacha20poly1305` 沿用依赖树里的 0.10、`LazyHead` 在加密流之上、SS 2022 与 AEAD 共用一种流且在应答头块校验、填充内容为零只有长度随机、UDP 包号从 0 开始并校验回显的客户端 session id、每个载体至多记 8 个服务端 session、obfs 的 `Host` 总带非 80 的端口与首包 16 KiB 上限、不接受 `2022-blake3-chacha20-poly1305`、测试里"未实现的协议"的例子换成 `hysteria2`、sing-box 覆盖 ssserver 发布包没有的 `aes-192-gcm` / `xchacha20-ietf-poly1305` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-30-phase2-m6b-snell-plan.md`：阶段 2 / M6b（Snell v4 / v5）实施计划（6 个任务）。开头「计划期决定」表记录核对公开协议描述、参考实现与本仓库得出的结论和与设计文字不同的决定（`reuse=false` 发 Connect、`reuse=true` 发 ConnectV2、主机名一律按文本写、发送固定以 0x3FFF 为帧上限、复用池至多 8 条并对失效的池连接重试一次、UDP 不走 `stream_udp` 而按帧收发、`udp-port` 是 UDP 会话那条 TCP 连接的端口、`obfs-host` 缺省仍是服务器主机名、sing-box 升到 1.14.2 与官方 snell-server 只在 Linux CI 上跑等）；末尾「执行期修正记录」与「延后事项」两张表。
```

换成

```markdown
- `docs/superpowers/specs/2026-09-30-phase2-m6-ss-snell-h2-design.md`：阶段 2 / M6 细化设计（Shadowsocks / Snell / HTTP/2 族），细化总设计的 M6 里程碑、不一致处以它为准，并关闭总设计的开放问题 Q7。三份计划的拆分（M6a Shadowsocks → M6b Snell → M6c `h2-connect` / `trust-tunnel`）；已决事项 M6-D1 ～ D8（四种协议都做、Snell 只实现 v4 / v5 而 `version` 缺省为 1 的行按 v1 拒绝、GPL 参考实现只取协议事实、新依赖只有 `blake3`、各协议的互操作参考、sing-box 升到有 `snell` 入站的 1.14.x、v5 的动态帧大小只影响发送方、`h2-connect` 的 UDP 每个目标一条流）；各协议的配置、线上格式、错误与三层测试；第 7 节是需登记的差异，第 9 节 V1 ～ V10 是写各份计划时必须核对的事项，第 10 节是三份计划的任务草图，第 12 节是 M6a 计划期的订正，第 13 节是 M6a 实施期的订正，第 14 节是 M6b 计划期的订正，第 15 节是 M6b 实施期的订正，第 16 节是 M6c 计划期的订正。
- `docs/superpowers/plans/2026-09-30-phase2-m6a-shadowsocks-plan.md`：阶段 2 / M6a（Shadowsocks）实施计划（7 个任务）。开头「计划期决定」表记录核对参考实现、SIP022 / SIP023 原文与本仓库得出的结论和与设计文字不同的决定（加密只用 RustCrypto 且 `chacha20poly1305` 沿用依赖树里的 0.10、`LazyHead` 在加密流之上、SS 2022 与 AEAD 共用一种流且在应答头块校验、填充内容为零只有长度随机、UDP 包号从 0 开始并校验回显的客户端 session id、每个载体至多记 8 个服务端 session、obfs 的 `Host` 总带非 80 的端口与首包 16 KiB 上限、不接受 `2022-blake3-chacha20-poly1305`、测试里"未实现的协议"的例子换成 `hysteria2`、sing-box 覆盖 ssserver 发布包没有的 `aes-192-gcm` / `xchacha20-ietf-poly1305` 等）与「承接事项」；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-09-30-phase2-m6b-snell-plan.md`：阶段 2 / M6b（Snell v4 / v5）实施计划（6 个任务）。开头「计划期决定」表记录核对公开协议描述、参考实现与本仓库得出的结论和与设计文字不同的决定（`reuse=false` 发 Connect、`reuse=true` 发 ConnectV2、主机名一律按文本写、发送固定以 0x3FFF 为帧上限、复用池至多 8 条并对失效的池连接重试一次、UDP 不走 `stream_udp` 而按帧收发、`udp-port` 是 UDP 会话那条 TCP 连接的端口、`obfs-host` 缺省仍是服务器主机名、sing-box 升到 1.14.2 与官方 snell-server 只在 Linux CI 上跑等）；末尾「执行期修正记录」与「延后事项」两张表。
- `docs/superpowers/plans/2026-10-01-phase2-m6c-http2-plan.md`：阶段 2 / M6c（HTTP/2 族：h2-connect、trust-tunnel）实施计划（7 个任务）。开头「计划期决定」表记录核对 RFC 9113 / 9298 / 9297、`h2` 0.4 源码、TrustTunnel 协议文档与 endpoint 源码得出的结论和与设计文字不同的决定（`h2-connect` 的凭据两种写法都收、`alpn` 固定 `h2`、连接专用字段加载时去掉、`headers` 替换自生成的同名字段、流数自己计且新连接在服务端 SETTINGS 之前只承载一条流、接收窗口取 1 MiB、`trust-tunnel` 的 `user-agent` 固定 `rurge`、没有 extended CONNECT 时错误出现在第一个数据报上、TrustTunnel endpoint 同时充当 `h2-connect` 的互操作参照等）；末尾「执行期修正记录」与「延后事项」两张表。
```

`CLAUDE.md`——把

```markdown
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（全部协议）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

换成

```markdown
cargo test -p rurge-proto h2                    # HTTP/2 族：会话池（流控、max-streams、GOAWAY、空闲关闭、extended CONNECT）、h2-connect 的 TCP 与 CONNECT-UDP（capsule、masque 路径），出站对回环假服务端（FakeH2Proxy）；trust-tunnel 的用例用过滤词 trust_tunnel
cargo test -p rurge-engine --test outbounds_http2   # 经 h2-connect / trust-tunnel 的端到端用例：凭据与拒绝、max-streams、trust-tunnel 不载 UDP、CONNECT-UDP、没有 extended CONNECT、经 underlying-proxy、重载沿用与替换
cargo test -p rurge-external-tests              # external：真实拉起测试辅助程序（socks-helper）——按需拉起、参数顺序与代理变量、再拉起与 2 秒间隔、连接被拒的重试、日志、整棵进程树的停止、经引擎的端到端、检查不拉起、重载沿用与替换
cargo test -p rurge-interop                     # 对 sing-box 1.14.2（HTTP/2 族之外的全部协议）、xray（只测 vmess）、shadowsocks-rust ssserver（只测 ss）、官方 snell-server v5.0.1（只测 snell，只在 Linux）、TrustTunnel endpoint v1.1.0（测 trust-tunnel 与 h2-connect 的 TCP，只在 Linux / macOS）与 OpenSSH sshd（只测 ssh，只在 Unix）的互操作测试；没装就跳过（RURGE_TEST_SING_BOX / RURGE_TEST_XRAY / RURGE_TEST_SSSERVER / RURGE_TEST_SNELL_SERVER / RURGE_TEST_TRUSTTUNNEL / RURGE_TEST_SSHD / RURGE_INTEROP_REQUIRED=1）
```

总设计（第 13 节 M6 行的参考服务端、开放问题 Q6）：

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| M6 | 按 M6 细化设计 M6-D5：shadowsocks-rust v1.25.0（含 2022 与多用户，三平台）与 sing-box 的 `shadowsocks` 入站：`ss`；sing-box 的 `snell` 入站（1.14.0 起，只支持 v5 / v6；`version: 5` 同时接受 v4 客户端）：`snell`，三平台，CI 固定的 sing-box 为此从 1.14.1 升到 1.14.2（M6-D6）；官方 snell-server v5.0.1 只有 Linux 版，只在 Linux CI 跑；sing-box 的 `http` 入站（HTTP/2 over TLS）：`h2-connect` 的普通 CONNECT；TrustTunnel endpoint v1.1.0 只有 Linux / macOS 版，只在这两个平台的 CI 跑；CONNECT-UDP over HTTP/2 与 obfs 没有可用的预编译参考服务端，只有回环假服务端与手工验收 |
```

换成

```markdown
| M6 | 按 M6 细化设计 M6-D5：shadowsocks-rust v1.25.0（含 2022 与多用户，三平台）与 sing-box 的 `shadowsocks` 入站：`ss`；sing-box 的 `snell` 入站（1.14.0 起，只支持 v5 / v6；`version: 5` 同时接受 v4 客户端）：`snell`，三平台，CI 固定的 sing-box 为此从 1.14.1 升到 1.14.2（M6-D6）；官方 snell-server v5.0.1 只有 Linux 版，只在 Linux CI 跑；TrustTunnel endpoint v1.1.0（只有 Linux / macOS 版，只在这两个平台的 CI 跑）：`trust-tunnel`（h2），它的 TCP 模式是标准的 HTTP/2 CONNECT + Basic 认证，同时充当 `h2-connect` 普通 CONNECT 的参照——sing-box 1.14.2 的 `http` 入站只说 HTTP/1（HTTP/2 要到 1.15.0，仍是预发布），不用于 HTTP/2 族；CONNECT-UDP over HTTP/2 与 obfs 没有可用的预编译参考服务端，只有回环假服务端与手工验收 |
```

`docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`——把

```markdown
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | M7 细化设计时选定 |
```

换成

```markdown
| Q6 | MASQUE 与 trust-tunnel 的参考服务端 | 部分已决（2026-10-01，M6c）：`trust-tunnel` 的 h2 用 TrustTunnel endpoint v1.1.0（Linux / macOS），它也是 `h2-connect` 普通 CONNECT 的参照；MASQUE 与 `trust-tunnel` 的 h3 在 M7 细化设计时选定 |
```

- [ ] **Step 4: 核对**

- `README.md` 与 `README_en.md` 的状态一段、特性表与路线图内容一致。
- 兼容性清单的 `h2-connect` / `trust-tunnel` 行与 P7 ～ P9 的文字一致。

- [ ] **Step 5: 门禁与提交**

跑门禁。副本上最后一次全工作区门禁：fmt、clippy 通过，`cargo test --workspace --no-fail-fast` 1536 通过、0 失败、2 忽略。

```bash
git add tests/interop .github docs README.md README_en.md CLAUDE.md
git commit -m "docs: M6c HTTP/2 族——兼容性清单、手工验收、README 与 CLAUDE.md；TrustTunnel endpoint 的互操作"
```

## 验收对照（设计第 1 节与第 5 节，M6c 部分）

| # | 验收项 | 由谁保证 |
| - | ------ | -------- |
| 1 | `h2-connect` / `trust-tunnel` 对参考实现的 TCP 转发通过 | Task 7 的 TrustTunnel endpoint 用例（Linux / macOS CI）；Task 3 / 4 的回环往返 |
| 2 | `h2-connect` 的 UDP 转发 | Task 5 经 `FakeH2Proxy` 的往返；Task 6 `udp_goes_through_h2_connect_as_connect_udp`；真实服务端靠手工验收（P10） |
| 3 | `max-streams` 与会话池 | Task 2 / 3 / 4 的池用例；Task 6 `max_streams_bounds_the_sessions_on_one_connection`；Task 7 的并发互操作用例 |
| 4 | 能力表不再为两种协议出 `W0007` | Task 6 `check_knows_http2` |
| 5 | 门禁全绿 | 各任务的门禁 |
| 6 | 需要真实节点的项目进手工验收清单 | Task 7：`docs/acceptance/phase2-manual.md` 的 M6c 一节 |

## 执行期修正记录

| # | 任务 | 与计划的出入 | 原因 |
| - | ---- | ------------ | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | `trust-tunnel` 的 `h3=true`（HTTP/3） | M7 |
| 2 | CONNECT-UDP over HTTP/2 没有可用的参考服务端（P10） | 手工验收；有参考服务端时补互操作 |
| 3 | `h2-connect` 对 endpoint 不发 `user-agent` 是否可行未实测（P10） | CI 首跑 |
| 4 | 服务端有 ALPN 但与 `h2` 不重合时的报错是 rustls 的原文（P7） | 接受 |
| 5 | 一个请求在挑中连接之后、发出之前连接死掉时不重试（P5） | 有用户报告再说 |
| 6 | 互操作数不了 endpoint 那边的连接数（连接数由 `FakeH2Proxy` 覆盖） | 接受 |
