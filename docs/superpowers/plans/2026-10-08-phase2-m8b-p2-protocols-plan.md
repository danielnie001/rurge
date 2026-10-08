# 阶段 2 / M8b（P2 协议项与 `W0007` 的归宿）实施计划

> **执行方式**：子代理驱动（superpowers:subagent-driven-development）——每个任务一个新的实现者，做完由新的评审者评审，全部做完后整分支评审一次。按 M8 细化设计 M8-D2，本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止：实现者先写失败的用例、看到红，再实现到绿；与计划的出入记进末尾的「执行期修正记录」。

**目标**：`ss` 能用手册列出的 11 种流式方法，`snell` 能说 v1 ～ v3，没写 `vmess-aead=true` 的 `vmess` 行不再被拒；`W0007` 只剩 rurge 不实现的协议与变体。

**做法**：三处都落在已有的结构里——`ss` 多一种"IV 加密钥流"的流与包，`snell` 多一种 Shadowsocks AEAD 式的分块记录流（请求头、应答、池、UDP 的数据报沿用 v4 的），`vmess` 只动配置层。不新增 crate。

**依据**：M8 细化设计（`docs/superpowers/specs/2026-10-08-phase2-m8-wrapup-design.md`）第 4 节；调研的出处在那里列着。

## 全局约束

- 不新增 crate。`rurge-proto` 只加两条依赖边：`chacha20`（0.9，已在锁文件）与 `salsa20`（0.10，已在锁文件）；`Cargo.lock` 里不出现新的包。
- 全工作区 `forbid(unsafe_code)`；MSRV 1.89，edition 2024。
- 凭据（口令、PSK、密钥、IV 之外的密钥材料）不进日志、错误文本、`Debug` 与 API 输出；新的凭据字段写进 `crates/rurge-config/tests/secrets.rs` 的标记配置。
- 测试只用回环地址与端口 0，等待都有时限，不用固定的 sleep 当同步；不联网，不改系统设置。
- 不下载、不安装任何东西（参照服务端只在 CI 上装；本机没有它们，互操作用例在本机跳过）。GPL 的实现（mihomo、open-snell、shadowsocks-libev）只取协议事实，不抄代码。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明的标题用中文。
- 门禁：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`（本机 `RUSTFMT` 指向 gnu 工具链的 rustfmt）。测试二进制无失败用例的异常退出（`STATUS_HEAP_CORRUPTION`）单独重跑那个二进制。

## 评审要点

计划的用例之外最可能咬到使用者的五种情形，各自归到拥有那段代码的任务里：

1. 服务端的 IV 被 TCP 拆成几段到达（或与第一段密文粘在一起）：流式的读端要攒够 IV 再解，不能把半个 IV 当密文（任务 2）。
2. `rc4` 没有 IV：读端不能等 IV 的字节，UDP 包整个是密文；空的 UDP 负载也要能往返（任务 2）。
3. Snell v2 池里的连接已被服务端关掉：与 v4 一样换新连接重发一次，不把 `snell: … closed` 抛给使用者（任务 3）。
4. 今天靠"没写 `version` 即被拒"过日子的配置：不写 `version` 的行现在真的去连 v1——口令错或对端其实是 v4 时，错误文本要说得出"口令或版本不对"，不能挂到空闲超时（任务 3）。
5. CTR 的计数进位：IV 的低位全是 `ff` 时跨到下一块要按 128 位进位（任务 2；向量里有一条专门的）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| ---- | ---- | ---- |
| `crates/rurge-config/src/diagnostic.rs` | 1 | 新码 `W0031`（`W_STREAM_CIPHER`） |
| `crates/rurge-config/src/spec/mod.rs` | 1、2、3 | `Remark`（每次加载按种类报一次的提示）；`NotImplemented` 去掉 `LegacyVmess`、文本改写 |
| `crates/rurge-config/src/config.rs` | 1 | 按种类去重地报 `Remark`；按协议种类的 `W0007` 文本 |
| `crates/rurge-config/src/spec/{vmess,snell,hysteria2,common,ss}.rs` | 1 | `vmess-aead`、`mode`、混淆口令长度、`ecn`、`udp-port` |
| `crates/rurge-policy/src/assemble.rs` | 1 | 订阅导入的行同样报 `Remark` |
| `crates/rurge-config/src/spec/ss.rs` | 2 | `SsMethod` 的 11 个流式取值 |
| `crates/rurge-proto/src/shadowsocks/stream.rs`（新） | 2 | 密钥流、`CipherStream`、UDP 包的封与解 |
| `crates/rurge-proto/src/shadowsocks/{mod,udp}.rs`、`testing/shadowsocks.rs` | 2 | 接进出站与假服务端 |
| `crates/rurge-config/src/spec/snell.rs` | 3 | `SnellVersion` 的 v1 ～ v3 |
| `crates/rurge-proto/src/snell/chunked.rs`（新） | 3 | v1 ～ v3 的分块记录流 |
| `crates/rurge-proto/src/snell/{mod,record,kdf,tunnel,pool,udp}.rs`、`testing/snell.rs` | 3 | 两种记录流共用隧道、池与 UDP |
| `crates/rurge-engine/tests/{outbounds_shadowsocks,outbounds_snell,outbounds_tls_family}.rs`、`crates/rurge/tests/cli.rs` | 1、2、3 | 经引擎的端到端与 CLI 的诊断 |
| `tests/interop/`、`.github/workflows/ci.yml` | 4 | 互操作与 mihomo 的安装 |
| 文档 | 5 | 兼容性清单、PRD、手工验收、CLAUDE.md、本计划的两张表 |

---

## Task 1：配置层——`vmess-aead`、`Remark`、`W0007` 的文本与三处小项

**文件**：`crates/rurge-config/src/{diagnostic.rs,config.rs}`、`src/spec/{mod,vmess,snell,hysteria2,common,ss}.rs`、`crates/rurge-policy/src/assemble.rs`、`crates/rurge-policy/src/registry.rs`（用例里的旧握手例子）、`crates/rurge/tests/cli.rs`、`crates/rurge-api/tests/api.rs`、`crates/rurge-engine/tests/{pipeline,outbounds_tls_family}.rs`（凡断言旧文本或旧行为的用例）。

**接口**（后面的任务照这些名字用）：

```rust
// diagnostic.rs
/// An `ss` stream cipher is in use: no integrity protection.
pub const W_STREAM_CIPHER: &str = "W0031";

