# 阶段 3 / M4c「Linux 与 macOS」实施计划

> **执行方式**：交给 Pi。M4b 计划做完、项目所有者评审之后，在一个会话里按任务的次序逐个做（技能 `superpowers:executing-plans`；没有子代理，不并行，不跳）。
>
> 每个任务的步骤：先写失败的用例、看到红；再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；最后提交（**只在本地，不推送**，M4 设计第 2 节）。
>
> 与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。
>
> 这一份有两个停点：
> - 任务 10：请项目所有者推送，然后读回 CI。Linux 与 macOS 的代码只有 CI 能编译。
> - 任务 11 之后：交项目所有者评审。

**目标**：Linux 与 macOS 上 `rurge run --enhanced-mode` 真的能打开。具体是：

- 两个平台的虚拟网卡：Linux 用 `/dev/net/tun`，macOS 用 utun，各有一个 unsafe 函数。
- 步骤的执行：Linux 用 `ip -batch`，macOS 逐条执行。
- 系统 DNS 的接管与交还：
  - Linux 经 systemd-resolved 的逐链路设置；
  - macOS 经 `networksetup` 改每个启用的网络服务，改之前先备份。
- Linux 的 `rp_filter`。
- 接管期间物理网卡的 DNS 服务器从哪里读。
- CI 里 clippy 的错误公开可读。
- L2 特权用例写好：门控，没有运行过。

Windows 照旧答"这一版还不能"（M4d）。

**架构**：`rurge-platform` 多两个模块。

- `tun`：`TunDevice`，负责打开、收、发，都是异步的（`tokio::io::unix::AsyncFd`）。每个平台一个实现（Linux、macOS）；别的平台一打开就失败。帧的处理与错误的说明不带 `cfg`。
- `enhanced`：系统一侧的接口 `System`，只有抽象的动作：配网卡、路由、DNS、调整、撤销、物理网卡的服务器。
  - `LinuxSystem` 与 `MacSystem` **不带 `cfg`**。要执行的程序经 `CommandRunner` 给进来，要读写的文件经一个根目录给进来，所以在 Windows 开发机上就能用假的执行器与临时目录测。
  - 带 `cfg` 的只有两样：两个设备，与挑平台的 `system()`。
- M4a 的翻译（`route::{linux,macos}`）由 `route::exec::run` 执行。脚本写进数据目录，用完删掉。

可执行文件这边：

- 平台后端变成 `System` 上薄薄的一层适配，不带 `cfg`。
- `Backend` 多一个方法 `physical_dns`。
- 管理器把它的结果写进 M4b 的 `PlatformSystemDns`。

**技术栈**：

- tokio（`AsyncFd`）。`rurge-platform → tokio` 是这一份唯一新的依赖边，tokio 是工作区已有的包。
- nix 0.31 重新导出的 `libc` 0.2.189（已有）。
- socket2 0.6（已有）。

不新增外部包，`Cargo.lock` 的包数不变。

**依据**：

- M4 设计（下面写"设计"指它）：
  - 第 2、3.1、3.8、4.5、5.1 ～ 5.3、5.5、6.1、6.3、7.3、7.5、8.1 ～ 8.3、8.5、9.1、9.2、13、14 节；
  - 已决事项 M4-D3、D4、D6、D10、D12、D14、D27、D29。
- M4a 计划（翻译、`Backend`、管理器、文件后端）与 M4b 计划（出口、`PlatformSystemDns`、`sockets.rs` 的名单）。**以这两份计划的「执行期修正记录」为准。**
- M4a 计划 P5："M4c 的开工前核对对着手册页再核一遍"。
- 设计第 17 节的 V2（Linux 与 macOS 的写法）与 V4（符号）已在写计划时核对，结果与出处在下面的「计划期决定」P3 ～ P8、P20。任务 0 再核一遍。

## 全局约束

**依赖边**

- 只新加 `rurge-platform → tokio`（`workspace = true`）。
- `rurge-platform` 照旧不依赖任何工作区 crate。
- `Cargo.lock` 的包数与 M4b 做完时相同。
- 新加的边记进 `docs/status.md` 的附录（任务 11）。

**`unsafe_code`**：新增两个放开的函数。

- `src/tun/linux.rs` 的 `set_iff`：包 `ioctl(TUNSETIFF)`。
- `src/tun/macos.rs` 的 `utun_connect`：包 `ioctl(CTLIOCGINFO)`、`connect`、`getsockopt(UTUN_OPT_IFNAME)`。

同步与写法：

- `crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 同步。今天是 7 个，加上这两个是 9 个；以 M4a / M4b 做完时的个数为准。
- `CLAUDE.md` 的 `unsafe_code` 一段在同一个提交里同步，提交说明里写明。
- 每个 `unsafe` 块的前一行用 `// SAFETY:` 写明前提。

**缺省的 `cargo test` 不碰系统**：从不执行系统命令，从不建网卡，从不改路由、DNS 与 `rp_filter`。

- `LinuxSystem` / `MacSystem` 的用例只用假的执行器与临时目录。
- **没有一条缺省用例调 `TunDevice::open`，也没有一条调真实平台的 `System::open`。** 在 Linux / macOS 的 CI 上它们是真的（P19）。
- L2（任务 9）由 `RURGE_TEST_PRIVILEGED=1` 门控；Pi 从不放开它。

**开发机编译不到的代码**：Linux 与 macOS 的代码在开发机上编译不到。这包括 `tun/linux.rs`、`tun/macos.rs`，以及 `system()` 里带 `cfg` 的两行。

- 这部分写得少、写得直；能不带 `cfg` 的一律不带，放在开发机上测。
- 只在一个平台上用到、又不带 `cfg` 的项，一律做成从 crate 根可达的 `pub`。否则在别的平台上，`dead_code` 在 `-D warnings` 下变红。
- 带 `cfg` 的模块里，`use` 只引自己用到的东西。
- 这部分能不能编译，要到任务 10 读回 CI 才知道，照实写。

**执行程序**

- 程序一律用绝对路径（设计第 5.1 节）。
- 一律经 `TimedRunner` 执行，每个程序 30 秒为限（P11）：一个挂住的工具不会让主循环一直等下去。

**名字与地址只在 DEBUG**（设计第 13 节、M4-D28）

- INFO 只说：开、关、恢复、改了 `rp_filter`、没有接管 DNS 的原因。
- 网络服务的名字、物理网卡的名字、DNS 服务器只在 DEBUG。
- 带网络服务名字的命令（macOS 的 `networksetup`）失败时，回出去的错误是固定的一句；原文与命令行只在 DEBUG（P7）。
- 别的步骤的错误照 `command::failure` 的写法。这些命令行里只有 rurge 自己的网卡名、路由前缀，与数据目录里脚本的路径。

**测试的约定**

- 测试只用回环与端口 0。L2 是例外：它的地址只存在于用例自建的两个网络命名空间里（P17）。
- CLAUDE.md 的「测试约定」整节适用。
- 门禁、本机的 rustfmt、只经整个工作区跑用例（不用 `-p`）、跑全量前 `df -h /d`、`STATUS_HEAP_CORRUPTION` 与增量编译内部错误的处理：同 M4a 计划的「全局约束」。

**不做的事**

- 只运行本仓库构建出来的二进制，与系统自带的只读工具。
- 不下载，不安装，也不 `rustup target add`。
- **不在开发机上执行 `ip`、`resolvectl`、`networksetup`、`ifconfig`、`route`、`netsh` 这类读写网络的命令**，也不读真实的网络环境。

**参考实现**

- GPL 的（sing-tun、sing-box、wg-quick、OpenVPN）只取事实，代码不抄。
- MIT 的（wireguard-go）可以简短引用，注明出处。

**写法**

- 注释、日志与错误文本用英文。
- 提交说明照 `git log` 的样子，标题用中文。

**计划没有写到、要做决定的地方**

- 能按设计与总设计的原则定的：自己定，记进修正记录，写"执行者的裁定"。
- 去向是项目所有者的（设计第 20 节），或会改变 M4-D1 ～ D30 的（P3 订正的 M4-D3 那一处除外）：停下来，列出来。

## 评审要点

这一份做完，Linux 与 macOS 的使用者第一次能打开增强模式。这也是第一次真的改使用者的系统。最可能咬到使用者的六种情形，与负责它们的用例：

1. **macOS 的 DNS 在崩溃之后回不去。**
   - 做法：每个服务先读原值、写进备份，再改。撤销时按原值写回，原来没有手动设的写 `empty`。服务已经被删掉的，撤销算做完。
   - 用例：任务 5 的 `the_dns_of_each_enabled_service_is_read_before_it_is_set`、`undo_writes_back_the_servers_or_empty_and_flushes`、`undo_of_a_service_that_is_gone_is_done`。
   - 崩溃之后的整条路，已经由 M4a 经文件后端的端到端用例覆盖。
2. **严格的 `rp_filter`**（RHEL 一族）把 rurge 自己连接的回包丢掉。
   - 做法：主网卡的有效值是 1 时放宽 `all`，关闭时写回。
   - 用例：任务 4 的 `strict_rp_filter_is_loosened_and_put_back` 一族。
3. **另一个全隧道 VPN 开着。**
   - 做法：Linux 不再用 `route replace`。那会悄悄换掉 VPN 同样前缀的路由，关闭时还把它删掉。现在两个平台上，同样的前缀已经在时，打开失败并说清楚原因。
   - 用例：任务 2 的 `linux_adds_routes_and_never_replaces_them`；决定见 P3。
4. **一个挂住的工具**（比如 D-Bus 卡住的 `resolvectl`）。
   - 做法：每个程序 30 秒为限（任务 2 的 `TimedRunner`）。
5. **权限不够。**
   - 做法：给一句说明，而不是一个裸的 `EPERM`。
   - 用例：任务 3 的 `open_errors_say_what_is_missing`。
6. **只有 CI 编译的代码坏了，却读不到错误。**
   - 做法：clippy 一步的错误也发成注解（任务 1）；任务 10 读回 CI，修到两个平台都绿。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `.github/workflows/ci.yml`、`.github/scripts/report-test-failures.py` | 1 | clippy 的输出也经报告脚本 |
| `crates/rurge-platform/src/command.rs`、`src/route/{exec.rs,linux.rs,macos.rs,mod.rs}` | 2 | `TimedRunner`；步骤的执行；翻译的四处订正 |
| `crates/rurge-platform/Cargo.toml`、`src/lib.rs`、`src/tun/{mod.rs,frame.rs,unsupported.rs}`、`src/enhanced/mod.rs` | 3 | 设备的接口与帧的处理；系统一侧的接口、`Unsupported`、`system()` |
| `crates/rurge-platform/src/enhanced/linux.rs` | 4 | Linux 的系统一侧（不带 `cfg`） |
| `crates/rurge-platform/src/enhanced/macos.rs` | 5 | macOS 的系统一侧与输出的解析（不带 `cfg`） |
| `crates/rurge-platform/src/tun/linux.rs`、`tests/unsafe_functions.rs`、`CLAUDE.md` | 6 | Linux 的设备、`set_iff` |
| `crates/rurge-platform/src/tun/macos.rs`、`tests/unsafe_functions.rs`、`crates/rurge/tests/sockets.rs`、`CLAUDE.md` | 7 | macOS 的设备、`utun_connect` |
| `crates/rurge/src/cli/enhanced/{backend.rs,mod.rs,file.rs,device.rs}`、`src/cli/run.rs` | 8 | 平台后端接上；`physical_dns`；物理网卡的服务器写进 `PlatformSystemDns` |
| 工作区 `Cargo.toml`、`tests/privileged/`（新） | 9 | L2 |
| 本计划末尾两张表 | 10 | 等 CI |
| `docs/api/phase3.md`、`docs/testing.md`、`docs/status.md`、兼容性清单、本计划末尾两张表 | 11 | 文档 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的 `main` 核对下面每一项。

- 有出入的，记进「执行期修正记录」，一项一行，写"计划写 X，代码是 Y，按 Y 做"；按记录调整后面的任务。
- **不改计划的正文。**
- 没有出入也记一行。

1. **工作树与前两份计划**
   - `git status` 干净；M4a、M4b 的提交都在。
   - 读两份计划末尾的两张表。凡是改了本计划要用的名字的，以那两张表为准。本计划要用的名字：
     - `route::{Step, OnFailure, SCRIPT, RouteSet}`，与 `RouteSet` 的字段；
     - `route::linux::{LinuxTools, configure, configure_ipv6, unconfigure_ipv6, add_routes, remove_routes, take_dns, give_dns_back}`；
     - `route::macos` 的各个函数、`set_dns`、`flush_dns`，与程序路径的常量；
     - `route::SPACE_V4` / `SPACE_V6`；
     - M4b 在 `dns.rs` 里"去掉 rurge 空间"的那个函数；
     - 可执行文件的 `Backend`（含 M4b 加的 `subnets`）、`Opened`、`Undo` 与 `Undo::stage`；
     - `backend()` 的位置与签名；
     - 文件后端的键；
     - `EnhancedMode` 的构造：M4b 怎样把增强模式的标志、排除名单与 `system_dns_changed` 交给它；
     - `PlatformSystemDns` 的 `physical` 字段的类型；
     - `crates/rurge/tests/sockets.rs` 的 `LISTED` 的形状；
     - `unsafe_functions.rs` 的 `LISTED` 现在有几项。
2. **会真的开设备的用例**（P19）
   - 跑 `git grep -n "not_available\|RURGE_ENHANCED_BACKEND\|env_remove(\"RURGE_ENHANCED" crates tests`。
   - 找出所有在不设 `RURGE_ENHANCED_BACKEND`（或设成空）时会走到平台后端的用例，尤其是 M4a 的 `the_platform_backend_is_not_available_in_this_version`（任务 8 删掉它）。
   - 全部列进记录。
3. **V2：命令的写法**
   - 网络可达时，对着手册页把 P3 ～ P8 与 P20 的各条再核一遍：ip(8)、resolvectl(1)、systemd-resolved(8)、networksetup(8)，以及 macOS 的 ifconfig(8)、route(8)、ipconfig(8)。
   - 不可达时，照 P20 记的出处，并记一行"没有再核"。
   - 手册页与计划不一致：改翻译与它的用例，记进记录。
4. **V4：符号**（离线读本机 `~/.cargo/registry/src/index.crates.io-*/` 下的源码。写计划时都核过，见 P20）
   - libc 0.2.189，Linux，在 `src/unix/linux_like/linux_l4re_shared.rs`：
     - `TUNSETIFF: Ioctl`；`IFF_TUN` / `IFF_NO_PI: c_int`；`IFNAMSIZ`；
     - `ifreq { ifr_name: [c_char; IFNAMSIZ], ifr_ifru: __c_anonymous_ifr_ifru }`，联合体里有 `ifru_flags: c_short`；
     - `ENOBUFS`、`EMSGSIZE`、`EINVAL`。
   - libc 0.2.189，macOS，在 `src/unix/bsd/apple/mod.rs`：
     - `ctl_info { ctl_id: u32, ctl_name: [c_char; MAX_KCTL_NAME] }`；
     - `sockaddr_ctl { sc_len, sc_family, ss_sysaddr, sc_id, sc_unit, sc_reserved: [u32; 5] }`；
     - `CTLIOCGINFO: c_ulong`、`AF_SYSTEM`（32）、`AF_SYS_CONTROL`（2）、`SYSPROTO_CONTROL`（2）、`UTUN_OPT_IFNAME`（2）、`MAX_KCTL_NAME`（96）。
   - nix 0.31.3：`pub use libc;`。
   - socket2 0.6.5：`Domain::from(c_int)`、`Protocol::from(c_int)`、`Socket::send_vectored`。
   - tokio 1.53：`AsyncFd`（`net` 特性，Unix）。
5. **`command.rs`**：`CommandRunner`、`SystemRunner`、`failure`、`run_with_timeout`，以及测试里的 `FakeRunner`（按空格把命令连起来，作为记录与应答的键）。
6. **CI**
   - `.github/workflows/ci.yml` 里的 clippy 一步：今天是一行没有名字的 `- run: cargo clippy --all-targets -- -D warnings`。
   - 同一个文件里的 "Report the failed tests" 一步。
   - `.github/scripts/report-test-failures.py` 的用法：只读一个日志文件；编译错误只认第一个测试二进制之前的 `error…:` 行。
7. **门禁**：跑一遍，记下结果行数、通过数与忽略数。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。

---

## Task 1：clippy 的错误也公开可读（CI）

**改哪里**（P18）

1. `.github/workflows/ci.yml` 的 `check` 任务，给 clippy 一步起名字，并把输出也写进文件：

   ```yaml
         - run: cargo fmt --all --check
         # Also into a file: an error in code only CI compiles (Linux, macOS) is
         # repeated as an annotation, which can be read without signing in.
         - name: Run clippy
           id: clippy
           shell: bash
           run: |
             set -o pipefail
             cargo clippy --all-targets -- -D warnings 2>&1 | tee "$RUNNER_TEMP/clippy.log"
   ```

2. "Report the failed tests"一步：clippy 失败时读 `clippy.log`，否则读 `test.log`：

   ```yaml
         - name: Report the failed tests
           if: failure()
           shell: bash
           env:
             CLIPPY: ${{ steps.clippy.outcome }}
           run: |
             if [ "$RUNNER_OS" = "Windows" ]; then python=python; else python=python3; fi
             log="$RUNNER_TEMP/test.log"
             if [ "$CLIPPY" = "failure" ]; then log="$RUNNER_TEMP/clippy.log"; fi
             "$python" .github/scripts/report-test-failures.py "$log"
   ```

3. 脚本开头的说明加一句：clippy 的日志同样读——那里只有编译错误与 lint，都出现在第一个测试二进制之前。

别的步骤不动。

**用例**：这个任务没有 Rust 的用例。

- 本机有 `python` 时，用一份手写的日志跑一遍脚本。日志的内容：几行 `   Compiling …`；一段 `error[E0425]: cannot find value \`x\` in this scope`，带它下面 `--> crates/…` 的几行；一行 `error: could not compile …`。期望输出里有 `::error title=compile error::`。
- 本机没有 `python`，就记一行。

**判据**：

- `git diff .github/` 只有这两处改动与那句说明。
- YAML 的缩进照文件原样（两个空格）。
- 门禁绿。这个任务不碰 Rust，跑一次只是确认没有弄坏。
- 提交 `ci: …`。

---

## Task 2：步骤的执行与 `TimedRunner`；翻译的四处订正（`rurge-platform`）

**改哪里**（设计第 5.1 节、M4-D4；P3 ～ P6、P11、P12）

1. `command.rs` 加：

   ```rust
   /// A [`CommandRunner`] that gives up on a program after the time it is given
   /// ([`run_with_timeout`]): a tool that hangs does not hold up its caller.
   pub struct TimedRunner(pub Duration);
   ```

2. `src/route/exec.rs`（新文件，不带 `cfg`）：

   ```rust
   /// Runs `steps` in order (M4 design 5.1). A step's script is written to a
   /// file of its own under `scratch` (made when missing; `create_new`, named
   /// `rurge-steps-<pid>-<n>.txt`), whose path takes the place of [`SCRIPT`] in
   /// the arguments; the file is removed once the program has run.
   ///
   /// A step that fails: `Fail` ends the run with its error (the steps after it
   /// do not run); `Ipv6Off` warns once and skips the other `Ipv6Off` steps of
   /// this run, which goes on; `Ignore` is a debug line, and the run goes on.
   /// `Ok(false)`: an `Ipv6Off` step failed.
   pub fn run(steps: &[Step], runner: &dyn CommandRunner, scratch: &Path) -> Result<bool, String>;

   /// Removes the scripts a run that died left under `scratch`.
   pub fn remove_leftovers(scratch: &Path);
   ```

   - 每一步之前写一行 DEBUG：程序与参数；有脚本时连同脚本的内容。
   - `Ipv6Off` 的 WARN 是 `IPv6 could not be set up on the virtual adapter; the enhanced mode goes on without IPv6`。不带名字与前缀；工具的原文在 DEBUG。
   - 脚本文件写不进去：这一步按它自己的 `OnFailure` 算失败，错误是 `cannot write a script for <程序的文件名>: <原因>`。

