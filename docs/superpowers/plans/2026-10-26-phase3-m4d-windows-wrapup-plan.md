# 阶段 3 / M4d「Windows 与收尾」实施计划

> **执行方式**：交给 Pi。M4c 计划做完、项目所有者评审之后，在一个会话里按任务的次序逐个做（技能 `superpowers:executing-plans`；没有子代理，不并行，不跳）。
>
> 每个任务的步骤：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**，M4 设计第 2 节）。
>
> 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 这一份有两个停点：
> - 任务 8：请项目所有者推送，然后读回 CI，照 M4c 计划任务 10 的做法。
> - 任务 9 之后：交项目所有者评审。M4 整个做完。

**目标**：Windows 上 `rurge run --enhanced-mode` 真的能打开；M4 的文档收尾。具体是：

- Wintun 的虚拟网卡：
  - 找到 `wintun.dll`，先校验签名者再加载；
  - 建适配器、开会话；
  - 一个读线程把包交给异步的一侧；发包直接写环。
- 步骤的执行：`netsh -f` 的脚本。成败由读回定：路由表、地址、MTU、DNS 服务器。
- 防环的 Windows 一半：`IP_UNICAST_IF` / `IPV6_UNICAST_IF`。
- 外部程序的 Windows 一半：`in_job`，让 `ProcessTree::contains` 真的回答。
- `--wintun-dll` / `RURGE_WINTUN_DLL`。
- 文档收尾（设计第 16 节）：兼容性清单、API 参考、手工清单的 M4 一节、`docs/testing.md`、两份 README、`CLAUDE.md`、`docs/status.md`、需求文档第 7 节的状态行。

**架构**：照 M4c 的拆法。

- **`enhanced/windows.rs` 的 `WindowsSystem` 不带 `cfg`**。程序经 `CommandRunner` 给进来，在开发机上用假的执行器测。
- 读回的解析（`netsh … show` 的输出）是不带 `cfg` 的纯函数，放在 `route/windows.rs`。
- 读线程的循环 `tun/reader.rs` 不带 `cfg`，对一个假的环测。
- 带 `cfg(windows)` 的只有三样：
  - `tun/windows/`：Wintun 的加载与设备；
  - `socket.rs` 的 `unicast_if`；
  - `process.rs` 的 `in_job`。
- 新增 14 个放开的 unsafe 函数，全在这三处（设计第 3.8 节）。

**技术栈**：`windows-sys` 0.61.2 多开七个特性（P2）；tokio（M4c 已加的边）；std 的 `OpenOptionsExt::share_mode`。不新增外部包，不加新的工作区依赖边。

**依据**：

- M4 设计（下面写"设计"指它）：第 2、3.1、3.8、5.4、5.5、6.3、7.4、8.1、8.4、8.5、9.6、13、14、16 节；已决事项 M4-D4、D11、D12、D13、D14、D21、D25、D29、D30。**先读它末尾的第 21 节"写计划时的订正"**：四份计划改了正文的地方都列在那里。
- M4a 计划：Windows 的翻译、P2 ～ P4、P14。
- M4b 计划：`ProcessTree::contains` 在 Windows 上先答 `false`（P15）。
- M4c 计划：`System`、`TunDevice`、`frame`、`exec`、`TimedRunner`，以及任务 10 等 CI 的做法。
- **以这三份计划的「执行期修正记录」为准。**
- 写计划时读过的出处见「计划期决定」P20。

## 全局约束

**依赖边**

- 不加新的工作区依赖边，也不加外部包。`Cargo.lock` 的包数与 M4c 做完时相同。
- `windows-sys` 的特性照 P2 加。
- 新开的特性记进 `docs/status.md` 的附录。

**`unsafe_code`**：新增 14 个放开的函数（P3）。

- 同步 `crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED`（数组的长度一起改），按（文件，函数）的次序。`ac0a98a` 上是 7 个，M4c 加上 `set_iff`、`utun_connect` 之后 9 个，M4d 加完 23 个；以 M4c 做完时的个数为准。
- 同步 `CLAUDE.md` 的 `unsafe_code` 一段：每次加函数的那个提交里一起改，提交说明写明。
- 每个 `unsafe` 块的前一行用 `// SAFETY:` 写明前提。
- **不写 `unsafe impl`**（`Send` / `Sync`）。那是一处不在函数上的放开，`every_unsafe_exception_is_on_the_list` 会变红。
- 句柄与模块一律存成 `usize`：指针转整数不需要 unsafe，`Send` / `Sync` 自然就有。只在放开的函数里转回指针。

**缺省的 `cargo test` 不碰系统**

- 没有一条缺省用例建 Wintun 适配器或开会话：**`TunDevice::open` 与真实平台的 `System::open` 不被任何缺省用例调用**（M4c 计划 P19）。
- `WindowsSystem` 的用例只用假的执行器与临时目录。
- 允许的只读调用：
  - 校验测试二进制自己的签名（`WinVerifyTrust`，不联网，P6）；
  - 加载 System32 里的 `kernel32.dll` 看函数缺失时的错误（它本来就载在进程里）；
  - 回环套接字上的 `IP_UNICAST_IF`；
  - 测试自己起的子进程上的 `IsProcessInJob`。
- 有真的 `wintun.dll` 时的用例，由环境变量 `RURGE_TEST_WINTUN_DLL` 放开，缺省不设。Pi 不设它，也不下载 `wintun.dll`。

**开发机**：Windows 的代码在开发机上编译、运行。

- **不在开发机上执行 `netsh`、`route`、`ipconfig`**。写法已按微软文档核对（P20）。
- **不建网卡**。
- 不改系统的任何设置。

**执行程序**

- 程序是 `%SystemRoot%\System32\` 下的 `netsh.exe` 与 `ipconfig.exe`（M4a 的 `WindowsTools::under`）。系统根目录取环境变量 `SystemRoot`，没有就是 `C:\Windows`。
- 一律经 `TimedRunner`，每个程序 30 秒为限（M4c 计划 P11）。

**名字与地址只在 DEBUG**（设计第 13 节）

- INFO 只说开关与恢复。
- `wintun.dll` 的路径可能带使用者的账户名，只在 DEBUG。
- 错误文本里不带路径；签名者的名字可以带，它不是个人信息。

**测试的约定**

- 测试只用回环与端口 0。
- CLAUDE.md 的「测试约定」整节适用。
- 门禁、本机的 rustfmt、只经整个工作区跑用例、跑全量前 `df -h /d`、`STATUS_HEAP_CORRUPTION` 与增量编译内部错误的处理：同 M4a 计划的「全局约束」。
- 新写的计时用例（读线程的循环）提交前单独循环跑二十次以上。

**不做的事**

- 只运行本仓库构建出来的二进制与系统自带的只读工具。
- 不下载，不安装。
- 不读真实的网络环境。

**参考实现**

- GPL 的（sing-tun）只取事实，代码不抄。
- MIT 的（wireguard-go、wireguard-windows）可以简短引用，注明出处。
- `wintun.h` 是 "GPL-2.0 OR MIT"：函数签名照它写（事实），不整段抄。

**写法**

- 注释、日志与错误文本用英文。
- 提交说明照 `git log` 的样子，标题用中文。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向是项目所有者的，停下来列出来，**尤其不要自己定**：
  - 总设计 Q3：附带 `wintun.dll`；
  - 总设计 Q5：WFP；
  - 总设计 Q4：Windows 的冒烟。
- 会改变 M4-D1 ～ D30 的（P8 订正的 M4-D12 那一处除外），同样停下来列出。

## 评审要点

这一份做完，Windows 的使用者第一次能打开增强模式；M4 整个做完。最可能咬到使用者的几种情形，与负责它们的地方：

1. **加载一个被换掉的 `wintun.dll`**（以管理员运行时就是以管理员执行它的代码）。
   - 做法：
     - 加载之前校验签名者，必须恰好是 `WireGuard LLC`；
     - 校验与加载期间以只许读的共享方式开着这个文件，别人不能写它、删它或给它改名；校验的就是这个开着的文件（P7）；
     - 它的依赖只从 System32 找。
   - 用例：任务 3 的 `an_unsigned_file_is_refused`。
2. **`netsh` 悄悄失败。** 它的退出码在脚本里一行失败时怎样，没有找到文档；输出的文字随系统语言变。
   - 做法：每一步之后读回：路由表、地址、MTU、DNS 服务器（P10、P11）。读回只认数字与地址，不认文字。
   - 用例：任务 2 的解析用例（含中文与按 GBK 读坏了的输出），任务 4 的读回用例。
3. **MTU 没有设上。** Wintun 适配器的缺省 MTU 没有找到文档（它能收发的最大包是 0xFFFF，`wintun.h`），不能当作 1500。
   - 后果：系统可能往虚拟网卡送大于 rurge 协议栈 MTU 的包。
   - 做法：读回 MTU；不是 1500 就是失败（P11）。
4. **另一个 rurge 已经开着增强模式。**
   - 做法：先 `WintunOpenAdapter` 探一下：找得到就说"正在被使用"，不去复用它（P8）。
   - 理由：复用的适配器在关闭时不会被移除，下一次打开会撞上它留下的路由。
5. **`IP_UNICAST_IF` 的字节序。** IPv4 是网络字节序，IPv6 是主机字节序（微软文档，P20）。
   - 用例：任务 1 的回环用例。
6. **只有 CI 编译的那两个平台被 M4d 的不带 `cfg` 的改动弄坏。**
   - 做法：任务 8 读回 CI。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-platform/Cargo.toml`、`src/socket.rs`、`src/process.rs`、`tests/unsafe_functions.rs`、`CLAUDE.md` | 1 | 特性；`unicast_if`；`in_job` 与 `ProcessTree::contains` |
| `crates/rurge-platform/src/route/windows.rs` | 2 | 读回的解析；`configure` 的翻译订正（P9） |
| `crates/rurge-platform/src/tun/{mod.rs,frame.rs,reader.rs}`、`src/tun/windows/{mod.rs,wintun.rs}`、`tests/unsafe_functions.rs`、`CLAUDE.md` | 3、5 | 读线程的循环；`wintun.dll` 的查找、签名校验与加载；适配器、会话、设备 |
| `crates/rurge-platform/src/enhanced/{mod.rs,windows.rs}` | 4 | `WindowsSystem`（不带 `cfg`）；`system()` 在 Windows 上接上（任务 5） |
| `crates/rurge/src/cli/{run.rs,enhanced/backend.rs}` | 6 | `--wintun-dll` / `RURGE_WINTUN_DLL` |
| 本计划末尾两张表 | 8 | 等 CI |
| 文档（任务 9 列全） | 9 | 收尾 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的 `main` 核对下面每一项。

- 有出入的，记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。
- **不改计划的正文。**
- 没有出入也记一行。

1. **工作树与前三份计划**
   - `git status` 干净；M4a、M4b、M4c 的提交都在；M4c 的等 CI 已经结束（它的修正记录里最后一次运行三个平台都绿，或者剩下的失败已列给所有者）。
   - 读三份计划末尾的两张表。本计划要用的名字，以那几张表为准：
     - `route::windows::{WindowsTools, configure, configure_ipv6, unconfigure_ipv6, add_routes, remove_routes, take_dns}`；
     - `route::exec::run`、`command::TimedRunner`；
     - `enhanced::{System, Adapter, Saved, NOT_AVAILABLE, STEP_TIMEOUT, system}`；
     - `tun::{TunDevice, DeviceSpec}`、`tun::frame::{explained, Os}`；
     - `ProcessTree` 与 `contains`；
     - `socket::bind_interface`、`interfaces()`、`pick_source`；
     - 可执行文件的 `backend(data_dir)`、`PlatformBackend`、`RunArgs`。
2. **名单**：`unsafe_functions.rs` 的 `LISTED` 现在有几项；`CLAUDE.md` 的 `unsafe_code` 一段写了几个函数。两者应当相同。
3. **V3：符号与特性**（离线读本机 `~/.cargo/registry/src/index.crates.io-*/windows-sys-0.61.2/`，写计划时都核过，见 P2）。
   - 函数是 `windows_link::link!` 声明的，要用 `grep "fn <名字>("` 找，不是 `pub fn`。
   - 不一致：照实改特性，记进记录。
4. **V2：`netsh` 的写法**
   - 网络可达时，对着 P20 的两页微软文档再核一遍：
     - 本计划要用的 `show route`、`show addresses`、`show subinterfaces`、`show dnsservers`；
     - M4a 的 `add address`、`set interface`、`set subinterface`、`add route`、`delete route`、`set dnsservers`，与 IPv6 一侧。
   - 不可达时，照 P20，并记一行"没有再核"。
