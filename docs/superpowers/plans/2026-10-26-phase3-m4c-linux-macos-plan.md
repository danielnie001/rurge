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

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