3. 订正 M4a 写的四处翻译，连同它们的逐字用例。提交说明里点名这四处：
   - `route::linux::add_routes`：每行改成 `route add P dev D`，不再是 `route replace`（P3）。
   - `route::linux::take_dns`：`resolvectl default-route D yes` 一步的 `OnFailure` 改成 `Ignore`（P4）；其余三步照旧 `Fail`。
   - `route::macos::configure_ipv6`：改成 `ifconfig D inet6 fd00:6152::1 prefixlen 128 alias`（P5）。
     - `fd00:6152::/64` 的路由由 `add_routes` 显式装：M4a 的 `RouteInputs::explicit_space` 在 macOS 上为真时，这条已在路由集里。
     - 任务 0 核对 M4a 的 `RouteSet::compute` 在 `explicit_space` 下给不给 IPv6 那一条。不给的话，在这里补上，记进修正记录。
   - `route::macos::set_dns`：原值是空的时写 `empty`，小写（P6）。

**测试里的执行器**：`FakeRunner` 看不到脚本的内容（P2），所以 `exec.rs` 的 `tests` 里另写一个执行器。它：

- 记下每次执行的命令；
- 按"程序的文件名，加上脚本里有没有某段文字"决定这一步成败；
- 在执行的那一刻读出脚本文件的内容。

**用例**（`exec.rs` 的 `tests`）

- `a_script_is_written_to_a_file_of_its_own_and_removed_after`：执行时文件在，内容与翻译给的逐字相同；跑完之后文件不在了；参数里的 `SCRIPT` 换成了那个路径。
- `the_first_failing_step_ends_the_run`：第二步是 `Fail` 并且失败：回它的错误，第三步没有执行。
- `an_ipv6_failure_skips_the_other_ipv6_steps_and_goes_on`：三步依次是 `Ipv6Off`、`Ipv6Off`、`Fail`（IPv4），第一步失败：第二步没有执行，第三步执行了，回 `Ok(false)`。
- `ignored_failures_go_on`：回 `Ok(true)`。
- `a_script_that_cannot_be_written_fails_its_step`：`scratch` 是一个文件，不是目录。
- `leftover_scripts_are_removed`：`scratch` 里有两个 `rurge-steps-*.txt` 与一个别的文件，只有前两个被删掉。
- 翻译的四处：改 M4a 的逐字用例；另加 `linux_adds_routes_and_never_replaces_them`。

**判据**：

- `exec.rs` 里没有 `cfg`。
- `git grep -n "route replace" crates/rurge-platform/src` 在用例之外什么也没有。
- 门禁绿。

---

## Task 3：设备与系统一侧的接口（`rurge-platform`）

**改哪里**（设计第 3.1、8.1、8.5 节；P2、P14 ～ P16）

1. **`Cargo.toml` 与 `lib.rs`**
   - `[dependencies]` 加 `tokio.workspace = true`。
   - `description` 加上 "the enhanced mode's virtual adapter, routes and DNS"。
   - `[lints]` 上面讲 unsafe 的那段说明，加上 TUN 设备（任务 6、7 的两个函数）。
   - `src/lib.rs` 加 `pub mod enhanced; pub mod tun;`。

2. **`src/tun/mod.rs`**：

   ```rust
   /// What to open.
   pub struct DeviceSpec {
       /// Linux: the interface's name. macOS: unused (the kernel picks `utunN`).
       pub name: String,
       pub mtu: usize,
   }

   /// The virtual adapter (M4 design 8.1): packets in and out, without offloads.
   pub struct TunDevice { /* imp, adapter, mtu, dropped: AtomicU64, warned: AtomicBool */ }

   impl TunDevice {
       /// Opens the adapter. Inside a tokio runtime (a `spawn_blocking` thread is):
       /// the descriptor is registered with its reactor. Only the platform's
       /// `System::open` calls it, and no test does (M4c plan P19).
       pub fn open(spec: &DeviceSpec) -> io::Result<TunDevice>;
       pub fn adapter(&self) -> &Adapter;
       pub fn mtu(&self) -> usize;
       /// Waits for at least one packet, then moves up to `max` without waiting.
       /// Each packet is allocated at its own length (`rurge_tun::Device`'s contract).
       pub async fn recv(&self, into: &mut Vec<Vec<u8>>, max: usize) -> io::Result<()>;
       /// Writes `packets` in order and leaves it empty. A packet the system has
       /// no room for is dropped and counted (a warning the first time, debug lines after).
       pub async fn send(&self, packets: &mut Vec<Vec<u8>>) -> io::Result<()>;
       /// The packets `send` dropped.
       pub fn dropped(&self) -> u64;
   }
   ```

   - 平台的实现：
     - `#[cfg(target_os = "linux")] mod linux;`
     - `#[cfg(target_os = "macos")] mod macos;`
     - 其余平台用 `mod unsupported;`：一个 `enum Device {}`，`open` 回 `io::Error::new(ErrorKind::Unsupported, enhanced::NOT_AVAILABLE)`。
   - **这一任务里，Linux 与 macOS 也先用 `unsupported`**，任务 6、7 再换掉。这样每个提交在 CI 的三个平台上都能编译。
   - `mod.rs` 定下各实现之间的约定：`open`、读一个包、写一个包、哪些错误算"没有地方"。
   - 丢包的计数与第一次的 WARN 在 `mod.rs` 里做，不带 `cfg`。

3. **`src/tun/frame.rs`**：`pub mod frame`，不带 `cfg`。只在一个平台上用到的项也是 `pub`（P2）。

   ```rust
   /// `name` as `ifreq`'s name: 1 to 15 bytes, no `/`, `:`, whitespace or NUL, NUL-padded.
   pub fn ifname(name: &str) -> io::Result<[u8; 16]>;
   /// The name in a NUL-terminated buffer the kernel filled in.
   pub fn name_of(buf: &[u8]) -> String;
   /// Darwin's address families, which utun puts before each packet in network order.
   pub const DARWIN_AF_INET: u32 = 2;
   pub const DARWIN_AF_INET6: u32 = 30;
   /// The four bytes utun wants before `packet`, from its IP version; `None` for neither 4 nor 6.
   pub fn utun_header(packet: &[u8]) -> Option<[u8; 4]>;
   /// The IP packet in what a utun socket gave: after a known family, not empty.
   pub fn utun_packet(datagram: &[u8]) -> Option<&[u8]>;
   /// What an error of opening the adapter means to the user (design 8.5).
   pub fn explained(err: io::Error, os: Os) -> io::Error;
   pub enum Os { Linux, MacOs }
   ```

   `explained` 的对照（错误的类别保留不变）：

   | 平台 | 错误类别 | 说明 |
   | --- | --- | --- |
   | Linux | `PermissionDenied` | `the enhanced mode needs root or CAP_NET_ADMIN` |
   | Linux | `NotFound` | `there is no /dev/net/tun: the kernel's tun module is not loaded` |
   | Linux | `ResourceBusy` | `the virtual adapter is in use: is another rurge running with the enhanced mode on?` |
   | macOS | `PermissionDenied` | `the enhanced mode needs root` |
   | 其余 | 其余 | `cannot open the virtual adapter: <原因>` |

4. **`src/enhanced/mod.rs`**：定义 `Adapter`、`Saved`、`NOT_AVAILABLE`、`System`、`Unsupported`、`system(scratch)`。接口照上面「架构」一段与 P2；方法的说明照 M4a 的 `Backend` 的写法。

   ```rust
   /// How the steps name the virtual adapter.
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct Adapter {
       pub name: String,
       /// Windows: the interface index (M4d).
       pub index: Option<u32>,
   }

   /// A change that outlives rurge, and what putting it back takes (design 9.2).
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub enum Saved {
       /// macOS: a network service's DNS servers before the takeover (empty: none were set).
       MacDns { service: String, servers: Vec<IpAddr> },
       /// Linux: `net.ipv4.conf.all.rp_filter` before it was loosened.
       RpFilter { value: u8 },
   }

   pub const NOT_AVAILABLE: &str = "the enhanced mode is not available on this platform in this version";
   /// Each program the steps run gets this long (M4c plan P11).
   pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);

   /// The system half of the enhanced mode (M4 design 5, 6, 7.3, 8). Every method
   /// blocks (programs, files) and is called on a blocking thread; an error is
   /// English text for the log and the API's 500, without the names of the
   /// user's networks (design 13).
   pub trait System: Send + Sync {
       fn open(&self) -> io::Result<TunDevice>;
       /// The address, MTU and link, and with `ipv6` the IPv6 address. `Ok(false)`:
       /// IPv6 was asked for and could not be had (warned); `Ok(true)` otherwise.
       fn configure(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String>;
       fn configure_ipv6(&self, dev: &Adapter) -> Result<bool, String>;
       fn unconfigure_ipv6(&self, dev: &Adapter) -> Result<(), String>;
       /// Adds `add`, then removes `remove` (going on past errors). `Ok(false)`:
       /// the IPv6 half of `add` could not be added; what of it was is removed again.
       fn routes(&self, dev: &Adapter, add: &RouteSet, remove: &RouteSet) -> Result<bool, String>;
       /// What `take_dns` will change that outlives rurge and is not in `saved`,
       /// read before it is changed.
       fn dns_saved(&self, dev: &Adapter, saved: &[Saved]) -> Result<Vec<Saved>, String>;
       /// `Ok(false)`: not on this system (warned, once an open).
       fn take_dns(&self, dev: &Adapter, ipv6: bool) -> Result<bool, String>;
       fn give_dns_back(&self, dev: &Adapter) -> Result<(), String>;
       fn adjust_saved(&self, primary: Option<&str>, saved: &[Saved]) -> Result<Option<Saved>, String>;
       fn adjust(&self) -> Result<(), String>;
       /// Puts one saved thing back: on a close, and after a crash. A kind this
       /// platform never saves is dropped with a warning (M4c plan P9).
       fn undo(&self, saved: &Saved) -> Result<(), String>;
       /// The DNS servers of the network outside the enhanced mode, for
       /// `server:system` while it is on (design 6.3). `None`: the system's own
       /// list is right (no takeover).
       fn physical_dns(&self, primary: Option<&str>, saved: &[Saved], network_changed: bool) -> Option<Vec<IpAddr>>;
   }

   /// This platform's (Linux and macOS: M4c; Windows: M4d); [`Unsupported`] elsewhere.
   /// Scripts go to `scratch` (the data directory's `enhanced`).
   pub fn system(scratch: PathBuf) -> Box<dyn System>;
   ```

   `Unsupported` 的行为：

   - 每个方法回 `Err(NOT_AVAILABLE)`；`open` 回 `Unsupported` 类别的 `io::Error`。
   - `physical_dns` 回 `None`。
   - `undo` 回 `Err(NOT_AVAILABLE)`。
   - 这一任务里 `system()` 在每个平台上都回 `Unsupported`；任务 6、7 换掉 Linux 与 macOS。

**用例**

- `frame.rs`：
  - `ifname_takes_up_to_fifteen_bytes`：`rurge` 得到前 5 个字节与 NUL；16 个字节的名字、空的名字、带 `/`、`:`、空格或 NUL 的名字，都得到 `InvalidInput`。
  - `name_of_stops_at_the_first_nul`。
  - `utun_header_follows_the_ip_version`：`0x45…` 得到 `[0,0,0,2]`；`0x60…` 得到 `[0,0,0,30]`；空的、`0x00` 开头的得到 `None`。
  - `utun_packet_strips_a_known_family`：家族是 2 或 30 时得到后面的部分；家族是 1、或者不足 5 个字节时得到 `None`。
  - `open_errors_say_what_is_missing`：两个平台的每一类各一条，错误的类别不变。
- `enhanced/mod.rs`：
  - `the_unsupported_system_says_so`：直接建 `Unsupported`，不经 `system()`。每个方法的错误是 `NOT_AVAILABLE`；`physical_dns` 是 `None`。

**判据**：

- `frame.rs` 与 `enhanced/mod.rs` 里没有 `cfg`。
- `git grep -n "TunDevice::open" crates` 只出现在 `enhanced/` 的生产代码里（这时还没有）与说明里。
- 门禁绿。

---

## Task 4：Linux 的系统一侧（`enhanced/linux.rs`，不带 `cfg`）

**改哪里**（设计第 5.2、6.1、6.3、7.3 节；M4-D6、D10；P2 ～ P4、P9、P10）

1. **找工具**：`route::linux::LinuxTools` 加

   ```rust
   /// `ip` and `resolvectl`, each the first of `/usr/sbin`, `/sbin`, `/usr/bin`,
   /// `/bin` (under `root`) that has it. Without `ip` there is no enhanced mode.
   pub fn find(root: &Path) -> Result<LinuxTools, String>;
   ```

   - 找不到 `ip`：错误是 `cannot find the ip program (iproute2) in /usr/sbin, /sbin, /usr/bin or /bin`。
   - 回的路径是 `/usr/sbin/ip` 这样的绝对路径。`root` 只用来看文件在不在；生产里 `root` 是 `/`。

2. **认存根**（纯函数）：

   ```rust
   /// Whether `resolv.conf` is systemd-resolved's stub: its only server is 127.0.0.53.
   pub fn is_resolved_stub(resolv_conf: &str) -> bool;
   ```

3. **`rp_filter`**（根目录参数化）
   - 文件是 `/proc/sys/net/ipv4/conf/<名字>/rp_filter`。名字直接作路径的一段，不经 `sysctl`，所以名字里的点不用换写法。
   - 读两个值：`all` 的与主网卡的。主网卡的名字是空的、`.`、`..`，或者含 `/` 时，是错误 `not an interface name`。
   - 有效值是两者中较大的那个。内核文档 ip-sysctl："The max value from conf/{all,interface}/rp_filter is used"。
   - 有效值是 1 时放宽：`all` 写 `2`；要记下的旧值是 `all` 原来的值。

4. **`LinuxSystem`**：

   ```rust
   pub struct LinuxSystem { /* runner: Arc<dyn CommandRunner>, root: PathBuf, scratch: PathBuf,
                               tools: Result<LinuxTools, String>, took_dns: AtomicBool, warned: AtomicBool */ }
   impl LinuxSystem {
       /// Never fails: a missing `ip` is the error of `open` (rurge runs without the enhanced mode).
       pub fn new(runner: Arc<dyn CommandRunner>, root: PathBuf, scratch: PathBuf) -> LinuxSystem;
   }
   ```

   `impl System` 的各个方法：

   - **`open`**
     1. `tools` 是错误就回它——**在碰设备之前**。
     2. `exec::remove_leftovers(scratch)`。
     3. `TunDevice::open(&DeviceSpec { name: "rurge", mtu: 1500 })`。
   - **`configure` / `configure_ipv6` / `unconfigure_ipv6` / `routes`**：M4a 的翻译加上 `exec::run`。
     - `routes` 的 IPv6 一半失败时：再跑一遍只含那一半的 `remove_routes`（`Ignore`），回 `Ok(false)`；然后照常减掉 `remove`。
   - **`dns_saved`**：回 `Ok(vec![])`。resolved 的逐链路设置随网卡消失，不进备份。
   - **`take_dns`**
     - `root/etc/resolv.conf` 是存根，并且找得到 `resolvectl`：执行 `route::linux::take_dns` 与 `exec::run`，置 `took_dns = true`，回 `Ok(true)`。
     - 否则：WARN，回 `Ok(false)`。WARN 每次打开至多一行（靠 `warned`），内容是 `the system's DNS is not systemd-resolved's stub; it is left as it is: write hijack-dns = *:53, or point the system's DNS at 198.18.0.2`。
     - 读不了 `resolv.conf`，同样当作不是存根；原因在 DEBUG。
   - **`give_dns_back`**：`took_dns` 时执行 `route::linux::give_dns_back` 与 `exec::run`（`Ignore`），置 `took_dns = false`；`warned` 复位。
   - **`adjust_saved`**

     | 情形 | 回 |
     | --- | --- |
     | `saved` 里已有 `RpFilter` | `None` |
     | `primary` 是 `None` | `None` |
     | 有效值是 1 | `Some(RpFilter { value: all 原来的值 })` |
     | 其余 | `None` |
     | 读不了：不是网卡名、文件不在、内容不是 0 ～ 2 的数 | `Err(cannot read rp_filter: <原因>)` |

   - **`adjust`**：`all` 写 `2`。INFO：`loosened net.ipv4.conf.all.rp_filter to 2: replies to rurge's own connections come in on another interface than the routes to their senders`。
   - **`undo`**
     - `RpFilter { value }`：`all` 写回 `value`。INFO：`put net.ipv4.conf.all.rp_filter back to <value>`。
     - `MacDns`：WARN `an undo of another platform is dropped`，回 `Ok(())`（P9）。
   - **`physical_dns`**
     - `took_dns` 时：读 `root/run/systemd/resolve/resolv.conf`（resolved 写的上游名单），用 `dns::parse_resolv_conf` 解析，去掉 rurge 的空间（用 M4b 的函数），回 `Some`。读不了就回 `Some(vec![])`，原因在 DEBUG。
     - 没有接管：回 `None`。

**用例**：根目录用临时目录；执行器用 `FakeRunner` 或任务 2 的测试执行器。**没有一条用例在找得到 `ip` 时调 `open`。**

- 找工具与 `open`：
  - `tools_are_looked_for_in_the_system_directories_in_order`：`/sbin/ip` 与 `/bin/ip` 都在，得到 `/sbin/ip`；`resolvectl` 只在 `/usr/bin`，得到它；都不在，得到 `ip` 的那句错误。
  - `without_ip_the_open_fails_before_the_device`：`open` 回那句错误。
- 认存根：
  - `the_stub_is_resolved_s_and_nothing_else`：`nameserver 127.0.0.53` 加 `search`、`options` 行，为真；以下为假：`127.0.0.53` 与 `8.8.8.8` 同在、只有 `127.0.0.54`、只有 `192.168.1.1`、空文件。
- 接管与交还 DNS：
  - `the_dns_is_taken_behind_the_stub_with_resolvectl`：四条命令逐字比对；`ipv6` 时 `dns` 那条多一个 `fd00:6152::2`。
  - `without_the_stub_the_dns_is_left_and_said_once`：两次 `take_dns` 只有一行 WARN（用 `LogCapture` 抓）。
  - `without_resolvectl_the_dns_is_left`。
  - `giving_back_reverts_only_what_was_taken`：没有接管时，不执行 `revert`。
- `rp_filter`：
  - `strict_rp_filter_is_loosened_and_put_back`：`all = 0`、主网卡 `1`，得到 `Saved::RpFilter { value: 0 }`；`adjust` 之后 `all` 是 `2`；`undo` 之后是 `0`。
  - `rp_filter_that_is_loose_or_off_is_left_alone`：`0 / 0`、`2 / 1`、`0 / 2` 得到 `None`；`1 / 0` 得到 `Some(1)`。
  - `a_saved_rp_filter_is_not_read_again`。
  - `an_interface_name_that_is_not_a_name_is_refused`。
- 物理网卡的服务器：
  - `the_physical_servers_are_resolved_s_upstreams_without_rurges_space`：文件里有 `198.18.0.2`、`192.168.1.1`、`fd00:6152::2`，只留下 `192.168.1.1`。
  - `without_the_takeover_the_system_list_stands`：回 `None`。
- 配置与路由：
  - `configure_with_ipv6_runs_two_batches_and_an_ipv6_failure_is_ok_false`。
  - `an_ipv6_route_failure_takes_the_ipv6_routes_out_again`。
  - `an_ipv4_route_failure_is_the_error`。
- 撤销：
  - `an_undo_of_another_platform_is_dropped`。

**判据**：

- `enhanced/linux.rs` 里没有 `cfg`。
- 这些用例在本机都跑了（`cargo test -j 8 --workspace --lib -- enhanced::`）。
- 门禁绿。

---

## Task 5：macOS 的系统一侧（`enhanced/macos.rs`，不带 `cfg`）

**改哪里**（设计第 5.3、6.1、6.3 节；M4-D6；P5 ～ P10、P20）

