# 阶段 3 / M2「进程识别」细化设计

| 项       | 内容 |
| -------- | ---- |
| 日期     | 2026-10-17 |
| 状态     | 草案。已决事项 M2-D1 ～ D19 是在项目所有者"自己决定、继续做"的常设授权下定的（代项目所有者作的裁定），所有者可以改；D17 ～ D19 与 D3、D6 的改写是一位评审者读过初稿之后加的 |
| 上位设计 | [阶段 3 总设计](2026-10-16-phase3-enhanced-mode-design.md) 第 3.5、3.6、7、12、13、14 节；不一致处以本文为准，订正列在第 10 节 |
| 需求     | FR-IN-07（进程识别）、FR-RULE-01 的 `PROCESS-NAME`（三平台可用；文件名 / 全路径 / 前缀三种模式；Windows 路径归一化） |
| 清单     | 3.1 节 `PROCESS-NAME`（L242）；内部集 `SYSTEM`（L306）；`GET /v1/requests/recent` 与 `active`（L873） |

M2 让 `PROCESS-NAME` 规则在三个平台上真的匹配，并让请求记录带上发起会话的进程。它不需要虚拟网卡：今天经系统代理与代理端口从本机进来的连接就有进程可查。开发机（一台 Windows 11 x86-64）上能测的是 Windows 的真实读取（只读系统的表）、三个平台的解析（夹具）与全部与平台无关的逻辑；Linux 与 macOS 的真实读取在 CI 与手工验收里核对。

## 1. 范围

| 做 | 不做（去向） |
| -- | ------------ |
| `rurge_platform::procinfo`（新模块）：三个平台查"一条本机连接属于哪个进程"（PID、文件名、路径）；表的快照、批量与限频；来源是不是本机 | 经虚拟网卡来的会话（M3：`Tun` 会话接上同一个查找） |
| 引擎：HTTP 与 SOCKS5 监听器上可能从本机来的会话在开始时在后台查一次，SOCKS5 的 UDP 关联查一次；规则走到要看进程的匹配器时等它（至多 200 毫秒） | 外部程序的进程树判断（M4，防环用；它要的 PID 由本里程碑给出） |
| `PROCESS-NAME` 按查到的进程匹配；Windows 的路径写法与不分大小写的比较；内部集 `SYSTEM` 的两条 `PROCESS-NAME` 只在 macOS 上编进集合 | `$request.processPath`（阶段 5；读同一个进程槽） |
| 请求记录与 API 的 `pid`、`processPath`；DEBUG 日志 | 进程的图标、代码签名、App Bundle 的显示名（不做） |
| `rurge check` 不再说 `PROCESS-NAME` 永不匹配 | `rurge rule match` 查真实的进程（离线命令没有连接；`--process` 照旧） |
| unsafe：五个新函数与钉住名单的用例 | 计费网络模式按进程放行（远期）；给使用者的查找开关（不做，M2-D3） |
| 只供测试的 `RURGE_PROCESS_BACKEND=off`；起二进制的辅助函数缺省设它 | |
| 文档：兼容性清单、`docs/api/phase3.md`、`docs/acceptance/phase3-manual.md`、CLAUDE.md、README；M1 停放的四件（第 7 节） | |

## 2. 数据模型

`rurge_config::session::ProcessInfo` 加 PID：

```rust
pub struct ProcessInfo {
    /// `None` when the process was given rather than found
    /// (`rurge rule match --process`).
    pub pid: Option<u32>,
    /// The executable's file name: what `PROCESS-NAME` name patterns match.
    pub name: String,
    /// The executable's full path, when it could be read.
    pub path: Option<String>,
}
```

`SessionInfo.process` 的含义改为"调用方事先知道的进程"：离线的 `rurge rule match --process <名字或路径>` 照旧填它（含 `/` 或 `\` 的是路径，名字取最后一段，PID 是 `None`）；经监听器进来的会话不填，进程由引擎在评估时按需交给规则（第 5.1 节）。匹配器先看它，再看评估时拿到的。它的文档注释（今天写着 "filled in by the platform layer from M3 on"）随之改。

## 3. 平台层（`rurge_platform::procinfo`）

### 3.1 接口

```rust
pub enum Protocol { Tcp, Udp }

pub struct Query {
    pub protocol: Protocol,
    /// The client's own endpoint: the session's source address.
    pub local: SocketAddr,
    /// TCP only: the port the client connected to (rurge's listening port).
    pub remote_port: Option<u16>,
    /// When the session began; decides which reads of the table can answer.
    pub since: Instant,
}

pub struct Owner {
    pub pid: u32,
    pub name: String,
    pub path: Option<String>,
}