// spec/mod.rs
/// What a valid line makes rurge say once per load and kind: the policy is
/// built all the same.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Remark {
    /// `vmess` without `vmess-aead=true`: the AEAD handshake is used anyway.
    VmessAead,
    /// An `ss` stream cipher, by its name (任务 2 开始产生).
    SsStreamCipher(&'static str),
}
impl Remark {
    pub fn code(&self) -> &'static str;      // W0029 / W0031
    pub fn warning(&self) -> String;         // the load warning
    pub fn imported(&self) -> String;        // how a warning about imported policies names it
}
pub struct SpecOutcome { /* 原有字段 */ pub remarks: Vec<Remark> }
```

**要做的**：

1. **`vmess`**：`to_spec` 对没写 `vmess-aead=true`（或写了 `false`）的行照常给出 spec，并 `remarks.push(Remark::VmessAead)`；`NotImplemented::LegacyVmess` 删掉。文本：`` `vmess-aead=false` has no effect: rurge always uses the AEAD handshake, which every server since V2Ray 4.28.1 accepts ``（`W0029`）。
2. **`Remark` 的上报**：`config.rs` 里与 `not_implemented_seen` 并排按 `Remark` 去重，每种每次加载一条，位置是第一条这样的策略行；`rurge-policy` 的 `assemble.rs` 对导入的行每个组、每种每次装配一条（文本形如 `` policy group `G`: imported `vmess` policies without `vmess-aead=true` use the AEAD handshake ``，与那里已有的 `W0007` 写法并排）。
3. **`W0007` 的文本**（码不变，M8-D10）：
   - 按协议种类的（现在只剩 `tailscale`）：`` policy type `tailscale` is not supported; a tailnet is reached through tailscaled's SOCKS5 proxy with a `socks5` or an `external` policy; such policies behave as REJECT ``。其它种类（将来若有）用不带出路的通用句：`` policy type `<kind>` is not supported; such policies behave as REJECT ``。
   - `NotImplemented::SnellVersion(6)`：`` `snell` version 6 is not supported: it is a beta protocol without a public specification; such policies behave as REJECT ``（任务 3 之前 1 ～ 3 的文本照旧）。
   - `NotImplemented::SsStreamCipher` 的文本任务 2 改，这里不动。
   - 会话日志的 `policy protocol not implemented: <类型>` 不变。
4. **`snell` 的 `mode`**：写在 `version` 不是 6 的行上是 `E0018`：`` `mode` only applies to `snell` version 6 ``（原来是 `W0028` 并忽略；手册："is a profile error"）。
5. **`ecn`**：写在不走 QUIC 也不是 `wireguard` 的策略上，从 `W0029` 改为 `W0028`（`` `ecn` does not apply to `<type>` policies; ignored ``，照那里 `W0028` 已有的句式）。`wireguard` 上照旧 `W0029`（M5-D6：只解析）。
6. **Hysteria 2 的混淆口令**：`salamander-password` / `gecko-password` 短于 4 字节时 `W0012`：`` `<参数>` is shorter than 4 bytes, which the reference server does not accept ``（不引用取值），照常使用。
7. **`ss` 的 `udp-port`**：写了却没有 `udp-relay=true` 时 `W0028`：`` `udp-port` only applies with `udp-relay=true`; ignored ``。

**先写的用例**（各在所属文件的测试模块；名字可照文件里的习惯调整，断言不能少）：

- `spec::tests`：`vmess` 行不写 `vmess-aead` → 有 spec、`remarks == [VmessAead]`、`not_implemented == None`；写 `vmess-aead=true` → 没有 remark；写 `false` → 同不写。
- `config::tests`：两条不写 `vmess-aead` 的 `vmess` 行 → 恰好一条 `W0029`，文本如上；不再有 `W0007`。
- `spec::snell::tests`：`version=4, mode=default` → `E0018`；`version=6, mode=unshaped` → 没有关于 `mode` 的诊断。
- `spec::common::tests`（或 `spec::tests`）：`trojan … ecn=on` → `W0028`；`hysteria2 … ecn=off` → 无；`wireguard … ecn=on` → 照旧 `W0029`。
- `spec::hysteria2::tests`：`salamander-password=abc` → `W0012` 且文本不含 `abc`；四字节的不告警。
- `spec::ss::tests`：`udp-port=8389` 不带 `udp-relay` → `W0028`；带 `udp-relay=true` → 无。
- `assemble::tests`：订阅里不写 `vmess-aead` 的 `vmess` 行是组的成员（不再是 REJECT 成员），一条导入告警。
- `crates/rurge/tests/cli.rs`：原来断言"旧握手一条 `W0007`"的用例改为断言一条 `W0029`、没有 `W0007`；`tailscale` 的 `W0007` 文本含 `tailscaled`。
- `crates/rurge-engine/tests/outbounds_tls_family.rs`：一条不写 `vmess-aead` 的 `vmess` 策略经回环的 `FakeVmess`（它只说 AEAD）转发成功。

**判据**：上面的用例先红后绿；全量门禁通过；`grep -rn "LegacyVmess" crates` 没有结果。

**提交**：`feat(config): vmess 没写 vmess-aead 时照样用 AEAD 握手；W0007 的文本；三处参数诊断`。

---

## Task 2：`ss` 的流式方法

**文件**：`crates/rurge-config/src/spec/{ss,mod}.rs`；`crates/rurge-proto/Cargo.toml`（两条依赖边）、根 `Cargo.toml`（工作区依赖 `chacha20 = "0.9"`、`salsa20 = "0.10"`）；`crates/rurge-proto/src/shadowsocks/stream.rs`（新）、`shadowsocks/{mod,udp,kdf}.rs`、`testing/shadowsocks.rs`；`crates/rurge-engine/tests/outbounds_shadowsocks.rs`；`crates/rurge/tests/cli.rs`；`crates/rurge-config/tests/secrets.rs`（加一条流式方法的行）。

**接口**：

```rust
// rurge-config spec/ss.rs —— SsMethod 追加（METHODS 表里按手册的名字）：
Rc4, Rc4Md5, Aes128Cfb, Aes192Cfb, Aes256Cfb, Aes128Ctr, Aes192Ctr, Aes256Ctr,
Salsa20, ChaCha20, ChaCha20Ietf,
impl SsMethod {
    /// `Some(n)`: a stream cipher whose connections and packets start with
    /// `n` bytes of IV (0 for `rc4`); `None`: not a stream cipher.
    pub fn iv_len(self) -> Option<usize>;
    pub fn is_stream(self) -> bool;   // iv_len().is_some()
    // key_len() 扩到流式方法（见下表）
}
// SsRead::stream_cipher 改名 unsupported_cipher：只剩手册没列的 9 个名字。
// NotImplemented::SsStreamCipher 改名 SsCipher，文本：
//   "`ss` cipher `<m>` is not supported (Surge no longer lists it); such policies behave as REJECT"
//   note() 仍是 "ss (<m>)"