1. **输出的解析**（纯函数）。夹具按 P20 记的出处手写，用例的注释写明 "not taken from a Mac"。

   ```rust
   /// `networksetup -listallnetworkservices`: the services, without the line that
   /// explains the asterisk; a disabled one (`*Name`) only when `disabled`, without the `*`.
   pub fn services(text: &str, disabled: bool) -> Vec<String>;
   /// `networksetup -getdnsservers <service>`: the servers set by hand; empty for
   /// "There aren't any DNS Servers set on <service>."; `None` for anything else
   /// (a line that is not an address — a scoped one too).
   pub fn dns_servers(text: &str) -> Option<Vec<IpAddr>>;
   /// `networksetup -listnetworkserviceorder`: each service with its device
   /// (`(1) Wi-Fi` then `(Hardware Port: Wi-Fi, Device: en0)`; `(*)` for a disabled one);
   /// a service without a device is left out.
   pub fn service_devices(text: &str) -> Vec<(String, String)>;
   /// `ipconfig getpacket <device>`: `domain_name_server (ip_mult): {a, b}`; empty when
   /// there is none (the tool prints nothing without DHCP).
   pub fn dhcp_dns(text: &str) -> Vec<IpAddr>;
   ```

   解析的细节：

   - 解释星号的那一行，按"含 `denotes that a network service is disabled`"来认，不按行号。
   - 服务名里的空格原样保留。
   - `service_devices` 取最后一个 `, Device: ` 之后、`)` 之前的部分。

2. **`MacSystem`**：

   ```rust
   pub struct MacSystem { /* runner: Arc<dyn CommandRunner>, resolv_conf: PathBuf, scratch: PathBuf,
                             state: Mutex<MacState { to_take: Vec<String>, before: Option<Vec<IpAddr>> }> */ }
   impl MacSystem {
       pub fn new(runner: Arc<dyn CommandRunner>, resolv_conf: PathBuf, scratch: PathBuf) -> MacSystem;
   }
   ```

   - 程序路径照 M4a 的常量，另加 `IPCONFIG = "/usr/sbin/ipconfig"`。
   - 锁只在同步代码里持有（这里没有 `await`）。

   `impl System` 的各个方法：

   - **`open`**：`TunDevice::open(&DeviceSpec { name: "utun".into(), mtu: 1500 })`。名字由内核挑。
   - **`configure` / `configure_ipv6` / `unconfigure_ipv6` / `routes`**：M4a 的翻译（任务 2 订正过的）加上 `exec::run`。`routes` 的 IPv6 一半失败时，处理同 Linux。
   - **`dns_saved(dev, saved)`**
     1. 执行 `networksetup -listallnetworkservices`，得到启用的服务。失败时回 `Err(cannot list the network services: <原文>)`——这条命令里不带服务名。
     2. `saved` 里没有任何 `MacDns`，说明这是这一次打开的第一次接管：读 `resolv_conf` 里的服务器，去掉 rurge 的空间，存进 `before`。
     3. 对每个启用的、不在 `saved` 里的服务，执行 `networksetup -getdnsservers <服务>`，交给 `dns_servers`：
        - 得到 `Some(列表)`：记一个 `Saved::MacDns`；
        - 命令失败，或得到 `None`：WARN `the DNS servers of a network service could not be read; its DNS is left as it is`（服务名与原文在 DEBUG），这个服务不接管。
     4. `to_take` 设为：启用的、并且在 `saved` 里的服务，加上第 3 步新记的服务。
     5. 回第 3 步新记的那些。
   - **`take_dns(dev, ipv6)`**
     - `to_take` 是空的：WARN `no network service's DNS could be pointed at the enhanced mode`（每次打开至多一行），回 `Ok(false)`。
     - 否则：逐个服务执行 `set_dns(服务, [198.18.0.2(, fd00:6152::2)])`，**直接经执行器，不经 `exec::run`**（P7）。
       - 失败：原文写进 DEBUG，回 `Err(cannot point a network service's DNS at the enhanced mode (the debug log has the details))`。
       - 全部成功：执行 `flush_dns`（经 `exec::run`，`Ignore`），回 `Ok(true)`。
   - **`give_dns_back`**：回 `Ok(())`；`warned` 复位。macOS 上会留下的改动都经 `undo` 写回。
   - **`adjust_saved`** 回 `Ok(None)`；**`adjust`** 回 `Ok(())`。
   - **`undo(MacDns { service, servers })`**
     1. 列出全部服务，启用的与停用的都要。失败就回 `Err`：这条撤销留在备份里，下次再试。
     2. 服务已经不在了：INFO `a network service the enhanced mode changed is gone; there is nothing to put back for it`（服务名在 DEBUG），回 `Ok(())`。
     3. 否则执行 `set_dns(服务, servers)`，`servers` 是空的就写 `empty`。
        - 失败：原文写进 DEBUG，回 `Err(cannot put back the DNS servers of a network service (the debug log has the details))`。
        - 成功：执行 `flush_dns`（`Ignore`）。
   - **`undo(RpFilter)`**：照 Linux 对 `MacDns` 的写法（P9）。
   - **`physical_dns(primary, saved, network_changed)`**（设计第 6.3 节，P10）
     - `!network_changed` 且 `before` 有值：回 `Some(before)`。
     - 否则，按下面的次序找：
       1. `primary` 是 `None`：回 `Some(vec![])`。
       2. 执行 `networksetup -listnetworkserviceorder`，交给 `service_devices`，找出主网卡所在的服务。
       3. `saved` 里那个服务的 `MacDns` 的 `servers` 不是空的：回它。
       4. 否则执行 `ipconfig getpacket <主网卡>`，交给 `dhcp_dns`，有就回它。
       5. 都没有：回 `Some(vec![])`。这时 `server:system` 查找失败，不退回系统解析库。
     - 命令失败都当作"没有"，原因在 DEBUG。
     - `network_changed` 为真时清掉 `before`：它只在打开之后、第一次网络变化之前是对的。

**用例**：执行器用 `FakeRunner`；`resolv_conf` 放在临时目录里。

- 解析：
  - `enabled_services_skip_the_heading_and_the_disabled`。
  - `all_services_keep_the_disabled_without_the_asterisk`：含一个带空格的名字 `USB 10/100/1000 LAN`。
  - `dns_servers_read_a_list_and_the_none_set_sentence`：两行地址得到两个地址；`There aren't any DNS Servers set on Wi-Fi.` 得到空的；`fe80::1%en0` 得到 `None`；`Error: …` 得到 `None`。
  - `service_devices_pair_each_service_with_its_device`：一个启用的、一个 `(*)` 停用的；`Device: )` 是空的那个被跳过。
  - `dhcp_dns_reads_domain_name_server`：一个服务器、两个服务器（`{156.26.1.1, 156.26.1.30}`）、没有那一行、输出是空的。
- 接管：
  - `the_dns_of_each_enabled_service_is_read_before_it_is_set`：先 `dns_saved` 再 `take_dns`，命令的次序依次是：list、两个 get、两个 set、两次清缓存。回的 `Saved` 是两个服务的原值。
  - `services_saved_already_are_not_read_again_but_set_again`。
  - `a_service_that_cannot_be_read_is_left_alone`：有 WARN，不执行 `set`。
  - `no_service_to_take_is_ok_false_and_said_once`。
  - `setting_fails_with_a_text_that_names_no_service`：错误文本里没有服务名，DEBUG 里有。
- 撤销：
  - `undo_writes_back_the_servers_or_empty_and_flushes`：`networksetup -setdnsservers Wi-Fi 192.168.1.1 1.1.1.1` 与 `… Ethernet empty`，各接一次清缓存。
  - `undo_of_a_service_that_is_gone_is_done`。
  - `an_undo_that_cannot_list_the_services_stays`。
- 物理网卡的服务器：
  - `physical_at_open_is_resolv_conf_before_the_takeover`。
  - `physical_after_a_change_is_the_saved_servers_or_dhcp`：主网卡 `en0` 对应服务 `Wi-Fi`。原值非空，得到原值；原值是空的，得到 `getpacket en0` 的服务器；都没有，得到空的。
  - `physical_without_a_primary_is_empty`。

**判据**：

- `enhanced/macos.rs` 里没有 `cfg`。
- 夹具旁边注明出处（P20）。
- 门禁绿。

---

## Task 6：Linux 的设备（`tun/linux.rs`，`set_iff`）

**改哪里**（设计第 8.2 节、M4-D12；P14、P15）

1. `src/tun/linux.rs`，带 `#[cfg(target_os = "linux")]`。在 `tun/mod.rs` 里挂上，换掉 Linux 上的 `unsupported`。

   - **打开**
     1. `OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open("/dev/net/tun")`。std 在 Unix 上缺省带 `O_CLOEXEC`。错误经 `frame::explained(…, Os::Linux)`。
     2. `set_iff(fd, &frame::ifname(&spec.name)?)`，回内核给的名字。
     3. `AsyncFd::new(file)`。
   - **unsafe 函数**：

     ```rust
     // SAFETY 写明：fd 是打开着的 /dev/net/tun；req 是在调用期间活着的、名字以 NUL 结尾的 ifreq
     #[allow(unsafe_code)]
     fn set_iff(fd: RawFd, name: &[u8; 16]) -> io::Result<String>
     ```

     - 函数里建 `libc::ifreq { ifr_name, ifr_ifru: libc::__c_anonymous_ifr_ifru { ifru_flags: (libc::IFF_TUN | libc::IFF_NO_PI) as c_short } }`。建联合体的值不需要 unsafe，读它才需要。
     - 调一次 `libc::ioctl(fd, libc::TUNSETIFF, &mut req)`。
     - 失败：`io::Error::last_os_error()`，经 `explained`。
     - 成功：读回 `ifr_name`（用 `frame::name_of`）。
     - 网卡不持久：描述符关闭时，网卡连同它的地址、路由一起消失。
   - **收**：`readable().await`，然后在 `try_io` 里非阻塞地 `read`。
     - 缓冲 64 KiB（P14）；读到的包按自己的长度拷出。
     - 读到 0 个字节的跳过。
     - 遇到 `WouldBlock`：清掉就绪，再等。
   - **发**：逐个非阻塞地 `write`。
     - `WouldBlock`、`ENOBUFS`、`EMSGSIZE`、`EINVAL` 算"没有地方"：丢掉这个包，计数（P14）。
     - 其余错误让 TUN 结束。

2. `enhanced::system()` 在 Linux 上回 `LinuxSystem::new(Arc::new(TimedRunner(STEP_TIMEOUT)), "/".into(), scratch)`。

3. 名单：
   - `tests/unsafe_functions.rs` 的 `LISTED` 加 `("src/tun/linux.rs", "set_iff")`，按次序排。
   - `CLAUDE.md` 的 `unsafe_code` 一段加上这个函数，照那一段的写法写函数名、包的调用与阶段 3 / M4c。

**用例**：缺省的用例不开设备（P19）。`every_unsafe_exception_is_on_the_list` 读的是源码，所以在本机就能核这张名单。

**判据**：

- 本机门禁绿。
- 这个文件在本机编译不到，所以逐条自查：
  - `use` 只引用到的东西；
  - `libc` 经 `nix::libc` 引入；
  - `AsyncFd` 的引入路径是 `tokio::io::unix::AsyncFd`；
  - 没有 `unwrap`；
  - 每个 `unsafe` 块的前一行有 `// SAFETY:`。
- 修正记录写一行："Linux 的设备只在 CI 上编译"。

---

## Task 7：macOS 的设备（`tun/macos.rs`，`utun_connect`）

**改哪里**（设计第 8.3 节、M4-D12；P14、P15）

1. `src/tun/macos.rs`，带 `#[cfg(target_os = "macos")]`，换掉 macOS 上的 `unsupported`。

   - **打开**
     1. `socket2::Socket::new(Domain::from(libc::AF_SYSTEM), Type::DGRAM, Some(Protocol::from(libc::SYSPROTO_CONTROL)))`。socket2 在 Unix 上会设 `CLOEXEC`。
     2. `utun_connect(socket.as_raw_fd())`，回 `utunN`。
     3. `set_nonblocking(true)`。
     4. `AsyncFd::new(socket)`。
     - 错误经 `frame::explained(…, Os::MacOs)`。
   - **unsafe 函数**：照 wireguard-go 的 `tun/tun_darwin.go`（MIT）的做法：`sc_unit = 0` 让内核挑单元号，再读回名字。

     ```rust
     // SAFETY 写明：fd 是打开着的 AF_SYSTEM / SYSPROTO_CONTROL 套接字；三个结构都在调用期间活着，长度是它们自己的
     #[allow(unsafe_code)]
     fn utun_connect(fd: RawFd) -> io::Result<String>
     ```

     1. 建 `libc::ctl_info { ctl_id: 0, ctl_name }`，名字是 `com.apple.net.utun_control`，以 NUL 结尾；调 `ioctl(fd, CTLIOCGINFO, &mut info)`。
     2. 建 `libc::sockaddr_ctl { sc_len: size_of 的值, sc_family: AF_SYSTEM as u8, ss_sysaddr: AF_SYS_CONTROL as u16, sc_id: info.ctl_id, sc_unit: 0, sc_reserved: [0; 5] }`；调 `connect`。
     3. 调 `getsockopt(fd, SYSPROTO_CONTROL, UTUN_OPT_IFNAME, buf, &mut len)`；名字用 `frame::name_of` 取出。
   - **收**：`readable().await`，然后在 `try_io` 里 `recv`（缓冲 64 KiB），交给 `frame::utun_packet`。得到 `None` 的跳过（DEBUG）。
   - **发**：
     - 用 `frame::utun_header` 取头；得到 `None` 的包丢掉、计数。
     - 用 `send_vectored(&[头, 包])` 发出，不拷贝包。OpenVPN 在 macOS 上同样用 `writev` 写 utun——这是事实，不抄代码。
     - `WouldBlock`、`ENOBUFS`、`EMSGSIZE`、`EINVAL`：丢掉、计数；其余错误让 TUN 结束。

2. `enhanced::system()` 在 macOS 上回 `MacSystem::new(Arc::new(TimedRunner(STEP_TIMEOUT)), "/etc/resolv.conf".into(), scratch)`。

3. 名单：
   - `LISTED` 加 `("src/tun/macos.rs", "utun_connect")`。
   - `CLAUDE.md` 的那一段同步，这时是九个函数。
   - **`crates/rurge/tests/sockets.rs`（M4b）的名单**加 `("crates/rurge-platform/src/tun/macos.rs", "Socket::new(", "utun 的控制套接字（AF_SYSTEM），不出站")`，理由照那张表的写法。

**用例**：同任务 6。

**判据**：

- 同任务 6 的各条。
- `sockets.rs` 的扫描用例绿。

---

## Task 8：可执行文件：平台后端接上、物理网卡的服务器

**改哪里**（设计第 3.7、6.3、9.1 节；P9、P13、P19）

1. **`cli/enhanced/backend.rs`**（M4a 写的）
   - `backend()` 改成 `backend(data_dir: &Path)`。两个调用者是 `run()` 与 `restore_what_a_crash_left`，照实改。
   - 平台后端是 `PlatformBackend { system: Box<dyn rurge_platform::enhanced::System> }`，由 `rurge_platform::enhanced::system(data_dir.join("enhanced"))` 建。
     - **不带 `cfg`**。Windows 上（M4d 之前）它是 `Unsupported`，回的错误与 M4a 的平台占位相同。
   - `PlatformBackend` 实现 `Backend`：
     - 每个方法转给 `system`。M4a 的方法拿的是 `&Opened`，转成 `Adapter { name, index }` 再交给 `system`。
     - `Opened` 由 `TunDevice` 得来：`device` 是 `Arc::new(PlatformDevice(tun))`，`name` 与 `index` 取自 `tun.adapter()`。
     - `close(dev)` 把它丢掉：最后一个 `Arc` 走了，描述符就关了。
     - `Undo` 与 `Saved` 互相映射：`MacDns`、`RpFilter` 一一对应。`Undo::Test` 到了平台后端：WARN `an undo the platform backend does not know is dropped`，回 `Ok(())`（P9）。
   - `Backend` 加一个方法：

     ```rust
     /// The DNS servers of the network outside the enhanced mode, for
     /// `server:system` while it is on (design 6.3); `None`: the system's own list.
     fn physical_dns(&self, primary: Option<&str>, saved: &[Undo], network_changed: bool) -> Option<Vec<SocketAddr>>;
     ```

     | 后端 | 回 |
     | --- | --- |
     | 平台后端 | `system.physical_dns` 的地址，配上端口 53 |
     | 文件后端 | 文件里的 `"physical_dns": ["127.0.0.1:5353", …]`；没有这个键时回 `None` |
     | `off` | `None` |

2. **`cli/enhanced/device.rs`**（新文件）：`PlatformDevice(rurge_platform::tun::TunDevice)` 实现 `rurge_tun::device::Device`。`recv`、`send` 包成 `BoxFuture`；`mtu` 照给。

3. **管理器**（M4a 的 `EnhancedMode`；M4b 已接上标志、排除名单与 `system_dns_changed`）：另外拿到 `PlatformSystemDns` 的 `physical` 那一份，照 M4b 交标志的做法。
   - 打开：`take_dns` 之后，把 `physical_dns(primary, 备份里的, false)` 写进去，再调（M4b 的）`system_dns_changed`。
   - `follow(…, network_changed = true)`：`take_dns` 之后，把 `physical_dns(primary, 备份里的, true)` 写进去，再调 `system_dns_changed`。
   - 重载（`network_changed = false`）：不读。
   - 关闭：交还 DNS 那一步之后写 `None`，再调 `system_dns_changed`。

4. **删掉 M4a 的 `the_platform_backend_is_not_available_in_this_version`**（在提交说明里点名）。
   - 理由（P19）：在 Linux / macOS 的 CI 上，它会真的去开设备。
   - 由任务 3 的 `the_unsupported_system_says_so` 代替。
   - 任务 0 第 2 条找出的别的这类用例同样处理，记进修正记录。

**用例**

- `backend.rs`：
  - `undo_and_saved_map_both_ways`。
  - `a_test_undo_on_the_platform_backend_is_dropped`：只走映射与 `undo`，不调 `open`。`undo` 对 `Test` 不碰系统。
- 管理器的单元用例（M4a 的假后端加上 `physical_dns`）：
  - `the_physical_servers_are_set_after_the_takeover_and_cleared_on_close`：核对槽里的值，以及 `system_dns_changed` 的次数与先后。
  - `a_change_of_network_reads_the_physical_servers_again`。
  - `a_reload_does_not_read_them`。
- 文件后端：`the_file_backend_gives_its_physical_servers`。

**判据**：

- `git grep -n "cfg(" crates/rurge/src/cli/enhanced` 什么也没有。
- `git grep -n "not_available_in_this_version" crates` 什么也没有。
- 门禁绿。

---

## Task 9：L2 特权用例（`tests/privileged`，Linux）

**改哪里**（设计第 14 节、M4-D27；P17）

1. **新的测试包**：工作区 `Cargo.toml` 的 `members` 加 `"tests/privileged"`。新包叫 `rurge-privileged-tests`，`publish = false`，照 `tests/external/Cargo.toml` 的写法，`[lints] workspace = true`。
   - `[[bin]] name = "l2-helper"`（`test = false`）：用例在两个命名空间里起的小程序。它有五种用法：

     | 用法 | 做什么 |
     | --- | --- |
     | `echo <地址>` | TCP 回显 |
     | `dns <地址> <名字> <IPv4>` | UDP 的 DNS 服务端，只答这个名字的 A 记录；每收到一个查询，把来源写到标准输出 |
     | `connect <地址>` | 发一行，等它回来；成败是退出码 |
     | `query <服务器> <名字>` | 发一次 A 查询，打印答案 |
     | `get <URL>` | 对回环上的 HTTP API 发一个 `GET`，打印正文 |

     只用 std（DNS 报文手写，或用工作区已有的 `hickory-proto`），在三个平台上都能编译。
   - `tests/linux_netns.rs`：开头 `#![cfg(target_os = "linux")]`。每个用例一开始先看 `RURGE_TEST_PRIVILEGED`：不是 `1`，就打印 `skipped: set RURGE_TEST_PRIVILEGED=1 in a disposable Linux VM, as root` 并返回。

2. **夹具 `Lab`**：在 `Drop` 里删掉它建的一切。
   - 两个命名空间：`rl2-a-<pid>`（当作"本机"）与 `rl2-b-<pid>`（当作"互联网"）。
   - 一对 veth：`10.231.0.2/30` 在 a，`10.231.0.1/30` 在 b（P17）。
   - a 里的默认路由经 `10.231.0.1`；两边的回环都起来。
   - `ip` 用 `LinuxTools::find("/")` 找到的那个。命令经 `std::process::Command` 执行，失败就 panic，带上输出。
   - `rurge` 二进制：先取 `RURGE_TEST_RURGE`；没有时取测试二进制所在的 `target/<profile>/` 下的 `rurge`；都没有，用例失败，并说明怎样给。