impl Finder {
    pub fn new() -> Finder;
    /// Blocking: the caller runs it on a blocking thread.
    pub fn owner(&self, query: &Query) -> Option<Owner>;
    /// Never blocks: whether `ip` is this machine's, when that is known
    /// without reading anything (`None`: the list of addresses is stale).
    pub fn is_local(&self, ip: IpAddr) -> Option<bool>;
}
```

同步、阻塞，由调用方放进 `spawn_blocking`（总设计第 7 节）。三个目标平台之外 `owner` 恒为 `None`。

**找哪一行**：会话的来源是客户端自己的套接字。TCP 找本地端点等于会话的来源地址、远端端口等于客户端连到的端口（rurge 的监听端口）的那一行——回环上 rurge 接受的那一端也在表里，它的本地端点是 rurge 的监听地址，不会被认错。UDP 找本地端点相等的那一行；没有时取同一地址族、同一端口、本地地址未指定（`0.0.0.0` / `::`）的那一行。IPv4 的查询先找 IPv4 的表，再在 IPv6 的表里找它的映射形式（`::ffff:a.b.c.d`：双栈套接字连 IPv4 地址时在那里）。比较地址与端口，不比较 IPv6 的作用域号。

**来源不是本机的不查**：本地地址既不是回环（`127.0.0.0/8`、`::1`、映射形式的回环）、也不是本机网卡上的地址时，`owner` 不读任何表就返回 `None`。本机地址的列表取自 `if-addrs`（已是 `rurge-platform` 的依赖），至多 2 秒旧：查询时列表已超过 2 秒就重取。不阻塞的预判 `is_local` 只看手里已有的：回环一定是；列表不超过 2 秒时按列表答；列表过期时答"说不准"（`None`），由阻塞的 `owner` 重取再判断。引擎用它把显然不是本机的来源挡在查找之外（第 4.2 节）。

**PID**：PID 为 0 的行跳过；Windows 上 PID 4 是 `System`（没有路径，不去打开它）。

### 3.2 快照、批量与限频

- 每种协议（TCP、UDP）一份快照，含 IPv4 与 IPv6 两张表，记下**读取开始**的时刻。M2 里引擎只发 TCP 的查询（SOCKS5 的 UDP 关联也按控制连接查，第 4.1 节）；UDP 的读取随平台层一起做、对夹具与本机测，M3 的虚拟网卡上的 UDP 流用它。
- 一次查询能不能用某份快照：TCP 在快照里找到了，且快照的时刻不早于会话开始之前 2 秒，就用它；快照的时刻不早于会话开始，找不到就是找不到；其余情形重读一次。UDP 找到了也要快照的时刻不早于会话开始。理由：TCP 连接在 rurge 接受它之前就建好了，会话开始之前不久的快照里已经有它，同一个本地端点 2 秒内换给另一个进程的可能小到可以不计；UDP 的套接字没有远端可以核对，只信会话开始之后的读取。
- 每种协议一把锁，一次只有一个查询在读或看这种协议的快照；排在后面的查询拿到锁时先看现有的快照——一次读取因此服务所有在它开始之前开始的会话，一阵新连接共用一两次读取。
- 同一种协议的两次读取之间至少隔 20 毫秒（从上一次读取结束算）；要重读而不够 20 毫秒时在锁里等够再读。持续不断的新会话最多让一张表每秒读 50 次左右。
- PID → 名字与路径：每份快照一张备忘，同一份快照里同一个 PID 只问一次系统；换了快照就重新问（PID 会被重用）。
- 读取失败：这次查询是 `None`；日志见第 6.3 节。

### 3.3 Windows

- **表**：`GetExtendedTcpTable`（`TCP_TABLE_OWNER_PID_CONNECTIONS`：只有连接，不含监听）与 `GetExtendedUdpTable`（`UDP_TABLE_OWNER_PID`），各按 `AF_INET` 与 `AF_INET6` 读一次。缓冲区是 `Vec<u32>`（四字节对齐）；先以 0 字节问大小，得 `ERROR_INSUFFICIENT_BUFFER`（122）就按给出的大小分配再读；表在两次调用之间长大了就再来，至多 4 次；别的返回值是这次读取失败。
- **行的解析**是纯函数（在每个宿主上编译、测试），DWORD 一律按小端读。表头是行数（偏移 0），行从偏移 4 开始：
  - TCP IPv4 行 24 字节：状态、本地地址（4 个字节依次就是地址的 4 段）、本地端口（这个 DWORD 的前两个字节，网络字节序）、远端地址、远端端口、PID；
  - TCP IPv6 行 56 字节：本地地址 16 字节、本地作用域号、本地端口、远端地址 16 字节、远端作用域号、远端端口、状态、PID；
  - UDP IPv4 行 12 字节：本地地址、本地端口、PID；UDP IPv6 行 28 字节：本地地址 16 字节、作用域号、本地端口、PID；
  - 行数大于缓冲区放得下的是坏数据，只读放得下的行（不 panic）；状态为 `TIME_WAIT`（11）的行跳过。
- **路径**：`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)`，把句柄交给 `OwnedHandle`（同 `process::windows::job_for`，`Drop` 时关闭），`QueryFullProcessImageNameW(句柄, PROCESS_NAME_WIN32, 缓冲区, 长度)`（缓冲区 32768 个 UTF-16 单元）；名字是路径最后一个 `\` 之后的部分。打不开或问不出时这个进程是"不知道"（例如以普通用户运行时，别的用户的某些服务进程）。
- `windows-sys` 加 `Win32_NetworkManagement_IpHelper` 特性（第 9 节 V1）。

### 3.4 Linux

- **表**：`/proc/net/tcp`、`tcp6`、`udp`、`udp6`（关掉 IPv6 的系统上没有后两个，当作空表）。每行按空白切：第 2 列本地端点、第 3 列远端端点、第 4 列状态、第 8 列 uid、第 10 列 inode；inode 为 0 的行（`TIME_WAIT` 等）跳过。端点写成 `地址:端口` 的十六进制：端口是普通的十六进制数；IPv4 地址是内核把网络字节序的 32 位数按**本机字节序**打印成的 8 位十六进制（小端机器上 `127.0.0.1` 是 `0100007F`），IPv6 地址是四段这样的 8 位十六进制。解析器带字节序参数，真实读取传本机的，夹具两种都测。
- **inode → PID**：一张跨快照的表（inode → PID），每次读表之后只留当前表里还有的 inode；找不到时先扫"最近找到过的进程"（至多 16 个）的 `/proc/<pid>/fd`，再扫 `/proc` 下全部数字目录；扫描把见到的每个 `socket:[N]` 都记进表（一阵新连接因此共用一次扫描）；读不了的 `fd` 目录（不以 root 运行时，别的用户的进程）不声不响地跳过。
- **路径**：`/proc/<pid>/exe` 的链接目标，去掉末尾的 ` (deleted)`；名字是它的文件名。链接读不了时名字取 `/proc/<pid>/comm`（内核把它截到 15 个字节），路径不知道；两个都读不了就是"不知道"。
- **不用 FFI，也不用 netlink**：读取写在一个很小的文件系统接口（读文件、读目录、读链接）上，在每个宿主上编译，用例给一棵内存里的 `/proc`；真实实现用 `std::fs`。

### 3.5 macOS

- **表**：`sysctlbyname("net.inet.tcp.pcblist_n")` 与 `sysctlbyname("net.inet.udp.pcblist_n")`：先以空缓冲区问大小，分配"大小加八分之一"；得 `ENOMEM`（表长大了）就加倍再来，至多 4 次；用调用返回的实际长度。
- **解析**是纯函数（在每个宿主上编译、测试），数字按小端读（macOS 的两种处理器都是小端），**不按记录大小切**——记录大小随 Darwin 版本变过（mihomo 按 `kern.osrelease` 在 384 与 408 字节之间选）——，而按每一项自己的长度与种类走：跳过开头的 `xinpgen`（它的第一个 u32 是自己的长度）；之后每一项以（长度 u32、种类 u32）开头，前进"长度向上取整到 8 的倍数"；长度不大于 24（结尾的 `xinpgen`）、剩下不足 8 字节或这一项越过缓冲区时停。这是 `netstat` 自己的走法。
  - 种类 `XSO_INPCB`（0x10）开始一条记录：外部端口在偏移 16、本地端口在 18（网络字节序），`inp_vflag` 在 44（`INP_IPV4` = 0x1、`INP_IPV6` = 0x2），外部地址 16 字节在 48（IPv4 在 60），本地地址 16 字节在 64（IPv4 在 76）。这一项短于 80 字节时这条记录不用。
  - 种类 `XSO_SOCKET`（0x1）属于当前记录：`so_last_pid` 在偏移 68。这一项短于 72 字节时这条记录不用。
  - 其余种类跳过。
  - 偏移取自 XNU 的 `bsd/netinet/in_pcb.h`（`xinpcb_n`）与 `bsd/sys/socketvar.h`（`xsocket_n`），两者都在 `#pragma pack(4)` 里；与 mihomo、sing-box 读的偏移一致（第 9 节 V5；XNU 是 APSL，mihomo 是 GPL，只取事实）。
  - 一份表里有项，却没有一条记录同时有够长的两项：布局对不上，这次是"不知道"（日志见第 6.3 节）。