// rurge-proto shadowsocks/stream.rs
/// One direction's keystream of a stream cipher. No `Debug`.
pub(crate) struct Keystream(/* per-method state */);
impl Keystream {
    /// `key` is the master key (`evp_bytes_to_key`), `iv` of `method.iv_len()` bytes.
    pub(crate) fn new(method: SsMethod, key: &[u8], iv: &[u8]) -> Keystream;
    pub(crate) fn encrypt(&mut self, data: &mut [u8]);
    pub(crate) fn decrypt(&mut self, data: &mut [u8]);   // differs from encrypt only for CFB
}
/// The stream of a stream cipher over `inner`: our IV in front of the first
/// write, the server's IV in front of what it sends.
pub(crate) struct CipherStream { /* … */ }               // AsyncRead + AsyncWrite
/// A datagram: a fresh IV, then `address ‖ payload` under it.
pub(crate) fn seal_packet(method: SsMethod, key: &[u8], plain: &[u8]) -> io::Result<Vec<u8>>;
pub(crate) fn open_packet(method: SsMethod, key: &[u8], packet: &[u8]) -> io::Result<Vec<u8>>;
```

| 方法 | 密钥 | IV | 构造 |
| ---- | ---- | -- | ---- |
| `aes-128-cfb` / `aes-192-cfb` / `aes-256-cfb` | 16 / 24 / 32 | 16 | CFB128：密钥流的下一块是上一块**密文**的 AES 加密（第一块是 IV 的） |
| `aes-128-ctr` / `aes-192-ctr` / `aes-256-ctr` | 16 / 24 / 32 | 16 | CTR：计数块从 IV 起，每块按 128 位大端整数加一 |
| `rc4-md5` | 16 | 16 | RC4，密钥 = MD5(主密钥 ‖ IV) |
| `rc4` | 16 | 0 | RC4，密钥 = 主密钥 |
| `chacha20-ietf` | 32 | 12 | `chacha20::ChaCha20`（块计数从 0 起） |
| `chacha20` | 32 | 8 | `chacha20::ChaCha20Legacy` |
| `salsa20` | 32 | 8 | `salsa20::Salsa20` |

CFB128、CTR 与 RC4 直接写在 `stream.rs` 里（AES 的单块加密用 `rurge-proto` 已依赖的 `aes` 0.8）；都要能在任意切分下续接（一次调用可以停在块的中间）。

**线上**（设计 4.1）：TCP 每个方向 `IV ‖ 密文`，客户端方向的明文是"SOCKS5 式的目标地址 ‖ 负载"——请求头照 AEAD 方法的做法经 `LazyHead` 与首段负载一起写出；UDP 每个包 `IV ‖ 密文(地址 ‖ 负载)`，每个包一个新的随机 IV。`obfs`、Shadow TLS、`udp-relay` / `udp-port` 与 AEAD 方法同样接。读端先攒够 IV（评审要点 1），`rc4` 不等（评审要点 2）。写端与 `AeadStream` 同一条约定：返回 `Pending` 的写须以同样的字节重试。

**诊断**：`to_spec` 对流式方法 `remarks.push(Remark::SsStreamCipher(name))`（任务 1 的机制，每种方法每次加载一条 `W0031`）。文本：`` `ss` cipher `<m>` is a stream cipher: it gives no integrity protection and is deprecated; use an AEAD cipher ``；`rc4` 另加一句 `` ; `rc4` also repeats its keystream on every connection ``。

**假服务端**：`FakeShadowsocks` 接受流式方法（脚本里方法即可），TCP 与 UDP。它的密钥流与出站共用 `Keystream`——流式方法的"封帧"只有一个 IV，独立再写一遍密码没有意义；`Keystream` 的正确性由下面的标准向量与任务 4 的互操作钉住。

**先写的用例**：

`stream::tests`（已知答案，密钥流 = 对全零明文加密；下面的十六进制已用 OpenSSL 3.2.4 在本机逐条算过）：

- AES，明文取 NIST SP 800-38A 的四块 `6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51 30c81c46a35ce411e5fbc1191a0a52ef f69f2445df4f9b17ad2b417be66c3710`：
  - `aes-128-cfb`，密钥 `2b7e151628aed2a6abf7158809cf4f3c`，IV `000102030405060708090a0b0c0d0e0f` → `3b3fd92eb72dad20333449f8e83cfb4a c8a64537a0b3a93fcde3cdad9f1ce58b 26751f67a3cbb140b1808cf187a4f4df c04b05357c5d1c0eeac4c66f9ff7f2e6`
  - `aes-192-cfb`，密钥 `8e73b0f7da0e6452c810f32b809079e562f8ead2522c6b7b`，同一 IV → `cdc80d6fddf18cab34c25909c99a4174 67ce7f7f81173621961a2b70171d3d7a 2e1e8a1dd59b88b1c8e60fed1efac4c9 c05f9f9ca9834fa042ae8fba584b09ff`
  - `aes-256-cfb`，密钥 `603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4`，同一 IV → `dc7e84bfda79164b7ecd8486985d3860 39ffed143b28b1c832113c6331e5407b df10132415e54b92a13ed0a8267ae2f9 75a385741ab9cef82031623d55b1e471`
  - `aes-128-ctr`，同上 128 位密钥，IV `f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff` → `874d6191b620e3261bef6864990db6ce 9806f66b7970fdff8617187bb9fffdff 5ae4df3edbd5d35e5b4f09020db03eab 1e031dda2fbe03d1792170a0f3009cee`
  - `aes-192-ctr` → `1abc932417521ca24f2b0459fe7e6e0b 090339ec0aa6faefd5ccc2c6f4ce8e94 1e36b26bd1ebc670d1bd1d665620abf7 4f78a7f6d29809585a97daec58c6b050`
  - `aes-256-ctr` → `601ec313775789a5b7a7f504bbf3d228 f443e3ca4d62b59aca84e990cacaf5c5 2b0930daa23de94ce87017ba2d84988d dfc9c58db67aada613c2dd08457941a6`
  - 进位（评审要点 5）：`aes-128-ctr`，同一密钥，IV `fffffffffffffffffffffffffffffffe` → `ba76aa54d5b56067c1a7903b3fddfa89 24df0c565cf42a68978713b67ad124fd 4d3f774ab9e47da2dbb9315ea3110680 a18d5905ebfe25a803df27c2211e58d6`
  - CFB 的解密：上面三条 CFB 的密文经 `decrypt` 回到明文。
- `rc4`（RFC 6229），密钥 `0102030405060708090a0b0c0d0e0f10`，密钥流前 32 字节 `9ac7cc9a609d1ef7b2932899cde41b97 5248c4959014126a6e8a84f11d1a9e1c`。
- `rc4-md5`：`Keystream::new(Rc4Md5, key, iv)` 的输出等于以 `MD5(key ‖ iv)` 为密钥的 `rc4` 的输出（用上一条已钉住的 RC4）。
- `chacha20-ietf`，密钥 `000102…1f`（32 字节），IV `000000000000004a00000000`，密钥流前 64 字节 `af051e40bba0354981329a806a140eaf d258a22a6dcb4bb9f6569cb3efe2deaf 837bd87ca20b5ba12081a306af0eb35c 41a239d20dfc74c81771560d9c9c1e4b`；全零的密钥与 IV → `76b8e0ada0f13d90405d6ae55386bd28 bdd219b8a08ded1aa836efcc8b770dc7 da41597c5157488d7724e03fb8d84a37 6a43b8f41518a11cc387b669b2ee6586`（RFC 8439 附录 A.1 的第一条）。
- `chacha20`，密钥 `000102…1f`，IV `0001020304050607`，密钥流前 64 字节 `f798a189f195e66982105ffb640bb775 7f579da31602fc93ec01ac56f85ac3c1 34a4547b733b46413042c94400491769 05d3be59ea1c53f15916155c2be8241a`。
- `salsa20`（ECRYPT 第 1 组第 0 条），密钥 `80` 后跟 31 个 `00`，IV 全零，密钥流前 64 字节 `e3be8fdd8beca2e3ea8ef9475b29a6e7 003951e1097a5c38d23b7a5fad9f6844 b22c97559e2723c7cbbd3fe4fc8d9a07 44652a83e72a9c461876af4d7ef1a117`。
- 每种方法：一段 100 KiB 的数据一次加密的结果，等于把它切成 1、15、16、17、4096 等长度交替的小段依次加密的结果（续接）。

`stream::tests`（流与包）：
- 两个 `CipherStream` 经 `tokio::io::duplex` 对接，11 种方法各往返 300 KiB，底层的读写被切成任意小段（含"IV 一个字节一个字节到"）。
- 线上的头 `iv_len` 个字节就是 IV、其后的密文用同一 IV 能解开；`rc4` 的线上第一个字节就是密文。
- `seal_packet` / `open_packet` 往返；两次封同一明文得到不同的包（`rc4` 除外：相同）；短于 IV 的包是错误；负载为空的包可以往返。

`shadowsocks::tests`（出站对 `FakeShadowsocks`）：11 种方法的 TCP 回显；`udp-relay=true` 的 UDP 往返（至少 `aes-256-cfb`、`chacha20-ietf`、`rc4`）；`obfs=http` 与 `obfs=tls` 各配一种流式方法；口令不对时目标收到的不是所请求的地址（假服务端按它解出的"地址"去连、连不上就关）——客户端看到连接结束，会话以错误或空应答结束而不挂住。

`crates/rurge-engine/tests/outbounds_shadowsocks.rs`：原来"流式方法的 REJECT"那条用例改为：手册的 11 种里取三种（`aes-256-cfb`、`chacha20-ietf`、`rc4-md5`）经引擎转发成功；手册没列的 `bf-cfb` 仍是 REJECT、会话日志 `policy protocol not implemented: ss (bf-cfb)`。

`crates/rurge/tests/cli.rs`：`rurge check` 对用了 `aes-256-cfb` 与 `rc4` 的配置给出两条 `W0031`、没有 `W0007`；`bf-cfb` 仍是一条 `W0007`。

**判据**：用例先红后绿；`cargo tree -p rurge-proto -e normal | grep -E "chacha20|salsa20"` 只出现 0.9.x 与 0.10.x；`git diff Cargo.lock` 里没有新的 `[[package]]`；全量门禁通过。

**提交**：`feat(proto): ss 的 11 种流式方法（TCP 与 UDP）`。

---

## Task 3：`snell` v1 ～ v3

**文件**：`crates/rurge-config/src/spec/{snell,mod}.rs`；`crates/rurge-proto/src/snell/chunked.rs`（新）、`snell/{mod,record,kdf,tunnel,pool,udp}.rs`、`testing/snell.rs`；`crates/rurge-engine/tests/outbounds_snell.rs`；`crates/rurge/tests/cli.rs`；`crates/rurge-engine/src/outbounds.rs`（工厂分支若按版本分支）。

**接口**：

```rust
// rurge-config spec/snell.rs
pub enum SnellVersion { V1, V2, V3, V4, V5 }
impl SnellVersion {
    pub fn number(self) -> u8;
    /// v1 to v3: the chunked record format of the original protocol.
    pub fn is_chunked(self) -> bool;
    /// v3 and later relay UDP.
    pub fn carries_udp(self) -> bool;
}
// SnellSpec.reuse 存的是生效的值：v2 恒 true，v1 / v3 恒 false，v4 / v5 取 `reuse`。
// SnellRead.not_implemented_version 只剩 Some(6)。