3. **三条用例**。共同的做法：经 `ip netns exec <a> …` 把 `rurge run` 起在 a 里；`--data-dir` 是临时目录；Profile 里 `http-api` 在 a 的回环上；等标准输出出现 `enhanced mode on`，30 秒为限。
   - `a_tcp_round_trip_and_a_ping_go_through_the_adapter_and_out_direct`
     1. b 里起 `l2-helper echo 10.231.0.1:17000`；规则是 `FINAL,DIRECT`。
     2. a 里 `l2-helper connect 10.231.0.1:17000` 成功；`ping -c 1 -W 5 10.231.0.1` 成功。
     3. `l2-helper get http://127.0.0.1:<端口>/v1/requests/recent`（带密钥）里，有一条到 `10.231.0.1:17000`、策略是 `DIRECT` 的记录。
   - `rurges_own_dns_queries_do_not_enter_the_adapter`
     1. b 里起 `l2-helper dns 10.231.0.1:53 example.l2 10.231.0.1`。
     2. Profile 里写 `dns-server = 10.231.0.1` 与 `always-real-ip = example.l2`。
     3. a 里 `l2-helper query 198.18.0.2 example.l2` 得到 `10.231.0.1`；b 的 DNS 服务端收到了查询。
     4. 请求记录里**没有**到 `10.231.0.1:53` 的 UDP 会话：rurge 自己的查询绑在 veth 上出去，没有经过虚拟网卡。
   - `a_killed_run_is_undone_by_the_next_start`
     1. a 里设 `net.ipv4.conf.all.rp_filter = 1`。`/proc/sys/net` 是每个命名空间自己的。
     2. 带 `--enhanced-mode` 起来之后，读到 `2`。
     3. `kill -9`；a 里不再有 `rurge` 网卡。
     4. 不带 `--enhanced-mode` 再起：读到 `1`，`state.json` 里没有 `enhanced_backup`。

4. **`tests/privileged/README.md`**，写明：
   - 这些用例做什么；
   - 只在一次性的 Linux 虚拟机里以 root 跑：先 `cargo build -p rurge`，再 `RURGE_TEST_PRIVILEGED=1 cargo test -p rurge-privileged-tests`；
   - 它们改什么：只改自己建的两个命名空间里的东西；
   - **写好之后没有在任何地方运行过**；
   - 在 CI 上启用是项目所有者的事（总设计 Q4）。

**用例**：本机与 CI 上，这些用例都跳过并打印原因；`l2-helper` 能编译。

**判据**：

- `cargo test -j 8 --workspace --test linux_netns` 在本机是空的（本机不是 Linux）。
- 门禁绿。
- Pi 不设 `RURGE_TEST_PRIVILEGED`。

---

## Task 10：等 CI（停点）

**做什么**（M4-D1、设计第 2 节）

1. **停下，请所有者推送。** 全量门禁在本机绿之后，对项目所有者说明：
   - M4c 的代码任务做完了；
   - Linux 与 macOS 的代码只有 CI 能编译，请推送。推送什么、推到哪里，由所有者定；
   - 推送之后请告诉 Pi。

   **Pi 不推送。**

2. **读回 CI。** 所有者说推送了之后，读那一次运行。公开可读的只有注解与任务摘要；任务日志要登录才能读。

   ```
   curl -s "https://api.github.com/repos/danielnie001/rurge/actions/runs?head_sha=<提交>"      # 运行号
   curl -s "https://api.github.com/repos/danielnie001/rurge/actions/runs/<运行号>/jobs"       # 三个平台的任务号与各步的结论
   curl -s "https://api.github.com/repos/danielnie001/rurge/check-runs/<任务号>/annotations"  # 注解
   ```

   - 未认证的请求一小时限 60 次，省着用。
   - 连不上网络，就请所有者把注解贴过来。

3. **按注解修。** 要修的有两类：clippy 一步的编译错误与 lint（任务 1 之后有注解），以及测试的失败。
   - 每一处修正一个提交，记进修正记录：运行号、平台、失败、修法。
   - 修完再停下，请所有者推送；重复第 2、3 步。
   - 直到 Linux 与 macOS 两个平台的 `check` 都绿（Windows 照旧绿）。

4. **修不了的，或要所有者定的**（例如 CI 环境本身的问题）：列出来交给所有者。
   - **不改 CI 去绕过失败。**
   - 不给失败的用例加 `#[ignore]`。

**判据**：

- 修正记录里有每一次运行的运行号与结论。
- 最后一次运行三个平台都绿；或者剩下的失败都已列给所有者。

---

## Task 11：文档

- **`docs/api/phase3.md` 的 M4 一节**（接着 M4a、M4b 写的往下写）：
  - Linux 与 macOS 上增强模式改系统的哪些东西：网卡 `rurge` / `utunN`、地址、路由、DNS、`rp_filter`；每一项怎样回去。
  - INFO 与 DEBUG 各说什么：`loosened net.ipv4.conf.all.rp_filter …`、`put … back`、没有接管 DNS 的那句、macOS 读不了一个服务的那句。
  - 权限不够时的三句错误。
  - **手工恢复 macOS 的 DNS**：`sudo networksetup -setdnsservers <服务> empty`，或写原来的服务器。
  - 接管期间 `server:system` 在两个平台上查哪里：
    - Linux：resolved 的上游名单；
    - macOS：打开前的 `/etc/resolv.conf`；网络变化之后，是那个服务原来手动设的服务器，没有就是 DHCP 给的。
    - **IPv6 路由器通告（RDNSS）给的服务器读不到**，登记。
  - **另一个全隧道 VPN 开着时打开失败**：同样的前缀已经在（`File exists`）。办法是先关掉那个 VPN（P3、P22）。
- **`docs/testing.md`**：
  - `LinuxSystem` / `MacSystem` 的用例不碰系统，经假的执行器与临时目录跑。
  - **没有一条缺省的用例开设备**；以后加用例的人照这一条。
  - 夹具是手写的（P20）。
  - L2 的跑法与门控（链接到 `tests/privileged/README.md`）。
  - 任务 1 之后，clippy 的错误也在注解里。
- **`CLAUDE.md`**：`unsafe_code` 一段已在任务 6、7 同步，这里核对一遍。「现在在哪里」不改，M4d 收尾时再改。
- **`docs/status.md`**：在阶段 3 下新写一节"M4c（Linux 与 macOS）"。
  - 做了什么。
  - 没做的，与不要当成已经有的：
    - **两个平台的设备与系统改动没有在真机上跑过**，只有 CI 的编译与单元用例；
    - L2 写好了，没有运行过；
    - macOS `-listnetworkserviceorder` 的输出格式凭记忆写，没有核对过；
    - Windows 照旧不能开。
  - 以后的会话必须知道的事：
    - 系统一侧不带 `cfg`，经执行器与根目录测；
    - `TimedRunner`；
    - 脚本写在数据目录的 `enhanced/` 下；
    - 用 `route add`，不用 `replace`；
    - macOS 的 IPv6 地址是 `/128`，范围的路由显式装；
    - 没有一条缺省用例开设备；
    - `Saved` 与 `Undo` 的映射；
    - `physical_dns` 什么时候读；
    - CI 的 clippy 日志。
  - 附录加 `rurge-platform → tokio`。
- **兼容性清单 9.1 节**：
  - 增强模式在 Linux 与 macOS 上的"目前"一栏照实改："有，CI 编译与单元用例，真机没有验证"。
  - 登记：另一个全隧道 VPN、IPv6 RDNSS 的服务器、Linux 改 `rp_filter`、没有 resolved 的 Linux 不接管 DNS。
- `docs/README.md` 不变：本计划已经在里面。
- 本计划末尾的两张表照实填。

**判据**：