- **取哪个 PID**：`so_last_pid`（最后用这个套接字的进程；mihomo 与 sing-box 都取它）。`so_e_pid`（委托时的"实际"进程，例如 `nsurlsessiond` 替 App 建的连接）不用，留给手工验收看真实的样子（第 7 节）。
- **路径**：`proc_pidpath(pid, 缓冲区, PROC_PIDPATHINFO_MAXSIZE)`（4096 字节）；名字是文件名；失败是"不知道"。
- `libc` 经 `nix::libc`（`nix` 0.31.3 重新导出 `libc`，`rurge-platform` 在 Unix 上已依赖 `nix`），不加依赖边；不读 `kern.osrelease`。

### 3.6 unsafe

新增五个标 `#[allow(unsafe_code)]` 的函数，每个只包一次 FFI 调用或一组紧密相关的调用，文档注释写明安全前提：

| 平台 | 函数（名字在计划里定） | 包的调用 |
| ---- | ---------------------- | -------- |
| Windows | TCP 表 | `GetExtendedTcpTable` |
| Windows | UDP 表 | `GetExtendedUdpTable` |
| Windows | 映像路径 | `OpenProcess`、`OwnedHandle::from_raw_handle`、`QueryFullProcessImageNameW` |
| macOS | PCB 表 | `sysctlbyname`（问大小与读） |
| macOS | 进程路径 | `proc_pidpath` |

连同已有的 `sysproxy::windows` 的 `notify_wininet` 与 `process::windows` 的 `job_for`，`rurge-platform` 里共七个。照总设计第 3.6 节，`rurge-platform` 加一条用例：读自己 `src/` 下的全部源码，找出每个 `#[allow(unsafe_code)]` 和紧跟它的 `fn` 的名字，与一张固定的名单（文件与函数名）比对——多一个、少一个、标在 `fn` 之外的东西上，都让它变红。`Cargo.toml` 里 `[lints.rust]` 上面的注释（今天写着"两个函数"）与 CLAUDE.md 的 `unsafe_code` 一段一并改。macOS 的两个函数在开发机上编译不到，第一次 CI 运行时核对。

## 4. 引擎

### 4.1 哪些会话查

- 经 HTTP 与 SOCKS5 监听器进来的会话（`ListenerKind::Http`、`Socks5`）：查。查询是 TCP，本地端点是会话的来源地址，远端端口是会话进来的监听端口（`in_port`）。
- SOCKS5 的 UDP 关联：每个关联查一次——查的是它的控制连接（TCP；本地端点是控制连接的对端，远端端口是 SOCKS5 的监听端口），关联的每条流共用这一次的结果。理由：UDP 的套接字没有远端可以核对，而控制连接与 UDP 套接字是同一个 SOCKS 客户端开的。
- rurge 自己发起的会话（`Internal`：内部 DNS 会话、连通性测试）不查。`Tun` 在 M3 接上同一个查找；`Forward` 今天没有监听器产生它，到时由它的里程碑定。
- 来源是不是本机由平台层判断（第 3.1 节）。引擎对上面两种会话先问不阻塞的 `is_local`：答"不是本机"的不查（第 4.2 节），答"是"或"说不准"的发起查找。

### 4.2 什么时候查、规则等多久