5. **Wintun**：读 P20 记的 `wintun.h` 的签名，与任务 3、5 要用的十一个入口对一遍。网络不可达就照 P20。
6. **门禁**：跑一遍，记下结果行数、通过数与忽略数。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：`IP_UNICAST_IF` 与 `in_job`（`rurge-platform`）

**改哪里**（设计第 7.4、11 节；M4-D11、D21；P2、P3、P20）

1. **`Cargo.toml`**：`windows-sys` 的特性加 `Win32_Networking_WinSock`。其余六个在任务 3 加（P2）。

2. **`src/socket.rs` 的 Windows 一半**
   - `bind_interface` 照旧绑源地址，之后再设 `unicast_if`。
   - 网卡的序号取自同一个 `if_addrs` 表里、那个源地址所在的那一项。在 Windows 上，IPv4 的项给 `IfIndex`，IPv6 的项给 `Ipv6IfIndex`（`if-addrs` 0.15 的 `windows.rs`）。
   - 序号是 `None` 时不设，只记一行 DEBUG。绑源地址仍然挡着（设计第 7.4 节）。

   ```rust
   /// Sends this socket's traffic out of interface `index` (M4 design 7.4): one
   /// `setsockopt`, `IP_UNICAST_IF` with the index in network byte order for
   /// IPv4, `IPV6_UNICAST_IF` in host byte order for IPv6 (the Winsock
   /// documentation of the two options).
   #[allow(unsafe_code)]
   fn unicast_if(socket: RawSocket, family: Family, index: u32) -> io::Result<()>
   ```

3. **`src/process.rs` 的 Windows 一半**

   ```rust
   /// Whether process `pid` is in `job` (M4 design 11): `OpenProcess` with
   /// `PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE`, a zero-wait
   /// `WaitForSingleObject` (an exited process is not in it, whatever
   /// `IsProcessInJob` would say of it), then `IsProcessInJob`. A process that
   /// is gone is not in it.
   #[allow(unsafe_code)]
   fn in_job(job: RawHandle, pid: u32) -> io::Result<bool>
   ```

   - 进程句柄接进 `OwnedHandle`，离开函数就关。
   - `OpenProcess` 失败，而错误是 `ERROR_INVALID_PARAMETER`（进程已经没了）：回 `Ok(false)`。其余错误照回。
   - 进程已经结束、但它的对象还被别的句柄留着（例如 `Child` 还没有被丢掉）：`WaitForSingleObject(h, 0)` 回 `WAIT_OBJECT_0`，回 `Ok(false)`。不靠 `IsProcessInJob` 对一个已经结束的进程怎么答。
   - `ProcessTree::contains` 在 Windows 上：有 `job` 时调 `in_job`；`Err` 当作 `false`，原因在 DEBUG。
   - 常数都在已开的特性里：`PROCESS_SYNCHRONIZE`、`PROCESS_QUERY_LIMITED_INFORMATION` 在 `Win32_System_Threading`，`WAIT_OBJECT_0`、`ERROR_INVALID_PARAMETER` 在 `Win32_Foundation`。

4. **名单**
   - `LISTED` 加 `("src/process.rs", "in_job")`、`("src/socket.rs", "unicast_if")`。
   - `CLAUDE.md` 的那一段同步。

**用例**（`#[cfg(windows)]`，跑在开发机与 CI 的 Windows 上）

- `socket.rs`：`unicast_if_on_a_loopback_socket_still_reaches_the_loopback`。
  1. UDP 套接字绑 `127.0.0.1:0`；
  2. 从 `if_addrs` 找到 `127.0.0.1` 那一项的序号，设 `unicast_if`；
  3. 发一个数据报给测试自己持有的、回环上的接收端，收到。
  - IPv6 的同样一条，用 `::1`。本机没有 `::1` 时跳过，写明原因。
- `process.rs`：
  - `a_child_in_the_job_is_contained_and_this_process_is_not`：用现有的 `contain` 起一个子进程（照 `process.rs` 已有用例的写法），`contains(子进程)` 为真，`contains(本进程)` 为假。
  - `an_exited_child_whose_handle_is_still_open_is_not_contained`：子进程结束、被等到，`Child` 还拿着它的句柄（进程对象还在）：为假。这一条走的是 `WaitForSingleObject` 那一步。
  - `a_process_that_is_gone_is_not_contained`：再丢掉 `Child` 之后，仍为假。

**判据**：

- `every_unsafe_exception_is_on_the_list` 绿。
- `git grep -n "unsafe impl" crates` 什么也没有。
- 门禁绿。

---

## Task 2：读回的解析与 `configure` 的订正（`route/windows.rs`，不带 `cfg`）

**改哪里**（设计第 5.4、5.5 节；P9 ～ P11、P20）

1. **解析**（纯函数）：

   ```rust
   /// `netsh interface ipv4|ipv6 show route`: (prefix, interface index) of each
   /// row. A row is any line with a token that is a prefix (`a.b.c.d/n`, `x::/n`)
   /// followed by one that is a number; headers and words are never read, so the
   /// system's language does not matter.
   pub fn routes_in(text: &str) -> Vec<(IpNet, u32)>;
   /// The addresses in the output of `show addresses` or `show dnsservers`:
   /// every token that is an address (after trimming `,;()[]` and a `%zone`).
   pub fn addresses_in(text: &str) -> Vec<IpAddr>;
   /// `show subinterfaces interface=<i>`: the MTU, the first number of the first
   /// line that starts with one.
   pub fn mtu_in(text: &str) -> Option<u32>;
   ```

   - 分词用 `split_whitespace`：它也按全角空格（U+3000）分开。
   - 输出经 `String::from_utf8_lossy` 读进来（`command.rs`）。中文系统上表头与"是 / 否、手动 / 系统"这些字会读成替换字符，数字与地址不受影响。

2. **读回的命令**：每个函数回一个 `Step`，程序是 `netsh`，参数直接写，不用脚本，`OnFailure::Ignore`。

   ```rust
   pub fn show_routes(t: &WindowsTools, v6: bool) -> Step;          // interface ipv4|ipv6 show route
   pub fn show_addresses(t: &WindowsTools, index: u32, v6: bool) -> Step;
   // ipv4: interface ipv4 show addresses name=<i>    ipv6: interface ipv6 show addresses interface=<i>
   pub fn show_mtu(t: &WindowsTools, index: u32, v6: bool) -> Step; // interface ipv4|ipv6 show subinterfaces interface=<i>
   pub fn show_dns(t: &WindowsTools, index: u32, v6: bool) -> Step; // interface ipv4|ipv6 show dnsservers name=<i>
   ```

3. **`configure` / `configure_ipv6` 的订正**（P9）：`set interface` 拆成两行，度量单独一行。

   ```
   interface ipv4 set interface interface=<i> forwarding=disabled weakhostsend=disabled weakhostreceive=disabled store=active
   interface ipv4 set interface interface=<i> metric=1 store=active
   ```

   IPv6 一样。连同 M4a 的逐字用例改，提交说明里点名。

**用例**

- `routes_in`：
  - `routes_are_read_from_the_english_table`：夹具照 P20 记的那份英文样子（列是 `Publish Type Met Prefix Idx Gateway/Interface Name`），含一行前缀很长、与序号之间只隔一个空格的。
  - `routes_are_read_whatever_the_language`：同一张表，表头与文字换成中文（手写，注明 "not taken from a machine"）；再换成替换字符（模拟按 GBK 输出、按 UTF-8 读）。三份读出来的结果相同。
  - `a_prefix_with_an_embedded_ipv4_address_is_read`：`fe80::5efe:10.0.0.3/128`。
  - `lines_without_a_prefix_and_a_number_are_skipped`：表头、分隔线、空行、`Ok.`。
- `addresses_in`：
  - 英文的 `IP Address: 198.18.0.1` 与 `Statically Configured DNS Servers: 198.18.0.2`；
  - 中文的同样两行（手写）；
  - 带 `%12` 的链路本地地址；
  - 掩码 `255.254.0.0` 也会被读出来：读回只问"有没有某一个地址"，不受它影响，用例写明这一点。
- `mtu_in`：英文与中文的 `show subinterfaces` 各一份（手写），得到 `1500`；没有数据行时是 `None`。
- 读回的四个命令：逐字比对。
- `configure` 的订正：逐字比对。

**判据**：

- `route/windows.rs` 里没有 `cfg`。
- 夹具旁边注明出处或"手写"。
- 门禁绿。

---

## Task 3：读线程的循环；`wintun.dll` 的查找、签名校验与加载

**改哪里**（设计第 8.4 节；M4-D13；P2 ～ P7、P20）

1. **`Cargo.toml`**：`windows-sys` 的特性加：
   - `Win32_System_LibraryLoader`
   - `Win32_Security_WinTrust`
   - `Win32_Security_Cryptography`
   - `Win32_Security_Cryptography_Catalog`
   - `Win32_Security_Cryptography_Sip`
   - `Win32_NetworkManagement_Ndis`（P2）

2. **`src/tun/reader.rs`**：不带 `cfg`，`pub`。

   ```rust
   /// What the reader thread needs of a Wintun session (a fake one in the tests).
   pub trait Ring {
       /// One packet; `Ok(None)` when there is none now.
       fn receive(&self) -> io::Result<Option<Vec<u8>>>;
       /// Waits at most `timeout` for packets to arrive.
       fn wait(&self, timeout: Duration);
   }

   /// The reader thread (M4 design 8.4): passes packets on in order through a
   /// bounded channel, waiting when it is full; waits on the ring when it is
   /// empty. Ends when `stop` is set (within one wait), when the receiving half is
   /// closed, or after passing on an error of the ring.
   pub fn read_loop(ring: &dyn Ring, tx: &mpsc::Sender<io::Result<Vec<u8>>>, stop: &AtomicBool, wait: Duration);
   ```

   - 通道是 tokio 的 `mpsc`，容量 1024。读线程不在运行时里，用 `blocking_send`。
   - 等待是 100 毫秒（`READ_WAIT`）。
   - `tun/mod.rs` 里 M4c 定下的"各实现之间的约定"（读一个包、写一个包、哪些错误算"没有地方"）如果只适合文件描述符（例如要求 `AsyncFd`），在 `mod.rs` 里把它改成两种实现都能接的样子，Linux 与 macOS 的行为不变，记进修正记录。

3. **`src/tun/windows/wintun.rs`**：带 `#[cfg(windows)]`。

   - **找 DLL**（纯函数，不带 `cfg`，放在 `tun/frame.rs` 或一个新的不带 `cfg` 的小模块）：

     ```rust
     /// Where wintun.dll is: `option` (`--wintun-dll`) made absolute, else next to the executable.
     pub fn wintun_dll(option: Option<&Path>, exe: io::Result<PathBuf>) -> Result<PathBuf, String>;
     ```

     - `exe` 读不到：错误 `cannot tell where rurge.exe is, to find wintun.dll next to it: give --wintun-dll`。
     - 路径在 DEBUG。

   - **打开、校验、加载**（`open` 的第一段）
     1. 文件不在：错误 `cannot find wintun.dll (next to rurge.exe, or where --wintun-dll points): get it from https://www.wintun.net/`，类别 `NotFound`。
     2. 以 `OpenOptionsExt::share_mode(FILE_SHARE_READ)` 只读地打开文件，一直开到加载完（P7）。`FILE_SHARE_READ`（`0x1`）在 `Win32_Storage_FileSystem` 里，为一个常数不开那个特性：在本地定义，注释写出处。
     3. `verify_signer(path, &file)`：校验的是这个开着的文件（`WINTRUST_FILE_INFO` 的 `hFile`），不是按路径再开一次。
     4. 签名者恰好是 `WireGuard LLC` 才继续。否则错误是：
        - `wintun.dll is not signed by WireGuard LLC (it is signed by <名字>)`；
        - 或 `wintun.dll is not signed`。
     5. `load(path)`。
     6. 加载成功的 `Wintun` 存进进程里的一个 `Mutex<Option<Arc<Wintun>>>`：只载一次，从不卸载。失败不缓存，下一次打开再试。

   - **两个放开的函数**：

     ```rust
     /// The simple display name of the signer of `file`, opened from `path`
     /// (M4 design 8.4): `WinVerifyTrust` on the open file
     /// (`WINTRUST_ACTION_GENERIC_VERIFY_V2`, no UI, `WTD_REVOKE_NONE`,
     /// `WTD_CACHE_ONLY_URL_RETRIEVAL`: never the network), the signer's
     /// certificate from the state data, `CertGetNameStringW`
     /// (`CERT_NAME_SIMPLE_DISPLAY_TYPE`); the state is closed whatever happened.
     /// `Ok(None)`: not signed (`TRUST_E_NOSIGNATURE`).
     #[allow(unsafe_code)]
     fn verify_signer(path: &Path, file: &File) -> Result<Option<String>, String>

     /// Loads wintun.dll from `path` (its dependencies from System32 only:
     /// `LOAD_LIBRARY_SEARCH_SYSTEM32`) and takes its eleven entry points; frees
     /// it again when one is missing.
     #[allow(unsafe_code)]
     fn load(path: &Path) -> io::Result<Wintun>
     ```

     - `verify_signer` 的其他失败（文件读不了、签名不受信任）：`Err(wintun.dll's signature cannot be trusted (0x<结果>))`。
     - `load` 缺一个入口：先 `FreeLibrary`（`Win32_Foundation`），再回 `Err(wintun.dll has no <入口名>: is it wintun.dll?)`。
   - **`Wintun`**：十一个函数指针，类型照 `wintun.h` 写成 `unsafe extern "system" fn(…)`；模块句柄存成 `usize`。函数指针本身就是 `Send` / `Sync`。