// rurge-proto snell/kdf.rs
pub(crate) const OUTPUT_LEN: usize = 32;
/// Argon2id's whole output: v1 keys ChaCha20-Poly1305 with all of it, the
/// later versions AES-128-GCM with its first half.
pub(crate) fn derive(psk: &[u8], salt: &[u8; SALT_LEN]) -> [u8; OUTPUT_LEN];

// rurge-proto snell/record.rs
/// How a connection's records are laid out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Format { /// v1: chunks under ChaCha20-Poly1305.
    ChunkedChaCha, /// v2, v3: chunks under AES-128-GCM.
    ChunkedAes, /// v4, v5.
    V4 }
/// 两种记录流的外壳，隧道、池与 UDP 只认它：
pub(crate) enum SnellStream { V4(V4Stream), Chunked(ChunkedStream) }
// 方法照旧：open(inner, psk, format)、read_ended、is_reusable、next_tunnel、poll_end、poll_record，
// 以及 AsyncRead / AsyncWrite。现有的 v4 实现改名 V4Stream，逻辑不动。
```

**线上**（设计 4.3；事实取自 open-snell 与 mihomo，只取事实）：

- 每个方向先发 16 字节 salt；会话密钥 = Argon2id(PSK, salt)（与 v4 同一组参数）的 32 字节输出——v1 全用（ChaCha20-Poly1305），v2 / v3 取前 16 字节（AES-128-GCM）。nonce 是 12 字节的小端计数，每次密封加一（`CountingAead` 已是这样）。
- 一块 = 密封(负载长度，2 字节大端，读时与 `0x3FFF` 相与) ‖ 密封(负载)；负载至多 `0x3FFF`。**零长度块只有密封的长度那一段**，它结束一个方向（v2 的复用靠它）。读端在读到长度 0 之后，下一个字节就是下一块的长度段。
- 请求 `01 ‖ 命令 ‖ 00 ‖ 主机长度 ‖ 主机 ‖ 端口`、应答（`00` 隧道，`02 ‖ 错误码 ‖ 长度 ‖ 文本`）与 v4 相同，请求头与首段负载合在一块里写出（经 `LazyHead`，同 v4）。
- 命令与复用：v1、v3 发 `0x01`，一条连接一个请求，不进池；v2 发 `0x05`，总是复用——两个方向都以零长度块结束之后连接回池（与 v4 `reuse=true` 的规则、池的大小与时限、"取出的失效连接换新连接重发一次"都相同，评审要点 3）。
- UDP：只有 v3（命令 `0x06`，数据报的格式与 v4 相同，每个数据报一块）；v1 / v2 的 `udp()` 是 `Unsupported`，引擎照 `udp-policy-not-supported-behaviour` 办。
- `obfs`：v1 ～ v3 可以是 `http` 或 `tls`（`ObfsClient` 已支持 `tls`）；v4 / v5 照旧只有 `http`。

**配置**：`version` 1 ～ 5 都产生 spec；不写 `version` 是 v1。`reuse=true` 写在 v1 / v3 上、`reuse=false` 写在 v2 上各是 `W0028`（`` `reuse` does not apply to `snell` version <n>: version 2 always reuses a connection, versions 1 and 3 never do ``）。`NotImplemented::SnellVersion` 的文本只剩 v6 的那一句（任务 1 已写）；`snell_default` 一类只为"缺省是 1 所以被拒"服务的说明随之去掉。

**错误文本**：沿用 v4 的几句——`snell: the server closed the connection without answering`、`snell: the server's data failed to decrypt (wrong psk or version?)`、`snell: the connection ended in the middle of a record`；评审要点 4 的情形（PSK 不对，或对端其实说的是另一个版本）落在第二句上：读到的第一块长度段解不开即报，不等更多数据。