- **会话开始时在后台查**，不论规则用不用得到进程：请求记录总有进程可看，使用者才知道 `PROCESS-NAME` 该写什么（M2-D3）。规则用不到进程的会话从不等它。
- **先不阻塞地看来源**：引擎先问 `is_local`（第 3.1 节）。答"不是本机"时不查：槽当场落定为"不知道"，不起任务、不占阻塞线程——局域网里把 rurge 当代理用的设备连进来的会话都是这样，它们的进程本机本来就看不到。答"是"或"说不准"时发起查找（"说不准"由阻塞的那一半重取本机地址再判断）。
- **一次查找**：一个 tokio 任务，先拿查找许可（引擎里一个 2 个许可的信号量，限制同时占用的阻塞线程），再 `spawn_blocking` 调平台的查找，结果落进槽——不论用了多久。
- **进程槽**：会话的进程槽（`rurge-inbound` 的 `SessionHandle` 上的新字段，类型暂名 `ProcessSlot`）只落定一次：查找结束时（"查到了某个进程"或"没找到"），或一开始就落定（不查的会话、`is_local` 答"不是本机"的会话：都是"不知道"）；能等它落定，等的一方自己定等多久；槽记着查找发起的时刻，规则的期限由它算。UDP 关联的槽由它的每条流共用（流的 `SessionHandle` 带着关联的槽）。
- **规则至多等 200 毫秒**：评估走到一个 `PROCESS-NAME`（顶层的、逻辑规则里的、集合里逐条求值的行），而这个会话的进程还没有交给这次评估时，引擎等槽落定，至多等到**查找发起之后** 200 毫秒（UDP 流的期限从关联的查找发起时算，所以关联开始 200 毫秒之后才开的流不再等），然后接着评估；一次评估至多等一次。机制同"IP 规则要先解析"：匹配器回 `Verdict::NeedsProcess`，引擎等进程、重评这一条规则（第 5.1 节）。
- **到期时**：这次评估按"不知道"判（`PROCESS-NAME` 不命中），会话的 `notes` 里加一句 `Process not known within 200 ms: PROCESS-NAME did not match`；查找照常做完，槽随后落定，记录随之带上进程（第 6.1 节）。记录里有进程、规则却没按它判的会话由这一句说明。
- **一直很慢时**（M2-D18）：不设断路器，也不让规则在等的查找插队。代价有界——每条走到 `PROCESS-NAME` 的会话至多多等 200 毫秒——，规则的结果不随此前的超时而变；一次读取服务所有在它开始之前开始的会话（第 3.2 节），排在许可后面的查找多半由同一次读取答出、在阻塞线程上只停留几微秒，插队省不下什么。规则等过的查找第一次超时时 WARN 一次（第 6.3 节）。

### 4.3 接口

```rust
// rurge-engine
pub struct OwnerQuery {
    pub transport: Transport,
    pub local: SocketAddr,
    pub remote_port: Option<u16>,
    pub since: Instant,
}

pub trait OwnerLookup: Send + Sync {
    /// Never blocks: `Some(false)` when `ip` is surely not this machine's.
    fn is_local(&self, ip: IpAddr) -> Option<bool>;
    /// Blocking; called on a blocking thread.
    fn owner(&self, query: &OwnerQuery) -> Option<ProcessInfo>;
}

// EngineShared: `None` looks nothing up (the default, and every existing test)
pub owners: Option<Arc<dyn OwnerLookup>>,
```

- **可执行文件**（`crates/rurge/src/cli/runtime.rs`）加一个适配器（暂名 `PlatformOwners`，照 `PlatformSockets`、`PlatformProcesses` 的样子）：持有一个 `rurge_platform::procinfo::Finder`，把 `OwnerQuery` 译成 `Query`、`Owner` 译成 `ProcessInfo { pid: Some(..), .. }`；`rurge run` 在设置 `shared.processes` 的地方（`crates/rurge/src/cli/run.rs`）设 `shared.owners`。`rurge-platform` 不依赖别的 crate（M1 的规矩），类型的映射在可执行文件里。
- **`rurge-rules`**：`LazyProcess` trait（`fn process(&self) -> BoxFuture<'_, Option<ProcessInfo>>`，同 `LazyResolver` 的样子）与 `NoProcess`；`RuleEngine::evaluate_with_process(s, mode, resolver, process)`。`evaluate` 与 `evaluate_traced` 照旧（传 `NoProcess`），它们今天的调用方不变。
- 规则只在 `Engine::choose_policy` 里评估（TCP 会话、UDP 流、内部会话三处都经过它），那里把会话的进程槽包成 `LazyProcess` 传进去；包装带着期限（查找发起的时刻加 200 毫秒），并记下这次评估是不是等到了期限，`choose_policy` 评估之后据此写第 4.2 节那一句 `notes`、DEBUG 行与第一次的 WARN（与子规则那一句写在同一处）。

## 5. 规则

### 5.1 匹配器

- `EvalCtx` 多一项"这次评估拿到的进程"，三种状态：还没问过、问过而不知道、问到了某个进程。
- `PROCESS-NAME`：`SessionInfo.process` 有值就用它；否则看 `EvalCtx`——问过了就按结果匹配（不知道是不命中），没问过就回 `Verdict::NeedsProcess`。
- `Verdict` 加 `NeedsProcess`。组合的规矩同 `NeedsResolve`：`AND` 遇到确定的不命中就不命中，`OR` 遇到命中就命中，`NOT` 原样传出，集合里逐条求值的行遇到命中就命中；结果还定不下来时，**要进程优先于要解析**——进程只在本机查、至多 200 毫秒，先拿到它可能让 DNS 查询变得不必要（`AND,((PROCESS-NAME,x),(IP-CIDR,…))` 在进程不是 `x` 时不解析）。
- 引擎的循环：`NeedsProcess` → 等进程、记进 `EvalCtx`、重评这一条规则；`NeedsResolve` → 照旧解析（失败照旧是 DNS 失败的决定）。两种需要各至多满足一次，所以一条规则至多评三遍。
- 预匹配的那一遍不等进程：`NeedsProcess` 在那里算不命中，规则留在自己的位置上再评（同 `NeedsResolve` 的读法）。`PROCESS-NAME` 本身不许带 `pre-matching`（手册："the originating process is not known at the DNS-query stage"，解析器早就拒绝），它只会经集合进到预匹配里。