4. **名单**
   - `LISTED` 加 `("src/tun/windows/wintun.rs", "load")`、`("src/tun/windows/wintun.rs", "verify_signer")`。
   - `CLAUDE.md` 的那一段同步。

**用例**

- `reader.rs`（三个平台；假的环用一个锁住的队列加一个条件变量）：
  - `packets_are_passed_on_in_order`。
  - `an_empty_ring_is_waited_on`：环开始是空的；用例等到 `wait` 被调过（有界的等待，不是固定等待），再放进一个包；收到。
  - `an_error_of_the_ring_is_passed_on_and_ends_the_loop`。
  - `stop_ends_the_loop_within_one_wait`：循环所在的线程结束时发一个信号；用例在自己的线程上以 `recv_timeout` 等它，上限是等待时长加余量，注释写明余量。
  - `a_closed_receiver_ends_a_loop_blocked_on_a_full_channel`：容量 1 的通道已满，`close()` 之后循环结束（同样以 `recv_timeout` 等）。
  - 这一组都是普通的 `#[test]`，不在运行时里（`blocking_send` 在运行时里会 panic）。提交前单独循环跑二十次。
- `wintun_dll`：给了相对路径就变成绝对路径；没给就是可执行文件旁边；`exe` 读不到时是那句错误。
- `#[cfg(windows)]`：
  - `an_unsigned_file_is_refused`：校验测试二进制自己（`std::env::current_exe()`，没有签名），得到 `Ok(None)`；把它复制到临时目录、名字叫 `wintun.dll`，经 `open` 的第一段得到 `wintun.dll is not signed`（校验在加载之前，它不会被当作 DLL 加载）。不改测试二进制本身。
  - `a_missing_file_says_where_to_get_it`。
  - `a_dll_without_the_wintun_entry_points_is_refused`：`load(<SystemRoot>\System32\kernel32.dll)` 得到缺入口的那句错误。kernel32 本来就载在进程里，这只加一次引用。
  - `a_real_wintun_dll_passes`：只在 `RURGE_TEST_WINTUN_DLL` 指着一个文件时跑，否则打印原因、返回。签名者是 `WireGuard LLC`；`load` 成功。**不建适配器。**

**判据**：

- `reader.rs` 与 `wintun_dll` 里没有 `cfg`。
- 门禁绿。
- 计时用例跑满二十次。

---

## Task 4：`WindowsSystem`（`enhanced/windows.rs`，不带 `cfg`）

**改哪里**（设计第 5.4、6.3 节；P8 ～ P13）

```rust
pub struct WindowsSystem { /* runner: Arc<dyn CommandRunner>, tools: WindowsTools,
                              scratch: PathBuf, dll: Result<PathBuf, String> */ }
impl WindowsSystem {
    pub fn new(runner: Arc<dyn CommandRunner>, tools: WindowsTools, scratch: PathBuf,
               dll: Result<PathBuf, String>) -> WindowsSystem;
}
```

**步骤的成败由读回定**（P10）：

- 执行时把每一步的 `OnFailure` 换成 `Ignore`，经 `exec::run` 跑。netsh 的退出码与输出只进 DEBUG。
- 然后读回，按原来的 `OnFailure` 解释读回的结果：
  - `Fail`：失败，回错误；
  - `Ipv6Off`：WARN 一行（M4c 的那一句），回 `Ok(false)`。

`impl System` 的各个方法：

- **`open`**
  1. `dll` 是错误就回它。
  2. `exec::remove_leftovers(scratch)`。
  3. `TunDevice::open(&DeviceSpec { name: "rurge".into(), mtu: 1500, wintun_dll: Some(dll) })`。
  - `DeviceSpec` 加 `wintun_dll: Option<PathBuf>`："Windows: where wintun.dll is; unused elsewhere"。
  - `Adapter` 的 `index` 是 `Some`。
- **`configure(dev, ipv6)`**
  1. M4a 的 `configure` 脚本，任务 2 订正过的。
  2. 读回：
     - `show_addresses(v4)` 里有 `198.18.0.1`，否则失败：`the virtual adapter did not get its address`；
     - `show_mtu(v4)` 是 1500，否则失败：`the virtual adapter's MTU is not 1500`。
  3. `ipv6` 时接着做 `configure_ipv6`。
- **`configure_ipv6`**：脚本；读回 `fd00:6152::1` 与 IPv6 的 MTU。缺一样就 WARN，回 `Ok(false)`，并照 `unconfigure_ipv6` 撤掉做了一半的。
- **`unconfigure_ipv6`**：脚本（`Ignore`），不读回。
- **`routes(dev, add, remove)`**
  1. `add` 的两份脚本。
  2. `show_routes(v4)`，有 IPv6 时再 `show_routes(v6)`。
  3. 用 `routes_in` 读出序号是这块网卡的那几行；`add` 里每一条都要在。
     - IPv4 缺：先 `remove_routes(add)` 撤掉（`Ignore`），再回错误 `<n> of the routes are missing after netsh added them`。缺的前缀在 DEBUG。
     - IPv6 缺：只撤 IPv6 的一半，回 `Ok(false)`。
  4. 照常减掉 `remove`（`Ignore`）。
- **`dns_saved`** 回 `Ok(vec![])`。网卡上的 DNS 设置不会在崩溃之后留下来起作用（P12）。
- **`take_dns(dev, ipv6)`**
  1. M4a 的 `take_dns`。
  2. 读回 `show_dns(v4)` 里有 `198.18.0.2`，否则失败：`the system's DNS could not be pointed at the enhanced mode`。
  3. `ipv6` 时读回 `show_dns(v6)` 里有 `fd00:6152::2`；缺了只 WARN。
  4. 回 `Ok(true)`。
- **`give_dns_back`** 回 `Ok(())`。
- **`adjust_saved`** 回 `Ok(None)`；**`adjust`** 回 `Ok(())`。
- **`undo`**：任何种类都是 WARN `an undo of another platform is dropped`，回 `Ok(())`（M4c 计划 P9）。
- **`physical_dns`** 回 `None`：`dns::servers()` 遍历全部网卡，虚拟网卡上的服务器落在 rurge 的空间里被去掉（设计第 6.3 节）。

**用例**：执行器用 `FakeRunner`，读回的输出用任务 2 的夹具。`FakeRunner` 看不到脚本的内容（M4c 计划任务 2）：脚本逐字的用例在 `route/windows.rs`，这里断言命令的次序、读回与结论；要看脚本内容的，用 M4c 在 `exec.rs` 的测试里另写的那个执行器。**没有一条用例在 `dll` 是 `Ok` 的时候调 `open`。**

- `without_wintun_dll_the_open_fails_before_the_device`。
- 配网卡：
  - `configure_runs_the_script_then_reads_back_the_address_and_the_mtu`：命令的次序与参数。
  - `a_missing_address_fails_configure`。
  - `an_mtu_that_is_not_1500_fails_configure`。
  - `an_ipv6_address_that_is_missing_is_ok_false_and_taken_out_again`。
- 路由：
  - `routes_are_read_back_by_the_adapter_s_index`：表里别的序号上同样的前缀不算。
  - `missing_ipv4_routes_take_the_added_ones_out_and_fail`。
  - `missing_ipv6_routes_are_ok_false`。
- DNS：
  - `the_dns_is_read_back`。
  - `a_dns_that_did_not_change_fails_take_dns`。
- 退出码：
  - `netsh_s_exit_status_does_not_decide`：假的执行器让脚本那一步回 `Err`，读回却都对：结果是成功。反过来，执行器回成功、读回不对：结果是失败。
- 其余：
  - `the_windows_system_never_saves_and_drops_foreign_undos`。
  - `physical_dns_is_the_system_s_own`。

**判据**：

- `enhanced/windows.rs` 里没有 `cfg`。
- 门禁绿。

---

## Task 5：Wintun 的适配器、会话与设备

**改哪里**（设计第 8.1、8.4、8.5 节；M4-D12；P3、P4、P8、P13）

1. **`src/tun/windows/wintun.rs` 再加十个放开的函数**。每个只包一次 Wintun 入口，或下面写的一组；句柄都是 `usize`：

   | 函数 | 包的调用 | 回 |
   | --- | --- | --- |
   | `create_adapter(w, name, tunnel_type, guid)` | `WintunCreateAdapter` | `io::Result<usize>` |
   | `open_adapter(w, name)` | `WintunOpenAdapter` | `Option<usize>`：`NULL` 回 `None`，错误码在 DEBUG |
   | `close_adapter(w, adapter)` | `WintunCloseAdapter` | |
   | `adapter_index(w, adapter)` | `WintunGetAdapterLUID`，再 `ConvertInterfaceLuidToIndex` | `io::Result<u32>` |
   | `start_session(w, adapter, capacity)` | `WintunStartSession` | `io::Result<usize>` |
   | `end_session(w, session)` | `WintunEndSession` | |
   | `read_event(w, session)` | `WintunGetReadWaitEvent` | 事件句柄，存成 `usize`。不关它：它归会话管 |
   | `wait_readable(event, ms)` | `WaitForSingleObject` | |
   | `receive(w, session)` | `WintunReceivePacket`，拷出，`WintunReleaseReceivePacket` | `io::Result<Option<Vec<u8>>>`：`ERROR_NO_MORE_ITEMS` 是 `None`；`ERROR_HANDLE_EOF`、`ERROR_INVALID_DATA` 是错误 |
   | `send(w, session, packet)` | `WintunAllocateSendPacket`，拷入，`WintunSendPacket` | `io::Result<bool>`：`ERROR_BUFFER_OVERFLOW`（环满）是 `Ok(false)` |

2. **`src/tun/windows/mod.rs`**：`TunDevice` 的 Windows 实现，换掉 Windows 上的 `unsupported`。

   - **`open`**（在 `spawn_blocking` 的线程上）
     1. 任务 3 的查找、校验、加载。
     2. **探**：`open_adapter("rurge")` 得到句柄时，`close_adapter`，回 `ResourceBusy` 类别的错误，经 `explained` 说成"正在被使用"（P8）。
     3. `create_adapter("rurge", "rurge", GUID)`。GUID 是常数 `b4a5c1e2-7d3f-4c6a-8e91-0f2d3b4c5a6e`。
     4. `adapter_index`：每 100 毫秒试一次，至多 5 秒（设计第 8.4 节）。
     5. `start_session`，环 `0x800000`（8 MiB）。
     6. `read_event`。
     7. 起读线程：`std::thread::Builder` 起名 `rurge-wintun-reader`，跑任务 3 的 `read_loop`；`Ring` 由 `receive` 与 `wait_readable` 实现。
     - 第 3 步之后任何一步失败：做过的倒着撤销（`end_session`、`close_adapter`）。
     - 错误经 `frame::explained(…, Os::Windows)`。
   - **`recv`**：异步地等通道的第一个元素，再不等待地取到 `max` 个。元素是 `Err` 就回它，TUN 因此结束——这就是 M4a 的"设备坏了"。通道关了：回 `the virtual adapter's reader ended`。
     - 接收端放在 `tokio::sync::Mutex` 里：等第一个元素时要跨 `await` 拿着它。只有收包任务调 `recv`，不会争用。
   - **`send`**：逐个 `wintun::send`，不 `await`。
     - `Ok(false)`：丢掉、计数（M4c 的 `mod.rs`）。
     - `Err`：让 TUN 结束。
   - **关闭**（`Drop`）：
     1. 置停止标志；
     2. 关掉通道的接收端（`Receiver::close`），让卡在满通道上的读线程醒来。`Drop` 拿着 `&mut self`，经 `Mutex::get_mut` 拿接收端，不用加锁；
     3. 等读线程结束；
     4. `end_session`；
     5. `close_adapter`——它建的适配器随之移除（`wintun.h`）。

