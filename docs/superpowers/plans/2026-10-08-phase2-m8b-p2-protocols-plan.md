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

1. **`vmess`**：`to_spec` 对没写 `vmess-aead=true`（或写了 `false`）的行照常给出 spec，并 `remarks.push(Remark::VmessAead)`；`NotImplemented::LegacyVmess` 删掉。文本：`` `vmess-aead=false` has no effect: rurge always uses the AEAD handshake, which every server since V2Ray 4.28.1 accepts ``（`W0029`；终审时改了这句文本，见「执行期修正记录」#23）。
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
| 1 | 1 | 计划没有列到导入行的 `W0007` 外框：`rurge-policy` 的 `assemble.rs` 里 `imported policies of type <种类> are not implemented in this version; they behave as REJECT` 改为 `imported policies of type <种类> are not supported; they behave as REJECT`——`W0007` 的文本归任务 1，M8-D10 之后它不再表示"还没轮到"。这一处由控制者在任务评审之后改，没有经过任务评审者，留给整分支评审；同一提交另把 `read_specs` 的文档注释写准，并让 `ecn` 的告警对 `trust-tunnel` 直接看 spec 的 `h3`（不再依赖前面分支的次序） | 0076f61 |
| 2 | 1 | 导入行的提示（`Remark`）按"组 × 种类"去重，照计划的文字；紧邻的导入行 `W0007` 照旧按种类跨组只说一次（落在第一个带进这种行的组上）——两者不一致是有意的：组的 `external-policy-modifier` 是使用者能消掉这类提示的地方（如给 `vmess` 行补 `vmess-aead=true`），所以每个组都要点名。两个组导入同一行时，这一行算先导入它的那个组的，提示只由那个组说。提示在读出 spec 时就报，早于之后可能把这一行剔掉的中继成环检查：极端情况下一个组会为随后被略去的行说一句，没有为此加机制 | 4203eb3 |
| 3 | 1 | `ecn` 写在不带 `h3=true` 的 `trust-tunnel` 上，文本是 `` `ecn` does not apply to `trust-tunnel` policies without `h3=true`; ignored ``：计划的通用句在这里不对（经 HTTP/3 时 `ecn` 生效）。被忽略的 `ecn` 在 spec 里置回 `auto`，重载不因它重建出站。顺带的变化：这条告警从"每个参数名每次加载一条"（`W0029`）变成"每行一条"（`W0028`） | 4203eb3、0076f61 |
| 4 | 1 | 计划没有写到的三处取舍：`snell` 的 `version` 本身写错时不再对 `mode` 报第二条（版本不明就判断不了 `mode` 写在哪个版本上）；Hysteria 2 的短混淆口令按参数各自检查、按字节计（两个口令都写、都短时另有两条 `W0012`），4 字节的下限读自 apernet/hysteria 的 Salamander 与 Gecko 两份源码（Gecko 把口令交给同一个 Salamander 构造）；行有错误或没有 spec 时不给提示（提示说的是"策略怎么建"，没建就无话可说） | 4203eb3 |
| 5 | 2 | `Keystream::encrypt` / `decrypt` 返回 `io::Result<()>`（计划的接口没有返回值）：`chacha20` 0.9.1 的两种 ChaCha20 都以 32 位计块，一条连接一个方向的密钥流在 (2^32 − 1) × 64 字节（256 GiB 少 64 字节）处到头，`cipher` crate 的缺省调用在那里 panic。改用先检查、后动手的调用：到头时这条连接以 `ss: the cipher's keystream is used up` 失败，数据不动，此后不再有一个字节发出（用例 `a_keystream_that_is_used_up_is_an_error_and_no_panic`、`a_stream_past_its_keystream_fails_and_sends_nothing_unencrypted`）。这个上限对 `chacha20-ietf` 同样成立（计划的延后事项 #3 原来只写了原版 `chacha20`）；`salsa20`（64 位计块）与手写的 AES-CFB、AES-CTR、RC4 实际上没有尽头 | 3ce73f9 |
| 6 | 2 | 服务端一个字节没回就结束或重置，按错误报出，文本沿用 AEAD 方法的两句（`ss: the server closed the connection without answering`、`ss: the server reset the connection without answering (wrong password or method?)`），另加 IV 收到一半就结束的 `ss: the connection ended in the middle of the server's IV`。计划对口令错误只说"会话以错误或空应答结束"；选错误是为了与 AEAD 方法在同一情形下一致，而且这是流式方法下口令或方法不对时唯一看得见的征兆。代价同 AEAD 方法：目标自己一个字节不回就关闭的会话也记成第一句 | 3ce73f9 |
| 7 | 2 | 计划接口之外的三处：加了 `StreamKey`（方法 + 主密钥，仿 AEAD 的 `MasterKey`——出站与 UDP 要一处同时拿着两者）；我方的 IV 与首段密文在同一次写里交给下层，一次写至多取 16 KiB（`MAX_WRITE`）；导入行的 `W0031` 不带"use an AEAD cipher"（订阅的行使用者改不了）：`` imported `ss` policies use the stream cipher `<方法>`: it gives no integrity protection and is deprecated ``。另：未知 `encrypt-method` 的 `E0018` 现在列出 19 个可用的名字（原来 8 个）——取值表就是方法表 | 3ce73f9 |
| 8 | 2 | 假服务端 `FakeShadowsocks` 不再连接、也不再发往回环以外的地址，对所有方法生效：流式方法在口令不对时解出的"地址"是任意地址，测试不能因此碰公网。评审之后收紧为只认 127.0.0.1 与 ::1 本身（macOS 上别的 127/8 地址既不应答也不拒绝，用例会挂住），流式方法那条路径连上游另限时 2 秒；同一轮补了 Salsa20 非零 IV 的向量（`salsa20_under_an_iv_is_ecrypts_too`，ECRYPT 第 6 组第 0 条：IV 没进到密码状态里时原有用例一条也不失败）与"停着密文时先发完、再 flush / shutdown"的用例（`a_flush_or_a_shutdown_sends_what_is_parked_first`） | 3ce73f9、d6d3835 |
| 9 | 3 | 设计 V5 的核对结果（读自 mihomo v1.19.32 的源码，只取事实）：零长度块在监听端两个方向上都只有密封的长度那一段；"再密封一段空负载"的写法只在它的 `ReadFrom` 路径上，监听端的应答包装到不了那里。所以读端只认前一种——计划写的就是这一种，设计 V5 说的"读端两种都要认"不做：唯一的参照服务端只写它，两份开源实现的读端也都不认另一种。代价：对写另一种的服务端，复用的 v2 连接上的下一个请求会解不开。v3 的 UDP 应答 `00` 是单独的一块，由共用的 `SnellUdp::open` 读掉 | 23af242 |
| 10 | 3 | 计划没有写明各版本怎样结束自己的方向：v1 / v3 以 TCP 的写关闭结束，不发零长度块（v1 的服务端不认得它）；v2 以零长度块结束，连接留着；服务端发来的零长度块在三个版本上都结束它的方向 | 23af242 |
| 11 | 3 | 错误文本与计划不同。计划：沿用 v4 的三句，口令或版本不对落在 `failed to decrypt` 那一句。实际上不同版本或不同 PSK 的服务端多半什么也不回就关，所以"没有应答"也带提示，并按服务端有没有发过字节分开：一个字节没到就关闭是 `snell: the server closed the connection without answering (wrong psk or version?)`；一个字节没到就被重置是新的一句 `snell: the server reset the connection without answering (wrong psk or version?)`（错误种类不变；解不开就直接关的服务端在请求还有字节没读走时，到达客户端的是重置）；服务端的 salt 已到而没有应答，仍是不带提示的 `snell: the server closed the connection without answering`（它读得了请求，不是 PSK 或版本的问题）。所有版本如此：M6b 登记过的 v4 / v5 文本随之改了。重置那一句是对"v4 的逻辑不动"唯一的例外（两种流共用 `poll_salt`）。已知限制见延后事项 #8 | 23af242、b106a60 |
| 12 | 3 | 其余出入：外壳 `SnellStream` 上没有 `read_ended`（没有调用者，留着过不了 `-D warnings`）；300 KiB 的往返经 61 字节与 4096 字节的管道（`two_streams_carry_300_kib_both_ways_whatever_the_slicing`），逐字节的情形由短块的用例承担（`short_chunks_cross_byte_by_byte`；1 字节的管道在 debug 构建里要一分钟）；`SnellScript` 加的是 `obfs_tls: bool`；`udp-port` 在 v1 ～ v3 上沿用既有规则，仍是 `W0029`；`open_udp` 在 v1 / v2 上不拨号，直接说这个版本不载 UDP；`SnellOutbound::new` 不论手造的 spec 怎么写，都按版本取生效的 `reuse` | 23af242、b106a60 |
| 13 | 3 | 假服务端的两处：目标一个字节没回就结束时，`FakeSnell` 在 v1 ～ v3 上不论哪种命令都回 `02 65 "Remote EOF"`（参照服务端的做法，读自 mihomo 的源码；第一版在这三个版本上是自拟的"不应答"——假服务端该钉住真实服务端的行为；v4 / v5 照旧，ConnectV2 回它、Connect 回 `02 ff "end of file"`），并加了 `reset` 开关；"只连回环地址本身"的判断与限时 2 秒的连上游（`is_localhost`、`connect_upstream`）收成 `rurge_proto::testing` 里的一份，`FakeSnell` 与 `FakeShadowsocks` 共用 | b106a60 |
| 14 | 4 | 计划之外加了 simple-obfs 对 mihomo 监听端的互操作：`ss` 的 `http` 与 `tls`（另一条 `tls` 配流式方法 `aes-256-cfb`），`snell` v1 配 `tls`、v4 配 `http`。mihomo 是 CI 上第一个会说 simple-obfs 的参照服务端；M8 设计 6.2 与总设计验收标准第 1 条把它列在"没有参照"里，随之订正（M8 设计第 12 节） | a521b65 |
| 15 | 4 | 读 mihomo v1.19.32 的源码得出、计划没有的事实，夹具照此处理：监听端可连不等于能转发（它先开监听端，稍后才让 tunnel 开始转发，其间交来的连接被直接关掉），所以就绪的判据是经一个额外的 `socks` 监听端真的转发成一次，而不是计划里的"日志行或端口可连"；`snell` 监听端的 `obfs-opts` 必须同时写 `mode` 与 `host`；写在 `listeners` 里的条目 `udp` 缺省就是 true（计划写的是"要显式写"，夹具照样写明）；它的流式方法表里没有单独的 `rc4` 与 `salsa20`；十三个 `CLASH_*` 环境变量是命令行参数的缺省值，从子进程的环境里去掉，另设 `DISABLE_LOOPBACK_DETECTOR=true`；`mode: direct` 决定 v2 用例所读的那行日志；主目录里多一个 `cache.db`。出处逐条写在 `tests/interop/README.md` 的「mihomo」一节 | a521b65 |
| 16 | 4 | 用例与计划的出入：v2 的复用由 mihomo 的日志证明（三个请求的客户端地址相同——池里的连接失效时 rurge 会不声不响地重拨，只看客户端一侧分不出来），请求数从两个加到三个；不写 `version` 的行另有一条对 v1 监听端的；`chacha20` 加了 UDP 往返；流式方法不写口令错误的用例（参照服务端会按解出的乱码去连任意地址，或一直读下去）；PSK 或版本不对的用例只收带提示的那两句（结束 / 重置），逐字比对；另加三条不需要参照服务端的本机用例（两条把用例里的策略行加载并构建一遍，一条对回环假服务端核对那两句话）；做往返的每一步经 `common::attempt`，把策略名与参照服务端的日志带进失败信息 | a521b65、6ac8bf8 |
| 17 | 4 | 一次拉起用到的端口由新的 `rurge_interop::free_ports` 一次取齐（取到的先占着，全部取到才放开）：一个一个地取时，同一份配置的两个监听端可能拿到同一个号。mihomo 与 `ssserver` 的夹具用它（`ssserver` 的端口同时对 TCP 与同号的 UDP 空闲）；v2 复用的用例把回显绑在拉起 mihomo 之前——就绪探测也在日志里留一行，它的目标端口事后可能被回显拿到 | 6ac8bf8 |
| 18 | 1 ～ 4 | 门禁（各任务提交的树上，本机；数字由门禁日志重新求和）：格式检查与 clippy 通过；全量的结果行 / 通过 / 失败 / 忽略依次是 69 / 2184 / 0 / 3（任务 1）、69 / 2214 / 0 / 3（任务 2）、69 / 2266 / 0 / 3 与修正之后的 69 / 2273 / 0 / 3（任务 3）、70 / 2291 / 0 / 3（任务 4，多出的结果行是新的测试二进制 `tests/mihomo.rs`）；任务 4 的修正只动了 `tests/interop`，只跑了 `rurge-interop`（16 个结果行、103 个通过）。任务 3 修正那一轮的头两遍全量各有一条与改动无关的偶发失败（延后事项 #7），第三遍全绿 | — |
| 19 | 5 | 注释里仍把 `W0007`、`OutboundError::Unsupported` 与没有 spec 的策略说成"还没实现""后面的里程碑"的地方，统一改为"rurge 不实现"（13 个文件；`rurge-engine` 的 `observe.rs` 测试夹具里不可能再出现的链标记 `!unsupported:vmess` 换成 `!unsupported:tailscale`）。只动注释与这一个标记。改后的全量门禁 70 / 2295 / 0 / 3：与任务 4 的那一次逐个二进制比对，只有 `rurge-interop` 的两个二进制多出它的修正加的 4 条 | 4959616 |
| 20 | 5 | 兼容性清单附录的统计整张重数：附录写着"由脚本生成"，实际自 M2a 之后一直由各份文档任务按增减改，已与表格对不上——改之前写的合计是 529 条，按它自己的规则（每行第一个状态标记）数出来是 538 条。用同一条规则的脚本核对过：它对最初两次由脚本生成的版本（提交 eebb6aa、109fc4e）算出的数字与附录逐格相同 | 文档提交 |
| 21 | 终审 | 整分支评审指出：兼容性清单把没写 `version` 的行对着 v4 / v5 服务端很快失败写成了事实，而它只在回环假服务端上验证过（对 mihomo 的同类用例还没有跑过），已经在 CI 上的两个 v4 / v5 参照一个连不通的用例也没有。`tests/interop/tests/snell.rs` 加两个：`a_line_of_another_version_or_psk_against_sing_box`（三个平台）与 `a_line_of_another_version_or_psk_against_snell_server`（Linux），各试两行——不写 `version` 的（按 v1 去连）与 PSK 不对的 `version=4`——，每行的会话须在 10 秒内以带提示的"结束"或"重置"两句之一失败，别的结果（含时限内没有失败）连同实际发生了什么与服务端的日志让用例失败；两行都试，两个夹具原样复用。判据收成 `tests/interop/tests/common/mod.rs` 的 `none_is_answered`（`unanswered` 从 `tests/mihomo.rs` 搬来，改为返回结果而不是当场 panic），对 mihomo 的同类用例改用它。本机能跑的两个：`only_the_two_sentences_of_an_unanswered_request_are_taken`（对回环上关闭、重置、回一段解不开的字节、留着连接不出声四种服务端，核对前两种通过、后两种各按实情记为失败；把判据改坏成"什么文本都收"与"挂住也算通过"各试过一次，都被它拦下）与 `the_mismatched_lines_load_and_build`。兼容性清单 4.2 `snell` 一行与手工验收清单 M6b、M8b 两节的三项给"很快失败"加了前提——服务端读不出请求就关闭或重置连接——，直到 CI 给出答案。两个新的互操作用例在本机跳过 | 5962f65、文档提交 |
| 22 | 终审 | 互操作用例里一张表的策略逐条都试。`common::attempt` 原来在第一条失败的策略上就让用例失败：九种流式方法里 `rc4` 排第一，它一坏另外八种那一次就看不到，而每跑一次 CI 都要推送一次。现在每一步的失败记进 `common::Failures`，用例在末尾失败一次——先是"几条里失败了几条"与每条失败的策略一行（策略名与那一步自己的 panic 文本，并成一行），最后是参照服务端的日志，只一遍：CI 的失败摘要只取每个失败用例输出开头的 60 行 / 3000 字符。`tests/shadowsocks.rs` 的流式方法用例与 `tests/mihomo.rs` 的各用例都经它（`chacha20` 的 TCP 与 UDP 因此在一次运行里都有结果）。本机用例 `every_method_that_fails_is_told_not_the_first_alone` 与 `a_method_that_fails_fails_its_test_at_the_end`；RED 是把新的签名接回旧行为（第一条失败即 panic）跑出来的。留意：每一步自己的 panic 仍由 panic 钩子当场打印，在捕获的输出里排在末尾那份报告之前 | 5962f65 |
| 23 | 终审 | `W0029` 的文本。任务 1 定的 `` `vmess-aead=false` has no effect: rurge always uses the AEAD handshake, which every server since V2Ray 4.28.1 accepts `` 对根本没写这个参数的行说不通。改为两种写法（没写、写了 `false`）都成立的一句：`` `vmess` without `vmess-aead=true` uses the AEAD handshake all the same: rurge has no other, and every server since V2Ray 4.28.1 accepts it ``；订阅导入行的那一句没有动。引用旧文本的三处用例同改；文档里兼容性清单 4.6 `vmess` 一行、手工验收清单 M8b 一节与 M8 设计 4.2 改成新文本（设计第 12 节第 4 条记下原文），本计划任务 1 的原文留着、旁边加了指向这一行的注 | 5caad39、文档提交 |
| 24 | 终审 | 已经报错的参数不再多说一句（评审者用 `rurge check` 量出的两行）：`trojan …, ecn=maybe` 在 `E0018` 之外还有一条 `` `ecn` does not apply to `trojan` policies; ignored ``（`W0028`；写在 `wireguard` 上则多一条 `W0029`），`ss …, udp-relay=maybe, udp-port=8389` 在 `E0018` 之外还有一条 `` `udp-port` only applies with `udp-relay=true`; ignored ``。现在取值本身是错误的 `ecn` 不再记进留给 `to_spec` 去说的名单，`udp-relay` 的取值是错误时不再对 `udp-port` 说话；同一处另有一种依赖：`trust-tunnel` 上 `ecn` 适用与否看 `h3`，`h3` 的取值是错误时也不再说 `ecn`（`h3=maybe, ecn=on` 原来同样是两条）。与任务 1 给 `snell` 的 `version` / `mode` 定的做法一致。只动了 M8b 自己加的这两条告警；M8b 之前就有的同类情形没有动，见延后事项 #12 | 5caad39 |
| 25 | 终审 | 回环假服务端 `FakeSnell` 的三处。不再按脚本的版本拒绝命令（`knows()` 去掉）：没有哪个参照服务端这样做，mihomo 的监听端只比对请求里的版本字节；随之去掉 `the_servers_refusal_of_udp_fails_the_open_on_v3` 里"v3 的行对 v2 的服务端"那一半（脚本化的拒绝那一半留着），别的用例都不依赖它。`reset` 字段的注释照代码写：它给接受的每条连接都设 zero linger，没有经过有序关闭就被丢下的连接因此收到重置，被拒的客户端就是这样。v1 ～ v3 上对 ConnectV2 回了 `Remote EOF` 之后不再关连接（mihomo 的监听端留着它；v4 / v5 上不变，仍然关闭）：三行的改动，用例的意思不变——客户端一侧"被拒的请求所在的连接不回池、下一个请求另拨"的断言照旧。延后事项 #10 里这两条随之划掉 | 0ab40a5 |
| 26 | 终审 | `README.md` 与 `README_en.md` 各有三处随合并变成不实的说法，就地订正，其余的 README 工作仍归 M8c：状态段 M6a 一句里流式旧方法"在 M8 之前按 REJECT 处理"（改为手册列出的 11 种在 M8b 实现）、M6b 一句里"v1–v3 与 v6 按 REJECT 处理"（改为 v1–v3 在 M8b 实现、只有 v6 按 REJECT 处理）、特性表出站协议一格的"Snell v4 / v5"（改为 v1–v5） | 文档提交 |
| 27 | 终审 | CI：参照服务端装不上时只让它的用例失败。原来任何一个安装步骤失败（下载失败、资产挪了地方）任务就停在那里，`cargo test` 根本不跑；而 `RURGE_INTEROP_REQUIRED=1` 是 sing-box 的安装步骤写进环境的——只放过那一步的话，全部互操作用例会不声不响地跳过。现在这个变量在工作流一级的 `env` 里（与 `RUSTUP_TOOLCHAIN` 并排），十一个安装步骤——十个参照服务端，加把 `RURGE_TEST_SSHD` 指向 `sshd` 的那一步——都带 `continue-on-error: true`。各夹具的 `…_or_skip` 逐个核对过：只有 Linux 版的（snell-server、tuic-server、masque-server）与只有 Linux / macOS 版的（TrustTunnel endpoint）用 `cfg!` 只在装它的平台上把缺失当失败，三个平台都装的（sing-box、xray、`ssserver`、dnsproxy、CoreDNS、mihomo）在哪里缺了都失败，`sshd` 的夹具没有 `cfg!`，它的两个用例只在 Unix 上编译。改后的文件用 PyYAML 解析过（两个任务，`check` 18 步、`bench` 5 步；变量对 `bench` 没有影响，它不跑测试）。代价：安装失败不再表现为一个失败的步骤，而是一批以"找不到二进制"失败的用例 | 33b38c0 |
| 28 | 终审 | 文档评审留下的五处：兼容性清单 1.2 的 `[Tailscale <name>]` 一行从 ❓ 改为 ⛔（与 4.2、4.6 讲 `tailscale` 的两行一致，备注指向可行性报告）；4.6 `ss` 流式方法一行从 ✅ 改为 🟡（它自己的备注登记着两处差异）；附录不再说"由脚本生成"（仓库里没有这样的脚本），数字随这两行改过并整张重数——第 1 节 66 / 55 / 7 / 1 / 3 / 0，第 4 节 88 / 67 / 17 / 1 / 3 / 0，合计 539 / 396 / 96 / 29 / 14 / 4；M8 设计第 12 节第 3 条"就地改了三处"实为四处；`CLAUDE.md` 状态段里 M8b 一句原来插在 M8a 一段中间，把讲偶发异常退出的那一句挪回 M8a 一段末尾 | 文档提交 |
| 29 | 终审 | 门禁（终审这一轮的代码改完的树上，本机；数字由门禁日志重新求和）：格式检查与 clippy 通过；全量 70 个结果行 / 2301 通过 / 0 失败 / 3 忽略。终审之前是 70 / 2295 / 0 / 3，多出的 6 条通过是这一轮加的本机用例：`rurge-config` 1 条，`rurge-interop` 的 `tests/shadowsocks.rs` 2 条、`tests/snell.rs` 3 条（两个新的互操作用例在本机打印 `skipping …` 后返回，也计作通过） | — |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | mihomo 与 `ssserver` 流式方法的互操作用例没有对真实二进制跑过：本机没有这两个参照，八个新的互操作用例在本机只打印 `skipping …`。夹具依据的事实都读自打了标签的源码（mihomo v1.19.32、shadowsocks-rust v1.25.0 与 shadowsocks-crypto v0.8.0；评审者独立重读过一遍，与夹具一致），没有二进制就核对不了的有 22 项，分五类：CI 步骤（三个资产能下载、SHA-256 相符，`.gz` 是单个可执行文件、`.zip` 里有一个 `.exe`，三个平台上能启动）；命令行与启动（`-d` / `-f`，不写缺省配置，主目录里只多出 `cache.db`，启动时不联网，去掉环境变量与关掉回环检测不妨碍启动）；配置的键（尤其"`obfs-opts` 的 `mode` 与 `host` 按字段名不分大小写匹配、缺 `host` 即报错"这一读法）；就绪（监听端起来到开始转发之间确有空当，经 `socks` 监听端的探测在 15 秒内成功）与 `direct` 模式下 `[TCP] … using DIRECT` 那行日志；协议行为（Snell v1 ～ v3 的 TCP、v2 的复用、v3 的 UDP，PSK 或版本不对时各平台是"结束"还是"重置"，`chacha20` 的 TCP 与 UDP，simple-obfs 的两种模式在 mihomo 的三种监听实现之前，`ssserver` 的发布包确实带流式方法、`rc4` 没有 IV）。要核对的清单在 `tests/interop/README.md`「mihomo」一节末尾与「shadowsocks-rust」一节。若不通的是 simple-obfs、Snell v1 ～ v3 或流式方法本身，要改的是 `crates/`。终审另加的两个互操作用例（执行期修正记录 #21：`tests/snell.rs` 里版本或 PSK 不符的行对 sing-box 与官方 snell-server）等的是同一次运行。它们问的是真实的 v4 / v5 服务端读不出请求时关不关连接，失败不一定是缺陷：服务端若留着连接不出声，要订正的是兼容性清单 4.2 `snell` 一行与手工验收清单里"很快失败"的说法——写明哪个服务端会让会话等到空闲超时；通过了，就把这两份文档里说它还没有核对的话去掉 | 下一次 CI 运行时核对（M8a 任务 6：CI 首跑的修正） |
| 2 | `salsa20` 没有可用的参照服务端：只有 shadowsocks-libev 支持它，libev 没有官方的预编译二进制；mihomo 的流式方法表里也没有它（同样没有的单独的 `rc4` 由 `ssserver` 覆盖）。它由 ECRYPT 的两条向量（其中一条 IV 不为零）与回环覆盖 | 手工验收（有 libev 服务端的人，`docs/acceptance/phase2-manual.md` 的 M8b 一节）；出现预编译的参照时补互操作 |
| 3 | `chacha20` 与 `chacha20-ietf` 经 `chacha20` 0.9.1 实现，两种都以 32 位计块：一条连接的每个方向在 (2^32 − 1) × 64 = 274 877 906 880 字节（256 GiB 少 64 字节）处到头，此后以 `ss: the cipher's keystream is used up` 失败（不 panic，不在回绕的密钥流下发出字节）。原版 `chacha20` 的块计数本是 64 位，别的实现过了这一点照常继续 | 接受（已登记进兼容性清单 4.2 `ss` 一行） |
| 4 | Snell v1 ～ v3 没有对官方服务端验证（官方已不提供 v1 ～ v3 的独立服务端；Surge 自带的服务端只能手工验收） | 手工验收（`docs/acceptance/phase2-manual.md` 的 M8b 一节） |
| 5 | Snell v6（`unshaped` / `unsafe-raw` 两种模式不涉及逆向得来的常数，是可选的第一步） | 见可行性报告；项目所有者表态之后 |
| 6 | M8b 之前的夹具（`SingBox::spawn` 等）仍一个一个地取端口：同一份配置的两个入站可能拿到同一个号 | 第一次 CI 运行出现时，或 M8c：改用 `rurge_interop::free_ports` |
| 7 | 本机连着跑全量门禁时见过两条与 M8b 无关的偶发失败，单独重跑都通过：`rurge-dns` 的 `bootstrap::tests::set_upstreams_takes_effect_for_the_next_resolve`（假 DNS 服务器把同一个问题数到了两次——这条用例的上游 100 毫秒没收到回答就重发）与 `rurge-engine` `tests/pipeline.rs` 的 `rebind_failure_leaves_no_listeners_and_a_later_rebind_recovers`（重绑刚放掉的端口时 os error 10048；任务 3 修正那一轮记下的情形是连跑几遍之后近两千个套接字停在 TIME_WAIT，而本机的动态端口只有一万四千个左右） | M8a 任务 6（CI 上若也出现）或 M8c：前者是计时依赖，后者是端口复用，各看一眼 |
| 8 | Snell"没有应答"提示的已知限制：只管读方向——重置若先出现在一次写上（首段负载很长，服务端读了开头就关），拿到的是系统自己的文本；提示以"服务端随它的第一次写才发 salt"为前提，抢先发了 salt、再因解不开而关闭的服务端得到的是不带提示的那一句；上传途中、服务端一个字节还没回时被重置，同样算"什么都没发"。另：真实的 mihomo 在各平台上给出的是"结束"还是"重置"那一句，通过的互操作用例不显示 | 接受（已登记进兼容性清单 4.2 `snell` 一行）；实际是哪一句等第一次 CI 运行或手工验收时记下 |
| 9 | `snell` 的 `udp-port`：代码注释（`spec/snell.rs`、`snell/mod.rs`、`tests/outbounds_snell.rs`）说它"服务于 v5 的 QUIC Proxy Mode"，兼容性清单 4.2 / 4.5 把手册的说法（服务端的 TCP 与 UDP 不在同一端口时用）等同于那个模式的 UDP 端口；任务 3 的实现者读到的手册页只有前一半，没有提 QUIC Proxy Mode（经抓取工具转述，没有核对原文）。行为不受影响：UDP 一律经主端口，`udp-port` 在 v1 ～ v5 上只解析（`W0029`）。v1 / v2 根本不载 UDP，在那里报 `W0028` 也讲得通，没有改 | M8c：对照手册原文订正措辞 |
| 10 | 评审留下的小项，都不影响行为：`shadowsocks/udp.rs` 的 `open_stream` 每收一个数据报分配并复制一次（AEAD 是就地打开）；`ShadowsocksOutbound` 用三个并列的 `Option`（`key` / `identity` / `stream`）表示四种封装；`shadowsocks/stream.rs` 一个文件里是密码、流、包与它们的用例；`ChunkedStream` 与 `V4Stream` 的写出、结束与失败标记是平行的两份代码（v4 的逻辑不许动）；`SnellScript` 用两个布尔 `obfs_http` / `obfs_tls` 而 `ShadowsocksScript` 用 `Option<ObfsMode>`（原来还列着 `FakeSnell` 的两条——`reset` 字段的注释，与 ConnectV2 上回了 `Remote EOF` 之后也关连接——，终审时改了，见执行期修正记录 #25） | 动到那些文件时再整理 |
| 11 | 只靠读代码、没有本机用例盖住的三处：假服务端连上游的 2 秒时限（三个平台上没有可移植的"既不应答也不拒绝"的回环地址）；`outbounds_snell.rs` 新加的引擎用例没有各自的 RED 记录（写下时下层已做完，由下层的同类用例与变异覆盖）；v2 复用的互操作用例里"回显先绑、mihomo 后起"的次序 | 接受；最后一项在 CI 上跑到时自然覆盖 |
| 12 | 一行已经有错误时仍多说一句的同类情形，M8b 之前就有、终审没有动（执行期修正记录 #24 只改了 M8b 自己加的两条告警）：`direct` 策略上取值写错的 `ecn` / `no-error-alert`，`E0018` 之外还有一条"不适用于 `direct`"的 `W0028`；`hysteria2`、`tuic-v5`、`masque`、带 `h3=true` 的 `trust-tunnel` 与 `external` 上取值写错的 `tfo`，另有一条"不适用"的 `W0028`；`vmess` 的 `tls` 取值写错时对 TLS 参数（如 `sni`）的 `W0028`，`trojan` 的 `ws` 取值写错时对 `ws-path` / `ws-headers` 的 `W0028`；`interface` 写成空值时对 `dns-follow-interface` 的 `W0028`；`underlying-proxy` 指向不存在的策略（`E0007`）时对 `interface` 的 `W0028`。都只是在错误之外多一条告警，不影响加载结果 | M8c 的配置小项里一并看；不改也无害 |