### 5.2 写法

一个 `PROCESS-NAME` 的值怎样分类（`rurge-config`，与平台无关：在 Linux 上 `rurge check` 一份为 Windows 写的 Profile，读法相同）：

| 值 | 模式 |
| -- | ---- |
| 以 `/` 开头；或以盘符加 `:\` / `:/` 开头（`C:\`、`d:/`）；或以 `\\` 开头（UNC） | 路径；同时以 `/` 或 `\` 结尾时是前缀 |
| 其余 | 文件名 |

通配符 `*` 与 `?` 照旧（手册："Wildcards `*` and `?` are supported"）。此前只认以 `/` 开头的路径，写进规则的 Windows 路径一律当作文件名、永不匹配；这是读法的变化，登记在清单。

### 5.3 比较

- 文件名模式比可执行文件的文件名，路径与前缀模式比完整路径（手册："it matches only the executable file name, regardless of its path"）。
- macOS 与 Linux 区分大小写（手册："Matching is case-sensitive in all modes."）。
- Windows 不区分大小写：文件名与路径都按 ASCII 不分大小写比，路径里的 `/` 当作 `\`（路径的这一半阶段 1 就是这样，`normalize_process_path`；文件名这一半是新的）。手册只讲 Mac，Windows 的文件系统不分大小写；这是 rurge 的读法，登记在清单。
- 不去掉 `.exe`：Windows 上写 `chrome.exe` 或 `chrome*`（mihomo 与 sing-box 也是这样）。

### 5.4 `rurge check`

`PROCESS-NAME` 从可执行文件的"永不匹配的规则类型"里去掉（`crates/rurge/src/capabilities.rs` 的 `INACTIVE_RULE_TYPES`），`W0013` 不再因它出现——三个平台一样。`rurge-config` 自己的用例里拿 `PROCESS-NAME` 当"永不匹配"的例子的那几条自带能力表，不受影响。

### 5.5 内部集 `SYSTEM`

`SYSTEM` 的最后两行是 `PROCESS-NAME,trustd` 与 `PROCESS-NAME,netbiosd`（`crates/rurge-rules/src/set_format.rs` 的 `SYSTEM_SET`，照手册的列表）。它们指 macOS 的两个系统服务；在 iOS 上 Surge 本来就不认 `PROCESS-NAME`。M2 之后照字面编进集合的话，在 Windows 与 Linux 上每条走到 `RULE-SET,SYSTEM` 而没被它的域名行接住的会话——常见的 Profile 把这一条写在最前面，所以几乎是全部会话——都要等一次进程查找，只为两个在那里不存在的服务。

所以（M2-D17）：这两行**只在 macOS 上**编进集合；别的平台上解析这个内部集时把以 `PROCESS-NAME,` 开头的行去掉（编译期的 `cfg!(target_os = "macos")`，两处解析内部集的地方都这样做：`registry.rs` 的 `Key::Internal` 分支与 `engine.rs` 用例里的那一处）。集合的文字不变（`internal_set_text` 照旧返回手册的列表，钉住它的 `the_built_in_sets_are_the_lists_the_manual_gives` 不改）。代价：在 Windows 与 Linux 上真叫 `trustd` 的进程不被 `SYSTEM` 接住（M2 之前任何平台上都没有进程被它接住）；`rurge rule match --platform macos --process trustd` 在别的系统上也不命中这一行（集合按 rurge 运行的系统编译）。两条都登记在清单 L306。

## 6. 记录、API 与日志

### 6.1 请求记录

`RequestRecord` 带着会话的进程槽，读记录时再看它：会话结束之后才查到的进程照样出现在记录里（被 `REJECT` 的会话往往在查找结束之前就结束了）。活动会话的列表同样读槽。

### 6.2 API

`GET /v1/requests/recent` 与 `GET /v1/requests/active` 的每条记录多两个键，紧跟 `listenPort`：

- `pid`：进程号，数字；
- `processPath`：可执行文件的完整路径，字符串。

不知道时是 `null`：会话不是从本机来的；rurge 自己发起的会话；查找还没有结束（结束之后再读记录就有；规则因此没按进程判的会话，`notes` 里有第 4.2 节那一句）；本机看不到发起的进程（虚拟机、WSL2、容器里的程序经网络连进来——它们在本机没有对应的套接字；经本机的转发程序来的连接，查到的是转发程序）；没有权限读那个进程（不以 root 运行的 Linux 上别的用户的进程，Windows 上某些系统进程）。路径读不到而只有名字时（Linux 的 `comm`、Windows 的 `System`），`pid` 有、`processPath` 是 `null`。键名同 Surge 的面板读的字段（取自第三方的面板，Surge 的手册没有写请求记录的形状）；记录的其余形状仍是 rurge 自己的（清单 L873，阶段 6 对齐）。

### 6.3 日志与个人信息

- 查找结束时一行 DEBUG：查到的 PID、名字与路径，或"没找到"，和查找用了多久；带会话号（UDP 关联带来源地址）。规则等到期时另一行 DEBUG，与 `notes` 那一句同文。
- 规则等过的查找第一次到期时 WARN 一次：`process lookup took longer than 200 ms; PROCESS-NAME rules decided without the process`（不带会话、地址与进程），之后同样的情形只在 DEBUG 与记录的 `notes` 里。
- INFO 及以上的行里没有进程（会话被拒、失败的 INFO 行照旧）：进程路径在 Windows 上常带用户名（总设计第 12 节）；请求记录与 API 照 Surge 带它。
- 平台读取失败：每一种（每种协议的表读不了；macOS 上布局对不上）在进程的一生里 WARN 一次，说是哪一张表与系统的错误，之后同样的失败只在 DEBUG 里。
- 不知道进程从不让会话失败，也不进会话的 `error`。

## 7. 诊断与文档

- **诊断**：没有新的诊断码；`W0013` 不再因 `PROCESS-NAME` 出现（第 5.4 节）。
- **兼容性清单**：L242 `PROCESS-NAME`（三个平台的查法与边界、Windows 的路径写法与不分大小写、不去掉 `.exe`、什么时候查、200 毫秒、看不见的来源、记录里怎样出现）；L306 `SYSTEM`（在 macOS 上 `PROCESS-NAME,trustd` 与 `netbiosd` 开始匹配；别的平台上这两行不编进集合，`rule match --platform` 不改变这一点，第 5.5 节）；L873 请求记录（`pid`、`processPath`）；附录的统计。
- **`docs/api/phase3.md`**：请求记录的两个键、什么时候是 `null`、DEBUG 日志行。
- **`docs/acceptance/phase3-manual.md`** 加 M2 一节：三个平台上真实的浏览器与命令行工具经系统代理与代理端口时记录里的进程；`PROCESS-NAME` 的文件名、路径、前缀三种写法真的命中（Windows：大小写不同的写法、UNC 路径上的程序；Linux：以普通用户与以 root 运行时别的用户的进程；macOS：经 `nsurlsessiond` 等系统服务代建的连接记录里是谁，即 `so_last_pid` 与 `so_e_pid` 的取舍）；虚拟机、WSL2、容器的客户端是"不知道"；几百条并发连接时查找是否在 200 毫秒里结束。
- **需求文档**：FR-IN-07 的写法已与本设计一致，不改。
- **CLAUDE.md**：状态一段、文档列表（本设计与 M2 的计划）、`unsafe_code` 一段（七个函数）、常用命令里 M2 的过滤词、`RURGE_PROCESS_BACKEND` 的说明与测试约定里的一句（第 8.1 节）；README 两份的阶段 3 进度。
- **M1 停放的四件**（M1 收尾时登记，随 M2 的文档任务处理）：一、DEBUG 日志里其实出现 DNS 服务器的地址（并发查询的 `udp://<地址>:53`），而 `docs/api/phase3.md` 与 CLAUDE.md 说"本机地址与 DNS 服务器的取值任何级别的日志里都没有"——订正；二、`--system-proxy` 遇上首读慢、又落在一个 `suspend` 的网络上时，系统代理会翻转一次（先设上，读完再恢复），没有写——补写；三、清单 `suspend` 一行没写"掉线不算离开"——补写；四、五条起守护进程的端到端用例依赖第一次读网络在 1 秒的预算里完成，第一次 CI 运行时留意——记进 M2 计划的延后事项。