3. **`frame.rs`**：`Os` 加 `Windows`，`explained` 加两行：

   | 错误类别 | 说明 |
   | --- | --- |
   | `PermissionDenied` | `the enhanced mode needs to run as Administrator` |
   | `ResourceBusy` | `the virtual adapter is in use: is another rurge running with the enhanced mode on?` |

4. **`enhanced::system()` 在 Windows 上**回 `WindowsSystem::new(Arc::new(TimedRunner(STEP_TIMEOUT)), WindowsTools::under(<系统根目录>), scratch, dll)`。
   - `dll` 来自 `system()` 新的参数 `wintun_dll: Option<PathBuf>`，经任务 3 的 `wintun_dll`。
   - 别的平台忽略这个参数。

5. **名单**：`LISTED` 加这十个；`CLAUDE.md` 同步，这时是 23 个。

**用例**

- `frame.rs`：`open_errors_say_what_is_missing` 加 Windows 的两行。
- 设备本身没有缺省用例（P19 的规矩）。逐条自查：
  - 句柄都是 `usize`；
  - 没有 `unsafe impl`；
  - 每个 `unsafe` 块前有 `// SAFETY:`；
  - 读线程在 `Drop` 里一定被等到；
  - `recv` 拿着的是 tokio 的锁（要跨 `await`），`Drop` 经 `get_mut`，不加锁；
  - 失败时倒着撤销。

**判据**：

- `every_unsafe_exception_is_on_the_list` 绿（23 个）。
- 门禁绿。
- 修正记录写一行"Wintun 的设备没有在任何地方运行过"。

---

## Task 6：`--wintun-dll`（可执行文件）

**改哪里**（设计第 9.6 节；M4a 计划 P14）

1. **`RunArgs`** 加 `--wintun-dll <路径>`（`env = "RURGE_WINTUN_DLL"`），**在每个平台上都定义**——可执行文件不加 `cfg`。
   - 不是 Windows 却给了它：用法错误，照 `run.rs` 里已有的用法错误的写法与退出码。
   - 文字是 `--wintun-dll is only for Windows`。
   - 判断写成一个纯函数，平台作为参数：`fn wintun_dll_applies(given: bool, windows: bool) -> Result<(), String>`。调用处传 `cfg!(windows)`，照 M4a 计划给 `RouteInputs::explicit_space` 传 `cfg!(target_os = "macos")` 的写法。用例在每个平台上把四种组合都测到。
   - 这个检查在 `run` 读配置、起任何监听之前做。
2. **`backend(data_dir)`** 改成 `backend(data_dir, wintun_dll)`，传给 `rurge_platform::enhanced::system`。`restore_what_a_crash_left` 传 `None`：Windows 上没有要撤销的种类。
3. **`--help`** 里它的说明：`Windows: where wintun.dll is (default: next to rurge.exe)`。

**用例**

- 纯函数的四种组合。
- `#[cfg(not(windows))] the_wintun_dll_option_is_a_usage_error_elsewhere`：起 `rurge run`（经 `rurge_command()`，给一份最小的配置与 `--data-dir`），以用法错误退出，标准错误有那句。
- **不加**在 Windows 上带着这个选项走到平台后端的用例（P19）。

**判据**：

- `git grep -n "cfg(" crates/rurge/src/cli/enhanced` 什么也没有。
- 门禁绿。

---

## Task 7：全量核对

- 跑一遍全量门禁，记下数字。
- 照 M4 设计第 14 节的 L1 一张单子，逐项核对每一项都有用例在，名字记进修正记录；缺的补上，或者写明为什么不能有。
- `git grep -n "TunDevice::open\|System::open" crates tests`：只出现在生产代码与说明里（P19）。

**判据**：修正记录里有这一张核对的结果。

---

## Task 8：等 CI（停点）

照 M4c 计划任务 10 的四步做：

1. 停下，请所有者推送；**Pi 不推送**。
2. 所有者说推送了之后，读那次运行的注解。
3. 按注解修，每一处一个提交，记进修正记录；修完再停下请所有者推送，重复。
4. 修不了的，或要所有者定的，列出来交给所有者；不改 CI 去绕过失败，不加 `#[ignore]`。

直到三个平台的 `check` 都绿。

**判据**：

- 修正记录里有每一次运行的运行号与结论。
- 最后一次运行三个平台都绿；或者剩下的失败都已列给所有者。

---

## Task 9：M4 的文档收尾

照设计第 16 节，逐份改。每份改完用 `git grep` 核对引用的用例名、文件与函数都在。

1. **兼容性清单**（`docs/surge-compatibility-matrix.md`）
   - 9.1 节各行：
     - 增强模式；
     - 基础前缀与 rurge 的空间；
     - `tun-excluded-routes`、`tun-included-routes`、`ipv6-vif`；
     - 开关的入口；
     - 实现基础（三个平台的设备）；
     - 权限。
     - "目前"一栏照实写："有；CI 编译与单元用例；真机没有验证"。
   - 6.3 节 `server:system` 一类（接管期间的去处）；`external` 的 `addresses`；10.3 节 `rurge run`（`--tun` → `--enhanced-mode`、`--wintun-dll`）；10.4 节 `/v1/features/enhanced_mode`。
   - 登记（总设计已登记的照旧）：
     - 非直连的 VPN 路由；
     - 外部程序与预匹配；
     - `setsid` 的子进程；
     - macOS 上多一次网络变化；
     - Linux 改 `rp_filter`；
     - 没有 resolved 的 Linux 不接管 DNS；
     - 另一个全隧道 VPN 开着时打开失败；
     - IPv6 RDNSS 的服务器读不到；
     - Windows 的多宿主解析与浏览器的 DoH；
     - `wintun.dll` 不随附（Q3 归项目所有者）。
2. **`docs/api/phase3.md` 的 M4 一节**（接着 M4a ～ M4c 写的往下写）：
   - Windows 一侧：
     - `wintun.dll` 的查找与签名校验，及其各句错误；它所在的目录应当只有管理员能写（P7）；
     - 管理员的说明；
     - "正在被使用"；
     - `netsh` 与读回；
     - 网卡的度量与转发设置；
     - `ipconfig /flushdns`。
   - 手工恢复 macOS DNS 的写法是 `empty`（M4c 计划 P6）。
   - 测试后端。
3. **`docs/acceptance/phase3-manual.md`** 加 M4 一节，照 M1 ～ M3 各节的样子：前置条件、Windows、macOS、Linux、与平台无关；编号接着现有的往下编。按设计第 16 节的单子逐项写成可以照着做的步骤、期望与要记下的证据：
   - 每个平台：
     - 开与关（命令行、API）；
     - 路由表的样子；
     - 系统 DNS 的接管与交还；
     - 强行结束之后再起的恢复（`kill -9` / `taskkill /F`）；
     - 防环：代理服务器、DNS 上游、资源下载都通，虚拟网卡的计数不因 rurge 自己的流量涨；
     - 睡眠唤醒与换网络；
     - 接管暂停；
     - `ping`；
     - 外部程序一律 DIRECT；
     - `ipv6-vif` 的三种取值；
     - `tun-excluded-routes` 与 `tun-included-routes`；
     - 与全隧道 VPN 共存：期望是打开失败、说清原因。
   - 平台专有的：
     - Windows：开了 Hyper-V / WSL / ICS 的机器（设计第 7.4 节）；`wintun.dll` 缺失与签名不对；管理员与非管理员；**`set interface … metric=1` 被接受**（P9）；读回在中文系统上照样认得出（P10）；**`taskkill /F` 之后再开，不报"正在被使用"**（P8 的前提：适配器随进程消失）；两个 rurge 同时开，后一个报"正在被使用"；Wintun 适配器设 MTU 之前的缺省 MTU 记下（P11）；真的 `wintun.dll` 的签名者是 `WireGuard LLC`（P5）。
     - Linux：严格 `rp_filter` 的发行版（RHEL 一族）；有无 resolved；L2 用例在一次性的虚拟机里跑一遍（`tests/privileged/README.md`）。
     - macOS：多个网络服务与手动设过 DNS 的服务；`-listnetworkserviceorder` 的输出格式与 M4c 的解析对得上（M4c 计划 P20）；IPv6 的 `/128` 地址与显式路由。
   - 与平台无关的：
     - `dns_tcp_max`（M3c 延后第 27 行，M4-D25）；
     - 非直连的 VPN 路由的限制与它的办法；
     - M3 一节里等 M4 的各项，此时可以做（只加一句提示，不改那些行）。
4. **`docs/testing.md`**：
   - `RURGE_ENHANCED_BACKEND`；
   - L2 的门控与跑法；
   - 扫描用例的名单怎样改；
   - 没有一条缺省用例开设备；
   - `RURGE_TEST_WINTUN_DLL`；
   - Windows 的读回夹具是手写的。
5. **两份 README**：特性表与路线图里增强模式的状态照实写——三个平台有代码，CI 编译与单元用例通过，真机没有验证。中英两份内容一致。
6. **需求文档第 7 节阶段 3 的状态行**：照实写，不写"齐备"——阶段 3 的需求还没有逐句对过代码（CLAUDE.md「称齐备」）。
7. **`CLAUDE.md`**（它只放规则与概况，进度写进 `docs/status.md`）：
   - `unsafe_code` 一段核对一遍（23 个函数）；
   - 「现在在哪里」：M4 做完了；三个平台的增强模式有代码、没有在真机上验证；下一步是 M4 的手工验收、阶段 3 的需求逐句对照，以及项目所有者的 Q2 ～ Q5。
   - 常用命令**不**加打开增强模式的命令：它改系统的路由与 DNS，以后的会话不应当照着手工跑。
8. **`docs/status.md`**：在阶段 3 下新写一节"M4d（Windows 与收尾）"，并写一段 M4 的总结。
   - 做了什么。
   - 没做的，与不要当成已经有的：
     - 三个平台的设备与系统改动都没有在真机上跑过；
     - L2 没有运行过；
     - 手工清单的 M4 一节一项也没有做；
     - 阶段 3 不称齐备。
   - 以后的会话必须知道的事：
     - 读回定成败；
     - 句柄存成 `usize`、不写 `unsafe impl`；
     - 先探后建；
     - `--wintun-dll`；
     - 没有一条缺省用例开设备。
   - 去向写着项目所有者的：Q2 ～ Q5、推送、手工验收。
   - 附录加 `windows-sys` 新开的特性。
9. **M4 设计的末尾**加一节"执行期的订正"：四份计划的「执行期修正记录」里改了设计正文的，逐条写，格式照第 21 节（"写计划时的订正"，写计划时已经写好；M4a 计划 P24 要的那一条在它的第 22 条）。正文不改。
10. 本计划末尾的两张表照实填。

**判据**：