**假服务端**：`FakeSnell` 的脚本加 `version`（1 ～ 5），v1 ～ v3 的分块读写在假服务端里独立实现（不调用 `ChunkedStream`；密封可以用同一个 AEAD crate）。v2 的服务端在一个请求结束后在同一条连接上等下一个请求。

**先写的用例**：

`chunked::tests`：
- 已知答案：PSK `secret`、salt `00 01 … 0f`，v2 / v3 的第一块（负载 `hello`）的线上字节——由测试里按"Argon2id（`kdf` 模块已有的已知答案）→ AES-128-GCM、nonce 0 封长度 `00 05`、nonce 1 封 `hello`"逐步算出期望值，不经 `ChunkedStream`；v1 同理（ChaCha20-Poly1305、32 字节密钥）。
- 两个流对接，三种版本各往返 300 KiB，任意切分；大写被切成至多 `0x3FFF` 的块。
- 零长度块结束一个方向、另一个方向继续；两个方向都结束后 `is_reusable`，`next_tunnel` 之后同一条流（同样的密钥与计数）承载下一个请求。
- 长度段解不开（PSK 不同、或对端发的是 v4 的 7 字节头）→ `failed to decrypt` 的那句，且错误里没有 PSK。
- 流在块的中间断掉 → `ended in the middle of a record`；在两块之间断掉 → 读到文件尾。