## 8. 测试

| 层 | 用例 |
| -- | ---- |
| `rurge-config` | `PROCESS-NAME` 值的分类表（文件名、`/` 路径、`/` 前缀、`C:\` 与 `c:/` 路径、`C:\dir\` 前缀、`\\server\share\` 前缀、带通配符的、只有一个 `/` 的）；`ProcessInfo` 带 PID |
| `rurge-rules` | `NeedsProcess` 在 `AND` / `OR` / `NOT` 与集合逐条求值的行里怎样传；要进程优先于要解析（`AND` 里进程不命中时解析器一次也没被问）；一次评估至多问一次进程（计数的 `LazyProcess`）；`SessionInfo.process` 有值时不问；预匹配不问；Windows 与别的平台的比较（纯函数带平台参数，同 `normalize_process_path_for`）；`SYSTEM` 编出来的集合在 macOS 上有两条进程行、在别的平台上没有（走到它而没被域名行接住的会话在 macOS 之外一次也不问进程），集合的文字照旧 |
| `rurge-platform` | Windows 四种行的解析（截断的缓冲、行数大于缓冲、`TIME_WAIT` 与 PID 0）；Linux 四个文件的解析（两种字节序、IPv4 映射、inode 0）与 inode 扫描对内存里的 `/proc`（先扫最近的进程、再扫全部、读不了的目录、表的修剪、` (deleted)` 与 `comm`）；macOS 的走法对测试里拼出的缓冲（两种记录大小、不认识的种类、太短的项、结尾的 `xinpgen`、IPv4 / IPv6 / 双栈的 `inp_vflag`、布局对不上）；与平台无关的部分对假的表来源（新鲜度的几种情形、一次读取服务一批查询、两次读取至少隔 20 毫秒、UDP 的未指定地址、IPv4 查询找映射形式、来源不是本机时不读表、本机地址列表的刷新、`is_local` 的三种回答：回环、列表新鲜时、列表过期时）；在本平台上查测试进程自己的回环 TCP 连接与 UDP 套接字（PID 与路径；只读；三个平台各一条，按平台编译）；unsafe 的名单 |
| `rurge-engine` | 假的 `OwnerLookup`：`PROCESS-NAME` 经 HTTP 与 SOCKS5 监听器命中；规则用不到进程的会话不等查找（假查找挂住时会话照样完成）；规则等查找；到期：假查找挂在一个由用例放开的闸上，规则在 200 毫秒之后按不命中判、`notes` 里有那一句，放开之后记录带上 PID 与路径（不用固定的等待，也不用暂停的时钟，V9）；`is_local` 答"不是本机"时假查找一次也没被调用、槽是"不知道"，答"说不准"时照查；内部会话不查；SOCKS5 UDP 关联只查一次，它的流都命中；请求记录带 PID 与路径，会话结束之后才查到的也带；规则等过的查找第一次到期时的 WARN 只有一行（`LogCapture`） |
| `rurge-api` | 请求记录的 `pid` 与 `processPath`：有值与 `null` |
| `rurge`（bin，`--test cli`） | `rurge check` 不再说 `PROCESS-NAME` 永不匹配；`RURGE_PROCESS_BACKEND` 的取值（`off` 时记录里没有进程，别的取值以 2 退出、在绑定任何端口之前）；起真实的二进制（去掉辅助函数缺省设的 `off`）：`PROCESS-NAME,<测试二进制的文件名>,REJECT` 加 `FINAL,DIRECT`，测试进程经 HTTP 代理连到自己的回环监听器，被拒，`GET /v1/requests/recent` 里那条记录的 `pid` 是测试进程的、`processPath` 是它的路径（总设计第 14 节验收标准 5："三个平台的 CI 上查测试进程自己"）；经路径与前缀写法各一条 |
| 基准（忽略的用例，不作门禁） | 开着 N 条回环连接时一次新查询的耗时（读表加问路径），在各平台上各自跑；数字记进计划的执行期修正记录，只用来知道典型的代价，什么也不卡 |

所有用例照现有的规矩：只用回环、端口 0、有界等待；起二进制的用例经 `rurge_command()` / `rurge_process()` 并给 `--data-dir`；进程识别的用例只读系统的表；计时的用例提交前单独循环跑二十次以上。

### 8.1 只供测试的后端（M2-D19）

`rurge run` 设 `shared.owners` 之前读 `RURGE_PROCESS_BACKEND`：没设时查本机；`off` 时不查（`shared.owners` 留 `None`，记录里没有进程，`PROCESS-NAME` 一律按不知道判）；别的取值是错误，以 2 退出（``unknown RURGE_PROCESS_BACKEND value `<取值>` (expected off)``），从不退回真实的查找——同 M1 的 `RURGE_NETWORK_BACKEND` 的规矩。它是仅供自动化测试使用的开关，不进 `--help`，不是给使用者的查找开关（M2-D3）。

`crates/rurge/tests/cli.rs` 的 `rurge_command()` 与 `rurge_process()` 缺省把它设为 `off`，所以现有的起守护进程的用例照旧不查进程（不多出阻塞线程、读表与 DEBUG 行）；查真实进程的端到端用例在构造之后去掉它。不另加隐藏的命令行选项：`external` 策略拉起的嵌套 `rurge run` 拿不到这个变量、照常查本机，查找只读系统的表，不影响用例（M1 的 `--network-backend` 是因为读真实的网络会让嵌套的守护进程清缓存、作废测速）。

## 9. 写计划时要核对的事项

- V1：`windows-sys` 0.61.2 里 `GetExtendedTcpTable` / `GetExtendedUdpTable` 的特性门（`Win32_NetworkManagement_IpHelper`）与参数类型；`TCP_TABLE_OWNER_PID_CONNECTIONS`、`UDP_TABLE_OWNER_PID`、`PROCESS_QUERY_LIMITED_INFORMATION`、`PROCESS_NAME_WIN32` 的名字；`HANDLE` / `BOOL` 是否要另开 `Win32_Foundation`（`job_for` 今天不开也编得过）。
- V2：`SessionHandle` 的全部构造点（`rurge-inbound` 自己的用例、别的 crate 的用例）与进程槽的构造方式；槽用 `OnceLock` 加 `Notify` 还是 `watch`。
- V3：工作区里对 `Verdict` 的每一处穷举匹配（加一个变体会让它们编不过）：`matcher.rs`、`set.rs`、`engine.rs`（`verdict_name`）、`registry.rs`、`rurge rule match --explain` 打印的名字与它们的用例。
- V4：请求记录在哪里建（`observe.rs` 的 `record_of`），活动会话的列表怎样读，`RequestRecord` 是否要 `Clone` / `PartialEq`（它将带着一个 `Arc` 的槽）。
- V5：macOS 的偏移：照 XNU 当前发布的 `bsd/netinet/in_pcb.h`（`xinpcb_n`、`xinpgen`、`XSO_*`）与 `bsd/sys/socketvar.h`（`xsocket_n`）重算一遍，与 mihomo / sing-box 的偏移（18、44、64、76、68）和 `network_cmds` 里 `netstat` 的走法对照；只取事实，代码不抄。
- V6：Linux：`/proc/net/tcp` 的列位置与十六进制编码的出处（内核 `net/ipv4/tcp_ipv4.c` 的 `get_tcp4_sock`，`tcp6`、`udp` 同理）；`/proc/<pid>/exe` 的 ` (deleted)` 后缀；`comm` 的长度。
- V7：`if-addrs` 的版本，`get_if_addrs()` 是否含回环，在 Windows 上一次的耗时。
- V8：明文 HTTP 的绝对 URI 转发是否一个请求一个会话（那样每个请求各查一次，同一条客户端连接上 2 秒内的请求共用快照）。
- V9：用例里的计时：用了暂停的时钟（`start_paused`）的用例里，`spawn_blocking` 不算运行时的工作，时钟可能在阻塞线程还在干活时自动前进，让 200 毫秒的期限提前到——碰到查找的用例不用暂停的时钟，或让假查找立刻返回。
- V10：端到端用例：测试二进制在三个平台上的文件名（`cli-<hash>`，Windows 上加 `.exe`）、`REJECT` 对 HTTP `CONNECT` 的应答、记录里 `rule` 的写法；`processPath` 与 `std::env::current_exe()` 比较之前两边各 `canonicalize`（Windows 上大小写与 `\\?\` 前缀可能不同）；写进规则的路径与前缀取 `canonicalize` 之后的。
- V11：`RURGE_NETWORK_BACKEND` 的读法与拒绝（`rurge_platform::netenv::backend_from_env` / `parse_backend`、`crates/rurge/src/cli/network.rs` 的 `backend`、`run.rs` 里它以 2 退出的地方），`RURGE_PROCESS_BACKEND` 照它的样子读、在同一个时机拒绝；`rurge_command()` / `rurge_process()` 设缺省变量的地方。
- V12：`SessionHandle` 的 `notes` 怎样加（`add_note`）、子规则那一句在 `choose_policy` 里的位置——到期的那一句写在同一处；`LogCapture` 抓一次性 WARN 的写法。

## 10. 对总设计的订正

1. 第 7 节"在规则评估之前查一次" → 会话开始时在后台查，规则走到第一个要看进程的匹配器时才等（第 4.2 节）；"每次都查还是只在规则用得到 `PROCESS-NAME` 时查，在 M2 按基准定" → 定为对监听器进来的会话总在后台查（M2-D3），基准照做、不作门禁。
2. 第 7 节"表按快照缓存并限频（没找到时重取一次，同一种表 100 毫秒内至多重取一次）" → 按时刻判断一份快照能不能用、一次读取服务一批查询、两次读取至少隔 20 毫秒（第 3.2 节）。
3. 第 7 节 Linux"先扫同一 uid 的" → 先扫最近找到过的进程、再扫全部（第 3.4 节）。
4. 第 7 节 macOS"按 `kern.osrelease` 选记录大小" → 按每一项自己的长度与种类走，不读 `kern.osrelease`；第 3.6 节表里 macOS 的 M2 一行因此去掉 `kern.osrelease`（第 3.5 节）。
5. 第 7 节"来源是本机的会话——经监听器从回环或本机地址来的" → 是不是本机在平台层判断；SOCKS5 的 UDP 关联按控制连接查（第 4.1 节）。
6. FR-RULE-01 的"Windows 路径归一化"落成：盘符与 UNC 写法是路径，Windows 上名字也不分大小写，不去掉 `.exe`（第 5.2、5.3 节）。

## 11. 已决事项

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M2-D1 | 查的是哪一个套接字 | 客户端自己的：TCP 按（来源地址，监听端口）找客户端那一行；UDP 按本地端点，退而取未指定地址的同端口行；IPv4 先找 IPv4 表再找映射形式 |
| M2-D2 | 哪些会话查 | HTTP 与 SOCKS5 监听器的会话；SOCKS5 的 UDP 关联按控制连接查一次、流共用；内部会话不查；是不是本机由平台层判断（回环或本机网卡的地址），引擎先问不阻塞的 `is_local`，答"不是本机"的不起查找 |
| M2-D3 | 什么时候查 | 会话开始时在后台总查；规则走到第一个要看进程的匹配器才等，至多等到查找发起之后 200 毫秒；到期时这次评估按"不知道"判、`notes` 里说一句，查找照常做完；不加给使用者的开关 |
| M2-D4 | 规则怎样等 | `Verdict::NeedsProcess`，组合时优先于 `NeedsResolve`；预匹配那一遍不等 |
| M2-D5 | 快照与限频 | 快照记读取开始的时刻；TCP 命中可用会话开始前 2 秒以内的快照，UDP 与"找不到"要会话开始之后的；每种协议一次一个查询；同一种表两次读取至少隔 20 毫秒；同时占用的阻塞线程至多 2 个 |
| M2-D6 | 结果放哪里 | 会话的进程槽（`SessionHandle`），查找结束时落定一次（不论用了多久），不查的会话一开始就落定为"不知道"；等多久由等的一方定；UDP 关联的槽由流共用；记录带着槽，之后查到的也出现 |
| M2-D7 | `ProcessInfo` | 加 `pid: Option<u32>`；`SessionInfo.process` 是事先知道的进程（离线命令） |
| M2-D8 | Windows | 两张带 PID 的表 + `QueryFullProcessImageNameW`；PID 4 是 `System`，0 跳过；打不开是"不知道" |
| M2-D9 | Linux | `/proc/net` + 扫 `fd`（最近的进程优先，跨快照的 inode 表）；`exe`，读不了取 `comm`；不用 FFI、不用 netlink |
| M2-D10 | macOS | `pcblist_n` 按项走；取 `so_last_pid`；`proc_pidpath`；`libc` 经 `nix::libc`；不读 `kern.osrelease` |
| M2-D11 | 路径的写法 | 以 `/`、盘符加 `:\` 或 `:/`、`\\` 开头是路径，再以 `/` 或 `\` 结尾是前缀；与平台无关 |
| M2-D12 | 大小写与扩展名 | Windows 上文件名与路径都不分大小写；macOS、Linux 区分；不去掉 `.exe` |
| M2-D13 | 记录与 API | `pid`、`processPath`，不知道是 `null`；进程只在 DEBUG 日志里 |
| M2-D14 | 失败 | 不知道进程从不让会话失败；平台读取的每种失败 WARN 一次，之后 DEBUG |
| M2-D15 | unsafe | 新增五个函数（Windows 三个、macOS 两个），`rurge-platform` 共七个，一条用例钉住名单 |
| M2-D16 | `rurge check` | `PROCESS-NAME` 不再是永不匹配的类型，三个平台一样 |
| M2-D17 | 内部集 `SYSTEM` 的进程行 | 只在 macOS 上编进集合（编译期判断），集合的文字照手册不变；别的平台上真叫 `trustd` / `netbiosd` 的进程不被它接住，`rule match --platform` 不改变这一点 |
| M2-D18 | 查找一直很慢时 | 不设断路器、不让规则在等的查找插队：代价有界（每条走到 `PROCESS-NAME` 的会话至多多等 200 毫秒），规则的结果不随此前的超时而变，一次读取服务整批排队的查找；第一次到期时 WARN 一次 |
| M2-D19 | 测试的后端 | 只供测试的 `RURGE_PROCESS_BACKEND=off`（别的取值以 2 退出，从不退回真实的查找）；起二进制的辅助函数缺省设它，查真实进程的用例去掉它；不加隐藏的命令行选项 |