- `git grep` 核对文档里引用的用例名、文件与函数都在。
- 门禁绿。
- 做完停下，交项目所有者评审。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 引用的名字 | 对着 `ac0a98a` 与 M4a / M4b 两份计划核对。M4a、M4b 做出来的名字，以它们的修正记录为准。名字不同就照实改、记进记录，不改语义 |
| P2 | 系统一侧不带 `cfg` | `LinuxSystem` / `MacSystem` 以 `CommandRunner` 与根目录（或 `resolv.conf` 的路径）为参数，在 Windows 开发机上用假的执行器与临时目录测。带 `cfg` 的只有两个设备，与 `system()` 里挑平台的两行（AR-02：都在 `rurge-platform` 里）。只在一个平台上用到的项做成 `pub`，免得别的平台上报 `dead_code`。理由：Linux 与 macOS 的代码开发机编译不到；能在本机测的越多，CI 那一轮的风险越小 |
| P3 | Linux 用 `route add`，不用 `route replace`（订正设计第 5.2 节与 M4-D3 里"Linux 用 `route replace`"一处） | `replace` 会悄悄换掉另一个程序装的、前缀相同的路由。例如 OpenVPN 的 `redirect-gateway def1` 装 `0.0.0.0/1` 与 `128.0.0.0/1`，后者与八个前缀之一相同。关闭时 rurge 把它删掉，那个 VPN 从此漏流量。`add` 遇到相同的前缀报 `File exists`：打开失败、倒着撤销，与 macOS 一样，使用者看得到冲突。设计担心的"与内核的连接路由重复"不会发生：rurge 空间里的前缀已经从路由集里去掉（第 4.5 节），重载与网络变化又只加差集 |
| P4 | `resolvectl default-route` 失败不算错 | `domain <网卡> '~.'` 已经把名字路由到这条链路。resolvectl(1) 对 `~.` 的说明："ensures that DNS traffic is preferably routed to the DNS servers on this interface, unless there are other, more specific domains configured on other interfaces"。`default-route` 管的只是"不匹配任何链路的域的名字"（同一页："suitable for lookups on domains no other link explicitly is configured for"）；设了 `~.` 之后，这样的名字就没有了。所以它失败不应让整个接管失败。另外三步照旧 `Fail`。没有 `resolvectl` 的系统（systemd 239 之前，手册页把逐链路的这些命令记作 "Added in version 239"）走"不接管"那条路 |
| P5 | macOS 的 IPv6 地址用 `/128`，加 `alias` | 写法是 `ifconfig <utun> inet6 fd00:6152::1 prefixlen 128 alias`，理由有两条。① 加 `alias`：wg-quick 的 darwin 脚本加地址一律带 `alias`（事实）；BSD 的 `ifconfig` 不带 `alias` 时，把新地址当作替换处理，带上它就只加不删。② 用 `/128`：前缀短于 128 时，FreeBSD 的内核会把这个前缀设为在链路上、自己装一条路由（`SIOCAIFADDR_IN6`；xnu 是否相同没有核对）。那样再显式装 `fd00:6152::/64` 会撞上 `File exists`，让 IPv6 整个当作没开。用 `/128` 时，内核装不装都不冲突：这一段的路由只由 rurge 显式装（设计第 4.5 节"macOS 显式装"，两个地址族一样）。IPv4 照旧：点对点的地址只带出一条到对端的主机路由，显式装 `198.18.0.0/15` 不冲突；它冲突时，说明别的程序占着 rurge 的地址范围，打开失败是对的 |
| P6 | `networksetup -setdnsservers … empty` 用小写 | 手册页（ss64 的 networksetup 页转录）写："type "empty" in place of the DNS server names"；读者报告说大小写都行。订正 M4a 的 `Empty` |
| P7 | 带网络服务名的命令直接经执行器 | `networksetup` 的读写不走 `exec::run`。失败时回固定的一句，不带服务名；原文与命令行在 DEBUG。理由：服务名可能是使用者自己起的（设计第 13 节） |
| P8 | 读不了原值的服务不接管 | 读不了原值，就没法写回，宁可不改：WARN 一行，那个服务的 DNS 照旧。一个服务也接管不了时，`take_dns` 回 `Ok(false)`，与没有 resolved 的 Linux 相同 |
| P9 | 撤销里的两种例外 | ① 服务已经被删掉：算做完（没有地方可以写回），记一行 INFO。② 别的平台的撤销动作（换了机器的数据目录），以及平台后端不认识的 `Test`：WARN，丢掉。留着的话，每次启动都失败一次，永远去不掉 |
| P10 | 物理网卡的服务器 | Linux：接管了才有，读 `/run/systemd/resolve/resolv.conf`（resolved 的上游名单）。macOS：打开时是接管之前的 `/etc/resolv.conf`；网络变化之后，是主网卡所在的服务原来手动设的服务器，没有就是 `ipconfig getpacket` 给的 DHCP 服务器；都没有就是空的，这时查找失败，不退回系统解析库。**IPv6 的 RDNSS 给的服务器，两种读法都读不到**，登记 |
| P11 | 程序的时限 | 每个程序 30 秒（`TimedRunner`，经 `run_with_timeout`，到时杀掉）。一批几十条路由的 `ip -batch` 只要毫秒；`networksetup` 慢的时候要几秒。这样挂住的工具（例如 D-Bus 卡住的 `resolvectl`）不会让主循环一直等 |
| P12 | 脚本文件 | 写在数据目录的 `enhanced/` 下。以 root 运行时，数据目录属于 root；不写 `/tmp`，因为别人可以在那里预先放符号链接。用 `create_new` 建，名字带进程号与序号，用完删掉。崩溃留下的，在下一次 `open` 时删掉 |
| P13 | M4a 接口的两处改动 | ① `backend()` 改成 `backend(data_dir)`：平台后端要知道在哪里写脚本。② `Backend` 加 `physical_dns`：M4b 的 `PlatformSystemDns::physical` 由管理器写 |
| P14 | 设备的收发 | 收：缓冲 64 KiB，大于 MTU 的包不会被截断成别的样子；读到的包按自己的长度拷出。发：`WouldBlock`、`ENOBUFS`、`EMSGSIZE`、`EINVAL` 时丢掉、计数（设计第 8.1 节"写不进就丢"），第一次 WARN，之后 DEBUG；其余错误让 TUN 结束，M4a 的"设备坏了"会把增强模式整个关掉 |
| P15 | `open` 要在运行时里调 | `AsyncFd::new` 需要当前的反应器。M4a 的后端方法在 `spawn_blocking` 的线程上调，那里有运行时的上下文 |
| P16 | 权限的说明 | 按 `io::ErrorKind`（`PermissionDenied`、`NotFound`、`ResourceBusy`）判断，不按平台的错误号。这样判断不带 `cfg`，能在本机测 |
| P17 | L2 | 新的测试包 `tests/privileged`。地址 `10.231.0.0/30` 只存在于用例自建的两个网络命名空间里：veth 只连这两个命名空间，不连宿主。这不是回环，是 CLAUDE.md"测试只用回环"之外唯一的地方：只在一次性的虚拟机里、以 root、由 `RURGE_TEST_PRIVILEGED=1` 放开。写好之后没有运行过，照实写。在 CI 上启用是总设计 Q4，归项目所有者 |
| P18 | CI 的 clippy 日志 | clippy 一步的输出也写进文件，失败时报告脚本读它。理由：M4c 的 Linux / macOS 代码只有 CI 编译，编译错误出在 clippy 一步；任务日志要登录才能读，没有这一改，Pi 读不到错误 |
| P19 | 没有一条缺省的用例开设备 | `TunDevice::open` 与真实平台的 `System::open` 不被任何缺省用例调用。在 Linux / macOS 的 CI 上它们是真的：以普通用户跑会得到 `EPERM`，但谁以 root 跑测试，就真的建了网卡。M4a 的 `the_platform_backend_is_not_available_in_this_version` 在 M4c 之后正是这样一条用例：删掉，由不经 `system()` 的 `the_unsupported_system_says_so` 代替 |
| P20 | 写法的出处（写计划时读到的） | **ip(8)**（man7.org）："-batch … First failure will cause termination of ip"；"-force Don't terminate ip on errors in batch mode. If there were any errors during execution of the commands, the application return code will be non zero"。**resolvectl(1)**（man7.org）：`dns`、`domain`、`default-route`、`revert`、`flush-caches` 的说明；`revert` "undoing all effects of dns, domain, default-route, …"。**ipconfig(8)**（apple-oss-distributions/bootp）：`domain_name_server (ip_mult): {192.168.4.1}`；"This command outputs nothing if DHCP/BOOTP is not active on the interface"。**ISC dhcp-users** 邮件列表 2009-03 贴的输出：`domain_name_server (ip_mult): {156.26.1.1, 156.26.1.30}`。**networksetup**：ss64 转录的手册页，`-listallnetworkservices` 一条写 "An asterisk (*) denotes that a network service is disabled"，`-setdnsservers` 一条写 "type "empty""；`There aren't any DNS Servers set on Ethernet.` 出自 osxdaily 一篇文章下读者贴的输出。`-listnetworkserviceorder` 的输出格式（`(1) 名字` / `(Hardware Port: …, Device: …)`）**凭记忆写，没有找到可以引用的原文**，留给手工清单核对。**libc / nix / socket2 / tokio 的符号**：读本机 registry 里的源码（任务 0 第 4 条）。**macOS 的 `ifconfig … alias`**：wg-quick 的 darwin 脚本（GPL，只取事实） |
| P21 | 停点 | 任务 10（等 CI，可能来回几次）与任务 11 之后。中途遇到设计第 20 节的问题，或会改变 M4-D1 ～ D30 的（P3 之外），停下来列出 |
| P22 | 与全隧道 VPN 共存 | 两个平台上，另一个程序装着与八个前缀之一相同的路由时，打开失败（P3），错误里带工具的原文（`File exists`）。不去换掉或绕开别人的路由。登记在 API 参考与兼容性清单里；手工清单（M4d）有一项 |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的，项目所有者可以改。计划的正文不改：正文里说得不对的地方，以这张表的行为准。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项（一）：工作树干净，`HEAD` = `bb67afa` = 本地 `main`，分支 `phase3-m4c`，`origin/main` 仍是 `ac0a98a`（本分支未推送）；`git log --oneline 2c6a215..b91d786` 18 个、`b91d786..bb67afa` 26 个，M4a、M4b 的提交都在。M4a 的修正记录 105 行与延后 15 行、M4b 的 80 行与 17 行读过；本计划要用的名字对着代码核过：`Step`（`route/steps.rs:10`，字段 `program` / `args` / `script` / `on_failure`）、`OnFailure`（`:24`）、`SCRIPT`（`:21`）、`RouteSet`（`route/set.rs:34`，字段只有 `v4` / `v6`；`compute` `:185`、`diff` `:251`、`contains` `:289`、`len` `:297`、`is_empty` `:302`）、`RouteInputs`（`:16`，`ipv6` / `excluded` / `included` / `external` / `explicit_space`）；`route/linux.rs` 的 `LinuxTools`（`:17`，`ip: PathBuf`、`resolvectl: Option<PathBuf>`，没有派生）与 `configure` `:26`、`configure_ipv6` `:37`、`unconfigure_ipv6` `:43`、`add_routes` `:51`、`remove_routes` `:65`、`take_dns` `:81`、`give_dns_back` `:107`；`route/macos.rs` 的 `configure` `:27`、`configure_ipv6` `:47`、`unconfigure_ipv6` `:62`、`add_routes` `:77`、`remove_routes` `:101`、`set_dns` `:127`、`flush_dns` `:139`，五个程序路径常量 `IFCONFIG` / `ROUTE` / `NETWORKSETUP` / `DSCACHEUTIL` / `KILLALL`（`:16`～`:20`，任务 5 另加 `IPCONFIG`），`EMPTY = "Empty"`（`:24`，P6 在任务 5 改成小写 `empty`）；`SPACE_V4` / `SPACE_V6`（`route/mod.rs:24` / `:26`）。任务 2 要核的那一条：`RouteSet::compute`（`set.rs:185`）在 `explicit_space` 下**给** IPv6 那一条——`set.rs:214`～`:220` 先 `retain` 掉 rurge 空间、再在 `explicit_space` 时 `push(SPACE_V4)` 并在 `inputs.ipv6` 时 `push(SPACE_V6)`，所以 macOS 上 `fd00:6152::/64` 已在集合里（IPv6 没开时不加，那时也没有 IPv6 地址与路由）；任务 2 不用补这一条。 | 本提交 |
| 2 | 0 | 第 1 项（二）：可执行文件一侧与扫描名单。`Backend`（`cli/enhanced/backend.rs:38`）的十三个方法：`open` / `configure` / `configure_ipv6` / `unconfigure_ipv6` / `routes` / `subnets`（`:61`，回 `Result<Vec<(IpNet, String)>, String>`）/ `dns_undo` / `take_dns` / `give_dns_back` / `adjust_undo` / `adjust` / `undo` / `close`；`Opened`（`:26`，`device: Arc<dyn Device>`、`name: String`、`index: Option<u32>` 带 `#[allow(dead_code)]`）；`Undo`（`undo.rs:12`）、`Stage`（`:26`）、`Undo::stage`（`:34`，`pub`）、`Backup`（`:67`）。`backend()`（`backend.rs:86`）不带参数、不设环境变量时回 `Arc::new(Platform)`；两个调用者是 `run.rs:1276`（`restore_the_enhanced_mode`）与 `:1503`（`run` 的守护进程一路）；`NOT_YET` 在 `:20`，用例 `the_platform_backend_is_not_available_in_this_version` 在 `:203`（见第 2 项那一行）。文件后端的键：`open` / `ipv6` / `routes` / `dns` / `adjusted` / `actions` / `subnets` 与 `fail` / `fail_undo` / `device_fails_after_ms`（`file.rs` 的模块说明 `:3`～`:10`，写入处 `:57`、`:79`、`:95`、`:98`、`:113`、`:166`、`:179`、`:207`、`:233`、`:241`、`:259`）。`EnhancedMode::new`（`mod.rs:356`）收 `backend` / `store` / `starter` / `links` / `ended`；M4b 把增强模式的标志、排除名单与 `system_dns_changed` 交给它的是 `Links`（`:76`）——四个字段 `egress` / `enhanced`（`Arc<AtomicBool>`）/ `exclude`（`Exclude`）/ `system_dns_changed`（`Arc<dyn Fn() + Send + Sync>`），任务 8 加第五个（R6）。`PlatformSystemDns`（`runtime.rs:204`）的 `physical` 是 `Arc<RwLock<Option<Vec<SocketAddr>>>>`（`:211`，私有；今天只有 `enhanced()` 一个访问器 `:221`，生产代码没有写它的地方，`:698` / `:714` 在测试模块里）——任务 8 照 R6 加一个交出 `Arc` 的方法。`system_dns_changed` 的调用六处：`mod.rs:607`（`follow` 只换表、不重新接管 DNS 的那一支，`:605` 的 `else if table_changed`）、`:702`（`take_the_dns_again`）、`:810`（打开第 4 步）、`:854`（第 7 步 `take_dns` 之后）、`:902`（关闭交还 DNS 之后）、`:917`（拿走表之后），与派工写的一致；开→关 4 次的用例是 `system_dns_changed_is_called_after_the_flag_the_takeover_and_the_giving_back`（`:1605`）。`crates/rurge/tests/sockets.rs` 的 `LISTED` 是 `[(&str, &str, &str); 24]`（`:57`，文件 / 写法 / 理由），写法共 15 种（`PATTERNS`，`:34`～`:50`），写法前一个字符是 `[A-Za-z0-9_]` 时不算命中（`:30`～`:33` 的说明，M4b 记录第 57 行）；`rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 是 `[(&str, &str); 7]`（`:18`）。R7：`git grep -n "cfg(" crates/rurge/src/cli/enhanced` 今天五行，都是 `#[cfg(test)]`（`backend.rs:193`、`file.rs:304`、`mod.rs:988`、`undo.rs:71`、`wanted.rs:51`），去掉这些之后是空的——任务 8 的判据照 R7 写。R10：`wait_for_the_sources`（`run.rs:1382`）今天的次序是建目录 → `lock_data_dir` → `Lock::Taken`（`:1393`～`:1394`）→ 打印 "waiting for the profile's remote sources"（`:1402`）→ 下载（`waiting()`，`:1407` 起），正常启动的恢复在之后的 `Lock::recover`；崩溃留下的系统 DNS 在这段下载期间还在，任务 8 照 R10 在拿到锁之后、打印与下载之前调一次 `restore_what_a_crash_left`。M4b 加的 `Backend::subnets` 不在本计划任务 3 的 `System` 里（那个接口任务 3 才建）：任务 8 的 `PlatformBackend` 照 M4b 的 `Platform` 的做法读 `rurge_platform::socket::egress::subnets`（`socket/egress.rs:91`），不改 `System`。M4b 在 `dns.rs` 里"去掉 rurge 空间"的函数是 `without_rurge_space`（`dns.rs:29`，`pub(crate)`，同 crate 用得到），`parse_resolv_conf` 是 `pub`、不带 `cfg`（`:103`）。 | 本提交 |
| 3 | 0 | 第 2 项：`git grep -n -E "not_available\|RURGE_ENHANCED_BACKEND" crates tests` 21 行，其中 `not_available` 一处。逐处读的结果：① `crates/rurge/src/cli/enhanced/backend.rs:203` 的 `the_platform_backend_is_not_available_in_this_version`（测试模块起于 `:193`）在不设环境变量时也走到平台后端——它直接 `let backend = Platform;`，不经 `backend()`；今天 `open` 回 `NOT_YET`、什么设备也不建，但 M4c 之后 `Platform` 包着真的 `System`，这一条会建真的网卡，任务 8 删掉它（P19，代替它的是不经 `system()` 的 `the_unsupported_system_says_so`）。② `backend.rs:123` 的 `Platform::subnets` 今天就读本机网卡（`rurge_platform::socket::egress::subnets`，`socket/egress.rs:91`，只读），没有用例走到它（①那条不调 `subnets`；M4b 记录第 41 行同此）。③ `backend()` 的两个调用者都只在 `rurge run` 里：`run.rs:1276` 的 `restore_the_enhanced_mode` 只在 `state.json` 里有增强模式备份时才构造后端，`:1503` 那一路只在 `--enhanced-mode` 时打开。测试起 `rurge` 二进制只有一处（`crates/rurge/tests/cli.rs:57` 的 `Command::cargo_bin`、`:71` 的 `Command::new`）；`rurge_command()`（`:56`～`:66`）与 `rurge_process()`（`:70`～`:79`）都设 `RURGE_ENHANCED_BACKEND=off`，`the_helpers_keep_the_enhanced_mode_off`（`crates/rurge/tests/run/enhanced_mode.rs:221`）钉着；没有一处 `env_remove("RURGE_ENHANCED_BACKEND")`（唯一一处 `env_remove("RURGE_ENHANCED_MODE")` 是 `cli.rs:2530`，去掉的是另一个变量——`--enhanced-mode` 的开关，免得它从环境里漏进来）。④ `run/enhanced_mode.rs:242` 设 `bogus`：`backend_from`（`backend.rs:93`）直接报错，一个后端也不构造（`:247` 断言那句文字）。⑤ 设 `file:` 的三处（`run/enhanced_mode.rs:28`、`:677`、`:1067`）：文件后端。其余命中都是说明文字（`backend.rs:15` / `:16` / `:21` / `:83` / `:149` / `:259`、`file.rs:3`、`mod.rs:7`、`cli.rs:51` / `:55`、`run/enhanced_mode.rs:4`）。结论：任务 8 之前，唯一一条走到平台后端、M4c 之后会真的开设备的用例是①，照计划删掉；②那条读本机的路今天没有用例（任务 9 的 L2 会走到）。 | 本提交 |
| 4 | 0 | 第 3 项（V2）：照协调者的 R8 记，Pi 不联网，协调者核过（2026-10-27）。ip(8)（man7.org）——`-batch` 的 "First failure will cause termination of ip"、`-force` 的 "Don't terminate ip on errors in batch mode" 与 P20 相同，P3 的 `add` 不 `replace` 不受影响。resolvectl(1)（man7.org）——`dns` / `domain` / `default-route` / `revert` / `flush-caches` 都在、都 "Added in version 239"；P4 引的 `~.` 与 `default-route` 两句原文都在，`revert` 的 "undoing all effects of dns, domain, default-route, …" 也在——与 P4、P20 相同。systemd-resolved.service(8)（man7.org）——stub-resolv.conf 列 `127.0.0.53`（另有 `127.0.0.54` 的代理模式）；`/run/systemd/resolve/resolv.conf` "containing information about all known DNS servers" 与"没有逐网卡的概念"那句，读 `manager_compile_dns_servers`（`src/resolve/resolved-manager.c`）与 `write_resolv_conf_server`（`resolved-resolv-conf.c`）核过：接管之后 rurge 那条链路（`default-route yes`、`~.`）的 `198.18.0.2` 也写进这个文件，物理网卡的服务器照样在——P10 成立；另记物理网络一个服务器也没有时文件里是编进 resolved 的回退服务器；`is_resolved_stub` 只认 `127.0.0.53`，"只有 `127.0.0.54`"判为假照计划做（系统罕见，判假的后果是不接管 DNS、WARN 一行，安全一侧）。networksetup（ss64 转录的手册页）——`-listallnetworkservices` 的 "An asterisk (*) denotes that a network service is disabled"、`-setdnsservers` 的 "type "empty""、`-listnetworkserviceorder` 的 "An asterisk (*) next to a service means that the service is inactive" 都在，与 P6 相同；`-listnetworkserviceorder` 的输出原文仍没有可引用的样本，照 P20 留给 M4d 的手工清单。macOS ifconfig(8)（keith.github.io 的 xcode-man-pages）——`alias` "Establish an additional network address for this interface"、`delete` 是 `-alias` 的别名、`prefixlen len` "(Inet6 only.)"、概要的 `ifconfig interface [address_family]`，与 M4a 的 `configure` / `unconfigure_ipv6` 和 P5 的 `/128 … alias` 相合。**没有再核**：`ipconfig getpacket`（P20 的出处是 apple-oss-distributions/bootp 与 ISC 邮件列表）与 macOS route(8)，照 P20 的出处写到手工清单核对。 | 本提交 |
| 5 | 0 | 第 4 项（V4，离线读本机 registry 源码；版本以 `Cargo.lock` 为准）：libc 0.2.189、nix 0.31.3、socket2 0.6.5、tokio **1.53.1**（计划写 1.53，照锁文件这一版核）。Linux 的符号有三处与计划写的文件不同：`ifreq` 在 `src/unix/linux_like/linux_l4re_shared.rs:344`（`ifr_name: [c_char; crate::IFNAMSIZ]` `:346`、`ifr_ifru: __c_anonymous_ifr_ifru` `:347`，联合体 `:328` 里 `ifru_flags: c_short` `:334`）、`EINVAL: c_int = 22` 也在那个文件（`:1388`）；但 `IFNAMSIZ` 在 `src/unix/mod.rs:287`（`pub const IFNAMSIZ: size_t = IF_NAMESIZE`），`IFF_TUN: c_int = 0x0001` 与 `IFF_NO_PI: c_int = 0x1000` 在 `src/unix/linux_like/mod.rs:1439` / `:1445`，`TUNSETIFF: Ioctl = _IOW::<c_int>(T_TYPE, 202)` 在同文件 `:1480`（`Ioctl` 在 linux-gnu 是 `c_ulong`，`linux/gnu/mod.rs:18`；musl / android 是 `c_int`），`ENOBUFS` / `EMSGSIZE` 按架构分文件——CI 的 x86_64-unknown-linux-gnu 在 `src/unix/linux_like/linux/gnu/b64/x86_64/mod.rs:401`（`ENOBUFS: c_int = 105`）与 `:386`（`EMSGSIZE: c_int = 90`），musl 在 `linux/musl/b64/x86_64/mod.rs:671` / `:655`。按分工，任务 6 用 `nix` 重新导出的 `libc`（`nix-0.31.3/src/lib.rs:107` 的 `pub use libc;`，不带 `cfg`）就都能用，路径不影响写代码。macOS（`src/unix/bsd/apple/mod.rs`）逐个对上：`sockaddr_ctl`（`:757`，`sc_len: c_uchar`、`sc_family: c_uchar`、`ss_sysaddr: u16`、`sc_id: u32`、`sc_unit: u32`、`sc_reserved: [u32; 5]`）、`ctl_info`（`:1302`，`ctl_id: u32`、`ctl_name: [c_char; MAX_KCTL_NAME]`）、`CTLIOCGINFO: c_ulong = 0xc0644e03`（`:2584`）、`AF_SYSTEM: c_int = 32`（`:2837`）、`AF_SYS_CONTROL: c_int = 2`（`:2844`）、`SYSPROTO_CONTROL: c_int = 2`（`:2847`）、`UTUN_OPT_IFNAME: c_int = 2`（`:3782`）、`MAX_KCTL_NAME: usize = 96`（`:4332`，计划只写了 96）。socket2 0.6.5：`impl From<c_int> for Domain`（`lib.rs:240`）、`for Protocol`（`:367`）、`Socket::send_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize>`（`socket.rs:661`）。tokio 1.53.1：`AsyncFd<T: AsRawFd>`（`src/io/async_fd.rs:181`）、`AsyncFd::new(inner) -> io::Result<Self>`（`:226`）、`get_ref`（`:323`）、`try_io`（`:902`），模块在 `src/io/mod.rs:255` 的 `cfg_net_unix!` 里（要 `net` 特性、Unix）。 | 本提交 |
| 6 | 0 | 第 5 项：`CommandRunner`（`crates/rurge-platform/src/command.rs:13`，唯一的方法 `fn run(&self, cmd: &[String]) -> io::Result<String>` `:16`）、`SystemRunner`（`:19`，实现 `:21`～`:22`）、`failure`（`:49`，私有：`fn failure(cmd: &[String], status: impl fmt::Display, detail: &str) -> String`）、`run_with_timeout`（`:81`，`pub`）、`run_all`（`:205`）、`run_best_effort`（`:211`）。测试里的 `FakeRunner` 在 `#[cfg(test)] pub(crate) mod testing`（`:223` / `:225`）里，记录与应答的键都是 `cmd.join(" ")`（`:257`～`:258`），`reply` / `fail` / `calls` 在 `:238` / `:244` / `:250`，没列的命令回空输出。计划点的名字都对上。 | 本提交 |
| 7 | 0 | 第 6 项：`.github/workflows/ci.yml:35` 的 clippy 一步确实是一行没有名字的 `- run: cargo clippy --all-targets -- -D warnings`（在 `:34` 的 `cargo fmt --all --check` 之后、装互操作服务端的步骤之前）；"Report the failed tests" 在 `:361`～`:366`，`if: failure()`、`shell: bash`，调 `"$python" .github/scripts/report-test-failures.py "$RUNNER_TEMP/test.log"`；测试一步（`:353`～`:358`）把 `cargo test --workspace --no-fail-fast` 的输出 `tee` 到 `$RUNNER_TEMP/test.log`。报告脚本的用法是 `<log file>` 一个参数（`report-test-failures.py:11`）；`COMPILE = re.compile(r"^error(\[E\d+\])?: ")`（`:42`）只在 `current is None` 时算（`:116`～`:117`，即第一个测试二进制之前，理由写在注释里：`rurge` 二进制的用例会印同样形状的 `error[E0022]: …`）。任务 1 要改的两处都对得上。 | 本提交 |
| 8 | 0 | 第 7 项：门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）——`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-0.log`）**83 个结果行、4513 通过、0 失败、13 忽略**，与 M4b 收尾时（M4b 修正记录第 80 行）相同；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，没有重跑。之后每个任务与这三个数比。 | 本提交 |
| 9 | 0 | 执行方式（协调者的裁定 R2、R3，派工要求另记一行）：M4b 收尾之后不等项目所有者评审就开本计划；任务 10（等 CI）不卡住后面的工作——做完任务 9 接着做任务 11，再做 M4d 计划的任务 0 ～ 7 与 9，最后停一次请所有者推送，本计划的任务 10 与 M4d 的任务 8 一起读回 CI；任务 11 末尾的"做完停下，交项目所有者评审"随之不停。它覆盖计划开头「执行方式」里的停点、任务 10 的位置与任务 11 的最后一条判据；依据是 M4a 修正记录第 104 行引的项目所有者 2026-10-26 的指示与常设授权。 | 本提交 |
| 10 | 1 | 任务 1 照需求做：`.github/workflows/ci.yml` 两处（clippy 一步换成带 `name: Run clippy` / `id: clippy` / `shell: bash` 与 `tee "$RUNNER_TEMP/clippy.log"` 的那一步；"Report the failed tests" 加 `env: CLIPPY: ${{ steps.clippy.outcome }}`，并按它选 `clippy.log` 或 `test.log`），`.github/scripts/report-test-failures.py` 只在开头的说明里加一段（代码不改），`cargo fmt` 一步、各安装步骤、测试一步、`bench` 任务都不动。本机的样本日志在派工要求的 `error[E0425]` 那段之外多放了一段 `error[E0308]`，好让"第二段编译错误"那一支也走到；`python -I` 跑两次（样本日志回两条 `::error title=compile error::` 与那条 `error: could not compile …`，不存在的日志回 `::notice title=No test log::`、退出码 0），PyYAML 读出 `check` 的十八个步骤：clippy 那一步是 `Run clippy`、报告那一步带 `env`。门禁与第 8 行同（83 个结果行、4513 通过、0 失败、13 忽略）。CI 还没有跑过这份改动：推送归项目所有者，读回 CI 在任务 10。 | 本提交 |
| 11 | 2 | P6 是任务 2 的，不是任务 5 的：本行订正第 1 行末尾那半句（“`EMPTY = "Empty"`（`:24`，P6 在任务 5 改成小写 `empty`）”）。本任务把它改成 `"empty"`（`route/macos.rs:25`，说明里照手册页的那一句），逐字用例 `an_empty_list_of_servers_is_empty` 随之改；第 1 行的其余内容不改，P5 那条照第 1 行核过的结果不补路由（`RouteSet::compute` 在 `explicit_space` 下给 IPv6 的 `/64`）。 | 本提交 |
| 12 | 2 | `route::exec` 的三处执行者的裁定：① `write_script` 用 `create_new` 撞上同名文件时取下一个号重试（进程号会被复用，一次崩溃留下的名字可能正与新的一次相同），所以 `rurge-steps-<pid>-<n>.txt` 的 `n` 可能跳过几个数；② `cannot_write` 在 `program.file_name()` 给不出名字时（路径以 `..` 结尾或就是根）写整个路径，其余情况只写文件名；③ 写脚本写到一半失败时那个文件留下，由下一次 `remove_leftovers` 删掉（`create_new` 已经建了它，任务只说程序跑完就删）。 | 本提交 |
| 13 | 2 | 判据 `git grep -n "route replace" crates/rurge-platform/src` 的输出现为空：M4a 的两处非用例文字（`add_routes` 的说明、`route_step` 的说明）与四处逐字用例都改成了 `route add`，新加的 `linux_adds_routes_and_never_replaces_them` 断言每一行以 `route add ` 开头、且不含 `replace`——照派工的要求，这三个字哪里也不写。 | 本提交 |
| 14 | 2 | `TimedRunner` 没有新用例（任务没要）：它的本体就是把 `run_with_timeout` 交给调用方，超时与杀掉的行为由 `command.rs` 里已有的 `a_program_that_does_not_end_is_killed_at_the_limit` 验证；有人用它从任务 4、5 起。 | 本提交 |
| 15 | 2 | `exec.rs` 的判据照协调者的裁定 R11（与任务 8 的 R7 同一条）：生产代码不带 `cfg`。`git grep -n "cfg(" crates/rurge-platform/src/route/exec.rs` 只有 `:163` 的 `#[cfg(test)]`，也就是 `tests` 模块的那一个，不算。 | 本提交 |
| 16 | 3 | 任务 3 照需求做：`Cargo.toml` 加 `tokio.workspace = true`（`Cargo.lock` 只在 `rurge-platform` 的依赖名单里多一行 `"tokio"`，包数不变）、`description` 末尾加 "the enhanced mode's virtual adapter, routes and DNS"、`[lints]` 上那段说明加虚拟网卡的两个函数（`[lints]` 本身不动）；`src/lib.rs` 加 `pub mod enhanced;` 与 `pub mod tun;`；新文件 `src/tun/mod.rs`（`DeviceSpec`、`TunDevice`、`drain`、`write_all`）、`src/tun/frame.rs`、`src/tun/unsupported.rs`、`src/enhanced/mod.rs`（`Adapter`、`Saved`、`NOT_AVAILABLE`、`STEP_TIMEOUT`、`System`、`Unsupported`、`system`）。本任务里 `system()` 在每个平台上都回 `Box::new(Unsupported)`，`tun/mod.rs` 只有 `mod unsupported;` 一支，两边都写在说明里。用例十条：`frame` 五条（需求列的四条，`open_errors_say_what_is_missing` 把两个平台、每一类与“其余”都走一遍）、`tun/mod.rs` 四条（需求要的三条，加 `every_packet_is_written_in_order_and_taken_out`）、`enhanced/mod.rs` 一条。没有计时用例。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：83 个结果行、**4530 通过、0 失败、13 忽略**，基线 4520 加本任务的 10 条。 | 本提交 |
| 17 | 3 | 执行者的裁定（四项）。① `icmp.rs` 模块说明里 "this crate does not depend on tokio (P13)" 那半句删掉（`crates/rurge-platform/src/icmp.rs:4`）：本任务加了这条依赖边，那句话不再成立；P13 的另一半（异步的部分在可执行文件里）照旧。② `Unsupported` 做成 `pub`：`system` 的说明按需求用 ``[`Unsupported`]`` 链接它，链接私有项会让 rustdoc 的 `private_intra_doc_links` 告警；它也是任务 8 之前唯一能直接构造的 `System`。③ `tun/mod.rs` 说明里的接口约定用 `text` 标记的代码块，不用 `ignore`（这个仓库里说明用的代码块一律是 `text`：`rurge-dns/src/fakeip/file.rs:3`、`rurge-proto/src/snell/{chunked.rs:5,record.rs:9,udp.rs:6}`）：`ignore` 的那一种会被 `cargo test` 记成一条被忽略的 doctest，门禁的忽略数就从 13 变成 14。④ `drain` 的 `max` 是这一次 `recv` 搬进 `into` 的包数，含先读到的那一个（与派工“`max` 是 2 时只搬两个”一致）；`into` 在每次调用前是空的（`rurge-tun/src/dispatch.rs:32` 的 `batch` 每轮 `drain(..)`），所以“`into` 到 `max` 个就停”与前者等价。 | 本提交 |
| 18 | 3 | 两条判据的输出。`git grep -n "cfg(" crates/rurge-platform/src/tun crates/rurge-platform/src/enhanced` 三行，都是 `#[cfg(test)]`（`enhanced/mod.rs:159`、`tun/frame.rs:107`、`tun/mod.rs:220`），也就是三个 `tests` 模块的那一个；生产代码没有 `cfg`（同协调者的裁定 R11、R7）。`git grep -n "TunDevice::open" crates` 一行（`crates/rurge-platform/src/tun/mod.rs:35`），是模块说明里“`TunDevice::open` 由平台的 `System::open` 调用、没有用例调用它（P19）”那半句；生产代码里没有，因为 `open` 的定义是 `pub fn open(spec: &DeviceSpec)`，调用者（任务 6、7 的 `System::open`）还没写。 | 本提交 |
| 19 | 4 | 照协调者的裁定 R13，`dns::parse_resolv_conf`（`crates/rurge-platform/src/dns.rs:110`）改用 `resolv_conf::Config::parse_with_errors`：`Config::parse` 遇到第一行它不认识的（`options` 里一个它不认识的词、一个不认识的关键字）就整份失败、名单变空，而 glibc 的 `res_init` 对不认识的选项与关键字一律不理。现在解析到的行照用，没解析到的行合成一句写进 DEBUG（不带文件的内容）。影响的是系统 DNS 的名单（`dns::servers()` 的 Unix 一支）、`netenv` 的 Linux / macOS，与任务 4 的认存根、物理网卡的服务器；用例 `an_unknown_option_does_not_empty_the_servers`。 | 本提交 |
| 20 | 4 | 任务 4 照需求做，`enhanced/linux.rs` 不带 `cfg`：`route::linux::LinuxTools::find`（`route/linux.rs:29`，私有常量 `DIRECTORIES`（`:44`）与 `find_program`（`:50`）——`root` 只用来看文件在不在，回的路径从字符串建，所以用例在 Windows 上也逐字比对 `/usr/sbin/ip`；找不到 `resolvectl` 时是 `None`）；`enhanced::linux::is_resolved_stub`（`enhanced/linux.rs:39`，经改过的 `dns::parse_resolv_conf`，名单恰好是 `[127.0.0.53]`）；`LinuxSystem`（`:46`，`new`（`:63`）从不失败、`tools` 存 `Result`、`open`（`:142`）先报缺 `ip` 再 `remove_leftovers` 与 `TunDevice::open`）、四个配置与路由方法（`configure` `:153`、`routes` `:172`）、`dns_saved`、`take_dns`（`:195`）、`give_dns_back`、`adjust_saved`（`:234`）、`adjust`、`undo`、`physical_dns`（`:285`），以及 `rp_filter` 的私有读写（`/proc/sys/net/ipv4/conf/<名字>/rp_filter`，不经 `sysctl`）。`enhanced/mod.rs:14` 加 `pub mod linux;`；新文件 `src/log_capture.rs`（`lib.rs:9` 的 `#[cfg(test)] mod log_capture;`）照 `rurge-tun` 的同一份写，两个分发器的理由照抄；`Cargo.toml` 的 `[dev-dependencies]` 加 `tracing-subscriber.workspace = true`（`Cargo.lock` 只在 `rurge-platform` 的依赖名单里多一行，包数不变）。用例十八条（`enhanced/linux.rs:313` 起的 `tests`），加已有的 `the_unsupported_system_says_so` 共十九条；两条用 `LogCapture` 抓日志，都是普通的 `#[test]`。没有计时用例。 | 本提交 |
| 21 | 4 | 执行者的裁定（六项）。① 写 `rp_filter` 失败的文字是 `cannot write rp_filter: <原因>`：需求只定了读的四种，写没有；两种都不带路径。② `warned` 的那句 WARN 在“`resolv.conf` 不是存根”与“找得到 `ip` 但没有 `resolvectl`”两种情形下都用需求给的原文（其中提到存根的那半句在后一种情形下不准确）；后一种情形另有一行 DEBUG 说明没有 `resolvectl`，读不了 `resolv.conf` 的原因在 `behind_the_stub` 的 DEBUG 里。③ `find_program` 用 `is_file()`：一个叫 `ip` 的目录不是程序。④ 需求之外多测的四处：`rp_filter` 的内容不是 0 ～ 2（`cannot read rp_filter: not 0, 1 or 2`）、`unconfigure_ipv6` 的一批与 `dns_saved` 回空（并进 `configure_with_ipv6_runs_two_batches_and_an_ipv6_failure_is_ok_false` 与 `the_dns_is_taken_behind_the_stub_with_resolvectl`）、`physical_dns` 读不了 resolved 的文件时 `Some(vec![])`（并进 `the_physical_servers_are_resolved_s_upstreams_without_rurges_space`）。⑤ `log_capture.rs` 放在 crate 根（`src/log_capture.rs`），不建 `tests` 目录，照派工。⑥ `route/linux.rs` 的模块说明改一句：路径的唯一查找处是 `LinuxTools::find`，仍不查 `PATH`。 | 本提交 |
| 22 | 4 | 判据与门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）。`git grep -n --untracked "cfg(" crates/rurge-platform/src/enhanced/linux.rs` 一行：`:312` 的 `#[cfg(test)]`，即 `tests` 模块的那一个（同 R11）。`cargo test -j 8 --workspace --lib -- enhanced::` 十九条全过（工作区的 `task-4-lib.log`）：`enhanced::tests::the_unsupported_system_says_so` 与 `enhanced::linux::tests::` 下十八条。全量门禁：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`gate-task-4.log`）83 个结果行、**4549 通过、0 失败、13 忽略**，即基线 4530 加本任务的 19 条，没有二进制异常退出。 | 本提交 |
| 23 | 4 | 评审修正（第 1 轮），只改 `enhanced/linux.rs` 的用例，生产代码不动。① `Fake::run` 从前把程序之后的每个参数都当路径去读（`dns`、`rurge`、`~.`、`yes`、`198.18.0.2`、`flush-caches`、`revert`、`-batch`、`-force`），也就是在测试进程的当前目录里找文件——用例临时目录之外（`CLAUDE.md`「测试约定」第一条），当前目录里真有同名文件时还会把它当脚本。现在只读名字带 `rurge-steps-` 的那一个参数（`:435`，与 `Call::line` 的同一个判断），`Call` 的说明也随之写明，`Fake` 的说明加一句“只读这一个文件”（`:387`～`:393`）。② `an_interface_name_that_is_not_a_name_is_refused` 末尾的 `!error.contains("/proc")` 在 Windows 上永真（这里的路径用反斜杠），换成断言错误里没有网卡名 `eth0`（`:741`，路径里正是它）：`/proc` 那一句留着，在 Linux 上有用。跑的是 `cargo test -j 8 --workspace --lib -- enhanced::linux`（工作区的 `task-4-fix-1.log`）：十八条全过；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过。只改用例，没有跑全量门禁。 | 本提交 |
| 24 | 5 | 任务 5 照需求做，`enhanced/macos.rs`（新文件，1099 行）不带 `cfg`：四条解析（`service_devices` `:40`、私有的 `numbered_service` `:66`、`services` `:78`、`dns_servers` `:97`、`dhcp_dns` `:116`），`MacSystem`（`:151`；`MacState { to_take, before, warned }` `:135`；`new` `:159`、`networksetup` `:173`、`ipconfig` `:178`、`command` `:185`、`run` `:199`、`run_step` `:206`、`list_services` `:221`、`cannot_read_a_services_dns` `:230`；`mac_dns` `:239` 把 `Saved` 认成它说的服务）与 `impl System`（`:246`）：`open` `:249` 先 `exec::remove_leftovers(&scratch)` 再 `TunDevice::open`（协调者的裁定 R4：P12 说崩溃留下的脚本在下一次 `open` 时删掉，计划里 macOS 这半边漏了；没有用例调 `open`，P19），`configure` `:257`、`configure_ipv6` `:265`、`unconfigure_ipv6` `:269` 是 M4a 的翻译加 `exec::run`，`routes` `:274` 调 `enhanced/mod.rs` 里新加的 `run_routes`（`enhanced/mod.rs:104`：`add` → IPv6 一半失败时只撤那一半 → `remove`，与原先 `LinuxSystem::routes` 同一次序，`LinuxSystem::routes` 改成调它，Linux 的十八条用例照旧绿），`dns_saved` `:288`、`take_dns` `:360`、`give_dns_back` `:396`、`undo` `:414`、`physical_dns` `:456` 照需求（`open`、`configure*`、`routes` 没有新用例：需求没点，P19），`adjust_saved` `:401` 回 `Ok(None)`、`adjust` `:410` 回 `Ok(())`。`enhanced/mod.rs:15` 加 `pub mod macos;`；`route/macos.rs:19` 的 `NETWORKSETUP` 改成 `pub(crate)`、`:24` 另加 `pub(crate) const IPCONFIG = "/usr/sbin/ipconfig"`（不另抄一份路径）。夹具五份手写，每份一行 “not taken from a Mac” 加出处：`-listallnetworkservices` 的表头是手册页转录、服务名凭记忆，`-getdnsservers` 的句子是 P20 的那条、一服务一行照 `-setdnsservers` 的参数形状，`-listnetworkserviceorder` 的形状凭记忆（P20 没有可引用的样本，注释写明 M4d 的手工清单核对），`ipconfig getpacket` 的 `domain_name_server (ip_mult): {…}` 照 apple-oss-distributions/bootp 与 ISC 邮件列表、周围的行走凭记忆。用例十八条（`:521` 起的 `tests`），五条解析、九条接管与撤销、四条物理网卡的服务器；`the_responder_is_never_a_physical_server` `:1081` 钉住 M4b 的 R33（崩溃后 `resolv.conf` 只剩 `198.18.0.2` 时 `before` 是空的）。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-5.log`）83 个结果行、**4567 通过、0 失败、13 忽略**，即基线 4549 加本任务的 18 条。 | 本提交 |
| 25 | 5 | 执行者的裁定（六项）。① `networksetup -getdnsservers` 的输出全空时 `dns_servers` 回 `Some(vec![])`，不当成 `None`：与服务说“没有手动设的服务器”一同对待，`-setdnsservers <服务> empty` 就能写回原样；改成 `None` 会让这个服务不被接管（P8），而一个空输出比不出一行地址更像“没有”。② `physical_dns` 的 `None` 与空名单是两件事：`primary` 是 `None` 时回 `Some(vec![])`（设计 6.3“没有已知的服务器时这次查找失败，不退回系统解析库”），只有“不接管”才回 `None`（macOS 上没有这一支：接管与不接管都由 `before`／名单说）。③ `undo` 里列服务失败也用 `cannot list the network services: <原文>`（与 `dns_saved` 同一句；需求只给了 `dns_saved` 的那一处，这条命令不带服务名，能带原文）。④ `service_devices` 认服务行的办法：`(` 之后第一个 `)` 后面还有文字（`(1) Wi-Fi`、`(*) Bluetooth PAN`），而 `(Hardware Port: …, Device: …)` 后面没有，所以不是服务行；`Device: )`（空）的那个服务连同它的名字一起丢掉。⑤ `warned` 用 `MacState` 里的一个 `bool` 而不是 `AtomicBool`：已经有 `Mutex`，锁只在同步代码里（这里没有 `await`）；`give_dns_back` 只复位 `warned`，不清 `to_take`（下一次接管由 `dns_saved` 重设）。⑥ `dns_saved` 读不了 `resolv.conf`（或文件不在）时 `before` 记空名单、原因只进 DEBUG：与 Linux 的 `physical_dns` 读不了 resolved 的文件时回 `Some(vec![])` 同一处理。 | 本提交 |
| 26 | 5 | 评审修正（第 1 轮），两处，都在 `enhanced/macos.rs`。① `numbered_service`（`:67`）只把 `(` 与 `)` 之间是数字或 `*` 的行当服务行：硬件端口名里带 `)`（`(Hardware Port: Foo (bar), Device: en7)`）从前会被当成一个叫 `, Device: en7)` 的服务的开头，真正的那个服务就配不上网卡。这一条订正第 25 行④的说法（那里写的是“`(` 之后第一个 `)` 后面还有文字”）；`SERVICE_ORDER` 夹具与 `service_devices_pair_each_service_with_its_device`（`:638`）另加一个端口名带括号的服务（`(Hardware Port: USB 2.0 Hub (left), Device: en7)`），断言它配上了 `en7`。② 协调者的裁定 R16：`physical_dns`（`:462`）网络变化之后的两种来源同样经 `dns::without_rurge_space` —— 备份里那个服务的原值先去 rurge 的空间，去掉之后是空的就照“原值是空的”往下读 DHCP（使用者照 5.2 的提示把系统 DNS 手动指到 `198.18.0.2` 时，`server:system` 从前会去问应答器，而 rurge 自己的查询到不了它，查找必然失败），DHCP 给的服务器同样去掉；`match servers.is_empty()` 的两支相同，顺手写成直接的 `Some(…)`。用例 `a_saved_responder_is_no_physical_server`（`:1076`）。跑 `cargo test -j 8 --workspace --lib -- enhanced::`（工作区的 `task-5-fix-1.log`）38 条全过；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；全量门禁 `cargo test -j 8 --workspace --no-fail-fast`（`gate-task-5-fix-1.log`）83 个结果行、**4568 通过、0 失败、13 忽略**，即上一轮的 4567 加 1 条。 | 本提交 |
| 27 | 6 | 任务 6 照需求做：`src/tun/linux.rs`（带 `#[cfg(target_os = "linux")]`：`Device { fd: AsyncFd<File> }`；`open` 先 `frame::ifname`、再以 `custom_flags(libc::O_NONBLOCK)` 开 `/dev/net/tun`、经 `set_iff` 要名字与 `IFF_TUN | IFF_NO_PI`、`AsyncFd::with_interest(file, Interest::READABLE)`；`read`、`try_read`、`try_write`、`no_room`、`set_iff`），`tun/mod.rs` 按平台分成两个 `cfg` 的 `mod`/`use`（Linux 上不再编 `unsupported`），`enhanced/mod.rs` 的 `system()` 在 Linux 上回 `LinuxSystem::new(Arc::new(TimedRunner(STEP_TIMEOUT)), "/".into(), scratch)`，`Arc` 与 `TimedRunner` 用全路径写在函数体里、不在文件顶上 `use`（免得别处 `dead_code`）；`tun/frame.rs` 加不带 `cfg` 的 `packet_len` 与用例 `a_read_of_nothing_is_the_device_gone`；`tests/unsafe_functions.rs` 的 `LISTED` 7 → 8 并加 `("src/tun/linux.rs", "set_iff")`；`CLAUDE.md` 的 `unsafe_code` 一段同步（“只有七个函数” → “只有八个函数”、加 `set_iff` 一句、末尾那句把它算进开发机上编译不到的两个函数里），提交说明里写明。三处过时的说明改成不再提“这个任务”的说法（`enhanced/mod.rs` 开头一段与 `system()` 的说明、`tun/mod.rs` 开头一段）；`git grep -n --untracked "this task\|This task\|In this task" crates/rurge-platform/src/` 为空。执行者的裁定，照协调者的 R14：① 写不经 tokio 的就绪——`try_write` 直接在非阻塞描述符上 `write`，描述符只注册 `READABLE`：经 `AsyncFd::try_io(WRITABLE)` 时缓存的就绪没置上就根本不写、直接回 `WouldBlock`（`write_all` 会把好包当“没有地方”丢掉，成批地丢）；② 读到 0 个字节是设备没了，不是跳过（订正需求里“读到 0 个字节的跳过”那半句与 `tun/mod.rs` 约定里 “An empty read … is skipped here” 那半句）：内核的 tun 只在缓冲长度为 0 时回 0，真读到 0 只能是描述符那头没了，“跳过再读”在就绪一直置着的描述符上是空转；`frame::packet_len` 回 `UnexpectedEof`，`read` 与 `try_read` 都经它，任务 7 的 macOS 设备同样；③ `c_char` 用 `from_ne_bytes` / `to_ne_bytes` 转：它在 x86-64 Linux 上是 `i8`、在 aarch64 Linux 上是 `u8`，`as c_char` 在后者上会被 clippy 说成多余的转换。本机编译不到：`tun/linux.rs` 全部（`Device` 的四个方法、`no_room`、`set_iff`）与 `tun/mod.rs`、`enhanced/mod.rs` 里 Linux 一支的 `cfg`，没有编译过，第一次 CI 核对。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-6.log`）83 个结果行、**4569 通过、0 失败、13 忽略**，即基线 4568 加本任务的 1 条。这一轮里四个测试二进制先后以 `STATUS_HEAP_CORRUPTION` 退出（`rurge-engine --test outbounds_shadow_tls`、`rurge-engine --test outbounds_masque`、`rurge-dns` 的 lib、`rurge-api --test api`；`CLAUDE.md`「这台开发机」的已知问题），每个单独重跑都通过，最后一次全量全绿（`gate-task-6.log` 是这一次）。 | 本提交 |
| 28 | 7 | 任务 7 照需求做：`src/tun/macos.rs`（新文件，带 `#[cfg(target_os = "macos")]`：`Device { fd: AsyncFd<Socket> }` `:38`；`open` `:46` 建 `AF_SYSTEM` / `SYSPROTO_CONTROL` 的 `DGRAM` 套接字、经 `utun_connect` 拿到名字、`set_nonblocking(true)`、`AsyncFd::with_interest(socket, Interest::READABLE)`；`read` `:62`、`try_read` `:79`、`try_write` `:93`（`send_vectored(&[头, 包])`，不拷包）、`skipped` `:115`（DEBUG）、`no_room` `:127`、`utun_connect` `:138`，三个 `unsafe` 块各有一行 `// SAFETY:`），`tun/mod.rs` 的模块改成 Linux / macOS / `not(any(…))` 三支（`:47`～`:59`），`enhanced/mod.rs` 加 macOS 的 `system()`（`:111`）并把 `Unsupported` 一支的 `cfg` 改成 `not(any(…))`（`:122`）；`tun/frame.rs` 加不带 `cfg` 的 `utun_to_front`（`:100`）与用例 `utun_to_front_moves_the_packet_to_the_front`（`:226`）；`tests/unsafe_functions.rs` 的 `LISTED` 8 → 9 加 `("src/tun/macos.rs", "utun_connect")`；`crates/rurge/tests/sockets.rs` 的 `LISTED` 24 → 25 加 `Device::open` 的 `AF_SYSTEM` 套接字一行、模块说明照实改成“utun 的控制套接字已有行、M4d 给设备加的套接字同样加一行”；`CLAUDE.md` 的 `unsafe_code` 一段同步（“只有八个函数” → “只有九个函数”、加 `utun_connect`、末尾那句把它算进开发机上编译不到的函数）。`utun_to_front` 是为了 `TunDevice` 的约定（包在缓冲开头）：utun 在包前面多四个字节的协议族，macOS 的两个读都经它把包挪到 `buf[0..]`；不带 `cfg` 放在 `frame.rs`，所以在本机能测（P2）。macOS 的设备只在 CI 上编译。执行者的裁定：① `system()` 的说明按平台各写一份（macOS 那份提到 `/etc/resolv.conf`），模块开头一段与 `Unsupported` 的说明照任务 6 改过的说法补上 macOS、仍不提“这个任务”；② 不是 IPv4 / IPv6 的包在 `try_write` 里做成 `EINVAL`，让 `no_room` 把它当“没有地方”丢掉并计数（需求给的写法），不另立一种错误。本机编译不到：`tun/macos.rs` 全部（`Device` 的四个方法、`skipped`、`no_room`、`utun_connect`）与 `tun/mod.rs`、`enhanced/mod.rs` 里 macOS 一支的 `cfg`，没有编译过，第一次 CI（macOS arm64）核对。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-7.log`）83 个结果行、**4570 通过、0 失败、13 忽略**，即基线 4569 加本任务的 1 条；没有二进制以 `STATUS_HEAP_CORRUPTION` 或 `STATUS_ACCESS_VIOLATION` 退出，没有重跑；`cargo test -j 8 --workspace --test unsafe_functions --test sockets` 单独跑一次全过，输出在工作区的 `task-7-lists.log`。 | 本提交 |
| 29 | 8 | 协调者的裁定 R15：IPv6 配不上的告警只写一次，地址那一半由管理器写、路由那一半由后端写（`docs/api/phase3.md:464` 的“由管理器写，后端不另写”）。`route/exec.rs` 的 `run` 对 `Ipv6Off` 的失败只写 DEBUG（连同错误），`warned` 去掉，说明改成照实的说法；`enhanced/mod.rs` 的 `run_routes` 在加路由的 IPv6 一半失败（`added == false`）时 WARN 一次，文字是 `the IPv6 routes could not be added to the virtual adapter; they are left out`（不带网卡名与前缀；不用计划里 “…goes on without IPv6” 的说法，理由：重载时 IPv6 本来开着、新的 IPv6 路由加不上，增强模式照旧带着 IPv6 与原来的路由，M4a 修正记录第 95 行）。它订正任务 2 需求第 32 行“`Ipv6Off` 的 WARN 是 `IPv6 could not be set up on the virtual adapter; the enhanced mode goes on without IPv6`”那一条：那句 WARN 不再由 `exec::run` 写。用例三条：`exec.rs` 的 `an_ipv6_failure_skips_the_other_ipv6_steps_and_goes_on`（没有 WARN）、`enhanced/linux.rs` 的 `an_ipv6_route_failure_takes_the_ipv6_routes_out_again`（恰好一行，就是那句）、`enhanced/linux.rs` 的 `configure_with_ipv6_runs_two_batches_and_an_ipv6_failure_is_ok_false`（没有 WARN）。`docs/api/phase3.md` 日志表里新那句的一行归任务 11。 | 本提交 |
| 30 | 8 | 任务 8 的主体照需求做（P13 ①、P19、协调者的裁定 R7）。`backend()` 改成 `backend(data_dir: &Path)`（`backend.rs:98`），平台后端由 `rurge_platform::enhanced::system(data_dir.join("enhanced"))` 建、不带 `cfg`；占位的 `Platform`、`NOT_YET` 与只给旧用例用的 `Nothing` 删掉，换成 `PlatformBackend { system: Box<dyn System> }`（`:145`），每个方法转给 `system`（`&Opened` → `Adapter`：`:151`），`open` 由 `TunDevice` 取 `adapter()` 的名字与序号、把它放进新的 `PlatformDevice`（`device.rs`，新文件，`recv` / `send` 包成 `BoxFuture`、`mtu` 照给），`close(dev)` 只是丢掉它。`Undo` ↔ `Saved` 的映射是 `saved_of`（`:120`）/ `undo_of`（`:132`），交给 `system` 的 `saved` 先去掉 `Test`；`undo(&Undo::Test { .. })` WARN `an undo the platform backend does not know is dropped` 并回 `Ok(())`（P9）。`Opened::index` 从此有人读，`#[allow(dead_code)]` 去掉。`Backend` 加 `physical_dns`（`:85`）：平台后端配端口 53（`:219`），文件后端读 `"physical_dns": ["127.0.0.1:5353", …]`、每次调用重读、没有键回 `None`、读不懂的项跳过、不记进 `actions`（`file.rs:264`），`Off` 回 `None`（`:283`）。两处模块说明改成照实的说法（`backend.rs` 开头、`mod.rs` 开头）。`run.rs` 的 `data_dir` 挪到 `backend(&data_dir)` 之前（`:1514` / `:1515`），`restore_what_a_crash_left` 与 `restore_the_enhanced_mode` 改拿数据目录（`:1264`、`:1274`，`recover_without_a_log` 在 `:1257`），`Lock::recover` 照改（`:1363`）。删掉 M4a 的 `the_platform_backend_is_not_available_in_this_version`（P19；由任务 3 的 `the_unsupported_system_says_so` 代替）。新用例六条：`backend.rs` 的 `undo_and_saved_map_both_ways` 与 `a_test_undo_on_the_platform_backend_is_dropped`（用 `Unsupported`：转给它就会回错）、管理器的 `the_physical_servers_are_set_after_the_takeover_and_cleared_on_close`、`a_change_of_network_reads_the_physical_servers_again`、`a_reload_does_not_read_them`（`Mock` 加 `physical` 与 `physical_asked`，都属于读、不进 `calls`；`fixture_on` 的第三个参数换成 `Record`）与文件后端的 `the_file_backend_gives_its_physical_servers`。 | 本提交 |
| 31 | 8 | 协调者的裁定 R6（写在哪几处）与执行者的裁定。`Links` 多一个 `physical: Arc<RwLock<Option<Vec<SocketAddr>>>>`（`mod.rs:98`），`PlatformSystemDns::physical()` 交出同一个 `Arc`（`runtime.rs:229`），`run.rs:1740` 交给 `Links`，`runtime.rs` 的旧用例改成经 `physical()` 写；`PlatformSystemDns::physical` 的字段说明改成照实的说法。写入点三处，都不多调一次回调（M4b 的 4 次有用例钉着）：打开第 7 步在 `take_dns` 回来之后、回调之前写 `physical_dns(wanted.primary, 备份里的, false)`（`mod.rs:920`）；`take_the_dns_again` 在 `take_dns` 回来之后、回调之前，只在 `network_changed` 时写 `(primary, 那里的 saved, true)`（`:688`、`:728`；`follow` 的两处调用在 `:521` / `:610`，IPv6 关掉那一次在前面、网络变化时也由它读）；关闭的 DNS 那一段末尾、回调之前写 `None`（`:971`）。重载（`network_changed == false`）不读；`take_the_dns_again` 在 `take_dns` 之前就返回的三条错误路径不读、`physical` 留着原来的（写在它的说明里）。`physical_dns` 经 `call` 放到阻塞线程（`read_physical`，`:744`），`call` 的 `Err` 当 `None` 并写一行 DEBUG。判据（R7）：`git grep -n "cfg(" crates/rurge/src/cli/enhanced` 去掉 `#[cfg(test)]` 之后什么也没有，`git grep -n "not_available_in_this_version\|NOT_YET" crates` 什么也没有。没有端到端的用例：`server:system` 在测试里会去问这台机器真的系统 DNS，失败时就离开了回环（`CLAUDE.md`「测试约定」）；`run.rs:1740` 那一行没有用例走到。文件后端的 `physical_dns` 是一次读，因此不记进 `actions`（同 `subnets`）。 | 本提交 |
| 32 | 8 | 协调者的裁定 R10：`wait_for_the_sources` 拿到锁之后、打印 `waiting for the profile's remote sources` 与下载之前调一次 `lock.recover(&args.runtime)`（`run.rs:1407`）：设计 9.5 的“拿到锁之后、做任何新改动之前”恢复；macOS 上崩溃留下的系统 DNS 指着已经不在的应答器，启动时的下载问系统解析库，不先恢复就要等到时限。`Lock::Taken(Some(_))` 恢复、`Taken(None)` 什么也不做，理由同 `Lock::recover` 的说明；恢复两次无害（`restore_the_enhanced_mode` 的注释）。不会嵌套运行时（任务 0 的顾虑 2）：`wait_for_the_sources` 是普通的 `fn`，恢复在它自己建运行时之前调。用例 `what_a_dead_run_left_goes_back_before_the_sources_are_waited_for`（`crates/rurge/tests/run/enhanced_mode.rs`）：`left_by_a_dead_run` 的数据目录、一个指向回环 `TestServer` 的 `#!include`、Profile 自己一个不认识的规则（启动以 2 结束，好读整份标准错误）；断言那行警告在 `waiting …` 之前、`actions` 里撤销各只有一次、备份已空。新建的 `run_with_the_file_backend_online` 是 `run_with_the_file_backend` 的联网版（GeoIP 的两个地址在回环）。没有计时，但仍单独循环跑了二十次，全过。 | 本提交 |
| 33 | 8 | 评审修正（第 1 轮），三处。① 协调者的裁定 R19（订正第 31 行“`call` 的 `Err` 当 `None` 并写一行 DEBUG”）：`PlatformSystemDns::servers()` 里槽中的 `None` 是“机器自己的列表”，不是“查找失败”，所以 `read_physical`（`mod.rs:748`）在 `call` 回 `Err` 时改成写 `Some(Vec::new())`（与 Linux 读不了 resolved 的 `resolv.conf`、macOS 没有主卡时两个平台自己的回答一致：查找失败，不退回机器自己的列表，设计 6.3），说明与 DEBUG 那行的文字照实改；`None` 只来自后端自己该回它的那几种（Linux 还没接管、文件后端没那个键、`Off`、以后的 Windows）。用例 `a_read_that_does_not_come_back_is_no_servers`（`mod.rs:1884`）：`Mock` 的 `physical_dns` 按 `physical_panics` panic，`spawn_blocking` 接住、`call` 变成 `Err`，打开之后槽里是 `Some(vec![])`（标准错误里那段 panic 是预期的）。② `PlatformBackend::close`（`backend.rs:235`）只改注释：TUN 在 `JOIN_LIMIT` 内结束了的话这里是最后一个所有者，丢掉它就关描述符、网卡随之没了；没在时限里结束的话，设备在 TUN 的放手时才关。③ 记一笔：第 30、31 行里 `run.rs` 的三个行号由 `e2f9c44`、`de07cbb` 改成了 `efa42ef` 上的（`:1508` / `:1509` → `:1514` / `:1515`，`:1734` → `:1740` 两处），与那两行的「提交」一栏（`41f3e77` 的“本提交”）不一致，违反了“计划末尾两张表里已写下的行不改写”；两行其余行号是 `41f3e77` 上的。以后：一行的行号以它的「提交」一栏为准，后来的提交挪了行不再去改；这两行不再改（也不改回去）。 | 本提交 |
| 34 | 9 | 协调者的裁定 R17（订正任务 9 需求的地址与夹具）：需求里凡写 `10.231.0.1` 的服务端地址（两条用例的目标、`dns-server`、请求记录与 DNS 服务端的断言）换成 `10.231.1.1`——b 的回环上另加的 `10.231.1.1/32`（回环先起来），veth 两端的 `/30` 照旧。理由：rurge 的基础前缀（`8.0.0.0/5` 等，`route/set.rs` 的 `BASE_V4`）比内核给 veth 的直连路由 `/30` 短，到 `10.231.0.1` 的流量永远走 veth、不进虚拟网卡：第一条用例在请求记录里找不到会话，第二条也证明不了防环；到 `10.231.1.1` 的流量长于默认路由，应用的会进虚拟网卡，rurge 自己的连接与查询若不绑在 veth 上也会绕回来——用例测的正是这个。`Lab` 在起 `rurge` 之前写 `/etc/netns/rl2-a-<pid>/resolv.conf`（一行 `nameserver 10.231.1.1`），`Drop` 里删掉 `/etc/netns/rl2-a-<pid>/` 整个目录：`ip netns exec` 会把 `/etc/netns/<名字>/` 下的文件绑到 `/etc/` 上（ip-netns(8)），不写的话，虚拟机的 `/etc/resolv.conf` 是 resolved 的存根时，a 里的 rurge 会经 D-Bus 对**宿主**的 resolved 执行 `resolvectl dns rurge …`，宿主看不到 a 里的网卡、接管失败、增强模式打不开；写了之后 rurge 看到的不是存根，WARN 一行、不接管系统 DNS，三条用例都不需要接管。`tests/privileged/README.md` 照实写了这两点与那个目录。 | 本提交 |
| 35 | 9 | M4b 计划「延后事项」第 11 行（`rurge_platform::socket::egress::subnets` 读本机网卡那一步没有用例走到、防环没有在真机上验过，去向 M4c）：本任务的三条用例在虚拟机里跑起来时会走到它们——`PlatformBackend::subnets` 真的读 a 里的网卡（`l2a` 与 `lo`），rurge 自己的连接与 DNS 查询经 veth 出去、不进虚拟网卡。**这些用例还没有在任何地方运行过**，所以 M4b 那一行仍然开着。 | 本提交 |
| 36 | 9 | 执行者的裁定（六项）。① `l2-helper` 的 `get` 带密钥：`get <URL> <密钥>`，请求头是 `X-Key`（派工提示第 2 条；计划的任务 9 表里只写了 `get <URL>`）。② DNS 报文的编码与解码放在包的库（`src/lib.rs` 与 `src/dns.rs`），六条单元用例在库里，`[[bin]]` 的 `test = false` 照计划保留；这样它们在本机与 CI 上都跑。③ 第三条用例除 `net.ipv4.conf.all.rp_filter` 外也把 veth（`l2a`）自己的值设成 1：虚拟机的 `net.ipv4.conf.default.rp_filter` 已是 2 时，`enhanced/linux.rs` 的 `adjust_saved` 读到两个值里较大的一个是 2，判为“不严格”、不放宽，用例就证实不了撤销。④ 端口：`http-api` 用 `127.0.0.1:0`，实际端口从 `rurge run` 打印的 `api on http://…` 那一行读；`17000`（回显服务端）与 `53`（DNS 服务端，`dns-server = 10.231.1.1` 的缺省端口）是写死的，GeoIP 的两个地址指的 `127.0.0.1:20001` 也是——它们都在用例自建的命名空间里（服务端在 b，GeoIP 的那个在 a），那里只有用例自己起的监听器，宿主上看不到，所以“从 20000–29999 里试绑一次”那一条不适用（它说的是宿主上的端口，为的是不撞上宿主上与并行的别的用例）；20001 落在那个范围里，a 里没有别的东西绑它（`http-api` 的 `:0` 由内核挑，临时端口从 32768 起）。⑤ `tests/linux_netns.rs` 在本机编译不到，另做了一遍类型检查：临时去掉文件头的 `#![cfg(target_os = "linux")]`、跑 `cargo clippy --workspace --test linux_netns -- -D warnings`（`rurge_platform::route::linux` 不带 `cfg`，所以整份能编），再把那一行改回去；改回去之后跑 `cargo test --workspace --test linux_netns`，本机是 0 条用例。⑥ `l2-helper` 的 `echo` 与 `dns` 绑定之后打印一行 `listening on <地址>`，用例等这一行、不固定等待；客户端 5 秒读写时限，成败是退出码。 | 本提交 |
| 37 | 9 | 评审修正（第 1 轮），五处。① 协调者的裁定 R20：三条用例在同一个测试二进制里并行跑，`std::process::id()` 对它们都一样，`rl2-a-<pid>` / `rl2-b-<pid>` / `/etc/netns/rl2-a-<pid>/` 因此相撞——第二个 `Lab` 的 `ip netns add` 失败、它的 `Drop` 又删掉第一个还在用的两个命名空间与那份 `resolv.conf`。`Lab` 的名字加上了取自 `static NEXT: AtomicU64` 的 `<n>`，并加了 `static ONE_AT_A_TIME: Mutex<()>`：`Lab::new` 一开始（建任何东西之前）拿锁，守卫是 `Lab` 的最后一个字段（`Drop` 清理完、`TempDir` 删掉之后才放），锁被毒化时 `PoisonError::into_inner` 照样拿。② 协调者的裁定 R21：`start_rurge` 不再列七个变量，而是把环境里名字以 `RURGE_` 开头的每一个都 `env_remove`（`RURGE_API_KEY`、`RURGE_REQUEST_LOG_SIZE`、`RURGE_OUTBOUND_MODE`、`RURGE_NO_NETWORK` 之类留着会改掉这一次运行），并把 `XDG_DATA_HOME` / `XDG_CONFIG_HOME` 指到 `Lab` 临时目录下的两个目录（用例以 root 跑，root 的家目录不是测试文件的地方）；这两条订正第 36 行没有提到的并行与环境。③ 第二条用例里“查询出了 veth、没经过虚拟网卡”那行注释只说了它证明得了的一半：来源是 `10.231.0.2` 只证明查询离开了 a、到了 b，绕不绕虚拟网卡由请求记录那条断言证明（绕一圈的查询同样以 `10.231.0.2` 出去）；注释照实改，代码不动。④ 文字：README 的「怎么跑」（root、`iproute2`、iputils 的 `ping` 与它用的 `-W`、`/dev/net/tun`；不需要 systemd-resolved，因为 a 的 `resolv.conf` 由用例自己写）与「它们改什么」（宿主上看得到的是 `/run/netns/` 下两个命名空间与 `/etc/netns/rl2-a-<pid>-<n>/`；`/etc/netns/` 原来没有时由用例建出、用完留着）照实改；`src/lib.rs` 的说明改成“`l2-helper` 在三个平台上都编译、`clippy` 也看得到，只有 `tests/linux_netns.rs` 是 Linux 的”；`tests/privileged/Cargo.toml` 与 `README.md` 末尾的换行补上。⑤ 第 1.1 节那句“`Cargo.lock` 包数不变”不对：`[[package]]` 从 518 个到 519 个，多的就是 `rurge-privileged-tests` 自己，没有新的第三方包——订正 `1eb68db` 的提交说明与报告第 1.1、6 节；提交说明不改（不 `--amend`）。 | 本提交 |
| 38 | 11 | 任务 11 照派工做（协调者的裁定 R9、R18）。`docs/api/phase3.md` 在 M4b 一节之后、「没有覆盖的」之前新写一小节「M4c：Linux 与 macOS」：开头一段点名订正 M4 一节开头粗体的「这一版开不了增强模式」与它下面三条、「只供测试的后端」里的“（这一版对每个动作答‘不可用’）”、M4b 一节的“平台后端仍然答‘这一版还不能’…”、“没有覆盖的”里的三条；小节内依次是「打开时改了什么，怎样回去」（一表）、「日志行」（INFO / WARN / ERROR 全列，DEBUG 只列带名字或地址的，文本逐行照代码）、「接管期间 `server:system` 去哪里问」（两段，含 R16、R19 与 IPv6 RDNSS 的登记）、「启动时的恢复多了一处」（R10）、「权限不够与网卡正在被使用」（三句 Linux + 一句 macOS + 其余一句）、「脚本与程序」（P11、P12）、「另一个全隧道 VPN 开着时」（P3、P22）、「手工恢复 macOS 的 DNS」，以及它自己的 `#### 没有覆盖的`。`docs/testing.md` 在「测试约定（全文）」末尾新写一段「阶段 3 / M4c 起：平台的系统一侧与设备」（系统一侧不带 `cfg` 与 `FakeRunner` + 临时目录、没有一条缺省用例开设备（P19）、夹具手写（P20）、L2 的门控与跑法（R17、R20、R21，没有运行过）、clippy 的注解（P18））；「只供测试的后端」里 `RURGE_ENHANCED_BACKEND` 不设时的说法照 R9 改成“Linux 与 macOS 上是真的系统一侧，Windows 答不可用”。`CLAUDE.md`：「现在在哪里」第三条照 R18 改写（阶段 3 做完 M4c、Linux 与 macOS 上真的能开、真机与 L2 没有跑过、CI 没有见过、Windows 照旧、M4d 未写），`unsafe_code` 一段核对过：九个函数与 `tests/unsafe_functions.rs` 的 `LISTED` 一致，没有改。`docs/status.md`：阶段 3 下新写一节「M4c（Linux 与 macOS）」，附录末尾加 M4c 的依赖账（`rurge-platform → tokio`、dev-dependencies 的 `tracing-subscriber`、`tests/privileged`，`[[package]]` 518 → 519）。兼容性清单 9.1：第一行的“目前”照 R9 写，登记四项（另一个全隧道 VPN、IPv6 RDNSS、Linux 的 `rp_filter`、没有 resolved 的 Linux 不接管 DNS）。`crates/rurge-platform/src/route/steps.rs` 的 `OnFailure::Ipv6Off` 说明照 `exec::run` 改（R15）。门禁（一台 Windows 11 x86-64 开发机上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过，`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `gate-task-11.log`）86 个结果行、**4583 通过、0 失败、13 忽略**，与基线相同；没有二进制异常退出，没有重跑。 | 本提交 |
| 39 | 11 | 执行者的裁定（三项）。① 兼容性清单 9.1 里“只处理 TCP / UDP / ICMP”与“fake IP `198.18.0.0/15`”两行备注中的“`rurge run` 今天不起虚拟网卡”与“真实设备随 M4”顺带照实改了（M4c 之后 Linux 与 macOS 上真的起网卡，Windows 到 M4d 为止不起）：派工只要“目前”一栏与四项登记，但不写这两处它们就与事实相反。② IPv6 RDNSS 那一条只照 P10 的说法写“两个平台的两处读法都读不到”，没有加任何观察到的细节。③ `docs/testing.md` 的 L2 跑法只链接 `tests/privileged/README.md`，不把里面的 `cargo test -p rurge-privileged-tests` 抄进来（派工与全局约束都不 `-p`）。 | 本提交 |
| 40 | 11 | 派工 F1（终审 I1，协调者的裁定 R23）：`rp_filter` 只在需要时放宽。管理器只在这一次 `adjust_undo` 回了 `Some`、**或者**备份里已有 `Stage::Adjust` 的一项时才调 `adjust()`（打开第 8 步与 `adjust_again`，`crates/rurge/src/cli/enhanced/mod.rs`）；备份里已有时再写一次是幂等的，别的程序（例如重新套用 `sysctl.d`）把 `all` 改回严格之后，下一次网络变化还能补上；`on.adjust = true` 照旧（它说的是这一阶段到过）。`Backend::adjust`（`crates/rurge/src/cli/enhanced/backend.rs`）与 `System::adjust`（`crates/rurge-platform/src/enhanced/mod.rs`）的说明各补一句。订正计划正文第 572 行“`adjust`：`all` 写 `2`”没有条件的那一句。`Mock` 加开关 `no_adjust`（打开时 `adjust_undo` 回 `Ok(None)`，像有效值不是 1、或没有主网卡的 Linux），三条用例：开关打开、备份空时，打开之后 `calls` 里有 `adjust_undo`、没有 `adjust`；开关打开、备份里有 `adjust` 那一项时照样写；开关打开、备份里没有那一项时，一次网络变化之后同样没有 `adjust`。现有用例的期望一条也没有改。 | 本提交 |
| 41 | 11 | 派工 F2（终审 I2）：L2 第三条用例 `a_killed_run_is_undone_by_the_next_start`（`tests/privileged/tests/linux_netns.rs`）在对的流上等对的那一句。那一次启动的 Profile 没有远程来源，走的是主路径，恢复的那行是 INFO `restored what the enhanced mode left last time`，而日志层写在标准输出（`crates/rurge/src/cli/run.rs` 的 `init_logging` 早于恢复那一步）；它原来等在标准错误上的 `a previous run left changes of the enhanced mode; they were undone` 只由 R10 那条路（等 Profile 的来源之前）写，这条用例不走它——那条路由 `crates/rurge/tests/run/enhanced_mode.rs` 用文件后端测。第一次启动之后对 `all` 是 `"2"` 的断言改成有界的轮询：最多 10 秒、每 100 毫秒读一次，到时仍不是 `"2"` 才失败（派工 F1：打开时还没有主网卡的话，放宽在网络任务第一次读到网络、`adjust_again` 那一步才做）。用例的说明写清它走的是哪一条恢复。另一处随之必要的改动：`Started` 的 `stderr` 从此没有用例读（它原来只被这一条用例读）；读线程照旧保留（丢掉 `Lines` 会让读线程停下、管道被填满），字段改名为 `_stderr`，否则 `dead_code` 在 `-D warnings` 下让编译变红。**这一条没有红**：文件整个在 `#![cfg(target_os = "linux")]` 之后，本机编译不到、也不能跑（不设 `RURGE_TEST_PRIVILEGED`）；没有编译过，第一次 CI 核对编译，运行要项目所有者决定（总设计 Q4）。 | 本提交 |
| 42 | 11 | 派工 F3（终审 M1）：做了一半的接管也算接管过。`LinuxSystem::take_dns`（`crates/rurge-platform/src/enhanced/linux.rs`）把 `took_dns.store(true, …)` 挪到 `self.run(&steps)?` 之前：失败的步骤之前的设置已经在链路上，而 `give_dns_back` 的 `resolvectl revert` 对什么也没设的链路没有影响（它本来就是 `Ignore`）；不挪的话，前几步成了、后面一步失败时 `198.18.0.2` 与 `~.` 要等设备关掉才跟着没，在那之前全机的查询发往一个没人答的地址。注释写明了为什么。新增用例 `a_takeover_that_fails_partway_is_given_back`：假执行器在 `domain` 那一步失败（它是 `Fail`；`flush-caches` 在 F6 之后是 `Ignore`），`take_dns` 回 `Err`，接着 `give_dns_back`，执行器看到了 `resolvectl revert rurge`；改之前没有，红的。 | 本提交 |
| 43 | 11 | 派工 F4（终审 M2）：关闭时设备先关、再写回 `rp_filter`。`close`（`crates/rurge/src/cli/enhanced/mod.rs`）的 `on.dev.clone()` 改成 `on.dev.take()`：从前 `on` 按值留着一个 `Arc`，交给 `backend.close(dev)` 的只是复制品，设备要在写回 `rp_filter` 与改写备份之后才关，设计第 9.4 节与 M4-D16 的次序因此不成立；`PlatformBackend::close`（`crates/rurge/src/cli/enhanced/backend.rs`）的注释照实补了一句“取出来、不复制”。用例 `a_close_hands_the_device_over_before_the_adjustments_go_back`：`Mock::close` 记下 `Arc::strong_count(&dev.device)`，TUN 已经在 `ended()` 回过的关闭里记到 1（改之前是 2），并断言 `close` 排在 `undo adjust` 之前。假的 TUN 不握设备的复制品（`MockStarter::start` 忽略它），所以用不着等它结束。 | 本提交 |
| 44 | 11 | 派工 F5（终审 M3 的一半，协调者的裁定 R24）：错误文本里不带脚本的路径。`run_step`（`crates/rurge-platform/src/route/exec.rs`）在写了脚本的步骤失败时，把错误里脚本路径（`path.to_string_lossy()`）的每一处换成 `…`——命令行里的与工具原文里的都换（`ip -batch` 自己写 `Command failed <路径>:<行>`）；DEBUG 那一行照旧带路径（DEBUG 可以）。`run_step` 的说明补了一段。订正计划正文第 101 行把“数据目录里脚本的路径”算作可以出现的那一句；错误里 rurge 自己的网卡名、地址与前缀不动，要不要照 P7 改成固定的一句归项目所有者（延后第 10 行）。新用例 `a_failing_script_leaves_its_path_out_of_the_error`：假执行器回显命令行与 `ip -batch` 的原文，错误里没有 `scratch` 的路径、有 `-batch …` 与 `Command failed …:1`；改之前有路径，红的。 | 本提交 |
| 45 | 11 | 派工 F6（终审存疑 Q5，协调者的裁定 R25）：接管时把 rurge 链路上全局的 DoT、DNSSEC、LLMNR、mDNS 关掉，清缓存失败不算错。`take_dns`（`crates/rurge-platform/src/route/linux.rs`）的步骤改成八步：先关四项（各 `Ignore`：尽力而为，老的 resolved 或没编进某项支持时失败不是不接管的理由），再设服务器（`dns` / `domain` 照旧 `Fail`），`default-route` 照旧 `Ignore`，最后 `flush-caches` 从 `Fail` 改成 `Ignore`（失败的后果只是缓存里已有的名字到期之前照旧是真地址）；注释写清了道理。`give_dns_back` 的 `revert` 把它们一并撤掉，不用改。两条现有用例由派工点名修改：`route/linux.rs` 的 `the_system_dns_is_taken_over_in_four_steps` 改成 `the_system_dns_is_taken_over_after_the_resolvers_own_settings_go_off`、期望照八步改，`enhanced/linux.rs` 的 `the_dns_is_taken_behind_the_stub_with_resolvectl` 列的命令跟着改；先改期望、看到红，再改实现到绿。 | 本提交 |
| 46 | 11 | 派工 F7（终审存疑 Q4，协调者的裁定 R26）：Linux 的权限提示只说 root。`explained`（`crates/rurge-platform/src/tun/frame.rs`）的 Linux `PermissionDenied` 一句逐字改成 `the enhanced mode needs root (in a container, root with CAP_NET_ADMIN)`，macOS 那一句不动；说明里补了为什么：`ip` 与 `resolvectl` 是机器上的外部程序，`setcap` 给 rurge 自己的文件能力到不了它们（`ip` 配地址那一步才以 `Operation not permitted` 失败），而容器里的 root 可能被拿掉这个能力。本行订正设计第 8.5 节那一句。现有用例 `open_errors_say_what_is_missing` 的期望照改（派工点名），先改期望看到红。 | 本提交 |
| 47 | 11 | 派工 F8（终审 M4，协调者的裁定 R27）：macOS 接管之前的名单为空就当不知道。`physical_dns`（`crates/rurge-platform/src/enhanced/macos.rs`）只在 `before` 非空时直接回它；空的当成不知道，往下走服务原值与 DHCP 的读法（网络变化时清 `before` 的那一支照旧）。`/etc/resolv.conf` 由 configd 异步重写，关闭之后马上再打开（API 连按两下）、或崩溃恢复刚写回就打开时，读到的可能只有 `198.18.0.2`，从前就存成 `Some(空)`、`server:system` 一直没有服务器。两个函数的说明照实改了。新用例 `a_list_that_is_empty_after_rurges_space_is_unknown`：`resolv.conf` 只有 `198.18.0.2` 与 `fd00:6152::2`，执行器回 `LIST`、`SERVICE_ORDER` 与 `getpacket en0` 的 `PACKET`，`dns_saved` 之后 `physical_dns(Some("en0"), &backup, false)` 是 DHCP 的 `192.168.4.1`；改之前是空的，红。现有的 `the_responder_is_never_a_physical_server` 不改（它没给那两条命令的回答，改之后仍是空的），跑过、仍绿。 | 本提交 |
| 48 | 11 | 派工 F9 的任务 11 留下的四处措辞与兼容清单开头一句（D1 ～ D5，协调者的裁定 R22）。D1：凡说 M4c 之后 Linux / macOS 增强模式的地方改成“有了平台后端的代码（不再答‘不可用’），照代码会建网卡、配地址、装路由、接管系统 DNS”，紧跟“这段代码一次也没有编译过（开发机编译不到，CI 还没有见过）、真机上没有跑过”；改了 `CLAUDE.md` 的「现在在哪里」那一句、`docs/status.md` 的 M4c 开头两处、`docs/api/phase3.md` 的 M4c 开头一条、`docs/surge-compatibility-matrix.md` 的 9.1 三处（“真的开”“起真的网卡”“上真的网卡”）与 `docs/testing.md` 的 D4 那一句（括号里补了同一句）；`git grep` 看过，M4a、M4b 两节与 `phase3.md` 的 M4 一节里的历史段落不动，由 M4c 一节订正。D2：`docs/api/phase3.md` 的「打开时改了什么」表里“脚本文件”一行 macOS 一栏不再是“同左”（macOS 每一步一个进程、不写脚本，`route/macos.rs` 里没有 `SCRIPT`）；「脚本与程序」第一条写明只有 Linux 写脚本（Windows 的 `netsh -f` 以后也写，M4d）。D3：`docs/status.md` 的“两条网络命名空间”改成“两个”，“这一次只改文档与一行注释”改成“……与 `steps.rs` 里的一处注释”。D5：兼容清单开头的“最近一次”改成阶段 3 / M4c，并在 M3 那一句之后加了一段 M4a ～ M4c。 | 本提交 |
| 49 | 11 | 派工 F9 的其余文档与两处登记。`docs/api/phase3.md` 的 M4c 一节：「系统 DNS」一行的 Linux 一栏照 F6 写全八步；INFO `loosened …` 的“什么时候”照 F1；DEBUG `cannot read /etc/resolv.conf` 与「接管期间 `server:system` 去哪里问」的 macOS 一条照 F8；「权限不够与网卡正在被使用」照 F7；「脚本与程序」照 D2 与 F5；「没有覆盖的」里 macOS 的 `-listnetworkserviceorder` 一条照协调者的裁定 R30 改成“格式与公开的样本对得上（两个出处），配对与 `ipconfig getpacket` 周围的行走仍要真机核”（`docs/status.md` 同一条同样改，延后第 2 行不改、仍开着）。兼容性清单 9.1：任务 11 那四行之后新加一行“接管期间别的链路上更具体的 DNS 域”（协调者的裁定 R29，终审 M5；Linux 的 `~.` 与 resolved 的最长匹配、VPN 的拆分 DNS、macOS 没有核对），「实现基础」一行照 F7 改，`rp_filter` 一行读过、与 F1 一致不用改。`docs/requirements.md` 的两处（2.1 的 Linux 行、6. 的虚拟网卡行）照 F7 改成 root（容器里还要 `CAP_NET_ADMIN`）。`docs/status.md` 的 M4c 一节：门禁之后加一段“整分支终审之后的一轮修正”（F1 ～ F8 各一句、本轮门禁的数）；“以后的会话必须知道的事”加 F1、F5、F6、F7 四条；“去向写着项目所有者的”加延后第 10 行（错误里 rurge 自己的网卡名、地址与前缀）；“没做的”加 Q1 ～ Q3、Q6 在手工清单（延后第 6 ～ 9 行）。延后事项第 6 ～ 10 行照协调者的裁定 R28 加。 | 本提交 |
| 50 | 11 | 最终修正第 1 轮（协调者的裁定 R31 订正 R22）：第 48 行的 D1 写下的“这段代码一次也没有编译过（开发机编译不到，CI 还没有见过）”说过了头，本行订正它——动系统的那一半不带 `cfg`（计划 P2），在开发机上编译、只用假的执行器测过；一次也没有编译过的只是开设备的代码（`tun/linux.rs`、`tun/macos.rs`）、挑平台的 `cfg`（`tun/mod.rs` 与 `enhanced/mod.rs` 的 `system()`）与 L2 用例文件，正是 `docs/status.md`「本机没有编译过的代码」一条列的。十三处照改（十二行；`docs/surge-compatibility-matrix.md:708` 那一行里两处）：`CLAUDE.md` 一处、`docs/api/phase3.md` 四处、`docs/status.md` 三处、`docs/surge-compatibility-matrix.md` 四处、`docs/testing.md` 一处，新文是“其中开设备的代码与挑平台的 `cfg` 一次也没有编译过（这台开发机编译不到，CI 还没有见过），动系统的那一半在这台开发机上编译、只用假的执行器测过，真机上都没有跑过”一句及其按上下文缩短的形式；顺带去掉 `crates/rurge-platform/src/route/linux.rs` 测试里 `plain(…)` 末尾的一处多余逗号（`f9583a3`）。第 48 行本身不改。 | 本提交 |
| 51 | 10 | 推送之后的 CI 读回（与 M4d 的任务 8 合成一次停点，M4d 记录第 10 行）：项目所有者 2026-10-28 推送了 `3a76b80`（M4a ～ M4d 的 116 个提交）。运行 40611702392（第 13 次运行）的四个任务——`bench` 与 `check` 的 ubuntu-latest、macos-latest、windows-latest——都成功：三个平台的 `cargo fmt --all --check`、`cargo clippy --all-targets -- -D warnings`（工具链 1.89.0）、参照服务端的安装（各平台按工作流的条件装或跳过）与 `cargo test --workspace --no-fail-fast` 都过，测试一步 Linux 8 分 15 秒、macOS 10 分 36 秒、Windows 14 分 8 秒。**Linux 与 macOS 专有的代码第一次被编译**——`tun/linux.rs`（`set_iff`）、`tun/macos.rs`（`utun_connect`）与挑平台的 `cfg`——clippy 没有警告，缺省用例通过；没有要修的，Linux 与 macOS 两个平台的 `check` 都绿，Windows 照旧绿（判据）。注解只有平台的：四个任务各一条 Node.js 20 弃用的警告（`actions/checkout@v4` 被强制跑在 Node.js 24 上），跑在 ubuntu-latest 上的两个任务各一条“`ubuntu-latest` 将从 2026-10-19 起迁到 Ubuntu 26”的通知，`check (macos-latest)` 一条 arm64 排队的通知；不改 CI（M4d 延后第 11、12 行）。结论读自公开的 API；任务日志要登录才能读，各平台跑了多少条用例、哪些打印了跳过没有读到。**没有变的**：缺省的用例不建网卡、不改系统设置，两个平台的设备与系统改动没有真的运行过、没有在真机上验证；CI 没设 `RURGE_TEST_PRIVILEGED`，L2 的三条（任务 9）在 Linux 上编译了、打印原因就返回，仍没有运行过（延后第 1 行） | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | L2 特权用例（`tests/privileged`，任务 9）写好了没有在任何地方运行过；与它一起没有验过的还有 `rurge_platform::socket::egress::subnets` 读本机网卡那一步与防环（M4b 延后第 11 行仍开着） | 项目所有者：总设计 Q4（在 CI 上启用、一次性 Linux 虚拟机） |
| 2 | macOS 的 `-listnetworkserviceorder` 输出格式凭记忆写（P20 没有可引用的样本），`ipconfig getpacket` 周围的行走同理；服务与网卡的配对只能在真机上核对 | 手工验收（`docs/acceptance/phase3-manual.md` 的 M4 一节，M4d 写） |
| 3 | IPv6 路由器通告（RDNSS）给的 DNS 服务器两个平台都读不到（P10） | 登记（兼容性清单 9.1）；以后的版本 |
| 4 | `PlatformDevice` 的两个 future 在 Linux / macOS 上是不是 `Send`、Linux 与 macOS 的设备与系统一侧第一次编译，都没有在开发机上编过（任务 8） | CI 读回（M4c 的任务 10，与 M4d 的任务 8 一起）；推送归项目所有者 |
| 5 | `run.rs` 里把 `physical_dns` 的结果交给管理器的那一行没有端到端用例（`server:system` 在测试里会离开回环） | 手工验收（M4 一节，M4d 写） |
| 6 | `networksetup` 失败时的退出码（终审 Q1）：参数无效、服务不在时它若仍以 0 退出，`take_dns` 会把没接管的当成接管了，`undo` 会把没写回的当成写回了、从备份里删掉；没有找到出处 | 手工验收（M4 一节，M4d 写）：例如 `networksetup -setdnsservers <不存在的服务> 1.1.1.1; echo $?` |
| 7 | 一个服务读得了原值、改不了时整个打开失败（终审 Q2）；有 VPN（NetworkExtension 装的服务）的机器上可能因此完全开不了 | 手工验收（M4 一节） |
| 8 | 运行期间网络服务改了名（终审 Q3）：备份按名字记，`undo` 把旧名字当作“服务没了”，改名的那个服务的 DNS 留在 `198.18.0.2` | 手工验收（M4 一节）；按服务 ID 记（`scutil`）是以后的版本 |
| 9 | NetworkManager 会不会盖掉 rurge 链路上的逐链路设置（终审 Q6） | 手工验收（M4 一节）：开着增强模式时 `resolvectl status <链路>` 仍是 `198.18.0.2` 与 `~.` |
| 10 | 非 `networksetup` 步骤失败时，错误文本带 rurge 自己的网卡名、地址与前缀（含 `tun-excluded-routes` 的前缀），进 ERROR、`rurge run` 的标准错误与 API 的 500（终审 M3 的另一半；计划第 101 行的裁定，设计第 13 节允许地址） | 项目所有者：要不要照 P7 改成固定的一句、原文进 DEBUG |