`snell::tests`（出站对 `FakeSnell`）：
- v1、v2、v3 的 TCP 回显；请求里的命令字节（v1 / v3 是 `01`，v2 是 `05`）。
- v2：连续三个请求只有一条连接；池里的连接被服务端关掉后下一个请求换新连接重发成功（评审要点 3）。
- v1 / v3：两个请求两条连接；`reuse=true` 被忽略。
- v3 的 UDP 往返（两个目标，全锥）；v1 / v2 的 `udp()` 是 `Unsupported`。
- `obfs=tls` 配 v1；`obfs=http` 配 v3。
- 服务端的拒绝（`02 …`）与错误的 PSK 各给出上面的错误文本；对一个只说 v4 的 `FakeSnell` 用 `version=1` 去连——`failed to decrypt`，在有界的时间内（评审要点 4）。

`spec::snell::tests`、`spec::tests`：`version` 1 ～ 3 有 spec、`not_implemented == None`；不写 `version` 是 `V1`；`reuse` 的两条 `W0028`；`version=6` 仍是 `NotImplemented::SnellVersion(6)`；v1 ～ v3 接受 `obfs=tls`，v4 拒绝它。

`crates/rurge-engine/tests/outbounds_snell.rs`：原来"v1 的 REJECT"那条用例改为 v1 经引擎转发成功；v2 的复用（一条连接）；v3 的 UDP；v1 的策略收到 UDP 时照 `udp-policy-not-supported-behaviour`（缺省 REJECT）；`version=6` 仍是 REJECT、会话日志 `policy protocol not implemented: snell v6`。