- `git grep` 核对通过。
- 门禁绿。
- 做完停下，交项目所有者评审。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 引用的名字 | 对着 `ac0a98a` 与 M4a ～ M4c 三份计划核对。三份计划做出来的名字，以它们的修正记录为准。名字不同就照实改、记进记录，不改语义 |
| P2 | `windows-sys` 0.61.2 的特性（读本机 registry 的源码核对） | `setsockopt`、`IP_UNICAST_IF` / `IPV6_UNICAST_IF`（都是 31）在 `Win32_Networking_WinSock`。`LoadLibraryExW`、`GetProcAddress`、`LOAD_LIBRARY_SEARCH_SYSTEM32` 在 `Win32_System_LibraryLoader`。`WinVerifyTrust` 与 `WINTRUST_DATA` 在 `Win32_Security_WinTrust`，后者要 `Win32_Security_Cryptography`。`WTHelperProvDataFromStateData` 与 `WTHelperGetProvSignerFromChain` 要 `Win32_Security_Cryptography_Catalog` 与 `Win32_Security_Cryptography_Sip` 两个一起开。`CertGetNameStringW` 在 `Win32_Security_Cryptography`。`ConvertInterfaceLuidToIndex` 在 `Win32_NetworkManagement_IpHelper`，要 `Win32_NetworkManagement_Ndis`（`NET_LUID_LH`）。`IsProcessInJob` 在 `Win32_System_JobObjects`；`OpenProcess`、`WaitForSingleObject`、`PROCESS_SYNCHRONIZE` 在 `Win32_System_Threading`；`FreeLibrary`、`WAIT_OBJECT_0`、`ERROR_INVALID_PARAMETER`、`TRUST_E_NOSIGNATURE` 在 `Win32_Foundation`（这几个特性已开）。合计新开七个。`FILE_SHARE_READ` 在 `Win32_Storage_FileSystem`：为一个常数不开它，本地定义（任务 3） |
| P3 | 14 个放开的函数 | `socket.rs` 的 `unicast_if`；`process.rs` 的 `in_job`；`tun/windows/wintun.rs` 的 `verify_signer`、`load`、`create_adapter`、`open_adapter`、`close_adapter`、`adapter_index`、`start_session`、`end_session`、`read_event`、`wait_readable`、`receive`、`send`。设计第 3.8 节的单子，一个不多 |
| P4 | 句柄存成 `usize` | Wintun 的适配器、会话、事件与模块句柄是裸指针，不是 `Send` / `Sync`。写 `unsafe impl Send` 是一处不在函数上的放开，名单的用例不认，`CLAUDE.md` 也不许。指针与整数互转不需要 unsafe，只在放开的函数里转回去用 |
| P5 | 签名者的名字 | 设计定的是 `WireGuard LLC`（M4-D13）。取证书的写法：`WinVerifyTrust` 的状态数据 → `WTHelperProvDataFromStateData` → `WTHelperGetProvSignerFromChain(…, 0, FALSE, 0)` → 签名者链的第一张证书 → `CertGetNameStringW(CERT_NAME_SIMPLE_DISPLAY_TYPE)`。真的 `wintun.dll` 的签名者没有在这台机器上读过（没有这个文件）：由 `RURGE_TEST_WINTUN_DLL` 的用例与手工清单核对 |
| P6 | 校验不联网 | 微软的 `WINTRUST_DATA` 文档："To ensure the WinVerifyTrust function does not attempt any network retrieval … WTD_CACHE_ONLY_URL_RETRIEVAL must be set"；`WTD_REVOKE_NONE` 不加做吊销检查。两者都设。`WTD_STATEACTION_VERIFY` 之后必须再以 `WTD_STATEACTION_CLOSE` 调一次（同一页） |
| P7 | 校验与加载之间不被换掉 | 校验通过到 `LoadLibraryExW` 之间，别人可能换掉这个文件。以 `share_mode(FILE_SHARE_READ)` 只读地开着它直到加载完：别人不能写它，也不能删它或给它改名（没有给 `FILE_SHARE_WRITE` 与 `FILE_SHARE_DELETE`）；校验的就是这个开着的文件（`WINTRUST_FILE_INFO` 的 `hFile`）。加载器自己打开 DLL 时要的是读与执行，按 NT 的共享规则与"只许读"相容——这一点写计划时没有找到文档，由 `a_real_wintun_dll_passes` 与手工清单核对；不相容的话，打开会以"共享冲突"失败，不会悄悄加载别的东西。这一层只防文件本身被换；它所在的目录本来就应当只有管理员能写（缺省在 `rurge.exe` 旁边），API 参考写明。std 的 `OpenOptionsExt`，不需要 unsafe。`LoadLibraryExW` 给的是绝对路径："If the string specifies a fully qualified path, the function searches only that path for the module"；`LOAD_LIBRARY_SEARCH_SYSTEM32`："%windows%\system32 is searched for the DLL and its dependencies"（微软文档） |
| P8 | **先探后建，不复用**（订正设计第 8.4 节与 M4-D12 里"先开后建、复用残留的"一处） | 微软的 `SwDeviceSetLifetime` 文档："After a software device is initially created by calling SwDeviceCreate, its default lifetime is set to SwDeviceLifetimeHandle … PnP stops enumerating the device after the device's handle is closed"；"only one handle can be open for a device"。Wintun 的 `adapter.c` 用 `SwDeviceCreate`、不改它的寿命（写计划时读到的）。由此推得：我们建的适配器只在建它的进程开着时存在；崩溃之后它不再被枚举，`WintunOpenAdapter` 找不到，`WintunCreateAdapter` 以同一个 GUID 再建。这是推论，没有在真机上看过：手工清单核对"`taskkill /F` 之后再开，不报'正在被使用'"。`WintunOpenAdapter` 找得到 `rurge`，说明另一个进程正开着它，答"正在被使用"。复用它是错的，原因有二：`WintunCloseAdapter` 只移除 `WintunCreateAdapter` 建的适配器（`wintun.h`："if adapter was created with WintunCreateAdapter, removes adapter"），复用的那块永远不会被移除；它上面留着的路由会让下一次 `add route` 撞上 |
| P9 | `set interface` 的度量单独一行（订正 M4a 的 Windows 翻译） | 两页微软文档对 `set interface` 的参数说法不一：2008 年的 IPv4 一页（cc731521）列着 `metric`（"the interface metric, which is added to route metrics"）；现行的 `netsh-interface` 一页（写计划时又读了一遍）列着 `forwarding`、`weakhostsend`、`weakhostreceive`、`dadtransmits` 等，IPv4 与 IPv6 都没有 `metric`。一行里有一个系统不认的参数，整行就可能失败。拆成两行之后，`metric` 万一不被接受，也只丢度量：rurge 的路由前缀比缺省路由长，不靠度量取胜，度量只决定同样长的前缀之间谁先。现行文档没有写转发与 weak host 三项的缺省；写出来是为了明确，不是为了改缺省。手工清单核对 `metric=1` 被接受 |
| P10 | 成败由读回定 | `netsh -f` 的脚本里一行失败时，netsh 是停下还是接着、退出码是什么，没有找到文档；输出的文字随系统语言变。所以执行时一律当作 `Ignore`，退出码与输出只进 DEBUG；之后读回，按原来的 `OnFailure` 解释（设计第 5.4 节"读回核对"的扩展：设计只写了路由，这里加上地址、MTU 与 DNS）。读回只认前缀、序号、地址与数字，不认任何文字。输出按 `from_utf8_lossy` 读：中文系统上 netsh 的输出不是 UTF-8（一般是 OEM 代码页，GBK），字会坏，数字与地址是 ASCII，不坏 |
| P11 | MTU 的读回 | Wintun 适配器的缺省 MTU 写计划时没有找到文档（`wintun.h` 只写了它能收发的最大包 `WINTUN_MAX_IP_PACKET_SIZE` 0xFFFF），手工清单记下。`set subinterface mtu=1500` 没有生效时，系统可能往虚拟网卡送大于 rurge 协议栈 MTU 的包，TCP 会坏。所以 MTU 必须读回：`show subinterfaces interface=<i>` 的数据行的第一个数 |
| P12 | Windows 不进备份 | 网卡上的 DNS 设置写在这块网卡（按 GUID）的配置里，下一次以同一个 GUID 建网卡时会带回来。在网卡有地址之前，系统到不了 `198.18.0.2`；网卡不在时，它的设置不起作用。所以不进备份，交还也不做（与设计第 5.4 节一致） |
| P13 | 读线程 | 设计第 8.4 节：通道 1024 个包，满了线程等；环空了等读事件，100 毫秒醒来看停止的标志。关闭时先关通道的接收端，再等线程：卡在满通道上的 `blocking_send` 因此醒来。循环不带 `cfg`，对假的环测，三个平台都跑 |
| P14 | `--wintun-dll` 不加 `cfg` | 在每个平台上定义，别的平台上给出它是用法错误（设计第 9.6 节）。可执行文件不加 `cfg`（AR-02） |
| P15 | 系统根目录 | 取环境变量 `SystemRoot`，没有就是 `C:\Windows`。改了管理员进程的环境变量的人，本来就能做更多的事。不为此加一个调 `GetSystemDirectoryW` 的放开的函数 |
| P16 | 错误文本 | 不带 `wintun.dll` 的路径：它可能带账户名，错误又会进 API 的 500（设计第 13 节）。路径在 DEBUG。签名者的名字可以带 |
| P17 | 中文夹具 | 没有找到中文系统上 `netsh … show route`、`show subinterfaces`、`show dnsservers` 的原文。夹具手写，注明 "not taken from a machine"。解析不认文字，所以夹具里的字写错了也不影响结论；手工清单在中文系统上核对一项 |
| P18 | 停点 | 任务 8（等 CI）与任务 9 之后 |
| P19 | 没有一条缺省的用例开设备 | 同 M4c 计划 P19。Windows 的 CI 机器以管理员运行，谁调了 `TunDevice::open`，就真的装了驱动、建了网卡。有真的 `wintun.dll` 的用例也只校验与加载，不建适配器 |
| P20 | 写法的出处（写计划时读到的） | **`wintun.h`**（github.com/WireGuard/wintun，`api/wintun.h`）：各入口的签名；`WintunCloseAdapter` "Releases Wintun adapter resources and, if adapter was created with WintunCreateAdapter, removes adapter"；`WintunReceivePacket` 的 `ERROR_HANDLE_EOF` / `ERROR_NO_MORE_ITEMS` / `ERROR_INVALID_DATA`；`WintunAllocateSendPacket` 的 `ERROR_BUFFER_OVERFLOW`，"thread-safe"；`WINTUN_MIN_RING_CAPACITY 0x20000`、`WINTUN_MAX_RING_CAPACITY 0x4000000`，容量须是 2 的幂；`WINTUN_MAX_IP_PACKET_SIZE 0xFFFF`。**`adapter.c`**（同一仓库）：`SwDeviceCreate`，不设寿命；名字冲突时给占着名字的接口改名。**`SwDeviceSetLifetime`**（learn.microsoft.com）：见 P8。**Winsock 的 `IPPROTO_IP` 与 `IPPROTO_IPV6` 两页**：`IP_UNICAST_IF` "must be an interface index in network byte order"；`IPV6_UNICAST_IF` 的序号是 "in host byte order"。**`WINTRUST_DATA`**：见 P6。**`LoadLibraryExW`**：见 P7。**`netsh interface`**（learn.microsoft.com 现行的一页，`windows-server/administration/windows-commands/netsh-interface`）：IPv6 的 `add address` / `add route` / `delete route` / `set interface` / `set subinterface` / `show addresses` 用 `interface=`，`set dnsservers` / `show dnsservers` 用 `name=`，都接受"name or index"；IPv4 的 `show addresses` 用 `name=`；`show subinterfaces` 用 `interface=`；`show route` 只有 `level` 与 `store`，没有网卡的参数；`set interface` 没有 `metric`，也没有写 `forwarding` / `weakhostsend` / `weakhostreceive` 的缺省。**`cc731521`**（2008 年的 IPv4 一页）：`set interface` 的 `metric`；`show route` 缺省 `store=active`。**`show route` 的列**（`Publish Type Met Prefix Idx Gateway/Interface Name`）：阿里云开发者社区转载的《深入解析 IPv6（第 3 版）》2.9 节里 `netsh interface ipv6 show route` 的输出 |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的，项目所有者可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项：核对过，没有出入（文件名一处见第 11 行）。`HEAD` = `8073709` = 本地 `main`，分支 `phase3-m4d`，工作树干净，`origin/main` 仍是 `ac0a98a`（本分支未推送）；`git log --oneline 2c6a215..b91d786` 18 个、`b91d786..bb67afa` 26 个、`bb67afa..8073709` 32 个，M4a、M4b、M4c 的提交都在。三份计划末尾的两张表读过（M4a 修正记录 105 行 / 延后 15 行、M4b 80 / 17、M4c 50 / 10）。本计划要用的名字逐个在代码里找到：`route::windows` 的 `WindowsTools`（`crates/rurge-platform/src/route/windows.rs:22`）与 `configure`（`:41`）、`configure_ipv6`（`:55`）、`unconfigure_ipv6`（`:68`）、`add_routes`（`:76`）、`remove_routes`（`:97`）、`take_dns`（`:127`）——前三个回一个 `Step`、后三个回 `Vec<Step>`；`route::exec::{run (`route/exec.rs:39`), remove_leftovers (`:65`)}`；`command::TimedRunner`（`crates/rurge-platform/src/command.rs:47`）；`enhanced::{Adapter (`enhanced/mod.rs:29`), Saved (`:37`), NOT_AVAILABLE (`:49`), STEP_TIMEOUT (`:53`，30 秒), System (`:59`), Unsupported (`:167`), system (`:102` / `:115` / `:127`，今天的签名是 `system(scratch: PathBuf) -> Box<dyn System>`)}`；`tun::TunDevice`（`tun/mod.rs:73`）与 `DeviceSpec`（`:66`，今天只有 `name` / `mtu` 两个字段）；`tun::frame::{Os (`tun/frame.rs:108`), explained (`:126`)}`；`ProcessTree`（`process.rs:25`）与 `contains`（`:91`）；`socket::bind_interface`（`socket/mod.rs:92` / `:97` / `:117` / `:138` 四份，按平台）与 `pick_source`（`:83`），`interfaces()`（`:55`）是私有的；可执行文件的 `backend(data_dir: &Path)`（`crates/rurge/src/cli/enhanced/backend.rs:102`）、`PlatformBackend`（`:149`，私有的结构体）、`RunArgs`（`crates/rurge/src/cli/run.rs:57`）。`set interface` 今天仍是一行里带着 `metric=1`（`route/windows.rs:165` 的模板、逐字用例 `:261` / `:276`），任务 2 照 P9 拆成两行 | 本提交 |
| 2 | 0 | 第 2 项：`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `[(&str, &str); 9]`（`:18`～`:28`：`job_for`、`pcblist`、`pid_path`、`image_path`、`tcp_table`、`udp_table`、`notify_wininet`、`set_iff`、`utun_connect`），`CLAUDE.md` 的 `unsafe_code` 一段写“只有九个函数”（`CLAUDE.md:47`）——两处相同，与 M4c 收尾时一样（M4c 记录第 38 行核过同一对）。`crates/rurge/tests/sockets.rs` 的 `LISTED` 是 `[(&str, &str, &str); 25]`（`:58`）；M4d 的生产代码不新建套接字（Wintun 不是套接字，`unicast_if` 设在已有的套接字上），这张名单在 M4d 不变 | 本提交 |
| 3 | 0 | 第 3 项（V3，离线读本机 registry 的源码，不联网）：`rurge-platform` 用的正是 `windows-sys` **0.61.2**（`Cargo.lock` 的 `[[package]] name = "rurge-platform"` 块里写着 `"windows-sys 0.61.2"`；锁文件里另有 0.52.0 与 0.60.2 两个版本，不是这个 crate 的），与计划写的版本相同，没有出入。逐个用 `grep "fn <名字>("` 对着 `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows-sys-0.61.2/src/` 找到（函数都由 `windows_link::link!` 声明，都是 `system` 调用约定）：`setsockopt`（`Windows/Win32/Networking/WinSock/mod.rs:256`，`Win32_Networking_WinSock`）、`IP_UNICAST_IF`（同文件 `:2342`，`i32 = 31`）与 `IPV6_UNICAST_IF`（同文件 `:2166`，`i32 = 31`）；`GetProcAddress`（`Windows/Win32/System/LibraryLoader/mod.rs:33`）、`LoadLibraryExW`（`:36`）、`LOAD_LIBRARY_SEARCH_SYSTEM32`（`:87`，`LOAD_LIBRARY_FLAGS = 2048u32`）；`WinVerifyTrust`（`Windows/Win32/Security/WinTrust/mod.rs:15`，`Win32_Security_WinTrust`）、`WINTRUST_DATA`（同文件 `:775`，`#[cfg(feature = "Win32_Security_Cryptography")]`）、`WTD_CACHE_ONLY_URL_RETRIEVAL`（`:973`，`4096u32`）、`CRYPT_PROVIDER_DATA`（`:156`，`#[cfg(all(feature = "Win32_Security_Cryptography_Catalog", feature = "Win32_Security_Cryptography_Sip"))]`）、`CRYPT_PROVIDER_SGNR`（`:279`，`Win32_Security_Cryptography`）、`WTHelperGetProvSignerFromChain`（`:12`，它的 `cfg` 在 `:11`，两个特性一起）、`WTHelperProvDataFromStateData`（`:14`，`cfg` 在 `:13`，同上）；`CertGetNameStringW`（`Windows/Win32/Security/Cryptography/mod.rs:141`）与它的 `CERT_CONTEXT`（同文件 `:2249`）；`ConvertInterfaceLuidToIndex`（`Windows/Win32/NetworkManagement/IpHelper/mod.rs:22`，参数是 `*const super::Ndis::NET_LUID_LH`，那一型在 `Windows/Win32/NetworkManagement/Ndis/mod.rs:2393`）；`IsProcessInJob`（`Windows/Win32/System/JobObjects/mod.rs:8`）；`OpenProcess`（`Windows/Win32/System/Threading/mod.rs:242`）、`WaitForSingleObject`（`:380`）、`PROCESS_SYNCHRONIZE`（`:795`）；`FreeLibrary`（`Windows/Win32/Foundation/mod.rs:4`）、`WAIT_OBJECT_0`（`:10027`）、`ERROR_INVALID_PARAMETER`（`:2803`）、`TRUST_E_NOSIGNATURE`（`:9816`）；`FILE_SHARE_READ`（`Windows/Win32/Storage/FileSystem/mod.rs:1823`）。与 P2 记的特性逐个相同：`WTHelperProvDataFromStateData` 与 `WTHelperGetProvSignerFromChain` 确实要 `Win32_Security_Cryptography_Catalog` 与 `Win32_Security_Cryptography_Sip` 两个一起开，`WINTRUST_DATA` 要 `Win32_Security_Cryptography`，`FILE_SHARE_READ` 在 `Win32_Storage_FileSystem` 里（照 P2 不为一个常数开它，本地定义） | 本提交 |
| 4 | 0 | 第 4 项（V2）：本任务跑在 `--offline` 下，不联网。协调者核过（2026-10-27），对着 **`netsh interface`**（learn.microsoft.com 的 `windows-server/administration/windows-commands/netsh-interface`，现行的一页）：`show route` 两族都只有 `[level=]` 与 `[store=]`；IPv4 的 `show addresses [name=]`、IPv6 的 `show addresses [interface=]`；`show subinterfaces [interface=]` 两族都有；`show dnsservers [name=]` 与 `set dnsservers [name=]`，`address` 是 IP 或 `none`（清空名单）；`set interface` 两族都没有 `metric`，`forwarding` / `weakhostsend` / `weakhostreceive` 没有写缺省；`add route` / `delete route` 有 `[interface=]`、`add route` 有 `[metric=]`；`set subinterface [interface=] [mtu=]`。与 P9、P20 相同，任务 2、4 照它做 | 本提交 |
| 5 | 0 | 第 4 项的附带（协调者核过，2026-10-27）：**`IPPROTO_IP` 的选项**（`win32/winsock/ipproto-ip-socket-options`）——`IP_UNICAST_IF` 是 "This DWORD parameter must be an interface index in network byte order"，读回时是主机字节序，`optval` 为 0 是“不指定”；**`IPPROTO_IPV6` 的选项**（`ipproto-ipv6-socket-options`）——`IPV6_UNICAST_IF` 的序号是 "in host byte order"，`optval` 为 0 同样是不指定。与 P20 相同；任务 1 的回环用例按这个字节序写 | 本提交 |
| 6 | 0 | 第 5 项（Wintun）：协调者核过（2026-10-27），对着 **`wintun.h`**（raw.githubusercontent.com/WireGuard/wintun/master/api/wintun.h）：十一个入口都在、都是 `WINAPI`——`WintunCreateAdapter(Name, TunnelType, RequestedGUID)` 回 `WINTUN_ADAPTER_HANDLE`；`WintunOpenAdapter(Name)`；`WintunCloseAdapter`（"Releases Wintun adapter resources and, if adapter was created with WintunCreateAdapter, removes adapter"）；`WintunGetAdapterLUID(h, NET_LUID *)`；`WintunStartSession(h, Capacity)`（"Must be between WINTUN_MIN_RING_CAPACITY and WINTUN_MAX_RING_CAPACITY (incl.)"）；`WintunEndSession`；`WintunGetReadWaitEvent` 回 `HANDLE`（"Do not call CloseHandle on this event - it is managed by the session."）；`WintunReceivePacket(h, DWORD *PacketSize)` 回 `BYTE *`（`ERROR_HANDLE_EOF` / `ERROR_NO_MORE_ITEMS` / `ERROR_INVALID_DATA`，"This function is thread-safe."）；`WintunReleaseReceivePacket`；`WintunAllocateSendPacket` 回 `BYTE *`（`ERROR_HANDLE_EOF` / `ERROR_BUFFER_OVERFLOW`，"thread-safe"）；`WintunSendPacket`。常数 `WINTUN_MIN_RING_CAPACITY 0x20000`、`WINTUN_MAX_RING_CAPACITY 0x4000000`、`WINTUN_MAX_IP_PACKET_SIZE 0xFFFF`。与 P20 相同，任务 3、5 照它写 | 本提交 |
| 7 | 0 | 第 5 项的签名校验（协调者核过，2026-10-27）：**`WINTRUST_DATA`**（`win32/api/wintrust/ns-wintrust-wintrust_data`）——`WTD_REVOKE_NONE` 的说明里有 "To ensure the WinVerifyTrust function does not attempt any network retrieval …, WTD_CACHE_ONLY_URL_RETRIEVAL must be set"；`WTD_CACHE_ONLY_URL_RETRIEVAL` 是 0x1000；`WTD_STATEACTION_VERIFY` 的那个句柄必须由后一次 `WTD_STATEACTION_CLOSE` 放掉（"This action must be specified for every use of the WTD_STATEACTION_VERIFY action"）；`WTD_CHOICE_FILE` 是 1 并用 `pFile`。与 P6 相同 | 本提交 |
| 8 | 0 | **没有再核的**（照实记一行）：任务 0 跑在 `--offline` 下，除第 4 ～ 7 行协调者核过的五页之外，`LoadLibraryExW` 的两句（P7）、`SwDeviceSetLifetime`（P8）、`adapter.c`（P8、P20）、`cc731521`（2008 年的 IPv4 一页：`set interface` 的 `metric`、`show route` 缺省 `store=active`，P9、P20）与 `show route` 的列（`Publish Type Met Prefix Idx Gateway/Interface Name`，P20）都照 P20 记的出处，这一次没有对着原文再核；任务 2 的读回解析因此只认数字与地址，不认列的顺序以外的文字 | 本提交 |
| 9 | 0 | 第 6 项：门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）。`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-0.log`）**86 个结果行、4590 通过、0 失败、13 忽略**——与派工给的基线（M4c 收尾时 `gate-final-fix-r1.log` 的数）相同，差 0；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，没有重跑。之后每个任务与这三个数比 | 本提交 |
| 10 | 0 | 协调者的裁定 R1（订正任务 0 第 1 条「M4c 的等 CI 已经结束」、任务 8 的位置与任务 9 的停点）：M4c 的等 CI **没有结束**，照 M4c 账本 R3 与本计划任务 8 一起在最后做，任务 0 不等它；任务 8 在任务 9 之后做，与 M4c 的任务 10 合成一次停点；任务 9 做完不另停，与最后那一次停合成一次（只停一次等项目所有者推送）。理由：M4d 的代码是 Windows 的，这台开发机就能编译与测试，不靠 M4c 的 CI 结果；推送只有所有者能做，停两次是让他多等一次。错了的代价：M4c 的 CI 失败要回头改 M4d 碰过的共用代码（`tun/mod.rs`、`enhanced/mod.rs`），多一轮修正 | 本提交 |
| 11 | 0 | 协调者的裁定 R2（订正任务 1 的文件名）：`unicast_if` 写在 `crates/rurge-platform/src/socket/mod.rs`（M4b 把 `socket.rs` 拆成了 `socket/mod.rs` 与 `socket/egress.rs`），`LISTED` 的那一行是 `("src/socket/mod.rs", "unicast_if")`；Windows 的 `bind_interface` 也在 `mod.rs`（`:117`），`interfaces()`（`:55`）与 `pick_source`（`:83`）同样在那里。错的代价：无（名单用例会变红，自己查得出来） | 本提交 |
| 12 | 0 | 协调者的裁定 R3（订正任务 5 的设备写法，补任务 3 的「改约定」）：Windows 的设备照 M4c 的约定写在 `crates/rurge-platform/src/tun/windows/mod.rs`——平台只给 `open` / `read` / `try_read` / `try_write` / `no_room`（`tun/mod.rs:8`～`:23` 的约定），`TunDevice` 仍是 `tun/mod.rs` 那一个共用的；`read` 拿 tokio `Mutex` 里的接收端、等第一个元素、拷进 `buf`，`try_read` 用 `try_lock` 加 `try_recv`（空是 `WouldBlock`，断开是 `the virtual adapter's reader ended`），`try_write` 调 `wintun::send` 且 `Ok(false)` 映成 `WouldBlock`，`no_room` 认 `WouldBlock`；计划写的「`recv` 等第一个、再不等待地取到 `max` 个」由 `TunDevice` 的 `recv`（`tun/mod.rs:115`）加 `drain`（`:172`）给出，Windows 不另写一套 `recv` / `send`。约定里的 `open` 改成连序号一起回（例如 `io::Result<(Device, Adapter)>`，Linux 与 macOS 给 `index: None`），`TunDevice::open`（`:91`，今天把 `Adapter { name, index: None }` 写死）用它；这一改在任务 5 做，任务 3 不碰设备。理由：M4c 的约定已经把发包的丢弃计数与两行日志放在共用的 `write_all`（`:196`）里，另写一套就要重复它们；序号是 Windows 的步骤要的（`netsh … interface=<序号>`）。错的代价：每个包多一次拷贝（至多 64 KiB）；约定的签名改一处 | 本提交 |
| 13 | 0 | 协调者的裁定 R4（订正任务 4 的两处 WARN，照 M4c 的 R15）：Windows 的读回解释里 `Ipv6Off` 的失败本身只写 DEBUG；`configure_ipv6` 读回缺了：DEBUG、撤掉做了一半的、回 `Ok(false)`（管理器写那一行 WARN，`crates/rurge/src/cli/enhanced/mod.rs:869`）；`routes` 的 IPv6 一半读回缺了：恰好一行 `the IPv6 routes could not be added to the virtual adapter; they are left out`（最好经 `enhanced::run_routes`，`crates/rurge-platform/src/enhanced/mod.rs:137`；形状由执行者定，记一行）；`take_dns` 的 IPv6 读回缺了：一行 WARN，文字由执行者定、不带网卡名与地址。理由：R15 的分工是地址那一半由管理器说、路由那一半由 `run_routes` 说，同一件事不说两次。错的代价：几行日志与它们的用例 | 本提交 |
| 14 | 0 | 协调者的裁定 R5（补任务 4）：任务 4 给 `DeviceSpec`（`tun/mod.rs:66`）加 `wintun_dll: Option<PathBuf>` 字段，Linux 与 macOS 的两处 `DeviceSpec { … }` 字面量补 `wintun_dll: None`（`crates/rurge-platform/src/enhanced/linux.rs:147`、`enhanced/macos.rs:255`）；`WindowsSystem::open` 的 MTU 写 `usize::from(route::MTU)`（`route/mod.rs:42` 的 `pub const MTU: u16 = 1500`），照 `enhanced/linux.rs:149` 的写法，不写字面量 `1500`；`Some(序号)` 在任务 5 之前拿不到（任务 4 的 `open` 在 Windows 上仍经 `unsupported` 失败），任务 4 不为它写用例。错的代价：无 | 本提交 |
| 15 | 0 | 协调者的裁定 R6（补任务 5 与 6 的交接）：任务 5 把 `enhanced::system(scratch)`（今天的签名见 `enhanced/mod.rs:102` / `:115` / `:127`）改成 `system(scratch, wintun_dll: Option<PathBuf>)` 时，同一个提交里可执行文件的 `backend()`（`crates/rurge/src/cli/enhanced/backend.rs:102`）先传 `None`；任务 6 换成 `--wintun-dll` 的值，`restore_the_enhanced_mode` 照计划传 `None`。不这样中间那个提交编译不过。错的代价：无 | 本提交 |
| 16 | 0 | 协调者的裁定 R7（订正任务 2、3、4、6 的「没有 `cfg`」判据，同 M4c 的 R7 / R11）：这几条判据指生产代码，测试模块的 `#[cfg(test)]` 不算；做法是 `git grep -n "cfg(" <文件>`，结果里去掉只有 `#[cfg(test)]` 的那一行之后为空。计划要求用例写在同一个文件的 `tests` 里，照字面两条要求互相矛盾。检查过本任务相关的三个文件：`route/windows.rs` 的 `cfg` 只有测试模块那一处，`enhanced/` 下 `git grep` 五行全是 `#[cfg(test)]`。错的代价：无 | 本提交 |
| 17 | 0 | 协调者的裁定 R8（补任务 6）：`crates/rurge/tests/cli.rs` 里起 `rurge run`、已经去掉 `RURGE_ENHANCED_MODE` 一类运行选项的助手（`rurge_command()` `:56`～`:66`、`rurge_process()` `:70`～`:79`，`run` 模块里的 `:2529`～`:2533` 与 `:3512`～`:3515`），同样 `env_remove("RURGE_WINTUN_DLL")`：开发者的环境里设着它时，用例的 `rurge run` 不应因此改了行为（在 Linux / macOS 上更会变成用法错误）。错的代价：一行 | 本提交 |
| 18 | 0 | 协调者的裁定 R9（订正任务 9 的 CI 说法，同 M4c 的 R9）：任务 9 写在 CI 读回之前——兼容性清单的「目前」一栏、两份 README、`docs/status.md`、`CLAUDE.md` 凡说到 CI 的地方照实写「Windows 的代码在这台开发机上编译、单元用例通过；Linux 与 macOS 的代码 CI 还没有见过」，不写「CI 编译与单元用例通过」；`CLAUDE.md`「现在在哪里」写明推送与读回 CI 都还在等；M4c 任务 11 已经写下的（Linux / macOS 的四项登记、`server:system` 的去处）不重复，接着写。依据是 `CLAUDE.md` 的「照实记录」。错的代价：读回之后多一个改文档的提交 | 本提交 |
| 19 | 0 | 从 M4b 终审带过来的（M4b 账本 R27、M4b 延后第 15 行）：任务 9 的手工清单要加一项——在物理网卡上分别 ping 一个外部的 IPv4 与一个外部的 IPv6 地址（经虚拟网卡，`PlatformPinger` 在 Windows 上用原始套接字），都收到回复，Windows 防火墙开着、关着各做一次。依据：Windows 原始 ICMPv6 套接字的校验和由谁算没有出处，回环上的往返证明不了 | 本提交 |
| 20 | 0 | 从 M4b 终审带过来的（M4b 延后第 13 行）：`IP_UNICAST_IF` 接上之后（任务 1 改 Windows 的 `bind_interface`），看 `PlatformPinger` 的载体（`crates/rurge/src/cli/pinger.rs`，按地址族与出口名缓存、只增不减）在同一块网卡换地址之后还能不能用；若网卡拔插换了序号仍会失效，照 M4b 终审 M2 在发送出错时淘汰那个载体。任务 1 做完时看一眼、照实记进它的报告 | 本提交 |
| 21 | 0 | 从 M4b 终审带过来的（M4b 延后第 9 行）：任务 9 的手工清单再加一项——增强模式开着时看网络快照（`rurge status` 或 HTTP API 里网络环境那一部分）里没有虚拟网卡。理由：排除名单 `Exclude` → `ReadOpts` → `netenv` 过滤网卡这一条链没有端到端证据，文件后端说什么就是什么 | 本提交 |
| 22 | 0 | 从 M4b 终审带过来的（M4b 延后第 10 行）：启动时的第一次下载发生在增强模式打开之前，没有用例走到「开着的时候启动下载」；去向是本计划的收尾任务（任务 9）——与增强模式打开的次序一起看，照实记进 `docs/status.md`（M4c 的 R10 把崩溃恢复挪到了下载之前，看的时候一起说） | 本提交 |
| 23 | 0 | 从 M4c 带过来的（M4c 记录第 29 行，R15）：`route::exec::run` 的 `Ipv6Off` 失败只写 DEBUG；加路由的 IPv6 一半失败时的那一行 WARN（`the IPv6 routes could not be added to the virtual adapter; they are left out`）在 `enhanced::run_routes` 里。任务 4 的 `WindowsSystem::routes` 也经 `run_routes`（或照它写同样的一行）；配 IPv6 地址失败时只有管理器那一行。与第 13 行（R4）是同一件事的两种说法，任务 4 照它们做、不多出第二行 | 本提交 |
| 24 | 0 | 从 M4c 带过来的（M4c 记录第 34 行，R17）：L2 用例的服务端放在只经默认路由可达的地址上（rurge 的基础前缀比内核给的直连路由短，直连网段上的流量不进虚拟网卡）。任务 9 的手工清单里「经虚拟网卡」的各项同样要选不在本机直连网段上的目标 | 本提交 |
| 25 | 0 | 从 M4c 带过来的（M4c 记录第 27 行，R14）：设备读到 0 个字节是设备没了（`tun::frame::packet_len`，`tun/frame.rs:56`）；Wintun 的会话读不经它（它没有「读到 0」这种回答），任务 5 不用照搬，但 `WintunReceivePacket` 的 `ERROR_HANDLE_EOF` 同样是设备没了 | 本提交 |
| 26 | 0 | 从 M4c 带过来的（M4c 记录第 44 行，R24）：带脚本的步骤失败时 `route::exec::run_step` 把错误文本里脚本的完整路径换成 `…`。Windows 的 `netsh -f` 也经 `run_step`，所以任务 2、4 自动带上；凡断言错误文本的用例期望 `…`，不期望数据目录里的路径（路径在家目录下时带账户名，理由同 P16）。网卡名、地址与前缀留在错误里（M4c 延后第 10 行，归项目所有者），Windows 照同一个分工，不另定 | 本提交 |
| 27 | 0 | 从 M4c 带过来的（M4c 记录第 40 行，R23）：管理器只在这一次 `adjust_undo` 回 `Some`、或备份里已有 `Stage::Adjust` 的一项时调 `adjust()`。任务 4 的 `WindowsSystem::adjust_saved` 回 `Ok(None)`，所以 Windows 上从不调 `adjust`；`adjust` 仍要实现（回 `Ok(())`），不为它写「被调用」的用例。本次核过：`enum Stage` 的 `Adjust` 那一项在 `crates/rurge/src/cli/enhanced/undo.rs` | 本提交 |
| 28 | 0 | 从 M4c 带过来的（M4c 记录第 42 行，终审 M1）：做了一半的接管也算接管过。任务 4 的 `WindowsSystem::take_dns` 若记同样的标志或备份，同样先记、再跑，失败时 `undo` 能把已改的还回去 | 本提交 |
| 29 | 0 | 从 M4c 带过来的（M4c 记录第 43 行，终审 M2）：`close` 用 `on.dev.take()`，设备在写回调整之前关掉（管理器里的事，平台无关）。任务 5 的 Wintun 设备要在最后一个 `Arc` 放掉时真的结束会话、关掉适配器（`Drop`），不靠别处另调；有读线程持着克隆的，停止时先让它退出 | 本提交 |
| 30 | 0 | 从 M4c 带过来的（M4c 记录第 46 行，R26）：Linux 打开设备的权限提示改成 `the enhanced mode needs root (in a container, root with CAP_NET_ADMIN)`（`tun/frame.rs` 的 `explained`），订正设计第 8.5 节的「root or CAP_NET_ADMIN」。任务 9 在设计末尾的订正一节记这一条；Windows 的那一句不受影响 | 本提交 |
| 31 | 0 | 从 M4c 带过来的（M4c 记录第 50 行，R31）：Linux 与 macOS 一次也没有编译过的只是开设备的代码、挑平台的 `cfg` 与 L2 用例文件；动系统的一半不带 `cfg`，在这台开发机上编译、只用假的执行器测过，真机上都没有跑过。任务 9 写 `CLAUDE.md`「现在在哪里」、两份 README、兼容性清单与 `docs/status.md` 时照这个说法，不写「这段代码一次也没有编译过」 | 本提交 |
| 32 | 0 | 从 M4c 带过来的（M4c 延后第 2、5、6、7、8、9 行与 R25 的一项）：这些都进任务 9 的手工清单（`docs/acceptance/phase3-manual.md` 的 M4 一节，今天还没有 M4 一节）——macOS 服务与网卡的配对（`-listnetworkserviceorder`、`ipconfig getpacket`）、`server:system` 经 `physical_dns` 的端到端、`networksetup` 失败时的退出码、一个服务改不了时整个打开失败（有 VPN 的机器）、运行期间服务改名、NetworkManager 会不会盖掉链路设置；另加 R25 的一项（接管期间 `resolvectl status <链路>` 显示 LLMNR、mDNS、DNSSEC、DNSOverTLS 都是 no，关闭之后 `revert` 撤掉） | 本提交 |
| 33 | 0 | 从 M4c 带过来的，归项目所有者（最后的答复里列出，任务 9 写进 `docs/status.md` 的去向）：M4c 延后第 10 行（错误文本里 rurge 自己的网卡名、地址与前缀）、延后第 1 行（L2 在 CI 上运行，总设计 Q4） | 本提交 |
| 34 | 0 | 从 M4a 延后第 10 行与 M4b 延后第 2 行带过来的一个名字：`Opened::index` 自 M4c 起有人读（`crates/rurge/src/cli/enhanced/backend.rs:158` 的 `adapter()` 把它交给平台后端），那一行的 `#[allow(dead_code)]` 已经不在了；`filled`（`crates/rurge/src/cli/enhanced/mod.rs:72`～`:73`）仍带着 `#[cfg_attr(not(test), allow(dead_code))]`，生产代码里唯一的读者没有（`:1235` 在 `:1082` 的测试模块里）——照 M4b 延后第 2 行的去向，归任务 9 的收尾：那时若仍没有生产读者，去掉标记与说明里的「wired up by …」，并照实记进 `docs/status.md` | 本提交 |
| 35 | 1 | 第 2 项照第 11 行（协调者的裁定 R2）做：`unicast_if` 写在 `crates/rurge-platform/src/socket/mod.rs`（`unicast_if` 在 `:163` 起，Windows 的 `bind_interface` 在 `:119`）；`LISTED` 里那一行是 `("src/socket/mod.rs", "unicast_if")`，计划第 2、4 项写的 `src/socket.rs` 按第 11 行读，正文不改 | 本提交 |
| 36 | 1 | 第 1 项：`crates/rurge-platform/Cargo.toml:28` 加 `Win32_Networking_WinSock`（`setsockopt`、`IP_UNICAST_IF`、`IPV6_UNICAST_IF`、`SOCKET_ERROR`、`WSAGetLastError` 都在它下面，与第 3 行 V3 的表相同；其余六个在任务 3 加）。`Cargo.lock` 不变（只加特性，不加包） | 本提交 |
| 37 | 1 | 第 2 项的做法：`interfaces()` 只取一次（`socket/mod.rs:123`），挑源地址与找序号用同一张表；序号取自 `name == interface && ip() == source` 的那一项的 `index`（`socket/mod.rs:134`～`:136`）。`if-addrs` 0.15 的 `src/windows.rs:49` / `:58` 与 `src/lib.rs:488` 读过：IPv4 的项给 `IfIndex`、IPv6 的给 `Ipv6IfIndex`，0 当作 `None`，与计划写的相同，没有出入。网卡名与错误只进 DEBUG（`:139`、`:151`），没有升到 INFO 以上 | 本提交 |
| 38 | 1 | 第 2 项的字节序：IPv4 用 `index.to_be()`、IPv6 用 `index` 原样（`socket/mod.rs:173`～`:174`），长度都是 4，与第 5 行（协调者核过的 `IPPROTO_IP` / `IPPROTO_IPV6` 两页）一致。把 IPv4 那一处临时改成主机字节序，`unicast_if_on_a_loopback_socket_still_reaches_the_loopback` 立刻变红（`setsockopt` 回 `WSAEADDRNOTAVAIL` 10049），说明这条用例真的分辨字节序；改回 `to_be()` 之后变绿 | 本提交 |
| 39 | 1 | 协调者的裁定 R11（订正任务 1 的第 2 项）：`unicast_if` 回 `Err` 时不往上抛——写一行 DEBUG、回 `Ok(())`，源地址照旧绑着（`socket/mod.rs:148`～`:154`）。理由：防环的第一层是绑源地址（强主机模型），`IP_UNICAST_IF` 是第二层，设不上与拿不到序号是同一种处境；一个连得上的连接不该因为第二层设不上而失败。错的代价：网卡的转发或 weak host send 开着时第二层不起作用，仍靠强主机模型挡（设计第 7.4 节登记的限制），由手工清单核对 | 本提交 |
| 40 | 1 | 执行者的裁定（提交的分法）：`CLAUDE.md` 的 `unsafe_code` 一段分两次改——本提交把「只有九个函数」改成「只有十个」并补上 `unicast_if` 那一句，任务 1 的第二个提交（`in_job`）再改成「十一个」。理由：common-brief 的「全局约束」要求「每次加函数的那个提交里一起改」，派工建议的提交分组把 `CLAUDE.md` 只放进第二个提交，两者在中间那个提交上冲突；分两次改之后每个提交都与 `LISTED` 一致，收尾时的两处文字与派工第 8 条相同。错的代价：无 | 本提交 |
| 41 | 1 | 第 3 项：`in_job` 写在 `crates/rurge-platform/src/process.rs` 的 `mod windows` 里（`:203` 起、`pub(super)`），挨着 `job_for`（`:149`）；签名与说明照任务抄。符号的特性：`IsProcessInJob` 在 `Win32_System_JobObjects`，`OpenProcess` / `WaitForSingleObject` / `PROCESS_QUERY_LIMITED_INFORMATION` / `PROCESS_SYNCHRONIZE` 在 `Win32_System_Threading`，`WAIT_OBJECT_0` / `ERROR_INVALID_PARAMETER` 在 `Win32_Foundation`——与第 3 行 V3 的表逐个相同，三个特性都已开，本任务不为它加特性 | 本提交 |
| 42 | 1 | 第 3 项的其余三条：`OpenProcess` 回空且错误是 `ERROR_INVALID_PARAMETER` 时回 `Ok(false)`，其余错误照回（`process.rs:218`～`:225`）；零等待的 `WaitForSingleObject` 回 `WAIT_OBJECT_0` 时回 `Ok(false)`（`:230`～`:233`）；`IsProcessInJob` 回 0 时回该错误的 `Err`（`:237`～`:240`）。句柄进 `OwnedHandle`，离开函数就关。`contains` 的 Windows 一支（`:108`～`:122`）：`job` 是 `None` 回 `false`，`Err` 回 `false` 并只写一行 DEBUG（`:118`，带 PID 与错误，没有名字或路径）；`contains` 的说明改成“`false` for a process that is not running, and for a tree that has already been ended”（`:90`～`:92`）——M4b 写的“直到 M4d 都判不出来”的两处文字都不在了 | 本提交 |
| 43 | 1 | 照任务点名删掉 `contains_is_false_until_m4d`（它钉的是 M4b 的“Windows 恒假”），换成需求的三条用例（`process.rs:348`、`:367`、`:385`）；用测试模块现成的 `long_runner()` 与 `ended_within()`，每条结束时子进程已结束（等不到就 `kill` 再断言），不留孤儿 | 本提交 |
| 44 | 1 | 一条照实的限制：把 `WaitForSingleObject` 那一步临时去掉之后，`an_exited_child_whose_handle_is_still_open_is_not_contained` 仍然绿——在这台机器上，对一个已经结束、但进程对象还被 `Child` 的句柄留着的进程，`IsProcessInJob` 自己就答假。所以那一步是照 M4 设计第 21 节第 8 条的守卫，这条用例分辨不出它；用例测的仍然是“结束的进程不在里面”这个行为。要不要为它另想一个能分辨的写法，归协调者看要不要 | 本提交 |
| 45 | 1 | 协调者的裁定 R10：端到端用例去掉 `#[cfg(unix)]`（`tests/external/tests/engine.rs:469`～`:473`），说明改成三个平台都跑、子进程经各平台的进程树认出（`socks-helper` 等监听之后才起子进程，Job Object 来得及收下它）。本机跑 `cargo test -j 8 --workspace --test engine -- what_the_program_started_goes_direct_too` 绿（1 个通过、7 个过滤掉、1.11 秒），循环二十次全过。`docs/api/phase3.md` 里“Windows 的 `ProcessTree::contains` 恒假”“M4d 之前只认程序本身”与 `docs/testing.md:183` 的 Windows 说法本任务不改，归任务 9（派工第 5 条） | 本提交 |
| 46 | 1 | 第 12 项（修正记录第 20 行 / M4b 延后第 13 行）：读过 `crates/rurge/src/cli/pinger.rs`（只读，没改）。结论：载体走 `bind_interface`（`make` 里 `:99`～`:113` 的 `Some(interface)` 一支，调用在 `:100`～`:101`），所以这一改也让 ICMP 的套接字带上 `IP_UNICAST_IF`；`egress` 是 `None` 时绑未指定地址（`:118`），不设。载体按（地址族、出口名）缓存、只增不减（`carrier` `:57`～`:90`，查找在 `:59`～`:64`，建成后 `carriers.push` 在 `:83`～`:88`，只有 `Drop for PlatformPinger`（`:125`～`:134`）才知道它们）：同一块网卡换了地址时，载体还绑着旧地址、带着旧序号，`send_to`（`echo` 里的 `:149`）会以 `WSAEADDRNOTAVAIL` 一类失败，直到进程重开都是如此（`carrier` 只在“建失败”时不记，建成之后不会因为发送出错而淘汰）；网卡拔插换了序号、而友好名没变时同理。换到另一块网卡（出口名不同）时会新建一个，旧的套接字与它的接收任务留到 pinger 被丢掉——那是“只增不减”本来的代价。要不要在发送出错时淘汰载体（M4b 终审 M2），照派工交给协调者定 | 本提交 |
| 47 | 1 | 门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-1.log`）**86 个结果行、4595 通过、0 失败、13 忽略**，比基线 4590 多 5——socket 两条、process 三条（删掉的那一条抵掉一条）、端到端那一条从“Windows 上不编译”变成在本机跑。没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，没有重跑。计时用例的循环二十次记在 `task-1-report.md` | 本提交 |
| 48 | 1 | 协调者的裁定 R12（补第 46 行的结论，覆盖 M4b 延后第 13 行「若……仍会失效」那一半）：发送失败的 ICMP 载体不再留着。`PlatformPinger::forget(&self, socket: &Arc<UdpSocket>)`（`crates/rurge/src/cli/pinger.rs:93`～`:106`）按指针（`Arc::ptr_eq`）在 `carriers` 里找那一个，找到就移除并 `abort` 它的接收任务，找不到什么也不做；`echo` 里 `send_to` 失败时先 `forget(&socket)`、再写一行 DEBUG（`icmp: an echo socket failed to send; the next echo makes a new one`）并把原错误回给调用方（错误本身不变）（`:166`～`:176`）。按指针比而不按（地址族、出口名）比：同一个键下的新载体不能被一次迟到的失败拿掉。代价：一个到不了的目标每次回声都重建一次套接字（并多一行 DEBUG）。`carrier` 的说明补了半句（`:54`～`:57`）。用例：`forget_drops_only_the_carrier_that_failed`（`:493`）与 `a_carrier_that_fails_to_send_is_made_again`（`:535`），三个平台都跑，本机循环二十次全过 | 本提交 |
| 49 | 2 | 派工第 0 条（先做的小提交）：任务 1 修正第 1 轮写在 `crates/rurge/src/cli/pinger.rs` 的四行注释带着中文与账本编号（`:93`～`:94` 的 `M4b 终审 M2` / `协调者的裁定 R12`、`:169` 与 `:532` 的 `M4b 终审 M2, R12`），改成英文并指向本计划的第 48 行（改后 `:93`～`:96`、`:169`～`:170`、`:533`～`:535`），按 80 列重新折行，意思不变。`git grep -n -P "^\s*//.*[\x{4e00}-\x{9fff}]" -- 'crates/*.rs' 'tests/*.rs'` 此后只剩 `crates/rurge-platform/src/netenv/linux.rs:247`（引用系统的回答 `是`，不动）。`cargo fmt --all --check` 通过，`cargo test -j 8 --workspace --bins -- pinger` 7 个通过 | 本提交 |
| 50 | 2 | 第 1、2 项：`routes_in`（`route/windows.rs:206`）、`addresses_in`（`:224`）、`mtu_in`（`:238`），签名与说明照需求第 1 项；四个读回的命令 `show_routes`（`:170`）、`show_addresses`（`:177`）、`show_mtu`（`:184`）、`show_dns`（`:194`），参数一项一个字符串（`readback` `:249`）、不用脚本、`OnFailure::Ignore`；`show_routes` 只给四个参数（`show route` 没有网卡的参数，按序号筛是调用方的事，计划 P20）。夹具手写八份（`ROUTES_EN` `:556`、`ROUTES_ZH` `:567`、`ADDRESSES_EN` `:578`、`DNS_EN` `:588`、`ADDRESSES_ZH` `:596`、`DNS_ZH` `:606`、`MTU_EN` `:615`、`MTU_ZH` `:623`），每份旁边写明 "not taken from a machine"。用例 `:630`～`:764`。执行者的裁定：替换字符那一份不另写常量，在 `routes_are_read_whatever_the_language` 里把 `ROUTES_ZH` 逐字符换成 `U+FFFD`——GBK 按 UTF-8 读出的替换字符个数与汉字个数不一定相等，逐字符换最贴近「文字坏、数字与地址不坏」，也少一份手写的东西。临时改一行验过用例真的分辨（都改回）：`routes_in` 改成取行里最后一个数时 `routes_are_read_from_the_english_table` 变红（`Ethernet 2` 那一行给出 2 而不是 12）；`configure` 去掉度量那一行时 `the_adapter_comes_up_in_one_script` 变红（后一条在下一个提交里才是期望的写法） | 本提交 |
| 51 | 2 | 第 3 项（计划 P9）：`interface_line` 去掉 `metric={METRIC}`（`route/windows.rs:267`），新增 `metric_line`（`:277`），`configure` 与 `configure_ipv6` 在 `interface_line` 之后、`subinterface_line` 之前加它。照需求第 3 项点名改两条 M4a 逐字用例的期望：`the_adapter_comes_up_in_one_script`（`:376`）与 `ipv6_is_a_script_of_its_own`（`:392`）。本提交的红：把 `configure` 里的 `metric_line` 那一行临时去掉（上一个提交上做的），`the_adapter_comes_up_in_one_script` 变红，改回后转绿 | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