`crates/rurge/tests/cli.rs`：`rurge check` 对不写 `version` 的 `snell` 行不再有 `W0007`；`version=6` 一条 `W0007`，文本含 `beta`。

**判据**：用例先红后绿；v4 / v5 原有的用例一条不改地通过（`V4Stream` 的逻辑没动）；全量门禁通过。

**提交**：`feat(proto): snell v1 ～ v3（分块记录流、v2 的复用、v3 的 UDP）`。

---

## Task 4：互操作

**文件**：`tests/interop/src/lib.rs`（或夹具所在的模块）、`tests/interop/tests/shadowsocks.rs`、`tests/interop/tests/snell.rs`、`tests/interop/tests/mihomo.rs`（新，若按参照服务端分文件）、`tests/interop/README.md`、`.github/workflows/ci.yml`。

**`ssserver`（CI 已装的 v1.25.0，发布包含 `stream-cipher`）**：现有的 `ssserver` 夹具只换 `method`，对 `rc4`、`rc4-md5`、`aes-128-cfb`、`aes-192-cfb`、`aes-256-cfb`、`aes-128-ctr`、`aes-192-ctr`、`aes-256-ctr`、`chacha20-ietf` 各做一次 TCP 往返（含一段跨多块的大负载），其中 `aes-256-cfb` 与 `chacha20-ietf` 另做 UDP 往返。核对（照它的源码与文档，本机跑不了）：这些方法名在它的配置里的写法、流式方法是否要额外的开关。

**mihomo v1.19.32**（GPL-3.0，只作为外部进程运行；环境变量 `RURGE_TEST_MIHOMO`，没装就跳过，`RURGE_INTEROP_REQUIRED=1` 时缺了就失败）：

- CI 新增一步，三个平台，放在 CoreDNS 那一步之后、`sshd` 那一步之前，照 sing-box 那一步的写法（`curl --retry`、校验 SHA-256、解包、`chmod +x`、Windows 上 `cygpath -w`）。资产与校验值（取自 GitHub 发布 API 的 `digest` 字段，2026-10-08）：

  | 平台 | 资产 | SHA-256 |
  | ---- | ---- | ------- |
  | Linux | `mihomo-linux-amd64-v1-v1.19.32.gz` | `306f81e723e60ce6b828899a6fe83e1d00e9ecefb2dc8d4d849312a5bc00efdc` |
  | Windows | `mihomo-windows-amd64-v1-v1.19.32.zip` | `00549b347d8e124600e46cac67d59293358454f465e9d86d2511567da56b7029` |
  | macOS | `mihomo-darwin-arm64-v1.19.32.gz` | `3312a6780652c622890fd4357c6a853bbf865464fd047ac7b7f52dab8de18652` |

  下载地址 `https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/<资产>`。`.gz` 是单个可执行文件的 gzip（不是 tar）：`gunzip` 之后改名为 `mihomo`；`.zip` 里找 `*.exe`。
- 夹具：配置渲染到临时目录，`mihomo -d <临时目录> -f <配置文件>` 作为回环子进程拉起，`Drop` 时结束。最小配置只有 `mode: direct`、`ipv6: false`、`log-level` 与 `listeners`；要核对并在 README 里写明出处的：就绪的判据（日志行或端口可连）、它在 `-d` 目录里写哪些文件、启动时不联网（`config.Init` 只建目录与缺省配置；`geo-auto-update` 缺省关；不写 `rules`、`dns`、`proxy-providers`）、`snell` 监听端的 `udp` 要显式写 `true`。
- 用例：`snell` 监听端 `version` 1、2、3 各做 TCP 往返；v2 连续两个请求；v3 的 UDP 往返；PSK 不对时的表现。`shadowsocks` 监听端 `cipher: chacha20`（8 字节 nonce 的那种）的 TCP 往返——这是 `ssserver` 没有的方法。
- `salsa20` 没有参照（只有 shadowsocks-libev 支持，它没有官方二进制）：在 README 里登记，不写用例。

**判据**：本机没有这些二进制，用例应打印说明并跳过（`cargo test -p rurge-interop` 通过）；夹具渲染出的配置有单元用例（照现有夹具的做法，对渲染结果的文本断言）；CI 步骤的 YAML 可解析。真正的验证在下一次 CI 运行，结果记进「执行期修正记录」。

**提交**：`test(interop): ss 的流式方法对 ssserver；CI 安装 mihomo，snell v1 ～ v3 与 ss 的 chacha20 对它`。

---

## Task 5：文档

- `docs/surge-compatibility-matrix.md`：4.2 节 `ss`、`snell`、`vmess` 三行与"策略指向 rurge 尚未实现的协议类型"一行（改写为"不实现的协议与变体"）；4.6 节 `ss` 流式方法一行（11 种 ✅、9 个名字 ⛔）、`snell` 的参数（`version` 1 ～ 5、`reuse` 按版本、`obfs` 按版本、`mode` 的错误）；4.5 节 UDP（Snell v3 起）；4.3 节 `ecn`；`hysteria2` 的混淆口令；设计第 8 节第 1 ～ 3 条逐条落到位。
- `docs/requirements.md`：FR-OUT-05（`snell` v1 ～ v5）、FR-OUT-06（流式方法与 Snell v1 ～ v3 已实现；VMess 旧握手不实现及理由）。
- `docs/acceptance/phase2-manual.md`：新增「M8b　流式方法、Snell v1 ～ v3」一节——用真实的 `ss` 流式节点转发；对 Surge 自带的 Snell 服务端（`[Snell Server]`，v1）转发；一条不写 `vmess-aead` 的 `vmess` 行对真实的 AEAD 服务端转发并看到 `W0029`。
- `tests/interop/README.md`：任务 4 写的部分核对一遍。
- `CLAUDE.md`：状态段、命令（`cargo test -p rurge-proto shadowsocks::stream`、`cargo test -p rurge-proto snell::chunked` 等）、文档索引加本计划。
- 本计划的「执行期修正记录」与「延后事项」。

**提交**：`docs: M8b——兼容性清单、PRD、手工验收、CLAUDE.md 与计划的两张表`。

---

## 计划期决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| P1 | 任务的边界 | 配置与协议实现同在一个任务里落地（任务 2、3）：配置层先接受一种方法或版本而协议层还不会，中间状态要么拒绝构建、要么按错的线上格式去连。只有 `vmess`（纯配置）与诊断文本单独成任务 1 |
| P2 | 密码的来源 | `chacha20` 0.9 与 `salsa20` 0.10 用依赖树里现成的（与 `aes` 0.8 同属 `cipher` 0.4 一代）；CFB128、CTR、RC4 手写，以 NIST SP 800-38A、RFC 6229 的向量钉住（十六进制已用 OpenSSL 在本机独立算过）。不为它们引入 `cfb-mode`、`ctr` 0.9、`rc4` |
| P3 | CTR 的计数宽度 | 128 位大端（OpenSSL、Go、libev 的做法）。shadowsocks-rust 的 AES-CTR 用的是 64 位计数，只在 IV 的低 64 位溢出时与此不同，互操作里碰不到 |
| P4 | 流式方法的假服务端 | 与出站共用 `Keystream`（"封帧"只有一个 IV）；正确性靠标准向量与 `ssserver` / mihomo 的互操作 |
| P5 | 手册没列的 9 个方法名 | 照旧认得、`W0007` + REJECT（文本改为"不支持"）：改成未知方法会让今天能加载的配置变成加载错误 |
| P6 | 提示的机制 | 新的 `Remark`（"策略照建，但每次加载按种类说一次"），与 `NotImplemented`（"不建策略"）并列；`vmess-aead` 与流式方法都走它。新码只有 `W0031` |
| P7 | Snell 两种记录流的关系 | `SnellStream` 变成两种实现的外壳，v4 的实现改名不改逻辑；隧道、池、UDP 不知道版本 |
| P8 | v2 的 `reuse` | 恒为真，照手册（"Snell v2 always reuses connections, and v1/v3 never do"）；写反了是 `W0028` |
| P9 | mihomo 的构建变体 | 取 `-v1`（GOAMD64=v1，任何 x86-64 都能跑），不取缺省的 v3 变体 |
| P10 | `salsa20` | 实现，但没有参照服务端；由 ECRYPT 的向量（也是 `salsa20` crate 自己的测试向量）与回环覆盖，登记 |

## 执行期修正记录

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | mihomo 与 `ssserver` 流式方法的互操作用例没有对真实二进制跑过（夹具的配置、就绪判据都只照源码读出） | 下一次 CI 运行时核对 |
| 2 | `salsa20` 没有可用的参照服务端 | 手工验收（有 libev 服务端的人）；出现预编译的参照时补互操作 |
| 3 | `chacha20`（原版）经 `chacha20` 0.9.1 实现，内部是 32 位块计数：单条连接超过 256 GiB 时出错 | 接受（已登记进兼容性清单） |
| 4 | Snell v1 ～ v3 没有对官方服务端验证（官方已不提供 v1 ～ v3 的独立服务端；Surge 自带的服务端只能手工验收） | 手工验收 |
| 5 | Snell v6（`unshaped` / `unsafe-raw` 两种模式不涉及逆向得来的常数，是可选的第一步） | 见可行性报告；项目所有者表态之后 |
