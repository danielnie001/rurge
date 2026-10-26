# 阶段 3 / M4a「平台无关的骨架」实施计划

> **执行方式**：交给 Pi，在一个会话里按任务的次序逐个做（技能 `superpowers:executing-plans`；没有子代理，不并行、不跳）。每个任务：先写失败的用例、看到红，再实现到绿；跑门禁；对照本任务的「判据」与「全局约束」逐条自查；再提交（每个任务一个或几个提交，**只在本地，不推送**——这是项目所有者把执行交给 Pi 时定下的做法，M4 设计第 2 节）。本计划写到"改哪里、接口是什么、要哪些用例、判据是什么"为止；与计划的出入记进末尾的「执行期修正记录」，**不改计划的正文**。做完任务 10 停下，交项目所有者评审。

**目标**：把增强模式的生命周期在不碰系统的前提下整个做出来：路由集的计算与三个平台的步骤（都是数据）、`state.json` 的增强模式备份与崩溃之后的恢复、可执行文件里的管理器与后端接口、`--enhanced-mode` 与 `/v1/features/enhanced_mode`、退出的次序、接管暂停的增强模式一半、重载与网络变化的增量、`rurge check` 的四个新诊断、`external` 的 `addresses` 进路由的排除；顺带做掉 M3 延后事项里去向是 M4a 的六条（`rurge-tun` 的三条、`start_tun` 的三条）。平台后端在这一份对每个平台都答"这一版还不能"，所以这一份做完，使用者仍然开不了增强模式；全部行为经一个只供测试的文件后端端到端地测。

**架构**：`rurge-platform` 多一个 `route` 模块：`RouteSet::compute` 是纯函数；三个平台把抽象的动作翻成 `Step`（一个程序、它的参数、可选的脚本、失败算不算错），翻译不带 `cfg`，三个平台在任何平台上都编译、都测；执行在 M4c / M4d。可执行文件多一个 `cli/enhanced/`：管理器 `EnhancedMode` 照 `SystemProxyManager` 的样子，持有后端、`StateStore`、一个起 TUN 的接缝与出口表的槽；后端接口 `Backend` 只有抽象的动作，会在崩溃之后留下的动作分两步（先说撤销它要什么、管理器写进备份、再做）。引擎的 `start_tun` 变成返回 `Result` 的全函数，一个引擎同时只有一个 TUN。

**技术栈**：ipnet 2（`rurge-config` 已用）、tokio、serde / serde_json；不新增外部包。

**依据**：M4 设计 `docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md`（下面写"设计"指它）第 2 ～ 5、9、11 ～ 15 节与 M4-D1 ～ D5、D14 ～ D19、D21 ～ D23、D26、D28；阶段 3 总设计 `docs/superpowers/specs/2026-10-16-phase3-enhanced-mode-design.md` 第 9 节；M3a 计划延后第 17 行的第二 ～ 四条、M3b 计划延后第 8、14、23 ～ 25 行（去向是本计划，设计第 15 节）。设计第 17 节的核对事项里本计划用到的（V2 的写法、V5 ～ V7、V10、V11）已在写计划时核对，结果在「计划期决定」P1 ～ P5、P18；任务 0 再对着当时的代码核一遍。

## 全局约束

- 新的依赖边只有：`rurge-platform → ipnet`、`rurge → rurge-tun`（可执行文件要用 `rurge_tun::device::Device`、`TunOptions`、`Pinger`）、`rurge → ipnet`（要用时）；都是工作区已有的包，`Cargo.lock` 的包数不变（518 个）。`rurge-api` 照旧只依赖 `rurge-engine` 与 `rurge-config`（它要的 TUN 计数经 `rurge-engine` 重新导出）；`rurge-platform` 照旧不依赖任何工作区 crate；`rurge-config` 照旧不依赖任何工作区 crate。新加的边记进 `docs/status.md` 的附录（任务 10）。
- `unsafe_code`：本计划不加放开的函数（`crates/rurge-platform/tests/unsafe_functions.rs` 的 `LISTED` 仍是七个）。
- **本计划里没有任何代码执行系统命令、建网卡、改路由或系统 DNS**：三个平台的步骤只生成为数据（任务 5）；平台后端对每个动作都答"这一版还不能"（任务 7）；用例只经文件后端与 `off`；起 `rurge` 二进制的辅助函数缺省 `RURGE_ENHANCED_BACKEND=off`（任务 7）。
- 测试只用回环与端口 0；CLAUDE.md「测试约定」整节适用：起守护进程的用例经 `crates/rurge/tests/cli.rs` 的 `rurge_command()` / `rurge_process()` 并给 `--data-dir`；等待都有时限；"断言什么都没发生"之外不用固定的 sleep；新写的计时用例提交前单独循环跑二十次以上；抓日志用 `LogCapture`（跑在当前线程的运行时上）。网络文件里出现的文档地址（`2001:db8::/32` 一类）只用来描述网络，从不作为被拨的目标。
- 名字与地址只在 DEBUG（设计第 13 节、M4-D28）：INFO 只说开、关、恢复、路由的条数与 IPv6 开没开；路由的前缀、网卡名、DNS 服务器、网络服务的名字只在 DEBUG；WARN 与 ERROR 里带的错误文本不另加地址。
- 锁不跨 `await`（`StateStore` 自己的锁是 tokio 的，照它今天的用法）。
- 先写失败的用例、看到红，再实现到绿。现有用例变红时：计划点了名的照计划改，在提交说明里说；没点名的停下来，记进修正记录，不顺手改期望值。
- 门禁（每个任务末尾一次）：`cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test -j 8 --workspace --no-fail-fast`。本机的 msvc 工具链没有 rustfmt：跑 `cargo fmt` 之前让 `RUSTFMT` 指向 gnu 工具链里同版本的 rustfmt（在用户目录下的 `.rustup\toolchains\stable-x86_64-pc-windows-gnu\bin\rustfmt.exe`；同版本，输出与 CI 一致）。聚焦地跑用例也经整个工作区（`cargo test -j 8 --workspace --lib -- <过滤词>`；集成测试 `cargo test -j 8 --workspace --test <名字> -- <过滤词>`），**不用 `-p`**（它另编一份依赖，`target/` 涨得很快，写满过 D: 盘）。跑全量之前 `df -h /d`，剩余不足约 15 GiB 就停下报告。测试二进制以 `STATUS_HEAP_CORRUPTION`（本机管控软件注入的钩子）或 `STATUS_ACCESS_VIOLATION` 退出时单独重跑那一个；rustc 在增量编译里报内部编译器错误时，删掉出错的那个目标在 `target/debug/incremental/` 下的目录再编。
- 只运行本仓库构建出来的二进制与系统自带的只读工具；不下载、不安装任何东西；**不在开发机上执行 `netsh`、`route`、`ipconfig`、`ip`、`resolvectl`、`networksetup` 这类读写网络的命令**，也不读真实的网络环境（它们的写法已按文档核对，P2 ～ P5）。
- 参考实现：GPL 的（sing-tun、sing-box、mihomo）只取事实，代码不抄；MIT / Apache 的（wireguard-go、wireguard-windows）可以简短引用并注明出处。
- 注释、日志与错误文本用英文，照所在文件的写法；提交说明照 `git log` 的样子（`feat(tun): …`、`feat(platform): …`、`docs: …`，标题用中文）。
- 计划没有写到、要做决定的地方：能按设计与总设计的原则定的，自己定，记进修正记录并写"执行者的裁定"；去向写着"项目所有者"的（设计第 20 节），或会改变设计的已决事项（M4-D1 ～ D30）的，停下来列出，交给项目所有者。

## 评审要点

这一份做完时使用者还开不了增强模式，但它定下的生命周期在 M4c / M4d 里原样用到真实的系统上。那时最可能咬到使用者的六种情形，各有负责的用例：

1. **打开到一半失败**（路由装不上、DNS 改不了）：已做的倒着撤销，`rurge run` 以 1 退出、API 回 500 带原因，`state.json` 不留撤销动作（任务 8 `a_step_that_fails_undoes_what_was_done_and_the_start_ends_with_1`、`post_failure_is_a_500_with_the_reason`）。
2. **`state.json` 写不进**（磁盘满、权限）：会在崩溃之后留下的那一步不做（任务 7 `a_backup_that_cannot_be_written_stops_the_step_before_it_is_done`）。
3. **强行结束之后再起**：上一次留下的按撤销动作恢复；配置读不了、有错误时也恢复（任务 7 `what_a_dead_run_left_is_undone_by_the_next_start`、`…_even_when_the_profile_does_not_load`；任务 8 `a_killed_run_with_the_enhanced_mode_on_is_undone_by_the_next_start`）。
4. **退出时有一个永不回来的拨号**：等 TUN 至多 3 秒，退出不挂住（任务 7 `closing_waits_for_the_tun_three_seconds_at_most`）。
5. **设备自己坏了**（网卡被移除）：增强模式整个关掉、系统 DNS 交还，不让系统 DNS 一直指着一个不存在的应答器（任务 8 `a_device_that_fails_turns_the_enhanced_mode_off`）。
6. **接管暂停**：暂停的网络上不打开、`POST` 只记下、离开暂停照记下的做；与系统代理那一半同一种语义（任务 9）。

---

## 文件结构

| 文件 | 任务 | 职责 |
| --- | --- | --- |
| `crates/rurge-tun/src/{lib.rs,dispatch.rs,context.rs,device.rs,tcp/shard.rs}`、`src/bytes.rs`（由 `src/udp/bytes.rs` 挪来）、`src/tests/{options.rs,rebuild.rs,…}` | 1 | `queue_bytes`；每个 TUN 一个 `RandomState`；片重建的 WARN 限频；`Device` 的约定 |
| `crates/rurge-dns/src/fakeip/file.rs` | 2 | 池的文件在 Unix 上 `0600` |
| `crates/rurge-engine/src/{tun.rs,engine.rs,lib.rs}`、`tests/{tun.rs,common/tun.rs,common/mod.rs}` | 2 | `start_tun` 返回 `Result`、一个引擎一个 TUN、`tun_ended`；重新导出 `Counters` |
| `crates/rurge-engine/src/state.rs` | 3 | `try_update`、`enhanced_backup` |
| `crates/rurge-platform/Cargo.toml`、`src/lib.rs`、`src/route/{mod.rs,set.rs,steps.rs,linux.rs,macos.rs,windows.rs}`（新） | 4、5 | 路由集；步骤与三个平台的翻译 |
| `crates/rurge-config/src/{diagnostic.rs,general.rs,spec/mod.rs,config.rs}` 与 `[Host]` 的读法所在的文件（任务 0 定位） | 6 | `W0035` ～ `W0038`；`addresses` 不再 `W0029`；`Config::external_addresses` |
| `crates/rurge-platform/src/socket.rs`（`egress` 子模块） | 7 | `EgressTable`（这一份只有数据；判定与读它的钩子是 M4b） |
| `crates/rurge/Cargo.toml`、`src/cli/enhanced/{mod.rs,backend.rs,file.rs,undo.rs,wanted.rs}`（新）、`src/cli/mod.rs` | 7 | 管理器、后端接口、平台占位 / 文件 / `off`、撤销动作、"想要的"的计算 |
| `crates/rurge/src/cli/{run.rs,network.rs}` | 7 ～ 9 | 恢复；开关、`Command::EnhancedMode`、`LoopControl`、退出的次序、设备坏了；`Takeover` 的一半；重载与网络变化 |
| `crates/rurge-engine/src/control.rs`、`crates/rurge-api/src/routes/features.rs` 与 `rurge-api` 的测试 | 8 | `Control` 的两个新方法；`/v1/features/enhanced_mode` |
| `crates/rurge/tests/cli.rs`、`crates/rurge/tests/run/enhanced_mode.rs`（新，`cli.rs` 的 `mod run` 的子模块，P17） | 7 ～ 9 | 辅助函数缺省 `off`；端到端用例 |
| `docs/api/phase3.md`、`docs/testing.md`、`CLAUDE.md`、`docs/status.md`、本计划末尾两张表 | 10 | 文档 |

模块的名字与拆法可以照实际调整，调了记进修正记录。

---

## Task 0：开工前核对

**做什么**：对着当时的 `main` 核对下面每一项，把出入记进「执行期修正记录」（一项一行，写"计划写 X，代码是 Y，按 Y 做"），按记录调整后面的任务；**不改计划的正文**。没有出入也记一行"核对过，没有出入"。

1. `git rev-parse HEAD`、`git status`：工作树干净；`HEAD` 是写计划时的提交（M4 的设计与四份计划的那个文档提交）或它之后只有文档的提交（`git log --stat <那个提交>..HEAD`）。不是的话把多出来的提交列进记录，碰到本计划要改的文件的逐个读。
2. `rurge-tun`：`dispatch.rs` 的 `route` 与 `shard_of`（`DefaultHasher::new()`，约第 94 行）与片的通道 `mpsc::Sender<Vec<u8>>`；`tcp/shard.rs` 的 `Shard::rebuild`（WARN 在约第 116 行）与片的任务 `run`（`recv_many`）；`udp/bytes.rs` 的 `QueuedBytes` / `Reserved`；`limit.rs` 的 `Limiter`；`TunOptions` 与 `check()`（`lib.rs` 约第 183 ～ 375 行；`shard_queue` 的检查在约第 333 行）；`Counters`（约第 625 行）；`Device`（`device.rs`）；`src/tests/options.rs`、`src/tests/rebuild.rs` 里现有的用例怎样让片停住、怎样让片重建；`src/tests/log_capture.rs`。
3. `rurge-engine`：`tun.rs` 的 `start_tun`（`git grep -n "start_tun("` 全工作区；写计划时只有 `tests/common/tun.rs` 约第 62 行与 `tests/tun.rs` 约第 560 行两处调用）、`fake_pool` 与 `let_go`；`state.rs` 的 `State`、`StateStore::update`、`write_atomic`（临时文件是 `state.json.tmp`）；`control.rs` 的 `Control`（约第 91 行）；`tests/tun.rs` 的 `an_association_returns_when_closed_and_lets_go_of_its_client`（约第 708 行）与 `tests/common/mod.rs` 的 `wait_until`（5 秒，约第 399 行）。
4. `rurge-dns`：`fakeip/file.rs` 的 `write`（约第 134 行：`std::fs::write` 临时文件再改名）。
5. 可执行文件：`run.rs` 的 `RunArgs`、`Command`（约第 281 行）、`LoopControl`、`Takeover`（`begin` / `follow` / `switch`，约第 600 ～ 700 行）、`network_changed`、`reload_and_refresh`、启动时打开系统代理的那一段与退出的那一段（`sysproxy.disable()` 在 `api_token.cancel()` 之前）、`holds_a_backup` / `recover_without_a_log` / `restore_what_a_crash_left` / `Lock::recover`、`run()` 里 `sysproxy.recover()` 与 `network::backend` 的位置；`sysproxy.rs` 的 `SystemProxyManager`、`backend()` / `backend_from`、`FileBackend`；`network.rs` 的 `spawn(…, wrap: fn(NetworkFacts) -> C)`、`facts_of`、`Started`；`runtime.rs` 的 `PlatformSockets`。
6. `crates/rurge/tests/cli.rs`：`rurge_command()`（约第 50 行）与 `rurge_process()`（约第 62 行）设的变量；`mod run`（约第 2458 行）里的 `rurge_run_with`、`rurge_run`、`spawn_daemon_full`、`spawn_command_with`、`api_call`、`api_port`、`wait_until`、`wait_for_exit`、`sysproxy_file`、`read_json`、`put_network`、`crashed_with_the_system_proxy_on`；`mod the_network`（约第 7217 行）里暂停的用例（`the_system_proxy_pauses_on_a_suspend_network` 一族）怎样写网络文件与 `[SSID Setting]`。
7. `rurge-config`：`general.rs` 的 `ipv6-vif`（约第 917 行）与 `tun-excluded-routes` / `tun-included-routes`（`parse_nets`，约第 932 行）；`diagnostic.rs` 的 `codes`（最新的是 `W0034`）、`codes::hint` 与它说明里"没有缺省建议的码"一段、测试里的 `WITH_DEFAULT` 与 `every_code_is_decided_once`；`[Host]` 的 `server:force-syslib` 在哪里读、在哪里能报诊断（`host.rs` 约第 145 行是值的读法）；`spec/mod.rs` 约第 600 行把 `addresses` 放进 `notes.inert`、约第 1027 行的用例 `assert_eq!(o.inert, ["addresses"])`；`config.rs` 的 `specs` 与 `external_on_own_listener`（约第 291 行，`external_addresses` 照它写）；下载到的文本里定义的 `external`（被 `W0033` 拒绝的）在不在 `specs` 里。
8. `rurge-platform`：`Cargo.toml`；`lib.rs` 的模块；`netenv` 的 `Snapshot`（`primary.routers` 先 IPv4 后 IPv6；`addresses` 里 IPv6 是非链路本地地址的 `/64`）；`ipnet` 2 的 `Ipv4Net::new_assert` / `Ipv6Net::new_assert` 是不是 `const fn`，`aggregate` 的签名。
9. `rurge-api`：`routes/features.rs` 的 `get_feature` / `set_feature`；它的用例与假的 `Control` 在哪里（`git grep -n "impl Control for" crates`）。
10. 门禁在开工前跑一遍，记下结果行数、通过数与忽略数（之后每个任务比较）。

**判据**：修正记录里有任务 0 的行；门禁的数字记下。这一项只改计划末尾的表，单独一个 `docs:` 提交或与任务 1 一起提交。

---

## Task 1：`rurge-tun` 的三条

**改哪里**（设计第 15 节、M4-D22；M3a 延后第 17 行的第二 ～ 四条）

1. `TunOptions` 加 `queue_bytes: usize`，说明照别的字段的写法："The bytes of packets that can wait in all the shards' queues together, counted by the memory each packet holds (its capacity). A packet that does not fit is dropped and counted (`queue_full`). More than zero." `Default` 给 `32 << 20`；`check()` 照 `udp_bytes` 的写法报 `queue_bytes must be more than zero`，放在 `shard_queue` 的检查之后（越界按字段的次序报第一个）。
2. 字节的计数复用 `udp::bytes` 的 `QueuedBytes` / `Reserved`：把 `src/udp/bytes.rs` 挪成 `src/bytes.rs`（`pub(crate)`，`udp` 与 `dispatch` 共用；模块说明改成两者都说）。片的通道的元素从 `Vec<u8>` 换成带着预留的一个小结构（`Ingress { packet: Vec<u8>, _held: Reserved }`，名字照实）：`dispatch::route` 在 `try_send` 之前按 `packet.capacity()` 预留（P9），预留不到 → `stats.drop(Reason::QueueFull, Some(src), Some(dst))`、不发；包被片取走、交给协议栈之后 `Ingress` 被丢掉，字节随之归还。`Inner` 里 `#[cfg(test)]` 的读数加一个 `queue_bytes()`，照 `udp_bytes` 的那个。
3. `Device` 的约定（`device.rs`，`recv` 的说明）加一句："Each packet is allocated to its length: the TUN counts what a waiting packet holds by its capacity (`TunOptions::queue_bytes`)."
4. `shard_of`：`Tun::start` 建一个 `std::hash::RandomState`（每个 TUN 一个），交给 `dispatch::run` / `route`（作参数或放进 `Context`，照实）；`shard_of(tuple, shards, &state)` 用 `state.hash_one(tuple)`。`DefaultHasher` 去掉。
5. 片重建的 WARN（P10）：每片一个 `Limiter::new(1, Duration::from_secs(60))`（放在 `Shard` 里、用一把小锁，或放在重建不清它的地方）；`rebuild` 里 `stack_panicked()` 照数，`allow(Instant::now())` 为真时写原来那行 WARN，否则同样的文字与 `shard` 字段写在 DEBUG；原来的那行 DEBUG 不变。`rebuild` 的说明改成 "at WARN at most once a minute for each shard, at DEBUG otherwise"。

**用例**（`src/tests/` 里照现有的分法放）

- `options::queue_bytes_must_be_more_than_zero`：照 `udp_bytes` 的那条。
- `a_shard_queue_beyond_queue_bytes_drops_and_counts_queue_full`：`queue_bytes` 小到只放得下几个包、让片停住（任务 0 找到的办法），发一串 SYN：放不下的计入 `queue_full`，`queue_bytes()` 从不超过上限。
- `queued_bytes_are_given_back_when_the_shard_takes_the_packets`：放开片之后 `queue_bytes()` 回到 0（等待有时限）。
- `shards_are_chosen_with_a_key_of_each_tun`：只测 `shard_of`：同一个 `RandomState` 对 64 个四元组两次选出的片相同；两个 `RandomState` 选出的不全相同（全相同的概率是 256⁻⁶⁴ 量级）。
- `a_shard_rebuilt_twice_in_a_minute_warns_once`（`LogCapture`）：照 `rebuild.rs` 里让片重建的办法，让同一片重建两次：WARN 一行、DEBUG 多一行同样的文字、`stack_panics` 是 2。
- 现有用例：`queue_full` 与重建的用例照旧通过；有用例数 WARN 行数而因此变红的，照这一改点名改。

**判据**：`git grep -n "DefaultHasher" crates/rurge-tun/src` 什么也没有；`TunOptions::default().check()` 通过；门禁绿，`rurge-tun` 的用例比开工前多这几条。

---

## Task 2：`Engine::start_tun` 返回 `Result`、一个引擎一个 TUN、池的文件 `0600`

**改哪里**（设计第 3.4 节、M4-D23；M3b 延后第 14、23 ～ 25 行；第 8 行在任务 7 的管理器里做）

1. `crates/rurge-dns/src/fakeip/file.rs` 的 `write`：Unix 上临时文件以 `0o600` 建（`std::os::unix::fs::OpenOptionsExt::mode`），临时文件已经在（上一次留下的）就先 `set_permissions(0o600)`，再写、再改名，改名之后的文件就是 `0600`；Windows 照旧（随数据目录的权限）。说明写明为什么（文件里列着应用查过的名字）。
2. `crates/rurge-engine/src/tun.rs`：
   ```rust
   /// … (说明照今天的；去掉 `# Panics`)
   ///
   /// # Errors
   ///
   /// Options that [`TunOptions::check`] refuses, a device whose MTU is below
   /// 1280, and a TUN that still runs on this engine (or whose pool's last save
   /// is still being written: see [`Engine::tun_ended`]). Nothing is opened or
   /// started then.
   pub async fn start_tun(
       self: &Arc<Self>,
       device: Arc<dyn Device>,
       pinger: Arc<dyn Pinger>,
       options: TunOptions,
   ) -> Result<TunHandle, String>
   ```
   次序：`options.check()?` → `device.mtu() >= 1280`，否则 `Err("the device's MTU is below 1280")`（P8）→ 占位（下一条）→ 开池 → `Tun::start`（此后不会 panic）。
3. 一个引擎一个 TUN（P7）：`Engine` 加一个只有一个许可的 `Arc<tokio::sync::Semaphore>`；`start_tun` 用 `try_acquire_owned`，拿不到 → `Err("a TUN is already running on this engine")`；许可交给保存池的那个任务，在最后一次保存与 `let_go` 之后丢掉；`start_tun` 在拿到许可之后的任何错误出口都丢掉它。另加：
   ```rust
   /// Resolves once no TUN runs on this engine: the last one has ended and its
   /// pool's last save is written.
   pub async fn tun_ended(&self)
   ```
   （取一个许可再丢掉；信号量从不关闭。）
4. `lib.rs`：`pub use rurge_tun::Counters as TunCounters;`（`rurge-api` 经它取计数，不直接依赖 `rurge-tun`）。
5. 两个测试调用者改成 `.await.expect("the TUN starts")`（或照实）。
6. `tests/tun.rs` 的 `an_association_returns_when_closed_and_lets_go_of_its_client`：最后一步"引擎放开客户端"从至多 5 秒改为至多 1 秒——`STOP_GRACE`：TUN 停止时写任务为还拿着客户端的会话至多等这么久，引擎要在这之内放开（M3b 延后第 14 行的一条）。`tests/common/mod.rs` 加 `wait_until_within(what, limit, check)`，`wait_until` 用它、仍是 5 秒。

**用例**

- `crates/rurge-engine/tests/tun.rs`（照那里的夹具与两道防线）：
  - `start_tun_refuses_options_that_check_refuses_and_opens_nothing`：`TunOptions { shards: 0, ..Default::default() }` → `Err`，文字是 `check` 的那句；数据目录里没有 `fake-ip.txt`；之后用合格的选项能起。
  - `start_tun_refuses_a_device_below_1280`：`MemoryDevice::pair(1279, …)` → `Err`；没有 `fake-ip.txt`。
  - `a_second_tun_on_one_engine_is_refused_until_the_first_has_ended`：起一个；第二个 `Err`（文字有 `already running`）；第一个 `stop()`、`join()`、`tun_ended()` 之后第二个能起（等待都有时限）。
- `crates/rurge-dns/src/fakeip/`：`#[cfg(unix)] the_pool_file_is_its_owners_alone`：写一次，权限是 `0o600`；临时文件预先以 `0o644` 放好再写一次，仍是 `0o600`。**这一条只在 CI 的 Linux / macOS 上编译、运行**，照实写进报告。

**判据**：`git grep -n "# Panics" crates/rurge-engine/src/tun.rs` 什么也没有；`start_tun` 的每个调用者都处理了 `Result`；改了时限的那条单独循环跑二十次都通过（`cargo test -j 8 --workspace --test tun -- an_association_returns_when_closed_and_lets_go_of_its_client`）；门禁绿。

---

## Task 3：`StateStore::try_update` 与 `enhanced_backup`

**改哪里**（设计第 3.4、9.2 节、M4-D5）

1. `State` 加 `pub enhanced_backup: Option<serde_json::Value>`（结构上已有 `#[serde(default)]`；`Default` 给 `None`），说明照 `system_proxy_backup`：引擎不解释它的形状。`STATE_VERSION` 不涨：加的是可缺省的字段，旧文件照读（任务 0 核对没有别处按版本号拒绝）。
2. 新方法：
   ```rust
   /// Applies `f` to a copy of the state and writes the copy to disk; only once
   /// it is written does the copy become the state. A write that fails leaves
   /// the state as it was, and is the error.
   pub async fn try_update(&self, f: impl FnOnce(&mut State)) -> io::Result<State>
   ```
   写法照 `update`（同一把锁、同一个 `write_atomic`、`spawn_blocking`），`spawn_blocking` 的任务失败也映射成 `io::Error`。`update` 不变。

**用例**（`state.rs` 的 `tests`）

- `try_update_changes_nothing_when_the_file_cannot_be_written`：把 `state.json.tmp` 建成一个目录（写临时文件失败）：`try_update` 回 `Err`，`snapshot()` 不变，磁盘上的 `state.json` 不变。
- `try_update_writes_before_it_changes_the_state`：成功时磁盘与内存一致。
- `enhanced_backup_round_trips_and_an_old_file_reads_without_it`：没有这个键的旧文件读出 `None`；写进一个值再读回相等。

**判据**：门禁绿。

---

## Task 4：路由集（`rurge-platform::route`）

**改哪里**（设计第 4 节、M4-D2、D3）

1. `crates/rurge-platform/Cargo.toml` 加 `ipnet.workspace = true`；`lib.rs` 加 `pub mod route;`。
2. `src/route/mod.rs`：模块说明（路由集与三个平台的步骤都是数据；执行在各平台的文件里，M4c / M4d）；常量：
   ```rust
   /// rurge's own space (design 4.5): the adapter, the responder, the stack and the fake addresses.
   pub const SPACE_V4: Ipv4Net = /* 198.18.0.0/15 */;
   pub const SPACE_V6: Ipv6Net = /* fd00:6152::/64 */;
   pub const ADAPTER_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 1);
   pub const ADAPTER_V6: Ipv6Addr = /* fd00:6152::1 */;
   pub const RESPONDER_V4: Ipv4Addr = Ipv4Addr::new(198, 18, 0, 2);
   pub const RESPONDER_V6: Ipv6Addr = /* fd00:6152::2 */;
   pub const MTU: u16 = 1500;
   ```
   （`ipnet` 2 的构造不是 `const fn` 的话写成函数，照实。）这组数与 `rurge-tun/src/space.rs`、`rurge_dns::fakeip::in_space` 是同一组；可执行文件里一条用例钉住它们一致（任务 7）。
3. `src/route/set.rs`：
   ```rust
   pub struct RouteInputs<'a> {
       pub ipv6: bool,
       /// `tun-excluded-routes`.
       pub excluded: &'a [IpNet],
       /// `tun-included-routes`.
       pub included: &'a [IpNet],
       /// The `addresses` of every `external` policy.
       pub external: &'a [IpAddr],
       /// The platform installs rurge's space as a route of its own (macOS:
       /// utun is point-to-point and its address brings no subnet route).
       /// Elsewhere the adapter's address brings it.
       pub explicit_space: bool,
   }

   /// The routes into the adapter: each family sorted, merged, each once.
   #[derive(Clone, Debug, Default, PartialEq, Eq)]
   pub struct RouteSet {
       pub v4: Vec<Ipv4Net>,
       pub v6: Vec<Ipv6Net>,
   }

   impl RouteSet {
       pub fn compute(inputs: &RouteInputs<'_>) -> RouteSet;
       /// What to add and what to remove to go from `old` to `new`.
       pub fn diff(old: &RouteSet, new: &RouteSet) -> (RouteSet, RouteSet);
       /// Whether traffic to `ip` goes into the adapter: one of the routes, or rurge's space.
       pub fn contains(&self, ip: IpAddr) -> bool;
       pub fn len(&self) -> usize;
       pub fn is_empty(&self) -> bool;
   }
   ```
   `compute` 的次序（设计第 4.2 ～ 4.5 节）：
   1. 基础前缀：IPv4 `1.0.0.0/8`、`2.0.0.0/7`、`4.0.0.0/6`、`8.0.0.0/5`、`16.0.0.0/4`、`32.0.0.0/3`、`64.0.0.0/2`、`128.0.0.0/1`；`ipv6` 为真时 IPv6 `100::/8`、`200::/7`、`400::/6`、`800::/5`、`1000::/4`、`2000::/3`、`4000::/2`、`8000::/1` 与 `64:ff9b::/96`。
   2. 洞：总是减去的（IPv4 `127.0.0.0/8`、`169.254.0.0/16`、`224.0.0.0/4`、`255.255.255.255/32`；IPv6 `fe80::/10`、`ff00::/8`）、`excluded`、`external`（每个作 `/32` / `/128`）；`ipv6` 为假时 IPv6 的洞不用。**rurge 的空间不被剥掉**：每个洞先去掉它与 rurge 空间相交的部分（洞在空间外的部分照减）。
   3. 求补：对每个基础前缀，没有洞与它相交就留下，被某个洞包含就扔掉，否则对半分开递归；再用 `aggregate` 合并。
   4. 加上 `included`（`ipv6` 为假时只加 IPv4 的），总设计 Q-03：包含赢。
   5. 整个落在 rurge 空间里的前缀去掉；`explicit_space` 为真时加上 `SPACE_V4`（`ipv6` 为真时加上 `SPACE_V6`）。
   6. 每个地址族 `aggregate`（排序、合并相邻的、去掉被包含的）。

**用例**（`set.rs` 的 `tests`，全是数据，任何平台都跑）

- `the_base_routes_are_eight_prefixes_each_way`：只看第 1 步（洞为空时的基础前缀，用一个内部函数或让 `compute` 的结果与"基础前缀减去总是减去的"手算值比较）：`0.0.0.0/8` 与 `::/8` 的其余部分不在，`64:ff9b::/96` 在。
- `loopback_link_local_multicast_and_broadcast_never_go_in`：`127.0.0.1`、`169.254.1.1`、`224.0.0.251`、`239.255.255.250`、`255.255.255.255`、`fe80::1`、`ff02::fb` 都不 `contains`；`128.0.0.1`、`1.1.1.1`、`2606:4700::1111` 都 `contains`（IPv6 开着）。
- `one_excluded_address_leaves_a_hole_and_no_more`：排除 `8.8.8.8/32`：它不在，`8.8.8.7`、`8.8.8.9` 在；结果的条数等于手算的值（写在用例的注释里）。
- `adjacent_holes_merge`、`a_hole_that_covers_a_base_prefix_takes_it_whole`、`a_hole_across_two_base_prefixes`（如排除 `0.0.0.0/1`）。
- `included_wins_over_excluded`：排除 `10.0.0.0/8` 并包含 `10.1.0.0/16`：`10.1.2.3` 在、`10.2.0.1` 不在。
- `rurges_space_is_never_stripped_and_pieces_inside_it_are_dropped`：排除 `198.0.0.0/8`：`198.18.0.2` 仍 `contains`、`198.1.0.1` 不在；包含 `198.18.5.0/24`：结果里没有这一条；`explicit_space` 为真时结果里有 `198.18.0.0/15`，IPv6 开着时有 `fd00:6152::/64`。
- `ipv6_off_uses_no_ipv6_item`：`v6` 是空的，排除与包含里的 IPv6 项都不出现。
- `external_addresses_are_holes`：`[1.2.3.4, 2001:db8::1]` 都不 `contains`。
- `diff_adds_and_removes_only_what_changed`：加的与减的互不相交，旧的减去要减的、加上要加的，等于新的。
- `nat64_is_in_and_the_rest_of_colon_colon_slash_8_is_out`：`64:ff9b::102:304` 在；`::1`、`::ffff:1.2.3.4` 不在。

**判据**：`set.rs` 里没有 `cfg`；门禁绿。

---

## Task 5：步骤与三个平台的翻译（`rurge-platform::route`）

**改哪里**（设计第 5 节、M4-D4；写法的核对在 P2 ～ P5）

1. `src/route/steps.rs`：
   ```rust
   /// One program to run (design 5.1).
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct Step {
       /// An absolute path: never looked up in `PATH`.
       pub program: PathBuf,
       pub args: Vec<String>,
       /// A script the program reads: it is written to a file of its own,
       /// whose path takes the place of [`SCRIPT`] in `args`.
       pub script: Option<String>,
       pub on_failure: OnFailure,
   }

   /// Where a script's path goes in `args`.
   pub const SCRIPT: &str = "{script}";

   #[derive(Clone, Copy, Debug, PartialEq, Eq)]
   pub enum OnFailure {
       /// The open fails, and what was done is undone.
       Fail,
       /// The IPv6 half: a warning, and this open goes on as if IPv6 were off (M4-D17).
       Ipv6Off,
       /// Undoing goes on: removing routes, giving back what went with the device.
       Ignore,
   }
   ```
   脚本每行一条命令、行尾 `\n`、不带 `\r`。
2. `src/route/linux.rs`（不带 `cfg`）：
   ```rust
   pub struct LinuxTools { pub ip: PathBuf, pub resolvectl: Option<PathBuf> }
   pub fn configure(t: &LinuxTools, dev: &str) -> Step;
   pub fn configure_ipv6(t: &LinuxTools, dev: &str) -> Step;
   pub fn unconfigure_ipv6(t: &LinuxTools, dev: &str) -> Step;
   pub fn add_routes(t: &LinuxTools, dev: &str, set: &RouteSet) -> Vec<Step>;
   pub fn remove_routes(t: &LinuxTools, dev: &str, set: &RouteSet) -> Vec<Step>;
   pub fn take_dns(t: &LinuxTools, dev: &str, ipv6: bool) -> Vec<Step>;   // resolvectl 不在时是空的
   pub fn give_dns_back(t: &LinuxTools, dev: &str) -> Vec<Step>;
   ```
   - `configure`：`ip -batch {script}`，脚本 `link set dev D mtu 1500` / `addr add 198.18.0.1/15 dev D` / `link set dev D up`；`Fail`。
   - `configure_ipv6`：`ip -batch`，`addr add fd00:6152::1/64 dev D`；`Ipv6Off`。`unconfigure_ipv6`：`ip -force -batch`，`addr del fd00:6152::1/64 dev D`；`Ignore`。
   - `add_routes`：IPv4 一批（`route replace P dev D`，`Fail`）、IPv6 一批（同样的行，`Ipv6Off`）；某个地址族是空的就不出那一批。
   - `remove_routes`：`ip -force -batch`，每行 `route del P dev D`；`Ignore`。
   - `take_dns`：`resolvectl dns D 198.18.0.2 [fd00:6152::2]`、`resolvectl domain D ~.`、`resolvectl default-route D yes`、`resolvectl flush-caches`；`Fail`。`give_dns_back`：`resolvectl revert D`；`Ignore`。
   - `ip` 与 `resolvectl` 的绝对路径由调用者找（M4c：`/usr/sbin`、`/sbin`、`/usr/bin`、`/bin` 依次找第一个有的），翻译只用给它的路径。`rp_filter` 不是命令（M4c 直接读写 `/proc`）。
3. `src/route/macos.rs`（不带 `cfg`；程序是固定的绝对路径：`/sbin/ifconfig`、`/sbin/route`、`/usr/sbin/networksetup`、`/usr/bin/dscacheutil`、`/usr/bin/killall`）：
   ```rust
   pub fn configure(dev: &str) -> Step;          // ifconfig D inet 198.18.0.1 198.18.0.1 netmask 255.254.0.0 mtu 1500 up — Fail
   pub fn configure_ipv6(dev: &str) -> Step;     // ifconfig D inet6 fd00:6152::1 prefixlen 64 — Ipv6Off
   pub fn unconfigure_ipv6(dev: &str) -> Step;   // ifconfig D inet6 fd00:6152::1 delete — Ignore
   pub fn add_routes(dev: &str, set: &RouteSet) -> Vec<Step>;    // 每条一个：route -n add -net P -interface D；IPv6：route -n add -inet6 -net P -interface D — IPv4 Fail，IPv6 Ipv6Off
   pub fn remove_routes(dev: &str, set: &RouteSet) -> Vec<Step>; // route -n delete [-inet6] -net P -interface D — Ignore
   pub fn set_dns(service: &str, servers: &[IpAddr], on_failure: OnFailure) -> Step; // networksetup -setdnsservers S A…（空的写 Empty）
   pub fn flush_dns() -> Vec<Step>;              // dscacheutil -flushcache；killall -HUP mDNSResponder — Ignore
   ```
   服务的名字是一个参数（可以有空格，不加引号：参数不经 shell）。
4. `src/route/windows.rs`（不带 `cfg`）：
   ```rust
   pub struct WindowsTools { pub netsh: PathBuf, pub ipconfig: PathBuf }
   impl WindowsTools {
       /// `netsh.exe` and `ipconfig.exe` in `<system root>\System32`.
       pub fn under(system_root: &Path) -> WindowsTools;
   }
   pub fn configure(t: &WindowsTools, index: u32) -> Step;          // Fail
   pub fn configure_ipv6(t: &WindowsTools, index: u32) -> Step;     // Ipv6Off
   pub fn unconfigure_ipv6(t: &WindowsTools, index: u32) -> Step;   // Ignore
   pub fn add_routes(t: &WindowsTools, index: u32, set: &RouteSet) -> Vec<Step>;    // IPv4 一份脚本（Fail），IPv6 一份（Ipv6Off）
   pub fn remove_routes(t: &WindowsTools, index: u32, set: &RouteSet) -> Vec<Step>; // Ignore
   pub fn take_dns(t: &WindowsTools, index: u32, ipv6: bool) -> Vec<Step>;          // set dnsservers（Fail）；ipconfig /flushdns（Ignore）
   ```
   `netsh` 的参数是 `["-f", SCRIPT]`；脚本每行一条（不带开头的 `netsh`），每个参数写成 `键=值`、网卡以序号指称（P4）；`<i>` 是序号：
   ```
   interface ipv4 add address name=<i> address=198.18.0.1 mask=255.254.0.0 store=active
   interface ipv4 set interface interface=<i> forwarding=disabled weakhostsend=disabled weakhostreceive=disabled metric=1 store=active
   interface ipv4 set subinterface interface=<i> mtu=1500 store=active
   interface ipv4 add route prefix=<前缀> interface=<i> metric=1 store=active
   interface ipv4 delete route prefix=<前缀> interface=<i> store=active
   interface ipv4 set dnsservers name=<i> source=static address=198.18.0.2 register=none validate=no
   interface ipv6 add address interface=<i> address=fd00:6152::1/64 store=active
   interface ipv6 set interface interface=<i> forwarding=disabled weakhostsend=disabled weakhostreceive=disabled metric=1 store=active
   interface ipv6 set subinterface interface=<i> mtu=1500 store=active
   interface ipv6 add route prefix=<前缀> interface=<i> metric=1 store=active
   interface ipv6 delete route prefix=<前缀> interface=<i> store=active
   interface ipv6 delete address interface=<i> address=fd00:6152::1 store=active
   interface ipv6 set dnsservers name=<i> source=static address=fd00:6152::2 register=none validate=no
   ```
   `configure` 的脚本是前三行（IPv4），`configure_ipv6` 是 IPv6 的 `add address`、`set interface`、`set subinterface` 三行；`take_dns` 的 IPv6 那一行只在 `ipv6` 为真时有。`ipconfig` 的参数是 `["/flushdns"]`。路由的读回核对与 `show route` 的解析在 M4d。

**用例**：每个函数至少一条逐字比对的用例（字面量；一个平台一个 `tests` 模块）：IPv4 与 IPv6 各有前缀时、没有 IPv6 时不出 IPv6 的步骤、空集合不出步骤（`add_routes` / `remove_routes` 对空的 `RouteSet` 回空的 `Vec`）、每一步的 `OnFailure`、脚本里没有 `\r`、服务名里的空格原样在一个参数里。

**判据**：`steps.rs`、`linux.rs`、`macos.rs`、`windows.rs` 里没有 `cfg`；三个平台的用例在 Windows 开发机上都跑了（`cargo test -j 8 --workspace --lib -- route::`）；门禁绿。

---

## Task 6：`rurge check` 的四个码与 `external` 的 `addresses`（`rurge-config`）

**改哪里**（设计第 11、12 节、M4-D21、D26）

1. `diagnostic.rs` 的 `codes` 加四个（名字照现有的写法，P20 的文字）：
   - `W0035`：`ipv6-vif = always`。
   - `W0036`：`tun-included-routes` 的一项包含整段私网——`10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16` 之一被它包含（含相等）。
   - `W0037`：`tun-excluded-routes` 的一项与 `198.18.0.0/15` 或 `fd00:6152::/64` 相交。
   - `W0038`：`[Host]` 里有 `server:force-syslib`。
   各在 `codes::hint` 里做决定（四个都有缺省建议，P20），并加进测试里的 `WITH_DEFAULT`；`every_code_is_decided_once` 绿。
2. 报的位置：`W0035` 在 `general.rs` 读出 `ipv6-vif = always` 时（`off` 迁移成 `disabled` 的那条不受影响）；`W0036`、`W0037` 在读完那一行之后，只对读出来的项判（读不出来、已经报了错的项不判）；`W0038` 在 `[Host]` 读出 `server:force-syslib` 的地方（任务 0 定位）。都是 Warning，不阻止加载。消息引用被告警的那一项（前缀与 `ipv6-vif` 的值不是凭据）。rurge 空间的两个前缀在 `rurge-config` 里另写一份（它不依赖别的工作区 crate），说明指向 `rurge_platform::route::SPACE_V4`；一致性由任务 7 的用例钉住。
3. `spec/mod.rs` 约第 600 行：去掉把 `addresses` 放进 `notes.inert` 的那几行（它在增强模式开着时有效，M4-D21）；约第 1027 行的用例改成断言 `inert` 里没有 `addresses`（点名改）。
4. `config.rs`：
   ```rust
   /// The addresses the `external` policies of the profile name (`addresses`),
   /// which the enhanced mode keeps out of the virtual adapter (M4 design 11).
   /// A policy that is not started (`W0033`) names none.
   pub fn external_addresses(&self) -> Vec<IpAddr>
   ```

**用例**（照诊断用例的写法；语料库快照有变化时 `cargo insta test --review` 审阅——变化只能是这四个码与 `W0029` 不再提 `addresses`）

- `ipv6_vif_always_warns`、`ipv6_vif_auto_and_disabled_say_nothing`。
- `including_a_whole_private_range_warns`：`192.168.0.0/16`、`10.0.0.0/8`、`8.0.0.0/5`（含 `10/8`）、`0.0.0.0/0` 各告警；`192.168.1.0/24`、`172.16.5.0/24` 不告警。
- `excluding_into_rurges_space_warns`：`198.18.0.0/16`、`198.0.0.0/8`、`fd00::/8` 告警；`198.20.0.0/16`、`fd01::/16` 不告警。
- `force_syslib_warns_and_system_and_syslib_do_not`。
- `external_addresses_are_no_longer_inert`、`external_addresses_lists_the_addresses_of_every_external_policy`（含不启动的那种不算，照任务 0 的结果）。

**判据**：`every_code_is_decided_once` 绿；`git grep -n "inert.push(\"addresses\")" crates/rurge-config` 什么也没有；门禁绿。

---

## Task 7：管理器、后端与恢复（可执行文件）

**改哪里**（设计第 5.1、9.1、9.2、9.4、9.5、9.10 节、M4-D5、D16、D19）

1. `crates/rurge/Cargo.toml` 加 `rurge-tun.workspace = true`（与要用时的 `ipnet.workspace = true`）；`src/cli/mod.rs` 加 `mod enhanced;`。
2. `src/cli/enhanced/undo.rs`：
   ```rust
   /// What undoing a change that outlives rurge takes (M4 design 9.2). Typed,
   /// never a command line: an edited state.json can make rurge do only what
   /// it would have done anyway.
   #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
   #[serde(tag = "kind", rename_all = "snake_case")]
   pub enum Undo {
       /// macOS: a network service's DNS servers before the takeover (empty: none were set).
       MacDns { service: String, servers: Vec<IpAddr> },
       /// Linux: `net.ipv4.conf.all.rp_filter` before it was loosened.
       RpFilter { value: u8 },
       /// The file backend: one of its steps.
       Test { step: String },
   }

   /// When a close undoes it: with the DNS (first) or after the device (last).
   pub enum Stage { Dns, Adjust }
   impl Undo { pub fn stage(&self) -> Stage }   // Test：step 是 "take_dns" 为 Dns，其余为 Adjust

   /// `state.json`'s `enhanced_backup`: `{"undo": [...]}` in the order the changes were made.
   #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
   pub struct Backup { pub undo: Vec<Undo> }
   ```
   `enhanced_backup` 读不懂（不是这个形状、有不认识的 `kind`）：ERROR 一行，**原样留着**，不猜着撤销（以后的版本也许读得懂）。
3. `src/cli/enhanced/backend.rs`：
   ```rust
   /// The device a backend opened: what the TUN reads and writes, and how the
   /// system names it.
   pub struct Opened {
       pub device: Arc<dyn rurge_tun::device::Device>,
       /// The interface's name: for the debug log and the egress table.
       pub name: String,
       /// Windows: the interface index the steps name the adapter by.
       pub index: Option<u32>,
       // private: what the platform needs to close it
   }

   /// What the manager asks of the system (M4 design 5.1). Every method blocks
   /// (programs, files) and is called on a blocking thread; an error is English
   /// text for the log and for the API's 500.
   pub trait Backend: Send + Sync {
       fn open(&self) -> Result<Opened, String>;
       /// The adapter's address, MTU, link and own settings, and with `ipv6` its
       /// IPv6 address. `Ok(false)`: IPv6 was asked for and could not be had (the
       /// backend has warned): this open goes on without it.
       fn configure(&self, dev: &Opened, ipv6: bool) -> Result<bool, String>;
       /// The IPv6 half alone, on an adapter that is up (a reload or a change of
       /// network turned IPv6 on). `Ok(false)` as for `configure`.
       fn configure_ipv6(&self, dev: &Opened) -> Result<bool, String>;
       fn unconfigure_ipv6(&self, dev: &Opened) -> Result<(), String>;
       /// Adds `add`, then removes `remove`; removing goes on past errors.
       /// `Ok(false)`: the IPv6 half of `add` could not be added (warned).
       fn routes(&self, dev: &Opened, add: &RouteSet, remove: &RouteSet) -> Result<bool, String>;
       /// The undo actions `take_dns` will need that are not among `saved` (the
       /// ones in the backup already), read before it is done (macOS: each
       /// enabled service and its servers; on a change of network, the services
       /// that are new).
       fn dns_undo(&self, dev: &Opened, saved: &[Undo]) -> Result<Vec<Undo>, String>;
       /// Points the system's DNS at the responder. `Ok(false)`: not on this
       /// system (Linux without systemd-resolved); the backend has said why.
       fn take_dns(&self, dev: &Opened, ipv6: bool) -> Result<bool, String>;
       /// Gives back what goes away with the device anyway (resolved's settings
       /// of the link): best effort. What outlives rurge goes back by `undo`.
       fn give_dns_back(&self, dev: &Opened) -> Result<(), String>;
       /// Linux: `Some` when `rp_filter` has to be loosened for the network's
       /// primary interface, with what to put back; `None` when it need not be,
       /// or when `saved` has it already.
       fn adjust_undo(&self, primary: Option<&str>, saved: &[Undo]) -> Result<Option<Undo>, String>;
       fn adjust(&self) -> Result<(), String>;
       /// Puts one saved thing back: on a close, and after a crash.
       fn undo(&self, undo: &Undo) -> Result<(), String>;
       fn close(&self, dev: Opened);
   }

   /// `RURGE_ENHANCED_BACKEND`: unset → the platform's; `off`; `file:<path>`.
   /// Anything else is an error: a typo must not reach the real system.
   pub fn backend() -> anyhow::Result<Arc<dyn Backend>>
   ```
   名字可以细调（记进修正记录）。平台后端在这一份对每个方法都回 `Err("the enhanced mode is not available on this platform in this version")`（`close` 什么也不做）；`off` 回 `Err("the enhanced mode backend is off (RURGE_ENHANCED_BACKEND=off)")`。
4. `src/cli/enhanced/file.rs`：文件后端（设计第 9.10 节；P19）。
   - 设备：一个空设备——`recv` 永不返回（TUN 的收包任务在停止时结束），`send` 丢掉写来的包，MTU 1500；名字 `rurge-test`，`index` 是 `None`。文件里有 `"device_fails_after_ms": N` 时，`recv` 等 N 毫秒后回一个 `io::Error`（"the device failed (test)"），供任务 8 的用例。
   - 文件（JSON；读、改、写回；保留它不认识的键）：
     ```json
     {"fail": "<动作>", "fail_undo": "<step>", "device_fails_after_ms": 200,
      "open": true, "ipv6": false, "routes": ["1.0.0.0/8", "…"], "dns": false, "adjusted": false,
      "actions": ["open", "configure", "…"]}
     ```
     动作名：`open`、`configure`、`configure_ipv6`、`unconfigure_ipv6`、`routes`（记成 `routes +<加的条数> -<减的条数>`，`routes` 字段是当前的集合，前缀的文字形式）、`dns_undo`（不记）、`take_dns`、`give_dns_back`、`adjust`、`undo <step>`、`close`。`fail` 等于某个动作名时那一步回 `Err("<动作> failed (test)")`；`fail` 是 `configure_ipv6` 时 `configure(…, true)` 与 `configure_ipv6` 回 `Ok(false)`、`ipv6` 字段留 `false`；`fail` 是 `routes` 时只在要加的那一半失败；`fail_undo` 等于某个撤销的 step 时那个撤销失败。
   - 会留下的两样：`take_dns`（`dns_undo` 在 `saved` 里没有 `Test { step: "take_dns" }` 时给它，否则空）与 `adjust`（`adjust_undo` 在 `saved` 里没有 `Test { step: "adjust" }` 时给它，否则 `None`）；撤销把对应的字段（`dns`、`adjusted`）写回 `false`、记 `undo <step>`。
   - 文件不在或读不懂：当作 `{}`；写不进：那一步回错误。
5. `src/cli/enhanced/wanted.rs`：
   ```rust
   /// What the enhanced mode should be now, from the generation in use and the network.
   #[derive(Clone, Debug, PartialEq, Eq)]
   pub struct Wanted {
       pub routes: RouteSet,
       pub ipv6: bool,
       /// The network's primary interface (`NetworkFacts::primary`), for the egress table.
       pub primary: Option<String>,
   }
   pub fn wanted(general: &General, external: &[IpAddr], ipv6_usable: bool, primary: Option<String>, explicit_space: bool) -> Wanted
   ```
   `ipv6`：`disabled` → 假、`always` → 真、`auto` → `ipv6_usable`（P18）；路由由 `RouteSet::compute` 算。`explicit_space` 由调用者按 `cfg!(target_os = "macos")` 给（P22），`wanted` 本身是纯的。
6. `rurge-platform/src/socket.rs` 加 `pub mod egress`，这一份里只有：
   ```rust
   /// What decides where rurge's own sockets go while the enhanced mode is on
   /// (M4 design 7.1). The decision and the subnets are M4b's.
   #[derive(Clone, Debug, Default, PartialEq, Eq)]
   pub struct EgressTable { pub routes: RouteSet, pub primary: Option<String> }
   ```
   可执行文件的出口表槽：`pub type EgressSlot = Arc<std::sync::RwLock<Option<Arc<EgressTable>>>>`（读写都不跨 `await`）；这一份里没有读它的人（M4b 的 `PlatformSockets::egress` 读）。
7. `src/cli/enhanced/mod.rs`：管理器。
   ```rust
   /// Owned by the `rurge run` main loop, which calls it one request at a time.
   pub struct EnhancedMode { /* backend, store, starter, egress, shared, on: Option<On> */ }
   /// For the API: whether the enhanced mode is on, and its TUN's counters.
   pub struct Shared { /* enabled: AtomicBool, tun: Mutex<Option<Arc<dyn RunningTun>>> */ }
   impl Shared { pub fn status(&self) -> rurge_engine::EnhancedStatus }  // 关着时计数全 0（任务 8 定义 EnhancedStatus；这一任务可以先用自己的结构，任务 8 换）

   impl EnhancedMode {
       /// Startup, under the data directory's lock (M4 design 9.5): the undo
       /// actions a run that died left in state.json are done, last first; each
       /// that is done leaves the backup. `None`: there was nothing to undo.
       pub async fn recover(backend: &Arc<dyn Backend>, store: &StateStore) -> Option<Result<(), String>>;
       pub fn new(backend: Arc<dyn Backend>, store: Arc<StateStore>, starter: Arc<dyn Starter>, egress: EgressSlot) -> EnhancedMode;
       pub fn enabled(&self) -> bool;
       pub fn shared(&self) -> Arc<Shared>;
       /// Already on: `Ok` at once.
       pub async fn enable(&mut self, wanted: Wanted) -> Result<(), String>;
       /// Never stops halfway; the first error is what comes back.
       pub async fn disable(&mut self) -> Result<(), String>;
       /// After a reload or a change of network: the difference only (任务 9).
       pub async fn follow(&mut self, wanted: Wanted, network_changed: bool);
   }
   ```
   - **打开**（设计第 9.3 节）：`open` → `configure(ipv6)`（`Ok(false)`：这一次 IPv6 当作没开）→ `starter.start(device)` → 出口表写进槽（`EgressTable { routes, primary }`）→ `routes(add = 要的, remove = 空)`（`Ok(false)`：记下的集合去掉 IPv6 的一半）→ `dns_undo(dev, saved)`（`saved` 是备份里已有的撤销动作）→ 写进备份（`try_update`）→ `take_dns(ipv6)` → `adjust_undo(wanted.primary, saved)` → 有就写进备份（`try_update`）→ `adjust` → `features.enhanced_mode = true`（`update`，P16）。任何一步 `Err`：倒着撤销已做的（关闭的同一套，只做做过的那几步；**接管 DNS 失败时也交还 DNS 并做已存的 DNS 撤销**——失败的接管可能改了一半，照系统代理的做法），本次加进备份的随撤销去掉，回这一步的错误。写不进 `state.json`：错误是 `cannot write state.json: <原因>`，那一步不做。完成后 INFO `enhanced mode on`（字段 `routes` 条数、`ipv6`），DEBUG 一行带网卡名。
   - **关闭**（设计第 9.4 节；P12）：`give_dns_back`，并做备份里阶段是 `Dns` 的撤销 → `routes(add = 空, remove = 当前)` → 清出口表的槽 → `tun.stop()`，等 `tun.ended()` 至多 `JOIN_LIMIT`（3 秒；超时 WARN `the TUN did not end within 3 seconds; going on`）→ `close(dev)` → 阶段是 `Adjust` 的撤销 → 备份写成剩下的（失败的那几条，`update`，P11）；全部成功则 `enhanced_backup = None`、`features.enhanced_mode = false`。每一步失败记 ERROR、继续；回第一个错误。INFO `enhanced mode off`。
   - **起 TUN 的接缝**（P6）：
     ```rust
     /// Starts the TUN the manager runs: the engine's, or a test's.
     pub trait Starter: Send + Sync {
         fn start(&self, device: Arc<dyn Device>) -> BoxFuture<'_, Result<Arc<dyn RunningTun>, String>>;
     }
     pub trait RunningTun: Send + Sync {
         fn stop(&self);
         /// Ends when the TUN and all it holds of the engine have ended
         /// (`TunHandle::join`, then `Engine::tun_ended`).
         fn ended(&self) -> BoxFuture<'_, ()>;
         fn counters(&self) -> rurge_engine::TunCounters;
     }
     ```
     生产里包 `Engine::start_tun(device, Arc::new(NoPinger), TunOptions::default())`（`ipv6: true` 是缺省值，M4-D17）。`NoPinger`：`echo` 立刻回 `Err(io::ErrorKind::Unsupported, "ICMP forwarding is not available in this version")`（P15，M4b 换成真的）。
8. 崩溃之后的恢复（`run.rs`）：
   - `holds_a_backup` 改成两种备份任一个在就为真（名字照实）。
   - `restore_what_a_crash_left`：先增强模式（`enhanced::backend()`；解析失败 → `warning: <错误>; the enhanced mode's changes were not undone`），再系统代理（今天的）。增强模式的结果写进标准错误：`warning: a previous run left changes of the enhanced mode; they were undone`；撤销了但写不回 `state.json`：`…; they were undone, but state.json could not be updated: the next start undoes them again`；失败：`warning: cannot undo what the enhanced mode left: <错误>; it stays in state.json and is tried on the next start`。
   - `run()` 里 `sysproxy.recover()` 之前：`EnhancedMode::recover(&enhanced_backend, &store)`（成功 INFO `restored what the enhanced mode left last time`，失败 ERROR）。
   - `RURGE_ENHANCED_BACKEND` 在读配置之前解析（照 `network::backend` 的位置与写法，P13）：值不认识 → `error: …`、`recover_without_a_log`、以 2 退出。
9. 起 `rurge` 的辅助函数：`crates/rurge/tests/cli.rs` 的 `rurge_command()` 与 `rurge_process()` 加 `.env("RURGE_ENHANCED_BACKEND", "off")`，说明照它们讲 `RURGE_PROCESS_BACKEND` 的那段加一句；`mod run` 的 `rurge_run_with` 加 `.env_remove("RURGE_ENHANCED_MODE")`。`cli.rs` 的 `mod run` 里加 `mod enhanced_mode;`（P17），新文件 `crates/rurge/tests/run/enhanced_mode.rs`。

**用例**

- 单元（`cli/enhanced/` 的 `tests`；一个记录调用的假后端与假的 `Starter`，`StateStore` 在临时目录里）：
  - `opening_goes_in_the_designed_order`：`open, configure, start, egress, routes, dns_undo, take_dns, adjust_undo, adjust`。
  - `the_backup_is_saved_before_the_step_it_undoes`：假后端在 `take_dns` 与 `adjust` 里读 `store.snapshot()`，对应的撤销动作已经在里面。
  - `a_backup_that_cannot_be_written_stops_the_step_before_it_is_done`：`state.json.tmp` 是目录；`enable` 回 `Err`（以 `cannot write state.json` 开头），`take_dns` 没有被调用，已做的都撤销了（`close` 被调用、出口表的槽是空的）。
  - `a_step_that_fails_undoes_the_steps_before_it_in_reverse`：对每个可以失败的动作各一次（参数化）。
  - `closing_goes_in_the_designed_order_and_never_stops_halfway`：`give_dns_back` 失败仍做其余各步，回的是它的错误；失败的撤销留在备份里，成功的去掉。
  - `closing_waits_for_the_tun_three_seconds_at_most`：假 TUN 的 `ended()` 永不结束；在暂停的时钟下（`#[tokio::test(start_paused = true)]`）`disable` 返回、`close` 被调用；WARN 那一行能在同一个用例里抓就抓（`LogCapture` 与暂停的时钟一起用有问题就只断言返回与 `close`，记进修正记录）。
  - `ipv6_that_cannot_be_had_is_left_off_for_this_open`：`configure` 回 `Ok(false)`：`routes` 收到的集合没有 IPv6，`take_dns` 的 `ipv6` 为假。
  - `an_unreadable_backup_is_left_alone`：`enhanced_backup` 是 `{"x": 1}`：`recover` 回 `Some(Err)`，备份原样。
  - `the_platform_backend_is_not_available_in_this_version`：平台后端的 `open` 回那句错误（这一份的平台后端里没有任何系统调用）。
  - `rurges_space_is_one_and_the_same_everywhere`：`rurge_platform::route` 的常量、`rurge-config` 的那一份、`rurge_dns::fakeip::in_space` 的边界（`198.17.255.255` 不在、`198.18.0.0` 在、`198.19.255.255` 在、`198.20.0.0` 不在，IPv6 的 `fd00:6152::` 与 `fd00:6152:0:1::` 同理）一致。
  - `wanted_follows_ipv6_vif`：三种取值 × `ipv6_usable` 的真假。
- 端到端（`crates/rurge/tests/run/enhanced_mode.rs`；`RURGE_ENHANCED_BACKEND=file:<数据目录>/enhanced.json`）：
  - `what_a_dead_run_left_is_undone_by_the_next_start`：手写 `state.json`（`enhanced_backup` 依次有 `Test{take_dns}`、`Test{adjust}`）与文件后端的文件（`dns: true, adjusted: true`），起 `rurge run`（不带 `--enhanced-mode`）：`actions` 依次多 `undo adjust`、`undo take_dns`（倒着），`state.json` 没有 `enhanced_backup`，`--log-file` 里有 `restored what the enhanced mode left last time`。
  - `what_a_dead_run_left_is_undone_even_when_the_profile_does_not_load`：同上，配置有错（以 2 退出），标准错误有 `a previous run left changes of the enhanced mode; they were undone`，撤销做了。
  - `an_undo_that_fails_stays_for_the_next_start`：`fail_undo: "take_dns"`：`adjust` 撤销了、`take_dns` 那条留在 `state.json`；去掉 `fail_undo` 再起一次就撤销了。
  - `the_helpers_keep_the_enhanced_mode_off`：`rurge_process()` 与 `rurge_command()` 给的环境里 `RURGE_ENHANCED_BACKEND` 是 `off`（读 `Command` 的环境断言，不起进程）。
  - `an_unknown_enhanced_backend_ends_the_start_with_2`。

**判据**：`git grep -n "Command::new\|CommandRunner\|SystemRunner" crates/rurge/src/cli/enhanced` 什么也没有；门禁绿；`closing_waits_…` 单独循环跑二十次都通过。

---

## Task 8：开关、API、退出的次序与设备坏了

**改哪里**（设计第 3.6、9.3、9.4、9.6 节、M4-D15、D16；P24）

1. `RunArgs` 加：
   ```rust
   /// Take the machine's traffic through a virtual adapter while rurge runs
   /// (the enhanced mode). Needs root or Administrator.
   #[arg(long, env = "RURGE_ENHANCED_MODE", value_parser = switch())]
   pub enhanced_mode: bool,
   ```
2. `rurge-engine/src/control.rs`：
   ```rust
   /// What the enhanced mode is now (`GET /v1/features/enhanced_mode`).
   #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
   pub struct EnhancedStatus { pub enabled: bool, pub counters: TunCounters }

   // Control 加：
   /// Turns the enhanced mode on or off. The error text is shown to the API caller.
   fn set_enhanced_mode(&self, enabled: bool) -> BoxFuture<'_, Result<(), String>>;
   /// Whether the enhanced mode is on, and what its TUN has counted (zeros when off).
   fn enhanced_mode(&self) -> EnhancedStatus;
   ```
   `rurge-api` 测试里的假 `Control`（与别处的实现，`git grep -n "impl Control for"`）补上两个方法。
3. `rurge-api/src/routes/features.rs`：`enhanced_mode` 的 `GET` 回
   `{"enabled": <bool>, "dropped": {"malformed": n, "fragment": n, "martian": n, "unsupported": n, "queueFull": n, "unknownFake": n, "busy": n, "beyondWindow": n, "rstLimited": n, "icmpLimited": n}, "stackPanics": n}`（P12 的键名）；`POST` 经 `set_enhanced_mode`，失败 500 带原因，成功 `{}` 与 INFO `enhanced mode switched via http-api`。`system_proxy` 照旧；`mitm`、`capture`、`rewrite`、`scripting` 照旧（`GET` 恒 `false`、`POST` 501）。模块说明改。
4. `run.rs`：
   - `Command::EnhancedMode(bool, oneshot::Sender<Result<(), String>>)`；`Command::TunEnded(u64)`（第 6 条）；`LoopControl` 加 `enhanced: Arc<Shared>`，实现两个新方法（照 `set_system_proxy` / `system_proxy_enabled`）。
   - 管理器在引擎建好之后建（`Starter` 拿 `engine.clone()`），`Shared` 交给 `LoopControl`。
   - 启动：监听器与 API 起来之后、打开系统代理之前（P25）：`--enhanced-mode` 且不在暂停的网络上（任务 9 接上暂停；这一任务先不管暂停）→ `enable(wanted)`；失败 → 标准错误 `error: cannot turn the enhanced mode on: <原因>`，以 1 退出（`enable` 自己已经撤销了）；成功 → 标准输出 `enhanced mode on`。**系统代理打开失败而以 1 退出之前，先关掉已经开着的增强模式。**
   - 主循环：`Some(Command::EnhancedMode(enabled, reply))` → 这一任务直接 `enable` / `disable`（任务 9 改经 `Takeover`），失败 ERROR 一行；回 `reply`。
   - 退出：`drop(following_the_network)` 之后、系统代理之前：开着 → `disable()`，成功标准输出 `enhanced mode off`，失败标准错误 `error: cannot turn the enhanced mode off: <原因>; what outlives rurge stays in state.json and is undone on the next start`。
5. 引擎的 `Starter`（任务 7 的接缝）在 `run.rs` 或 `cli/enhanced/` 里实现，`RunningTun::ended()` 是 `TunHandle::join()` 再 `Engine::tun_ended()`。
6. 设备坏了（P24）：管理器每次打开之后起一个看着 TUN 的任务：`join()` 的结局是 `Ending::DeviceFailed(_)` 时向主循环发 `Command::TunEnded(这一次打开的序号)`；主循环收到、序号是当前这一次的 → ERROR `the virtual adapter failed; the enhanced mode is off`（原因只在 DEBUG），`disable()`。`Takeover` 记的"想要"不变，不重试。`Ending::Stopped`（管理器自己停的）什么也不发。

**用例**

- `rurge-api` 的用例（照 `features` 现有的写法，经假 `Control`）：`enhanced_mode_get_has_the_counters_and_the_shape`（十个键与 `stackPanics` 都在，值来自假 `Control`）；`enhanced_mode_post_goes_to_the_control_and_its_error_is_a_500`。
- 端到端（`run/enhanced_mode.rs`；期望的路由条数一律由用例用 `RouteSet::compute` 与 `RouteInputs { explicit_space: cfg!(target_os = "macos"), … }` 算出，不写死）：
  - `the_enhanced_mode_opens_at_start_and_closes_at_stop`：`--enhanced-mode` 与 `--system-proxy`（两个文件后端）：起来之后 `actions` 是 `open, configure, routes +N -0, take_dns, adjust`，`dns`、`adjusted` 为真，`state.json` 有两个撤销动作、`features.enhanced_mode` 为真；API `stop` 之后 `actions` 末尾依次是 `give_dns_back, undo take_dns, routes +0 -N, close, undo adjust`，`state.json` 没有 `enhanced_backup`；标准输出里 `enhanced mode off` 在 `system proxy restored` 之前。
  - `turning_it_on_and_off_through_the_api`：不带 `--enhanced-mode` 起：`GET` → `enabled: false`、十个计数与 `stackPanics` 都是 0；`POST {"enabled": true}` → 200 `{}`、`GET` → `true`；再 `POST true` → 200，`actions` 不多；`POST false` → 关。
  - `a_step_that_fails_undoes_what_was_done_and_the_start_ends_with_1`：`fail: "take_dns"`：以 1 退出，标准错误有 `cannot turn the enhanced mode on: take_dns failed (test)`；`actions` 里打开的几步之后是 `give_dns_back, undo take_dns, routes +0 -N, close`；`state.json` 没有 `enhanced_backup`。
  - `post_failure_is_a_500_with_the_reason`：`fail: "routes"`，`POST true` → 500，正文带 `routes failed (test)`；`GET` → `false`。
  - `a_killed_run_with_the_enhanced_mode_on_is_undone_by_the_next_start`：`--enhanced-mode` 起来，`Child::kill()`，再起（不带）：撤销照做（同任务 7 的那条）。
  - `ipv6_vif_always_opens_with_ipv6`：`actions` 有 `configure` 且文件的 `ipv6` 为真，`routes` 里有 IPv6 的前缀；`ipv6_that_fails_goes_on_without_it`（`fail: "configure_ipv6"`）：照开，`ipv6` 为假，没有 IPv6 的前缀，`--log-file` 里有一行 WARN。
  - `a_device_that_fails_turns_the_enhanced_mode_off`：`device_fails_after_ms: 200`：等到 `actions` 末尾出现关闭的几步（`give_dns_back … close, undo adjust`），`GET` → `false`，`--log-file` 有 `the virtual adapter failed`。

**判据**：门禁绿；新写的、等待守护进程状态变化的用例各单独循环跑二十次都通过。

---

## Task 9：接管暂停、重载与网络变化

**改哪里**（设计第 9.7 ～ 9.9 节、M4-D18；P18）

1. `network.rs`：
   ```rust
   /// What the main loop is told of a change of network.
   pub struct NetworkChange { pub facts: NetworkFacts, pub ipv6_usable: bool }
   /// Whether the network has IPv6 the enhanced mode can use (`ipv6-vif = auto`):
   /// the primary interface has an IPv6 gateway and a global IPv6 address.
   pub fn ipv6_usable(snapshot: &Snapshot) -> bool
   ```
   `ipv6_usable`：`primary.routers` 里有 IPv6 地址，且 `addresses` 里有落在 `2000::/3` 的。`spawn` 的 `wrap` 改成 `fn(NetworkChange) -> C`；`Command::NetworkChanged(NetworkChange)`；启动时由 `started.snapshot` 算一次，存在主循环里（"当前的 `ipv6_usable`"）。
2. `Takeover` 加 `enhanced_wanted: bool`（`--enhanced-mode` 的值）：
   - `begin`：暂停时两样都不开（今天对系统代理就是这样）；不暂停时由 `run()` 照任务 8 打开。
   - `follow`：进入暂停——增强模式开着就记下想要、`disable()`（失败 ERROR `cannot turn the enhanced mode off for the pause; …`）；离开暂停——`(想要, 开着)` 为 `(true, false)` 时 `enable`（失败 ERROR，不重试），`(false, true)` 时 `disable`。先增强模式、后系统代理（与退出的次序一致）。
   - 新的 `switch_enhanced(enabled)`：暂停时只记下（INFO `the takeover is paused on this network; the enhanced mode is left as it is, and set as asked when the pause ends`）；不暂停就 `enable` / `disable`；成功后记下想要。照 `switch` 的写法与说明。
   - 主循环的 `Command::EnhancedMode` 改经 `switch_enhanced`。
3. 重载：`reload_and_refresh` 成功之后、`takeover.follow` 之后：增强模式开着 → `enhanced.follow(wanted, false)`，`wanted` 用新的一代的配置（`engine.runtime().config` 的 `general` 与 `external_addresses()`）、当前的 `ipv6_usable` 与 `engine.network()` 的主网卡算。
4. 网络变化：`network_changed` 里 `takeover.follow` 之后：增强模式仍开着 → 更新当前的 `ipv6_usable`，`enhanced.follow(wanted, true)`。
5. `EnhancedMode::follow`（设计第 9.7、9.8 节）：
   - IPv6 从关到开：`configure_ipv6`（`Ok(false)` → 这一次仍当没开），加 IPv6 的路由，DNS 再 `dns_undo` / 写备份 / `take_dns(true)`；从开到关：`unconfigure_ipv6`，减 IPv6 的路由，`take_dns(false)`。
   - 路由按 `RouteSet::diff` 加减（一次 `routes(add, remove)`）。
   - 出口表的槽换成新的。
   - `network_changed` 为真时：`dns_undo` → 写备份 → `take_dns`（macOS 上新出现的服务；文件后端上 `dns_undo` 是空的，`take_dns` 再记一次）；`adjust_undo` → 写备份 → `adjust`。
   - 任何一步失败：ERROR（带失败的步骤与原因），照开着，已做的保留，不回滚到旧的集合；`GET` 照实是 `true`。

**用例**（端到端；网络文件与 `[SSID Setting]` 的写法照 `mod the_network` 里暂停的用例）

- `the_enhanced_mode_waits_on_a_suspend_network_and_opens_when_it_is_left`：带 `--enhanced-mode` 起在暂停的网络上：没有动作、`GET` 是 `false`；换成不暂停的网络：开了；换回暂停的：关了；再离开：又开了。
- `a_post_during_the_pause_is_noted_and_done_when_it_ends`：暂停中 `POST true` → 200、没有动作；离开暂停 → 开。开着时进入暂停、暂停中 `POST false` → 离开暂停时不开。
- `a_reload_changes_only_the_routes_that_changed`：开着时给 `tun-excluded-routes` 加一项、重载：文件的 `routes` 等于新算的集合，`actions` 多一条 `routes +a -b`（a、b 由用例用 `RouteSet::diff` 算出）；没有 `open`、`close`、`take_dns`。
- `a_reload_turns_ipv6_on_and_off`：`ipv6-vif = disabled → always → disabled`：依次多 `configure_ipv6` 与 IPv6 的路由、`take_dns`；再 `unconfigure_ipv6`、减 IPv6 的路由、`take_dns`。
- `ipv6_auto_follows_the_network`：`ipv6-vif = auto`；网络文件的主网卡加上 / 去掉 IPv6 网关与一个 `2001:db8:1::/64` 的地址（只出现在网络文件里）：IPv6 随之开、关。
- `a_follow_that_fails_keeps_the_enhanced_mode_on`：开着之后在文件里写 `fail: "routes"`、改配置、重载：`--log-file` 有 ERROR，`GET` 仍是 `true`。

**判据**：门禁绿；新写的、等网络变化或重载的用例各单独循环跑二十次都通过。

---

## Task 10：文档

- `docs/api/phase3.md`：M4 一节的开头（这一份做到的）：`--enhanced-mode` / `RURGE_ENHANCED_MODE`；`GET` / `POST /v1/features/enhanced_mode` 的形状与 500；日志行（INFO 与 DEBUG 各说什么，`the virtual adapter failed`）；`state.json` 的 `enhanced_backup` 与恢复（启动时、配置读不了时）；暂停的语义；`rurge check` 的 `W0035` ～ `W0038` 与 `external` 的 `addresses` 不再是 `W0029`；**这一版的平台后端答 "not available"（M4c / M4d 之后才真的能开）**。
- `docs/testing.md`：`RURGE_ENHANCED_BACKEND`（`file:` 的文件形状、动作名、`fail` / `fail_undo` / `device_fails_after_ms`；`off`；辅助函数缺省 `off`），"只供测试的后端"那一句加上它；新用例的过滤词（`route::`、`enhanced`）。
- `CLAUDE.md`：「测试约定」里讲 `rurge_command()` / `rurge_process()` 的那条加"设 `RURGE_ENHANCED_BACKEND=off`"；常用命令下面列只供测试的后端的那一句加上 `RURGE_ENHANCED_BACKEND`。「现在在哪里」不改（M4d 收尾时改）。
- `docs/status.md`：阶段 3 下新的一节"M4a（平台无关的骨架）"——做了什么；没做的与不要当成已经有的（平台后端不能用；出口表没有读的人；`NoPinger`；Linux / macOS 上 `0600` 的用例只在 CI 上跑）；以后的会话必须知道的事（`try_update` 与 `update` 各用在哪里；备份只记会在崩溃之后留下的；`Starter` 接缝；辅助函数缺省 `off`；`Command::TunEnded`）；附录加两条依赖边。
- 兼容性清单：`[General]` 的 `ipv6-vif`、`tun-excluded-routes`、`tun-included-routes` 与 `external` 的 `addresses` 的"目前"一栏照实改一句（解析、`rurge check` 的告警与路由集已有，平台的执行随 M4c / M4d）；四个新码登记在诊断码的表里（若清单有这张表）。
- 本计划末尾两张表照实填。

**判据**：`git grep` 核对文档里引用的用例名、文件与函数都在；门禁绿。

---

## 计划期决定

| # | 事项 | 决定 |
| - | ---- | ---- |
| P1 | 计划引用的名字与行号 | 写计划时对着 `ac0a98a` 核对过：`dispatch.rs` 的 `route` / `shard_of`、`Shard::rebuild`、`QueuedBytes` / `Reserved`、`Limiter`、`TunOptions::check`、`Counters`、`MemoryDevice`、`Pinger`；`start_tun` 与它的两个测试调用者、`fake_pool` / `let_go`、`State`、`StateStore::update` / `write_atomic`、`Control`；`features.rs`；`run.rs` 的 `RunArgs`、`Command`、`LoopControl`、`Takeover`、`network_changed`、`reload_and_refresh`、退出的一段、恢复的一族；`sysproxy.rs` 的管理器与文件后端；`network::spawn` / `Started` / `facts_of`；`rurge_command()` / `rurge_process()` 与 `mod run` 的辅助；`codes` / `codes::hint`；`spec/mod.rs` 的 `addresses`；`Config::specs`；`netenv::Snapshot`。执行时名字不同的照实改，记进修正记录，不改语义 |
| P2 | Windows 的接口度量与路由度量 | 都写 `1`，不写设计第 5.4 节的 `0`：`netsh` 的文档没有说 `metric=0` 是什么，有报告说 `0` 在网关度量上意为"自动"。度量只在前缀一样长时起作用（基础前缀都比默认路由具体）；`1` 已经比自动度量给物理网卡的值小（Windows 的自动接口度量按链路速度给 5 起），系统 DNS 按接口度量排先后时也排在前面。订正设计第 5.4 节 |
| P3 | Windows 的路由不写下一跳 | `add route` 省掉 `nexthop`（文档："The gateway address, if the prefix isn't on-link"）：虚拟网卡上的路由是在链路上的。订正设计第 5.4 节的 `0.0.0.0` |
| P4 | `netsh` 的写法 | 按微软文档核对（"netsh interface" 一页，learn.microsoft.com/windows-server/administration/windows-commands/netsh-interface；Windows Server 2008 的 IPv4 一页，cc731521）：`add address` 用 `name=`、`set interface` / `set subinterface` / `add route` / `delete route` 用 `interface=`、`set dnsservers` 用 `name=`，都接受"名字或序号"；`store=active`（"until next boot"）在 `add address`、`set interface`、`set subinterface`、`add route`、`delete route` 上有，`set dnsservers` 没有 `store`（网卡的 DNS 写在网卡自己的配置里，随网卡去掉）；`set interface` 有 `forwarding`、`weakhostsend`、`weakhostreceive`、`metric`。每个参数写成 `键=值`，不靠位置；脚本经 `netsh -f <文件>`。IPv6 的写法照 IPv4（同一页的 IPv6 部分：`add address` 用 `interface=`）——M4d 的开工前核对对着 IPv6 那一页再核一遍 |
| P5 | macOS 与 Linux 的写法 | 按手册页的记忆与 sing-tun / wireguard-go 的源码事实写，**写计划时没有逐页读手册**：macOS `ifconfig <utun> inet A A netmask M mtu 1500 up`、`ifconfig <utun> inet6 A prefixlen 64`、`route -n add [-inet6] -net P -interface <utun>`、`networksetup -setdnsservers <服务> <地址…>` / `Empty`、`dscacheutil -flushcache`、`killall -HUP mDNSResponder`；Linux `ip -batch`（遇第一个错误停）与 `ip -force -batch`（遇错继续、有错时退出码非零）、`route replace`、`addr add`（地址族由地址定）、`resolvectl dns|domain|default-route|flush-caches|revert`。M4c 的开工前核对对着 `ip(8)`、`resolvectl(1)`、`ifconfig(8)`、`route(8)`、`networksetup(8)` 的手册页再核一遍，有出入改翻译与它们的用例 |
| P6 | 起 TUN 的接缝 | 管理器经 `Starter` / `RunningTun` 起停 TUN，生产里包 `Engine::start_tun`。理由：设计第 9.4 节的"至多 3 秒"要一条确定的用例（一个永不结束的 TUN），引擎做不出一个永不结束、又不碰别的东西的 TUN |
| P7 | 一个引擎一个 TUN | 一个许可的信号量，许可随保存池的任务走，最后一次保存之后放掉；`Engine::tun_ended()`；管理器的 3 秒包括它，所以关了马上再开不会撞上"已经在运行"——除非上一个 TUN 真的没结束，那时这个错误是对的 |
| P8 | `start_tun` 也拒绝 MTU 小于 1280 的设备 | 回 `Err`，与选项一起让 `start_tun` 成为全函数（设计第 3.4 节只说了选项） |
| P9 | 排队的字节按容量计 | `Vec::capacity()`：它是包占着的内存。守约的设备上容量等于长度；不守约的设备只是让预算更快用完（丢包、计 `queue_full`），不会让内存失控 |
| P10 | 片重建的 WARN | 每片一个 `Limiter::new(1, 60 s)`；被挡下的那行降为 DEBUG；`stack_panics` 每次都数 |
| P11 | 备份的增删 | 加用 `try_update`（写不进就不做那一步）；去掉用 `update`（写不进只记 ERROR：下次启动再撤销一遍，撤销对已经撤销的无害） |
| P12 | 关闭时撤销的次序与 API 的键名 | 关闭按撤销动作的阶段（DNS 在第一步、`rp_filter` 在关设备之后），不按列表的倒序；崩溃之后的恢复按列表的倒序。`GET` 的 `dropped` 里的键用 camelCase（`queueFull`、`unknownFake`、`beyondWindow`、`rstLimited`、`icmpLimited`），与 `stackPanics` 和 API 别处一致 |
| P13 | `RURGE_ENHANCED_BACKEND` 的读法 | 照 `RURGE_NETWORK_BACKEND`：在读配置之前解析，不认识的值让启动以 2 结束，系统代理的恢复照做 |
| P14 | `--wintun-dll` | 放到 M4d（这一份没有用它的人） |
| P15 | ICMP | 这一份的 TUN 用一个立刻失败的 `NoPinger`（M4b 换成真的）；平台后端打不开设备，使用者碰不到 |
| P16 | `features.enhanced_mode` | 照系统代理那一项：打开完成时写 `true`、全部撤销之后写 `false`；启动时不读（M4-D15）。Windows 上没有会留下的动作，所以单独写，不随备份写 |
| P17 | 端到端用例的位置 | `crates/rurge/tests/run/enhanced_mode.rs`，`cli.rs` 的 `mod run` 里 `mod enhanced_mode;`（Rust 按 `tests/run/enhanced_mode.rs` 找到它；`tests/` 下的子目录不是独立的测试目标）：用得上 `rurge_process()` 与 `mod run` 的辅助，又不让八千多行的 `cli.rs` 更长。CLAUDE.md 的"一律经 `cli.rs` 的 `rurge_command()` / `rurge_process()`"照旧成立 |
| P18 | `ipv6-vif = auto` 的判据 | 主网卡有 IPv6 网关（`Primary.routers` 里有 IPv6 地址）且有全局的 IPv6 地址（`Snapshot.addresses` 里有落在 `2000::/3` 的 `/64`）；随网络变化的命令送进主循环 |
| P19 | 测试后端的设备 | 一个空设备（从不送包、丢掉写来的包），不用设计第 9.10 节说的 `MemoryDevice`：后者的另一端没人读时写会等，要多一个取包的任务。订正设计第 9.10 节 |
| P20 | 四个码的文字与建议 | `W0035` 消息 "`ipv6-vif = always` may break apps on networks without IPv6 connectivity"，建议 "use `ipv6-vif = auto` to turn IPv6 on only where the network has it"；`W0036` 消息 "`tun-included-routes` takes in the whole private range {范围} ({项}); this is normally unnecessary and can cause system issues"，建议 "include only the addresses that have to go through rurge"；`W0037` 消息 "`tun-excluded-routes`: {项} overlaps rurge's own range (198.18.0.0/15 or fd00:6152::/64), which always goes through the virtual adapter"，建议 "exclude only addresses outside 198.18.0.0/15 and fd00:6152::/64"；`W0038` 消息 "`server:force-syslib` asks the system's resolver, which answers with fake addresses while the enhanced mode is on"，建议 "use `server:system` to keep real addresses while the enhanced mode is on"。四个都只在一处报，都有缺省建议 |
| P21 | `Config::external_addresses` | 只取会被起的 `external`；被 `W0033` 拒绝的若仍在 `specs` 里，按它的标记跳过（任务 0 核对） |
| P22 | rurge 空间的显式路由 | `RouteInputs::explicit_space`：macOS 为真（可执行文件按 `cfg!(target_os = "macos")` 给），其余为假；`RouteSet` 本身不知道平台 |
| P23 | 停点 | 做完任务 10 停下；中途遇到设计第 20 节的问题或会改变 M4-D1 ～ D30 的，停下来列出 |
| P24 | 设备自己坏了 | 设计没有写这个情形。TUN 以 `Ending::DeviceFailed` 结束时，管理器经主循环把增强模式整个关掉（交还 DNS、撤销会留下的），ERROR 一行，不重试，`Takeover` 记的"想要"不变。理由：macOS 上系统 DNS 指着应答器、应答器已经不在，不关就是整机断 DNS。补进设计第 9.4 节的订正（任务 10 不改设计正文；M4d 收尾时在设计的订正里写） |
| P25 | 启动时的次序 | 增强模式先于系统代理打开（系统代理只指向早已起来的监听器；增强模式失败时系统代理还没有动）；系统代理打开失败而以 1 退出之前先关掉增强模式 |

## 执行期修正记录

下面写"执行者的裁定"的，是 Pi 在设计与计划的原则下定的，项目所有者可以改。计划的正文不改：正文里说得不对的地方，这张表的行覆盖它。

| # | 任务 | 修正 | 提交 |
| - | ---- | ---- | ---- |
| 1 | 0 | 第 1 项：核对过，没有出入。`HEAD` = `2c6a215`（写计划的提交），工作树干净，`main` 与它同一个提交、`2c6a215..HEAD` 没有多出来的提交；本分支 `phase3-m4a` 没有推送（`origin/main` = `ac0a98a`） | 本提交 |
| 2 | 0 | 第 2 项：`dispatch.rs` 的 `route` / `shard_of`（`:62` / `:94`，`DefaultHasher::new()` `:95`）、片的通道 `mpsc::Sender<Vec<u8>>`（`:25`）、`Shard::rebuild`（`:112`，WARN `:116`）、片的 `run` 与 `recv_many`（`:1089` / `:1121`）、`udp/bytes.rs` 的 `QueuedBytes` / `Reserved`（`:16` / `:53`）、`limit.rs` 的 `Limiter`（`:8`）、`TunOptions` 与 `check()`（`lib.rs:218` / `:317`，`shard_queue` 的检查 `:333`）、`Counters`（`:628`）、`device.rs` 的 `Device` 都对上。计划 P9 与任务 1 说的 `queue_bytes` 今天不存在：`TunOptions` 只有按**包数**的 `shard_queue`（`lib.rs:244`，缺省 `:284`），`route` 按包的个数丢包并计 `queue_full`（`dispatch.rs:79` / `:82`）。按计划在任务 1 新加这个字段与它的一条 `check()`、它的边界用例 | 本提交 |
| 3 | 0 | 第 3 项：`fake_pool` 是 `Engine` 的字段（`engine.rs:199`），不是函数；`let_go` 在 `tun.rs:23`。`Counters` 在 `lib.rs:628`（计划写"约 625"）、`an_association_returns_when_closed_and_lets_go_of_its_client` 在 `tests/tun.rs:710`（计划写"约 708"），按实际行号做；`start_tun` 的调用只有 `tests/common/tun.rs:62` 与 `tests/tun.rs:560` 两处，与计划一致 | 本提交 |
| 4 | 0 | 第 4 项：核对过，没有出入（`fakeip/file.rs:134` 的 `write` 先写 `.tmp` 再 `rename`，没有设权限——任务 2 要加的 `0600` 就加在这里） | 本提交 |
| 5 | 0 | 第 5 项：核对过，没有出入。`run.rs` 的 `RunArgs` `:55`、`Command` `:281`、`LoopControl` `:289`（`impl Control` `:305`）、`Takeover` `:600`（`begin` `:618`、`follow` `:634`、`switch` `:680`）、`network_changed` `:705`、`reload_and_refresh` `:793`、`holds_a_backup` `:1053`、`recover_without_a_log` `:1072`、`restore_what_a_crash_left` `:1091`、`Lock::recover` `:1139` 都对上；`run()` 里 `network::backend`（`:1255`）在 `load_checked`（`:1289`）之前、`sysproxy.recover()`（`:1376`）在 `StateStore::open`（`:1368`）之后；退出时 `sysproxy.disable()`（`:1629`）在 `api_token.cancel()`（`:1637`）之前。`sysproxy.rs` 与 `network.rs` 的名字也都对（`backend()` `:87`、`backend_from` `:94`、`FileBackend` `:104`、`SystemProxyManager` `:161`；`spawn` `:172` 收 `wrap: fn(NetworkFacts) -> C`、`facts_of` `:45`、`Started` `:100`），`runtime.rs` 的 `PlatformSockets` 在 `:227` | 本提交 |
| 6 | 0 | 第 6 项：核对过，没有出入。`rurge_command()` / `rurge_process()` 在 `tests/cli.rs:50` / `:62`，两个都设 `default_directories()`、一个不存在的网络文件与 `RURGE_PROCESS_BACKEND=off`，**都没有** `RURGE_ENHANCED_BACKEND`（任务 7 在这里加缺省的 `off`）；`mod run` `:2458`、`mod the_network` `:7217`、暂停的一族从 `the_system_proxy_pauses_on_a_suspend_network` `:7519` 起，`[SSID Setting]` 用常量 `OFFICE`（`:7508`），"系统代理"用 `sysproxy_file` 的一个 JSON 文件（`with_a_system_proxy` `:7511`）；`mod run` 里的辅助行号：`rurge_run_with` `:2504`、`rurge_run` `:2529`、`spawn_daemon_full` `:2568`、`spawn_command_with` `:2635`、`api_port` `:2729`、`api_call` `:2739`、`wait_for_exit` `:2785`、`sysproxy_file` `:2815`、`read_json` `:2819`、`put_network` `:2827`、`wait_until` `:2840`、`crashed_with_the_system_proxy_on` `:4613` | 本提交 |
| 7 | 0 | 第 7 项：`W0038`（`server:force-syslib`）报在 `[Host]` 的读法**外面**——`config.rs:823`～`:830` 的那个循环里（`line_failure`，`:582`），判据是 `HostEntry.value` 等于 `HostValue::System(SystemMode::ForceSyslib)`（值在 `host.rs:145` 读，`value` 是 `pub`，`host.rs:51`，`entry.span` 跟着它）；`host.rs` 的解析只回 `HostEntry` / `ParseError`，任务 6 不改它。其余对上：`ipv6-vif` `general.rs:917`、两条 `tun-*` `:932` / `:933`（`parse_nets` 在 `:701`，计划写的"约 932"是调用它的两行）、`codes` 最新 `W0034` `diagnostic.rs:265`、`codes::hint` `:314`（说明里"没有缺省建议的码"一段 `:274`～`:313`）、`WITH_DEFAULT` `:394`、`every_code_is_decided_once` `:492`、`addresses` 进 `inert` `spec/mod.rs:601`、用例 `:1027`、`specs` `config.rs:225`、`external_on_own_listener` `:292` | 本提交 |
| 8 | 0 | 第 7 项（P21 的答案）：被 `W0033` 拒绝的 `external` **不在** `Config::specs` 里。`NotImplemented::external_in`（`spec/mod.rs:210`）把它标成 `DownloadedExternal` / `ManagedExternal`，于是 `PolicySpec` 不建（`:675` 的 `(!failed && not_implemented.is_none()).then(…)`），`not_implemented` 留在 `SpecOutcome` 上（`:183`、`:694`），`config.rs:1546` 只 `specs.extend(outcome.spec)`。所以任务 6 的 `Config::external_addresses` 只遍历 `specs` 里 `ProtoSpec::External` 的那些，没有要按标记跳过的行（P21 后半句用不上） | 本提交 |
| 9 | 0 | 第 8 项：`netenv::Snapshot`（`:78`）里 `Primary.routers` 是"先 IPv4 后 IPv6"（`:48`～`:55`），`Snapshot.addresses` 里的 IPv6 项是主机位清零的 /64 网络地址（`network_addresses`，`:233`～`:252`，用例 `:414`），P18 的判据（落在 `2000::/3`）在今天的 `addresses` 上直接成立。`rurge-platform/Cargo.toml` 今天**没有** `ipnet`（任务 4 加；工作区 `Cargo.toml:20` 已有 `ipnet = "2"`）。`ipnet` 2.12.1：`Ipv4Net::new_assert` / `Ipv6Net::new_assert` 是 `const fn`（`ipnet.rs:657` / `:1045`），`aggregate` 的签名是 `fn aggregate(networks: &Vec<Ipv4Net>) -> Vec<Ipv4Net>`（`:952`，v6 `:1328`）——收 `&Vec<_>`，不是 `&[_]` | 本提交 |
| 10 | 0 | 第 9 项：核对过，没有出入。`routes/features.rs` 的 `FEATURES`（`:12`，已有 `enhanced_mode` 这个名字）、`get_feature`（`:29`，现在只有 `system_proxy` 读真值）、`set_feature`（`:43`）；`git grep -n "impl Control for" crates` 两处：`crates/rurge-api/tests/api.rs:46` 的 `FakeControl` 与 `crates/rurge/src/cli/run.rs:305` 的 `LoopControl`——任务 8 给 `Control` 加两个方法时，两处都要跟着实现 | 本提交 |
| 11 | 0 | 第 10 项：门禁基线（一台机器上的一次运行，不是 CI）——`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast` **81 个结果行、4304 通过、0 失败、13 忽略**（没有重跑任何二进制；13 个忽略都是原来就忽略的辅助用例与基准）。之后每个任务与这三个数字比 | 本提交 |
| 12 | 1 | 任务的第 2 条：`src/udp/bytes.rs` 挪成 `src/bytes.rs`（`pub(crate)`，模块说明同时讲 `udp_bytes` 与 `queue_bytes`，两者一个按数据报载荷长度、一个按包的容量计）；`udp/mod.rs` 与 `context.rs` 的引用跟着改，`tests/udp/budget.rs` 的 `use` 跟着改。新增 `Reserved::none()`（`#[cfg(test)]`），给不经队列的测试用例用一个零字节的预留（执行者的裁定） | 本提交 |
| 13 | 1 | 任务的第 2 条的必然结果：片的通道元素从 `Vec<u8>` 变成 `dispatch::Ingress`，于是 `ShardState::turn` / `last_turn` 的第二个参数也变成 `&mut Vec<Ingress>`（包的预留要在交给栈之后才归还，`ingress` 在 `ingest` 之后才被丢掉）。用例一侧加 `tests::support::ingress()`（把 `Vec<Vec<u8>>` 包成 `Vec<Ingress>`），现有 59 处 `turn` / `last_turn` 调用点机械改写；`tests/outcomes/shard_task.rs` 自己起片的 `run`，`try_send` 用 `Ingress::unheld`（执行者的裁定） | 本提交 |
| 14 | 1 | 任务的第 5 条：`event!` 的级别不能是运行期的值（静态 callsite 的 metadata 要常量级别），所以一行两句做不到；重建的那句话写成 `tcp/shard.rs` 的常量 `REBUILT`，`warn!` 与 `debug!` 两个分支都写 `"{}", REBUILT`，日志的整行与改动前一样（执行者的裁定） | 本提交 |
| 15 | 1 | 任务的第 1、4 条：`route` 与 `dispatch::run` 各多两个参数（`&Arc<QueuedBytes>` / `Arc<QueuedBytes>` 与 `&RandomState` / `RandomState`），没有放进 `Context`——`Context` 是读设备的任务与 UDP 会话共用的，这两样只有读设备的任务用；`tests/filter.rs` 的四处 `route` 调用加了一个本地的 `queues()` 辅助（执行者的裁定） | 本提交 |
| 16 | 1 | 任务的用例落点：新的 `src/tests/dispatch.rs`（`tests/mod.rs` 里 `mod dispatch;`）放 `a_shard_queue_beyond_queue_bytes_drops_and_counts_queue_full`、`queued_bytes_are_given_back_when_the_shard_takes_the_packets`、`shards_are_chosen_with_a_key_of_each_tun`；`options::queue_bytes_must_be_more_than_zero` 进 `tests/options.rs`，`a_shard_rebuilt_twice_in_a_minute_warns_once` 进 `tests/rebuild.rs`（照现有的分法） | 本提交 |
| 17 | 1 | 任务要的"让片停住"、任务 0 没有现成的办法，本任务用了两种：`a_shard_queue_beyond_queue_bytes_drops_and_counts_queue_full` 直接调 `route`、片的通道**从不读**（片不取包，完全确定，不起 TUN）；`queued_bytes_are_given_back_when_the_shard_takes_the_packets` 用现成的 `GatedDevice` 加 `writer_queue: 1`：写者堵在 `device.send` 上、片堵在写者的队列上，于是不再取包，`gate.open()` 之后用 `eventually`（10 s）等 `queue_bytes_queued() == 0`。没有为用例改生产代码的行为（执行者的裁定） | 本提交 |
| 18 | 1 | `options_out_of_range_are_refused` 的"Each of the seventeen texts"没有把 `queue_bytes` 加进那条用例（它仍说十七个），越界的断言放在新增的 `queue_bytes_must_be_more_than_zero` 与 `the_first_option_out_of_range_is_the_one_reported`（补了"`shard_queue` 在前、`writer_queue` 在后"两条），`the_edges_of_the_ranges_pass` 补了 `queue_bytes: 1`（执行者的裁定） | 本提交 |
| 19 | 1 | 任务的判据：`git grep -n "DefaultHasher" crates/rurge-tun/src` 没有输出（退出码 1，`udp::bytes` 的旧路径也搜不到了）；`TunOptions::default().check()` 由 `options::the_defaults_pass_the_check` 通过；门禁（一台机器上的一次运行，不是 CI）**81 个结果行、4309 通过、0 失败、13 忽略**——基线 4304 加上本任务的 5 条；没有重跑任何二进制 | 本提交 |
| 20 | 2 | 任务第 2、3 条：MTU 的下限在 `engine/src/tun.rs` 写成模块常量 `MIN_MTU: usize = 1280`（`rurge-tun` 的同名常量是私有的 `const`，不导出，本任务不改它的对外接口）；一个引擎一个 TUN 的字段是 `Engine::tun_permits`，`pub(crate)`——`tun.rs` 与 `engine.rs` 是同一个 crate 的两个模块，私有字段它读不到（执行者的裁定） | 本提交 |
| 21 | 2 | 任务的用例：`TunHandle` 不实现 `Debug`，`Result::expect_err` 用不了，三处写成 `let Err(error) = … else { panic!(…) }`（照 `fakeip/file.rs` 里现成的写法）；三条新用例放在 `tests/tun.rs` 新加的一节 `// ---- starting a TUN` 里（`// ---- the pool` 之前）；`file.rs` 的用例不引用 `FILE_NAME`，用 `fake-ip.txt` 这个字面名字（执行者的裁定） | 本提交 |
| 22 | 2 | 判据与门禁（一台机器上的一次运行，不是 CI）：`git grep -n "# Panics" crates/rurge-engine/src/tun.rs` 没有输出（退出码 1）；`start_tun` 的两个调用者（`tests/common/tun.rs`、`tests/tun.rs`）都 `.expect("the TUN starts")` 了 `Result`；改了时限的 `an_association_returns_when_closed_and_lets_go_of_its_client` 与新用例 `a_second_tun_on_one_engine_is_refused_until_the_first_has_ended` 各单独循环跑 20 次全过；门禁 **81 个结果行、4312 通过、0 失败、13 忽略**——基线 4309 加上本任务的 3 条；`#[cfg(unix)]` 的 `file::write_temporary` 与 `the_pool_file_is_its_owners_alone` 在这台机器上没有编译过（只有 `x86_64-pc-windows-msvc` 一个目标），所以不在这个数里，等 CI 的 Linux / macOS 运行 | 本提交 |
| 23 | 3 | 任务的第 1、2 条：`enhanced_backup` 放在 `system_proxy_backup` 之后（说明照它写：管理器拥有它的形状、引擎不读它），`STATE_VERSION` 仍是 `1`；`try_update` 紧接在 `update` 之后，照 `update` 的写法（同一把锁、克隆、`expect("State serializes")`、`spawn_blocking` + `write_atomic`），锁里的状态只在写成功之后才换掉。`StateStore` 的文档句从「on every `update`」改成「on every write」（它现在有两个写入口）；`io::Error::other` 把 `spawn_blocking` 的任务失败映射成 `io::Error`，写失败的那个 `io::Error` 原样返回（执行者的裁定） | 本提交 |
| 24 | 3 | 任务的用例：`try_update_changes_nothing_when_the_file_cannot_be_written` 把 `state.json.tmp` 建成目录，并用同一个路径的一次 `std::fs::write` 取到应当出现的 `io::Error`，只断言两者的 `kind()` 相同（错误种类随平台而变，`PermissionDenied` 与 `IsADirectory` 都是对的；不把某一平台的字面写进断言）；断言磁盘上的 `state.json` 不变之前，先用一次成功的 `update` 写出一份；`try_update_writes_before_it_changes_the_state` 里 `f` 断言它看到的是旧的状态（不是缺省）；三条都只用 `tempfile::tempdir()` 里的路径，都不等事情发生，因此不循环跑二十次 | 本提交 |
| 25 | 3 | 判据与门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast` **81 个结果行、4315 通过、0 失败、13 忽略**——基线 4312 加上本任务的 3 条；没有重跑任何二进制 | 本提交 |
| 26 | 4 | 任务的第 1、2 条：`ipnet.workspace = true` 加在 `if-addrs` 之后；`lib.rs` 的 `pub mod route;` 放在 `procinfo` 与 `service` 之间（字母次序）；`route/mod.rs` 照派工的裁定 5 `pub use set::{RouteInputs, RouteSet};`。常量用 `Ipv4Net::new_assert` / `Ipv6Net::new_assert`（`const fn`，值已按前缀截断）；`SPACE_V6` 一行被 rustfmt 折成两行 | 本提交 |
| 27 | 4 | 任务第 2 条（执行者的裁定）：第 1 步的基础前缀由 `set.rs` 的私有函数 `base(ipv6)` 给出（IPv4 八条，IPv6 开时九条），用例 `the_base_routes_are_eight_prefixes_each_way` 直接对它断言，不把 `compute` 的结果与一份手算的大列表比——后者要把 `128.0.0.0/1` 被 `169.254.0.0/16`、`224.0.0.0/4`、`255.255.255.255/32` 分成的 43 条全写出来。`compute` 的其余部分仍按计划的六个步骤 | 本提交 |
| 28 | 4 | 任务第 3 条次序 6（执行者的裁定）：`ipnet` 2.12.1 的 `Ipv4Net::aggregate` 认不出「刚好停在 `255.255.255.255` 之前」的区间（它给区间的末尾加一，而 `u32::MAX` 没有可加的），所以一碰到 IPv4 的最后一条地址就把广播地址也算进结果——总是减去的 `255.255.255.255/32` 会在合并之后回来（一条 `240.0.0.0/4` 盖住它，用例 `loopback_link_local_multicast_and_broadcast_never_go_in` 抓到的就是这个）。`compute` 因此在合并前记下广播地址有没有被路由，合并之后用求补集把它去掉；`tun-included-routes` 显式包含它时不剥。`aggregate` 照旧是唯一的合并（没有另写一遍） | 本提交 |
| 29 | 4 | 任务的用例：12 条都在 `set.rs` 的 `#[cfg(test)] mod tests` 里（派工的裁定 1），`set.rs` 里没有别处的 `cfg`。`rurges_space_is_never_stripped_and_pieces_inside_it_are_dropped` 里「结果里有 `198.18.0.0/15`」按字面断言（排除了 `198.0.0.0/8` 的那组输入下它单独成一条），另加一条：什么也不排除时它被并进 `192.0.0.0/3`、不再单独出现（派工的裁定 4）。`included_wins_over_excluded` 原打算断言「`9.255.255.255` 不包含」是错的（它在 `8.0.0.0/5` 里、没有被排除），改成 `10.9.9.9`（执行者的裁定） | 本提交 |
| 30 | 4 | 任务用例里手算的数写进注释的地方：`one_excluded_address_leaves_a_hole_and_no_more`（55 - 1 + 27 = 81，并写出 55 与 43 各是哪些基础前缀留下的）、`adjacent_holes_merge`（55 - 1 + 26 = 80，且断言两个相邻的 `/32` 与一个 `/31` 结果相同）、`a_hole_that_covers_a_base_prefix_takes_it_whole`（54）、`a_hole_across_two_base_prefixes`（43）。这四处的数都是先用一段独立的脚本按同一套集合运算核过、再与实现的输出对上的 | 本提交 |
| 31 | 4 | 判据与门禁（一台机器上的一次运行，不是 CI）：`set.rs` 里只有 `#[cfg(test)]` 一处 `cfg`；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast` **81 个结果行、4327 通过、0 失败、13 忽略**——基线 4315 加上本任务的 12 条。`rurge-engine` 的 `outbounds_tuic` 二进制第一次以 `STATUS_HEAP_CORRUPTION` 退出（CLAUDE.md「这台开发机」指的那种），单独重跑 10 个用例全过，上面的数把它算进去了 | 本提交 |
| 32 | 5 | 任务的第 1 条（执行者的裁定）：`steps.rs` 多一个 `pub(crate) fn script(lines: &[String]) -> String`——Linux 与 Windows 的十几份脚本都由它拼（每行一条、行尾 `\n`、不带 `\r`），不把同一段拼接在 `linux.rs` 与 `windows.rs` 各写一遍。`Step` 的字段、`SCRIPT` 与 `OnFailure` 照任务抄 | 本提交 |
| 33 | 5 | 任务的第 2 条（执行者的裁定）：`LinuxTools` 不带派生（任务没有写）；`add_routes` 的 `ip` 不带 `-force`——设计 5.2 只给减路由写了 `-force`，加的路由是 `replace`，重复的不报错 | 本提交 |
| 34 | 5 | 任务第 2 条的 `give_dns_back`：`resolvectl` 是 `None` 时回空的 `Vec`（任务只对 `take_dns` 写了这一句）。`None` 的意思是这台机器没有 systemd-resolved、系统 DNS 从来没有被接管，所以也无从交还 | 本提交 |
| 35 | 5 | Windows 的私有辅助：`interface_line(index, family)` / `subinterface_line(index, family)` / `route_lines(nets, index, family, add)` / `netsh_step(t, lines, on_failure)`——两个地址族的行只差 `ipv4` / `ipv6` 这一个词，所以地址族是一个参数；度量写成常量 `METRIC: u8 = 1`（P2）；IPv4 的 `mask=` 来自 `SPACE_V4::netmask()`，IPv6 地址的后缀来自 `SPACE_V6::prefix_len()`（执行者的裁定） | 本提交 |
| 36 | 5 | 用例一侧（执行者的裁定）：三个 `tests` 模块各有两个辅助（`set(v4, v6)` 造 `RouteSet`，`scripted` / `plain` 造 `Step`），脚本的字面量用 `concat!` 分行写；`scripts_are_lines_ending_in_a_newline_and_no_carriage_return` 断言整组步骤里每一份脚本（Linux 6 份、Windows 8 份）的 `\n` 与 `\r`，其余字段由逐字比对的用例覆盖。这条用例第一次跑 GREEN 时 Linux 数成 5、Windows 数成 7（我自己数错），改成 6 与 8——是断言的口算错误，不是行为改动 | 本提交 |
| 37 | 5 | 判据与门禁（一台机器上的一次运行，不是 CI）：`linux.rs` / `macos.rs` / `windows.rs` 里各只有一处 `cfg`（`#[cfg(test)]` / `:172` / `:179` / `:185`），`steps.rs` 里没有；`cargo test -j 8 --workspace --lib -- route::` 41 条通过（`set` 的 12 条加本任务的 29 条）；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过（`resolvectl_step` 收 `&PathBuf` 被 `clippy::ptr_arg` 拦下，改成 `&Path`）；`cargo test -j 8 --workspace --no-fail-fast` 的日志里 80 个结果行、4334 通过、0 失败、13 忽略，`rurge-engine` 的 `smart` 二进制以 `STATUS_HEAP_CORRUPTION` 退出（CLAUDE.md「这台开发机」那种），单独重跑 22 条全过，合起来 **81 个结果行、4356 通过、0 失败、13 忽略**——基线 4327 加上本任务的 29 条 | 本提交 |
| 38 | 6 | 任务的第 1、5 条：四个码的名字照 `codes` 现有的写法定为 `W_IPV6_VIF_ALWAYS` / `W_INCLUDED_PRIVATE_RANGE` / `W_EXCLUDED_RURGE_SPACE` / `W_FORCE_SYSLIB`（`diagnostic.rs:267` / `:269` / `:272` / `:275`），消息与建议逐字照 P20；四个都是缺省建议（`codes::hint` `:389`～`:398`）并进了 `WITH_DEFAULT`（`:449`～`:452`）。名字是执行者的裁定 | 本提交 |
| 39 | 6 | 任务第 2 条的 rurge 空间副本（协调者的裁定 R5）：`general.rs:717` 的 `pub const RURGE_SPACE_V4: Ipv4Net` 与 `:719` 的 `pub const RURGE_SPACE_V6: Ipv6Net`，都用 `new_assert` 写成 `const`，说明指向 `rurge_platform::route::SPACE_V4` / `SPACE_V6` 并写明一致性由可执行文件里的一条用例钉住。放在 `general.rs`（判这两个码的解析同一个文件）而没有在 `lib.rs` 里加 re-export，所以任务 7 的用例读 `rurge_config::general::RURGE_SPACE_V4` / `RURGE_SPACE_V6`；模块与名字是执行者的裁定 | 本提交 |
| 40 | 6 | 任务第 2 条的判定：`W0036` 一项一条，`{范围}` 是它包含的第一段私网，次序 `10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`（`general.rs:727`～`:742`），只判 IPv4 项；`W0037` 一项一条，相交按两个方向任一包含算（`general.rs:744`～`:746`；`ipnet` 2.12.1 没有 `intersects`）。两处都只判 `parse_nets` 读出来的项，读不出来的项只留它自己的 `W0012`。消息里的 `{项}` 是解析后的 `IpNet` 的显示（主机位清零后的写法），不是行上的原文；执行者的裁定 | 本提交 |
| 41 | 6 | 任务第 2 条的 `W0038`：报在 `config.rs` 的 `[Host]` 循环里（`:840`～`:853`，任务 0 的定位），判据是 `h.value == HostValue::System(SystemMode::ForceSyslib)`、位置用 `h.span`；`host.rs` 没有改。任务第 3 条的 `spec/mod.rs` 那三行在 `:598`～`:601`，去掉之后只有 `tfo` 还从 `inert` 里被拿掉；任务第 4 条的 `Config::external_addresses` 在 `config.rs:304` | 本提交 |
| 42 | 6 | 计划没点名的三条现存用例跟着改了（执行者的裁定，理由就是任务第 1、3 条本身）：`every_key_the_manual_lists_is_read` 的样例值 `ipv6-vif = always` → `auto`、`tun-excluded-routes = 10.0.0.0/8, fd00::/8` → `10.0.0.0/8, 2001:db8::/32`（`general.rs:1419` / `:1420`；第一次写的是 `fd01::/8`，它被前缀截成 `fd00::/8`，用例红）、`the_same_parameters_with_good_values_still_get_their_sentence` 里 `addresses` 那一项从 `W0029` 改成一句也没有（`config.rs:3816`）、`an_error_in_one_parameter_leaves_the_sentences_of_the_others` 里 `local-port=0, addresses=…` 那一项现在只余 `E0018`（`config.rs:3814`）。计划点名的 `an_external_line`（`spec/mod.rs:1027`）照计划改成断言 `inert` 里没有 `addresses` | 本提交 |
| 43 | 6 | 任务第 1、3 条的快照（协调者的裁定 R4）：没有用 `cargo insta`，用一条命令 `$env:INSTA_UPDATE = "always"; cargo test -j 8 --workspace --test corpus; Remove-Item Env:INSTA_UPDATE` 跑。`git diff -- '*.snap'` 只有 `corpus__corpus__kitchen-sink.snap` 一个文件，两处变化与预期一致：`kitchen-sink.conf:71` 的 `W0029 addresses` 没了，多一行 `W0038` 指 `:104` 的 `printer.local` 并带缺省建议。insta 把这个文件写成 LF，而工作区其余快照是 `core.autocrlf=true` 检出的 CRLF，用一次 CRLF 归一化还原（`git diff` 仍是那两行，再跑语料库仍绿）；执行者的裁定 | 本提交 |
| 44 | 6 | 判据与门禁（一台机器上的一次运行，不是 CI）：`git grep -n 'inert.push("addresses")' -- crates/rurge-config` 什么也没有（退出码 1）；`every_code_is_decided_once` 绿；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast` 的日志里 80 个结果行、4344 通过、0 失败、13 忽略，`rurge-engine` 的 `outbounds` 二进制以 `STATUS_HEAP_CORRUPTION` 退出（CLAUDE.md「这台开发机」那种），单独重跑 19 条全过，合起来 **81 个结果行、4363 通过、0 失败、13 忽略**——基线 4356 加上本任务的 7 条 | 本提交 |
| 45 | 7 | 范围（协调者的裁定 R6）：本任务只定 `Starter` / `RunningTun` 两个接口、单元用例用的假实现（假后端 `Mock`、假 `Starter`、立刻结束或永不结束的假 TUN、`Empty` 设备）与 `NoPinger`；包着 `Engine::start_tun` 的生产 `Starter` 留给任务 8。`EnhancedMode::follow` 到任务 9 才加，这里没有留占位 | 本提交 |
| 46 | 7 | 死代码（协调者的裁定 R3）：`#[cfg_attr(not(test), allow(dead_code))]` 加在 `cli/enhanced/mod.rs` 的 `Starter`、`RunningTun`、`NoPinger`、`Shared`、`Status`、`Shared::status`、`egress_slot`、`filled`、`EnhancedMode::new` / `enabled` / `shared` / `enable` / `disable`，`wanted.rs` 的 `wanted`，与 `backend.rs` 的 trait 方法 `configure_ipv6` / `unconfigure_ipv6`；注释写「wired up by M4a task 8 / 9」或 M4b（`filled`）/ M4d（`Opened::index`）。`Opened::index` 用的是不带条件的 `#[allow(dead_code)]`：测试也不读它，`not(test)` 的条件在测试目标上会留一条警告。被 `run()` 用到的 `backend()`、`recover`、`Undo` / `Backup` / `Stage`、`Backend` / `Opened`、文件后端都没有加 | 本提交 |
| 47 | 7 | `closing_waits_for_the_tun_three_seconds_at_most` 只断言 `disable` 返回、`close` 被调用，以及暂停的时钟上它恰好等到 `JOIN_LIMIT`（3 秒）；可执行文件这个 crate 里没有 `LogCapture`，本任务不为一行 WARN 另造一个（协调者的裁定 3） | 本提交 |
| 48 | 7 | `Shared::status()` 回的是本模块自己的 `Status { enabled, counters }`：`rurge_engine::EnhancedStatus` 任务 8 才定义，届时换掉；关着时计数全 0（协调者的裁定 4） | 本提交 |
| 49 | 7 | 文件后端只把做成的动作记进 `actions`：失败的那一步不记，它对应的字段（`open`、`ipv6`、`routes`、`dns`、`adjusted`）也不改。`fail: routes` 只让「要加的那一半」失败，只有减的调用照常做成（关闭要用它）。这是协调者裁定 5 留给执行者的选择，`file.rs` 的模块说明里写了同一句 | 本提交 |
| 50 | 7 | `RunningTun::ended()` 回 `BoxFuture<'_, rurge_tun::Ending>`（不照任务里的 `()`）：假 TUN 回 `Ending::Stopped` 或一个永不结束的 future，关闭只等它结束、不看结局（协调者的裁定 6） | 本提交 |
| 51 | 7 | 执行者的裁定（计划没写到的地方）：(a) 打开时把备份的整张表攥在 `open` 里、按做的次序追加，`save_undo(&[Undo])` 整张写回——先写的「打开前那份 + 新增」会在第二次 `try_update` 时把 `take_dns` 冲掉，用例 `the_backup_is_saved_before_the_step_it_undoes` 抓到的就是这个；(b) 打开失败回退时重新读一次备份（读到的包含这次加进去的），读不到才用打开前的那份并且不重写备份；(c) `recover` 与 `close` 在没有剩下任何撤销动作时一并把 `features.enhanced_mode` 写回 `false`（不这样做，一次崩溃恢复之后这个标记会一直停在 `true`）；(d) `disable` 读不懂备份时记 ERROR、不重写它，其余各步照做；(e) `Opened` 派生 `Clone`（每次后端调用要把它移进 `spawn_blocking`），`Undo::label()` 是文件后端的 `fail_undo` 与日志用的名字（`take_dns`、`adjust`、`rp_filter 1`、`mac_dns <服务>`）；(f) `cli/mod.rs` 写 `pub mod enhanced;`（照同文件其余模块的写法），`wanted` 是 `pub(crate) mod wanted` 且不在 `mod.rs` 里 re-export（为一个还没人用的名字加 re-export 会让 `unused_imports` 变红） | 本提交 |
| 52 | 7 | `opening_goes_in_the_designed_order` 里 `egress` 那一行由假后端记：`routes` 被调用时若出口表的槽已经填着就记一条（只记一次）。管理器自己没有可观察的日志，而这一句要断言的正是「出口表写在路由之前」。只有减的那次 `routes`（关闭时用）记成 `routes off`，好让失败回退的次序读得出来 | 本提交 |
| 53 | 7 | 判据与门禁（一台机器上的一次运行，不是 CI）：`git grep -n "Command::new\|CommandRunner\|SystemRunner" -- crates/rurge/src/cli/enhanced` 什么也没有（退出码 1）；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast` 的日志里 80 个结果行、4379 通过、0 失败、13 忽略，`rurge-engine` 的 `outbounds_http2` 二进制以 `STATUS_HEAP_CORRUPTION` 退出（CLAUDE.md「这台开发机」那种），单独重跑 15 条全过，合起来 **81 个结果行、4394 通过、0 失败、13 忽略**——基线 4363 加上本任务的 31 条（`rurge` 的单元用例 26 条、`tests/cli.rs` 的 `enhanced_mode` 5 条）。`closing_waits_for_the_tun_three_seconds_at_most` 与 `run::enhanced_mode` 那 5 条各单独循环跑 20 次全过 | 本提交 |
| 54 | 7 | 判定与门禁（第二个提交）：自查时发现 `disable` 读不懂备份的那条分支（第 51 行 (d)）没有用例，补上 `a_backup_a_close_cannot_read_is_left_alone`（关掉的其余各步都做、备份原样留下、不做任何 `undo`）；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast` **81 个结果行、4395 通过、0 失败、13 忽略**（第 53 行的 4394 加这一条），这一次没有二进制异常退出。`rurge` 的单元用例 27 条、`tests/cli.rs` 的 `enhanced_mode` 5 条 | 本提交 |
| 55 | 7 | 评审的 C1（Critical）：`configure` 原来写成 `asked && call(…)`，`&&` 短路，`ipv6-vif = disabled`（`wanted.ipv6` 为假）时网卡的地址 / MTU / 链路根本不设——文件后端的 `actions` 里没有 `configure`，任务 8 的端到端用例会红，M4c / M4d 上是一块没有地址的网卡。改成总调 `configure`，`ipv6 = asked && 它的回答`（`mod.rs:407`～`:408`）。用例 `an_open_without_ipv6_still_configures_the_adapter`（`mod.rs:1240`；先红：调用次序里没有 `configure`） | 本提交 |
| 56 | 7 | 评审的 C2（Critical）：`close` 重写的备份原来取 `closing.failed`（做了而失败的），于是打开失败回退时「根本没做」的撤销动作（`on.dns` / `on.adjust` 为假的那一阶段）被写没了：上一次关闭留下的 `RpFilter` / `MacDns` 再也没人还原。改成按位置记「撤销成功的那几条」（`Closing::undone: Vec<usize>` 与 `Closing::remaining`），剩下的 = 失败的 + 没做到的；`features.enhanced_mode = false` 照旧只在什么也不剩时写；`recover` 每一条都会做，也改成同一套（`mod.rs:179`～`:207`、`:258`～`:265`、`:495` / `:531` / `:543`、`:564`）。用例 `an_open_that_fails_leaves_the_undo_actions_it_never_reached`（`mod.rs:1271`；`rp_filter` 与 `mac_dns` 两条，`fail = routes`；先红：备份成了 `None`） | 本提交 |
| 57 | 7 | 评审的 I1（Important）：文件后端的 `configure_ipv6` 先 `step()`，`fail` 命中时已经回了 `Err`，后面那个 `Ok(false)` 分支到不了。改成先读文件、`fail` 是它就回 `Ok(false)`（不写、不记），否则照旧（`file.rs:118`～`:130`）；假后端 `Mock::configure_ipv6` 也一样（任务 9 的 `follow` 要靠它）。用例 `the_ipv6_half_on_an_adapter_that_is_up_can_be_had_or_not`（`file.rs:386`；先红：回了 `configure_ipv6 failed (test)`） | 本提交 |
| 58 | 7 | 评审的两个小项：`wanted.rs:26` 上那句注释从 `wired up by M4a task 9` 改成 `task 8`（任务 8 的启动就调 `enable(wanted)`；第 46 行里抄的那句不改）；`enable` 读不懂备份就回错误、什么也不做的那条分支补上 `an_unreadable_backup_stops_an_open_before_it_starts`（`mod.rs:1311`：备份是 `{"x": 1}` 时回以 `state.json's enhanced backup cannot be read` 开头的错误、后端一个调用也没收到、备份原样、`enabled()` 为假；它第一次跑就是绿的，行为原来就对） | 本提交 |
| 59 | 7 | 判据与门禁（修正第 1 轮）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`task-7-fix-1-gate.log`）**81 个结果行、4399 通过、0 失败、13 忽略**，没有二进制异常退出——第 54 行的 4395 加这一轮的四条（`rurge` 的单元用例 31 条，`tests/cli.rs` 的 `enhanced_mode` 仍 5 条）。这一轮的新用例不涉及时钟，没有循环跑；`a_step_that_fails_undoes_the_steps_before_it_in_reverse` 等现有用例一条期望也没改，仍然全绿 | 本提交 |
| 60 | 8 | 任务第 2、3 条：`EnhancedStatus` 定义在 `crates/rurge-engine/src/control.rs:90`～`:95`，与 `Control` 一样从 `crates/rurge-engine/src/lib.rs:29` re-export（`rurge-api` 只依赖 `rurge-engine`，需要这个名字）；`crates/rurge-api/src/routes/features.rs:33` 的 `enhanced_json` 把 `TunCounters` 的 snake_case 字段映射成 P12 的 camelCase 键与顶层 `stackPanics`，`rurge-tun` 没有 serde | 本提交 |
| 61 | 8 | 任务第 4 条的次序（M4-D16、P25）：`run.rs:1651`～`1659` 的打开在 `Takeover` 与系统代理之前；`run.rs:1663`～`1678` 系统代理打不开、以 1 退出之前先把增强模式关掉（标准输出 `enhanced mode off`）；退出时 `run.rs:1828`～`1838` 在 `drop(following_the_network)` 之后、系统代理之前 | 本提交 |
| 62 | 8 | 协调者的裁定 R20：`RunningTun` 多一个 `ending()`（`crates/rurge/src/cli/enhanced/mod.rs:80`，只等 `TunHandle::join`），关闭仍等 `ended()`（join 再 `tun_ended`，3 秒为限）；生产的 `Starter` 放在新文件 `crates/rurge/src/cli/enhanced/engine.rs`（`starter` `:14`、`EngineTun` `:38`），没有放进 `run.rs` | 本提交 |
| 63 | 8 | 协调者的裁定 R21：`EnhancedMode::new` 多收一个 `TunEnded`（`crates/rurge/src/cli/enhanced/mod.rs:140`，`Arc<dyn Fn(u64)>`）；序号归管理器（`opens` 与 `On::sequence`，每次打开成功加一），`is_current_open`（`:317`）让主循环只处理当前这一次；打开成功后 `watch`（`:358`）用 `tokio::spawn` 等 `ending()`，`DeviceFailed` 时原因只在 DEBUG、序号交给回调；`run.rs:1584`～`1600` 把回调接成 `Command::TunEnded(n)`，队列满时用一个任务把消息送进去（不丢）；`run.rs:1802`～`1812` 收到当前这一次的序号才 ERROR `the virtual adapter failed; the enhanced mode is off` 并 `disable()` | 本提交 |
| 64 | 8 | 协调者的裁定 R22：`run.rs:621` 的 `wanted_now` 给 `ipv6_usable = false`，注释 `// M4a task 9: from the network (P18)`；`general` 与 `external` 取 `engine.runtime().config` 这一代，`primary` 取 `engine.network().primary` 的 `interface`，`explicit_space` 是 `cfg!(target_os = "macos")` | 本提交 |
| 65 | 8 | 计划没写到、执行者的裁定：`ipv6_that_fails_goes_on_without_it` 要一行 WARN，而文件后端只回 `Ok(false)`、管理器原来什么也不说，所以 `open` 在 `asked && !configured` 时 `tracing::warn!("IPv6 could not be configured; the enhanced mode goes on without it")`（`crates/rurge/src/cli/enhanced/mod.rs:437`～`:441`，M4-D17 的「只告警、按没开处理」）；用例断言日志里有 WARN 与这条文字 | 本提交 |
| 66 | 8 | 计划没写到、执行者的裁定：端到端用例要等守护进程迈出第一步，而那时文件后端的 JSON 还不存在，任务 7 的 `actions()` 走 `read_json` 会 panic；改成先 `backend_json`（`crates/rurge/tests/run/enhanced_mode.rs:32`，读不到就是 `Value::Null`）再由 `actions` / `installed` 取字段（`:40` / `:276`），`actions` 的语义不变 | 本提交 |
| 67 | 8 | 计划没写到（只改断言消息）：`crates/rurge-api/tests/api.rs` 的 `features_collections_stop_and_unknown_paths` 里那句 "only system_proxy is live" 改成 "only system_proxy and enhanced_mode are live"；期望值一个也没改 | 本提交 |
| 68 | 8 | 判据与门禁（一台机器上的一次运行，不是 CI）：先红——把启动的打开、主循环的 `EnhancedMode`、`GET` 的 `enhanced_mode` 与 `watch` 各临时去掉（或短路）之后，`rurge` 的单元 1 条红（`task-8-red-unit.log`）、`rurge-api` 2 条红（`task-8-red-api.log`）、`tests/cli.rs` 的 `enhanced_mode` 8 条红（`task-8-red-e2e.log`），随后逐字改回；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`task-8-gate.log`）**81 个结果行、4410 通过、0 失败、13 忽略**——第 59 行的 4399 加本任务的 11 条（`rurge` 的单元 1 条、`rurge-api` 2 条、`tests/cli.rs` 的 `enhanced_mode` 8 条）；没有二进制异常退出。本任务新写的 8 条端到端用例各单独循环跑 20 次，全过 | 本提交 |
| 69 | 8 | 评审的 I1：`--enhanced-mode` 起的端到端用例原来等文件里最后一步是 `adjust` 就断言 `state.json`，而 `features.enhanced_mode` 是 `open` 的第 9 步、在 `adjust` 之后写，有一个几毫秒的窗口会红。改成等启动印出的 `enhanced mode on`（`enable` 整个返回之后才印，文件的最后一步与 `state.json` 都已写完）：`the_enhanced_mode_opens_at_start_and_closes_at_stop`（`tests/run/enhanced_mode.rs:315`，先取 `api on` 那一行再等，免得 `wait_for_line` 把 API 端口那一行吃掉）、`a_killed_run_with_the_enhanced_mode_on_is_undone_by_the_next_start`（`:514`）、`ipv6_vif_always_opens_with_ipv6`（`:552`）、`ipv6_that_fails_goes_on_without_it`（`:581`） | 本提交 |
| 70 | 8 | 评审的 I2：`a_device_that_fails_turns_the_enhanced_mode_off` 原来等文件最后一步是 `undo adjust` 就断言 `GET` 与备份，而 `close()` 在 `undo adjust` 之后还要重写备份、最后才 `shared.set_off()`。改成两样都成立才往下：文件最后一步是 `undo adjust`，且 `GET` 说 `enabled: false`（只等 `GET` 不够：打开完成之前它也是假）；之后再断言动作次序与 `state.json`（`:619`～`:630`） | 本提交 |
| 71 | 8 | 评审的 M1、M2、M3（选 (b)）：`cli/enhanced/{mod.rs,engine.rs}` 里注释中的 `R20` / `R21` 改成指到本表的第 62、63 行（仓库的惯例是执行期修正记录的行号；本表里的「协调者的裁定 R20」等字样不改）；`backend.rs:39`～`:46` 的 `configure` / `configure_ipv6` 说明改成「管理器记那一行 WARN、后端不必再记」（M4c / M4d 照旧说明会记两行；`routes` 的 `Ok(false)` 仍是后端记）；假 TUN 的 `ending()` 改成 `stop()` 之前不答、之后答 `Stopped`（`mod.rs:872`～`:880`，`stop` 用 `Notify`），新增 `a_tun_a_close_stopped_tells_the_loop_nothing`（`mod.rs:1037`：关闭之后等 200 ms 再断言没收到，注释写明等的是「什么都没发生」与余量），原用例最后一句消息改成它真正测的「the one failure was told once」 | 本提交 |
| 72 | 8 | 判据与门禁（修正第 1 轮）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`task-8-fix-1-gate.log`）**81 个结果行、4411 通过、0 失败、13 忽略**——第 68 行的 4410 加 M3(b) 的 1 条单元用例；没有二进制异常退出，没有重跑。I1 的 4 条、I2 的 1 条端到端与 M3(b) 的 1 条单元各单独循环跑 20 次，全过 | 本提交 |
| 73 | 9 | 任务第 1 条：`NetworkChange`（`network.rs:68`）与 `ipv6_usable`（`:81`）；`2000::/3` 直接比首段的高三位（`is_global`，`:92`），没有新加 `rurge → ipnet` 的依赖边（约束允许这个边，但为一个布尔值不值得；工作区的 `ipnet` 也不在 `rurge` 的依赖里，任务写的"`rurge` 已依赖"与代码不符）。`spawn` 的 `wrap` 改成 `fn(NetworkChange) -> C`（`:207`），`tell` 算一次 `ipv6_usable` 一起送（`:323`）；`Command::NetworkChanged(NetworkChange)`；`run()` 在 `network::start` 之后算一次（`run.rs:1637`），每一条 `NetworkChanged` 更新它。执行者的裁定 | 本提交 |
| 74 | 9 | 协调者的裁定 5：`Takeover` 与重载、网络变化这样拿到管理器和 `ipv6_usable`——`reload_and_refresh` 多两个参数（`enhanced: &mut EnhancedMode`、`ipv6_usable: bool`，`run.rs:923`），四个调用者都传；`network_changed` 多两个参数（`enhanced: &mut EnhancedMode`、`ipv6_usable: &mut bool`，`:815`），在 `takeover.follow` 之前更新当前值（裁定 R23），`takeover.follow` 之后以"变化之前开着、之后仍开着"为条件 `enhanced.follow(wanted_now(...), true)`（裁定 R24）；`reload_and_refresh` 里同样在 `takeover.follow` 之后、同样一个条件，`network_changed` 传 `false`。`Takeover::new(system_proxy, enhanced_mode)`（`:661`）多收 `--enhanced-mode`，`begin` 移到打开之前（`:1750`），暂停时两样都不开。新增 `switch_enhanced`（`:788`）照 `switch` 的写法，`Command::EnhancedMode` 改经它（`:1914`） | 本提交 |
| 75 | 9 | 任务第 5 条：`EnhancedMode::follow`（`mod.rs:438`）返回 `()`：失败的步骤各记一行 ERROR（`failed`，`:524`，`step` 与 `error` 两个字段，文字 "cannot follow the change; the enhanced mode goes on with what it has"），增强模式照开、已做的保留、不回滚。`On` 多一个 `ipv6`（`:172`，`open` 里写 `asked && configured`）。`take_the_dns_again`（`:536`）与 `adjust_again`（`:571`）都是先 `saved_undo` → `dns_undo`/`adjust_undo` → `save_undo` → 步骤本身，任何一步失败只记日志、后面的不再做（与打开的备份规矩一致）；IPv6 变了和网络变化同时发生时合成一次 `take_dns`（裁定 R24）。出口表按裁定 R25：`routes` 之前写旧集与新集的并（`union`，`:242`），成功后写新集，失败留并集；`primary` 每次换成新的；`routes` 回 `Err` 时 `On` 记的集合照旧（不知道装上了多少）。`routes` 回 `Ok(false)` 时只去掉 `next.v6`、`ipv6` 与 `on.ipv6` 不变，与 `open` 一致。`backend.rs` 的 `configure_ipv6` / `unconfigure_ipv6` 去掉 `#[allow(dead_code)]`（`cli/enhanced/` 里只剩 `filled` 与 `Opened::index`） | 本提交 |
| 76 | 9 | 任务用例的落点：六条端到端都在 `crates/rurge/tests/run/enhanced_mode.rs`（`:710`、`:748`、`:794`、`:833`、`:881`、`:913`），用的是 `mod the_network` 的 `wifi` / `World` / `wait_for_log` / `OFFICE`：它们改成 `pub(super)`（`cli.rs` 里），没有复制一份；新增的小辅助是 `world_file` / `run_enhanced` / `enhanced_on` / `ask_for_the_enhanced_mode` / `done`（`:665`～`:702`），`routes_of` 拆成 `routes_set` / `strings`（`:258` / `:270`）以便 `RouteSet::diff` 也算出同一份期望。执行者的裁定 | 本提交 |
| 77 | 9 | 任务只列了端到端用例，执行者另加了五条单元用例钉住任务正文里读不到的行为：`ipv6_is_usable_with_a_gateway_and_a_global_address`（`network.rs:948`，任务要的六种情形）、`a_follow_that_changes_nothing_asks_for_no_routes` / `a_follow_that_cannot_install_keeps_the_table_a_superset` / `a_follow_takes_the_dns_over_once_for_ipv6_and_the_network` / `a_follow_that_turns_ipv6_off_gives_the_half_back`（`mod.rs:1773`～`:1879`，裁定 R24、R25 与 IPv6 的加一半、去一半）；出口表在端到端里没有读者，只有单元用例看得见 | 本提交 |
| 78 | 9 | 判据与门禁（一台机器上的一次运行，不是 CI）：先红——六条端到端在改动前全红（`task-9-red-e2e.log`：两条暂停的先把增强模式打开了，四条改配置或改网络之后文件里的动作没变、日志里没有 ERROR），随后实现到绿；`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`task-9-gate.log`）**81 个结果行、4422 通过、0 失败、13 忽略**——第 72 行的 4411 加本任务的 11 条（`tests/cli.rs` 的 `enhanced_mode` 6 条、`rurge` 的单元 5 条）；没有二进制异常退出。六条新写的端到端各单独循环跑 20 次全过（`task-9-loop-*.log`） | 本提交 |
| 79 | 9 | 评审 I1（协调者的裁定 R29）：`EnhancedMode::follow`（`crates/rurge/src/cli/enhanced/mod.rs:447`）里 IPv6 从开到关的次序反过来——先 `take_the_dns_again(&dev, false)`（DNS 交还在网卡还有那个地址时）、再按 `RouteSet::diff` 一次 `routes(add, remove)`、最后 `unconfigure_ipv6`（`:461`、`:521`、`:527`）；从关到开与 IPv6 不变的次序不变（`configure_ipv6` → 路由 → DNS（IPv6 变了或网络变化）→ `adjust_again`（网络变化））；一次 `follow` 至多一次 `take_dns` 照旧（从开到关又碰上网络变化时，DNS 只在第一步做）；每一步失败照旧记 ERROR、后面的照做。这一行覆盖计划任务 9 第 5 条"从开到关：`unconfigure_ipv6`，减 IPv6 的路由，`take_dns(false)`"那半句的次序。`follow` 的说明与注释写两个方向的次序与理由（设计第 9.4、9.7 节）。用例跟着改：单元 `a_follow_that_turns_ipv6_off_gives_the_half_back`（`:1906`，`dns_undo`、`take_dns`、`routes off`、`unconfigure_ipv6`）与端到端 `a_reload_turns_ipv6_on_and_off`（`crates/rurge/tests/run/enhanced_mode.rs:838`，`take_dns`、`routes +0 -n`、`unconfigure_ipv6`） | 本提交 |
| 80 | 9 | 评审 I2：`a_post_during_the_pause_is_noted_and_done_when_it_ends`（`crates/rurge/tests/run/enhanced_mode.rs:748`）每次换网络之后用 `wait_for_log` 读掉那一行（第一次回 `Home` 读 `takeover resumed`，`:770`；回 `Office` 读 `takeover paused on this network`，`:778`），最后那一次等到的才是守护进程说的第 4 行。照评审的办法验证过它会红：临时让 `switch_enhanced` 在 `enabled == false` 时不记下想要，用例在 `it is left off, as asked during the pause` 这一句红（`task-9-fix-1-i2-red.log`），随后逐字改回（`git diff` 里 `run.rs` 一个字节不剩） | 本提交 |
| 81 | 9 | 评审 I3：`ipv6_auto_follows_the_network`（`crates/rurge/tests/run/enhanced_mode.rs:886`）改为等这一段 `follow` 的整段动作（先记下动作条数 `before`，`wait_until` 等 `actions(&file).get(before..)` 恰好等于 `["configure_ipv6", "routes +n -0", "take_dns", "adjust"]`，去掉 IPv6 那段用 I1 的新次序 `["take_dns", "routes +0 -n", "unconfigure_ipv6", "adjust"]`，`before` 在 `:904` / `:921`），等到了再读路由、`ipv6` 与计数；文件后端每步重写文件、不是原子的，等到之后才读。顺带在网络上钉住一次 `follow` 只有一次 `take_dns`（`take_dns` 依次是 1、2、3）。另三条重载的用例不受影响（`POST /v1/profiles/reload` 的回复在 `reload_and_refresh` 含 `follow` 之后才发），两条暂停的用例也不受影响（`GET` 变真、变假是打开、关闭的最后一步） | 本提交 |
| 82 | 9 | 评审 M1、M2：`crates/rurge/src/cli/enhanced/mod.rs` 里本任务加的六处 `R24` / `R25`（`union` 的说明 `:240`、`follow` 的说明、`follow` 里 DNS 那一行、三条单元用例的说明）改成 "the phase 3 / M4a plan's execution record row 75"（照第 71 行的写法）。第 73 行说的"任务写的'`rurge` 已依赖'与代码不符"订正一句：那句出自协调者的派工，不在计划的任务正文里；第 73 行不改 | 本提交 |
| 83 | 9 | 门禁（修正第 1 轮，一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`task-9-fix-1-gate.log`）**81 个结果行、4422 通过、0 失败、13 忽略**——与第 78 行相同（本轮没有加删用例，只改了次序与三处用例的等法）；没有二进制异常退出，没有重跑。改过的三条端到端各单独循环跑 20 次全过（`task-9-fix-1-loop-summary.log`：60 行 `ok. 1 passed`，0 次失败） | 本提交 |
| 84 | 10 | 任务第 1 条（`docs/api/phase3.md`）：文末加了 `## M4　增强模式（平台接入）` 一节，九个小节——这一节与"这一版开不了"、启动参数与环境变量、`GET` / `POST`、日志行、`state.json` 的 `enhanced_backup` 与恢复、接管暂停的增强模式一半、重载与网络变化、`rurge check` 的四个新码与 `external` 的 `addresses`、只供测试的后端、没有覆盖的。开工时核出任务正文有两处要照代码改：一、"`W0035` ～ `W0038` 的消息与建议从 `crates/rurge-config/src/diagnostic.rs` 逐字取"只对建议成立——建议在 `diagnostic.rs` 的 `codes::hint`（`WITH_DEFAULT`），消息在 `general.rs`（`W0035` ～ `W0037`）与 `config.rs`（`W0038`）；二、`W0036` / `W0037` 消息里的占位符照代码写作 `{range}` / `{item}`（P20 写的是 `{范围}` / `{项}`）。这一节同时订正了本页「接管暂停时的系统代理端点」末尾"增强模式的那一半随 M4，今天没有"那句（M4a 有了，今天只经文件后端） | 本提交 |
| 85 | 10 | 任务第 2 条（`docs/testing.md`）：三处。一、「只供测试的后端」里三个后端之后加了 `RURGE_ENHANCED_BACKEND` 一段（`file:<路径>` 的文件形状与 `actions`、动作名逐个照 `file.rs`、失败的动作不记、`fail` / `fail_undo` / `device_fails_after_ms`、`configure_ipv6` 回"没有"不是失败、`off` 与那句错误、未知取值以 2 退出并先恢复系统代理、不设时是平台后端、`external` 到不了、文件不是原子写）；二、「命令」加五行（`route::` 41 个、`tests::dispatch::` 与 `queue_bytes` 等 5 个、引擎 `--test tun` 的 4 个、`cli::enhanced` 37 个、`run::enhanced_mode` 19 个，都是这个提交里跑出来的数）；三、「测试约定（全文）」末尾加「阶段 3 / M4a 起：增强模式的测试后端」五条（辅助函数缺省 `off`、文件后端自己设、文件不是原子写所以等 `GET` 或等一整段动作、`0600` 的用例只在 CI 上编译、这个 crate 里没有 `LogCapture` 所以 3 秒那一行 WARN 没有用例钉住） | 本提交 |
| 86 | 10 | 任务第 3 条（`CLAUDE.md`）：只改两处——「测试约定」里 `rurge_command()` / `rurge_process()` 那一条的括号里加"设 `RURGE_ENHANCED_BACKEND=off`"，「常用命令」下面列只供测试的后端那一句加 `RURGE_ENHANCED_BACKEND`；`docs/testing.md` 的同一段同时有了（"改一条规矩，两处一起改"）。「现在在哪里」没有改（协调者的裁定 R28） | 本提交 |
| 87 | 10 | 任务第 4 条（`docs/status.md`）：阶段 3 下加了「M4a（平台无关的骨架）」，开头一句说它订正上一节"代码一行也没有写"那句；"做了什么"一句、M3 延后事项各做在哪个任务（M3a 延后第 17 行的第二 ～ 四条 → 任务 1；M3b 延后第 8 行与第 14 行的一条 → 任务 7 与任务 2；M3b 延后第 23 ～ 25 行 → 任务 2）、"没做的"六条、"必须知道的事"八条、去向写着项目所有者的四项。附录末尾加 M4a 的两条依赖边（`rurge-platform → ipnet`、`rurge` → `rurge-tun`）与"计划里设想过的 `rurge → ipnet` 没有加"，`Cargo.lock` 仍是 518 个包（数 `name = ` 开头的行） | 本提交 |
| 88 | 10 | 任务第 5 条（`docs/surge-compatibility-matrix.md`）：`[General]` 的 `ipv6-vif`、`tun-excluded-routes`、`tun-included-routes` 三行与 `external` 一行里 `addresses` 那句各加了"**目前（阶段 3 / M4a）**"（解析、`rurge check` 的告警、路由集已有，平台的执行随 M4c / M4d；`addresses` 另说它进了路由集的计算、原来的 `W0029` 去掉了），开头"最近一次"改成 2026-10-26 / 阶段 3 / M4a。清单里没有诊断码的表，所以四个码写在 `docs/api/phase3.md` 的「M4」一节，这里说明一句 | 本提交 |
| 89 | 10 | 任务的判据（`git grep` 的核对）：文档里引用的用例名、文件与函数 95 个都对着代码核过（唯一没有命中的是我自己写错、没有进文档的一个名字）；引用的英文日志行、错误文本与四个码的消息、建议 45 条逐字对着代码核过，全部存在；四个码另经 `rurge check` 端到端看到（`W0036` 的 `{item}` 对 `10.1.2.3/8` 显示成 `10.1.2.3/8`，主机位照留）；三处负面的也核过——`DefaultHasher` 在 `crates/rurge-tun` 里没有了、`inert.push("addresses")` 没有了、`crates/rurge/Cargo.toml` 里没有 `ipnet`。清单与结果在 `task-10-report.md` | 本提交 |
| 90 | 10 | 协调者的裁定 R27：前面各行「提交」一栏里写"本提交"的，按任务分组（取法 `git blame -s` 对着这张表）：第 1 ～ 11 行（任务 0）是 `64c2470`；第 12 ～ 19 行（任务 1）是 `d08831d`；第 20 ～ 22 行（任务 2）是 `7df7113`（第 22 行后来被 `d7e845c` 挪到第 23 行之前，`git blame` 因此把它算在 `d7e845c` 上）；第 23 ～ 25 行（任务 3）是 `d326270`；第 26 ～ 31 行（任务 4）是 `77eed1a`；第 32 ～ 37 行（任务 5）是 `61af9ee`；第 38 ～ 44 行（任务 6）是 `45b7032`；第 45 ～ 53 行与第 55 ～ 59 行（任务 7 的两个提交与修正第 1 轮）是 `ed3c71a` 与 `9f50e01`，第 54 行是 `fd1f269`（任务 7 的第二个提交）；第 60 ～ 68 行（任务 8）是 `afe3ff5`，第 69 ～ 72 行（任务 8 的修正第 1 轮）是 `b8400b0`；第 73 ～ 78 行（任务 9）是 `edaf896`，第 79 ～ 83 行（任务 9 的修正第 1 轮）是 `2bfa78f`。本行起的各行都是这一行所在的提交 | 本提交 |
| 91 | 10 | 订正第 19 行：那一行末尾"没有重跑任何二进制"只对任务 1 那一次全量成立。任务 1 全量跑了四遍：第 2 遍的 `rurge-policy` 与第 3 遍的 `rurge-inbound` 各以 `STATUS_HEAP_CORRUPTION` 退出（本机的终端管控软件注入的钩子，`CLAUDE.md`「这台开发机」；两个 crate 与本任务的改动无关），都单独重跑过、通过（任务 1 的报告「门禁」一节）。第 19 行不改，以这一行为准 | 本提交 |
| 92 | 10 | 订正第 40 行：那一行说 `W0036` / `W0037` 消息里的 `{项}` 是"主机位清零后的写法"，不对。`parse_nets`（`crates/rurge-config/src/general.rs:701`）用 `item.parse::<IpNet>()`；`ipnet` 2.12.1 的 `FromStr` 走到 `parser.rs` 的 `read_ipv4_net` / `read_ipv6_net`，它们用 `Ipv4Net::new` / `Ipv6Net::new`，而这两个 `new`（`ipnet.rs`）只查前缀长度、把地址原样存下，不截断主机位；`Display`（`impl fmt::Display for Ipv4Net`）照存的地址写。所以 `{项}` 是**解析之后再写出来的原值**：写法规范化（IPv6 写成压缩形式），主机位照留，`10.1.2.3/8` 就显示成 `10.1.2.3/8`（`rurge check` 上看到过）。第 40 行不改，以这一行为准 | 本提交 |
| 93 | 10 | 照实记一笔：`64c2470`（任务 0 的提交）的提交说明里混进了 ESC 与 BEL 两个控制字符——Pi 的 shell 把说明里的反引号当成了转义符，`e` 与 `a` 变成了控制字符。本地历史不改写（协调者的裁定 R8）；此后各提交的说明都先写进文件再 `git commit -F` | 本提交 |
| 94 | 10 | 门禁（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（`task-10-gate.log`）**81 个结果行、4422 通过、0 失败、13 忽略**——与第 78、83 行相同（本任务只改文档，没有加删用例）；没有二进制异常退出，没有重跑 | 本提交 |

| 95 | 终审 | 终审的 I1 与存疑 S2（协调者的裁定 R33）：`routes` 回 `Ok(false)` 时 `follow` 不再把 `next.v6` 清空，而是记下 `old.v6` 减去 `remove.v6`（也就是旧集与新的交集，后端只撤掉它新加的那一半、照常减掉要减的），出口表换成这一份（仍是网卡里路由的超集）；这一次调用之前 IPv6 没开着的（`open`，或 `follow` 里 IPv6 从关到开）按 IPv6 没开往下做（`ipv6` 与 `on.ipv6` 为假、DNS 以 `false` 接管、`open` 的 INFO `enhanced mode on` 的 `ipv6` 为假），本来就开着的照开着、不多接管 DNS；`open` 里 `old` 为空，仍是不清。这一行覆盖第 75 行里「`routes` 回 `Ok(false)` 时只去掉 `next.v6`……与 `open` 一致」那半句；理由是设计 M4-D17 与 M4c、M4d 两份计划写的后端做法。`Mock` 加一个让 `routes` 回 `Ok(false)` 的开关（照真后端：只在 `add.v6` 非空时，装进去的不含它），三条单元用例（IPv6 前后都开、从关到开、打开）先红；`backend.rs` 的 `routes` 说明、`mod.rs` 第 5 步与 `follow` 的说明、`docs/api/phase3.md` 的「重载与网络变化」照改 | 本提交 |
| 96 | 终审 | 终审的 I2（协调者的裁定 R34）：Windows 的 IPv4 与 IPv6 两张 DNS 名单分开存，`take_dns(index, false)` 原来只换 IPv4 那一份。改成在 IPv4 那一份脚本之后、`ipconfig /flushdns` 之前另加一步：一份只含 `interface ipv6 set dnsservers name={index} source=static address=none register=none validate=no` 的 `netsh -f` 脚本，`OnFailure::Ignore`（第一次打开时没有可清的；没绑 IPv6 的网卡可能不认这一行，那时本来也没有它的名单）。依据微软 netsh-interface 页的 `set dnsservers`：`address` 是 IP 地址或 `none`（清空名单）；**没有在真机上跑过**。`windows.rs` 的逐字用例跟着改，另加三步次序的 `turning_ipv6_off_clears_the_ipv6_dns_list`（先红：只有两步） | 本提交 |
| 97 | 终审 | 终审的 I3 与 M1（协调者的裁定 R35）：`switch_enhanced` 关的那一支不论 `disable()` 成败都记下 `enhanced_wanted = false`，再把错误交回（API 照旧 500 带原因）——`disable` 先 `self.on.take()`、从不停在半路，回 `Err` 只表示某条撤销没成，模式已经关了；不记下的话，进出一次暂停会违背使用者最后的请求重新打开。三句日志改成两句照实的说法（逐字：`the enhanced mode is off for the pause, but part of it could not be undone; what outlives rurge stays in state.json and is undone on the next start`，与 `the enhanced mode is off after the adapter failed, but part of it could not be undone; what outlives rurge stays in state.json and is undone on the next start`），级别仍是 ERROR；`Takeover::follow` 去掉增强模式的 `(false, true)` 那一支（只对系统代理成立，说明写清为什么），`follow` 与 `enhanced_wanted` 的说明照改。`crates/rurge-api/tests/api.rs` 的用例改成「打开被拒之后仍关着」；新端到端 `a_close_that_fails_is_still_what_the_user_asked_for`（先红：离开暂停时又打开了，`open` 是 2）。`docs/api/phase3.md` 的「日志行」「接管暂停：增强模式的那一半」与 `POST` 一节照改 | 本提交 |
| 98 | 终审 | 终审的 I4（协调者的裁定 R36）：撤销失败的 ERROR 行的 `step` 不再带 macOS 的网络服务名（它可以是使用者改的任何文字，设计第 13 节说这类名字只在 DEBUG）。`Undo` 多一个只用于日志的 `kind()`：`Test` 是它的步骤名、`MacDns` 是 `mac_dns`、`RpFilter` 是 `rp_filter <值>`；`recover` 与关闭那两处 ERROR 用它，紧挨着另记一行 DEBUG 带完整的 `label()`（含服务名）。`label()` 不变，仍是文件后端的动作名与 `fail_undo` 的键；`undo.rs` 加 `the_logs_name_of_an_undo_has_no_service_name_in_it`（先红：`mac_dns Wi-Fi`）。这一行覆盖第 51 行 (e) 里「`Undo::label()` 是……日志用的名字」那半句 | 本提交 |
| 99 | 终审 | 终审的 M2（协调者的裁定 R37）：`EnhancedMode::recover` 在没有 `enhanced_backup` 而 `features.enhanced_mode` 为真时（Windows 的备份总是空的，Linux 不用改 `rp_filter` 时也是），用 `update` 把它写回 `false`（写不进只记 ERROR，与别处一样），仍回 `None`；三个没有日志的出口照旧什么也不印。单元用例 `a_crash_that_left_no_undo_action_puts_the_flag_back`（先红：标记仍是 `true`）。补第 51 行 (c) 没覆盖到的情形，`docs/api/phase3.md` 的 `state.json` 一节补一句 | 本提交 |
| 100 | 终审 | 终审的存疑 S1（协调者的裁定 R38）：`rurge-tun` 的 `TunHandle` 多一个公开的 `ending()`——等停止令牌（`Life::stopped`，停止与设备坏了都会取消它）再读第一个结局，**不等 TUN 的任务**；`cli/enhanced/engine.rs` 的 `EngineTun::ending()` 改用它，`RunningTun` 的 `ending` 与 `EnhancedMode::watch` 的说明改成不再等 `join`（关闭那一侧照旧 `ended()`：`join` 加 `tun_ended`，3 秒时限）。用例 `ending_answers_before_a_dial_under_way_comes_back`（`crates/rurge-tun/src/tests/outcomes/device_fails.rs`；先红：变成等 `join` 之后超时）：拨号停在半路、设备坏掉，`ending()` 在 5 秒内回 `DeviceFailed`，300 毫秒的固定等待里 `join()` 还没回，放开拨号之后 `join()` 回来。这一行补上 M3b 延后第 8 行在设备坏了这一处的界；`docs/testing.md`、`docs/status.md` 与 `docs/api/phase3.md` 的说明照改 | 本提交 |
| 101 | 终审 | 终审的五个小项：`tests/dispatch.rs` 的说明 `256^-63` 改成 `256^-64`；`tcp/shard.rs` 的 `.expect` 文字改成 `the shard's rebuild warnings`（这个 crate 的 `expect` 写被锁住的那样东西的名字）；`Engine::tun_ended` 的说明补一句「它会拿一下那个 TUN 的位置（许可），那一瞬间同时来的 `start_tun` 会被当成已经在运行拒绝，调用方在它返回之后再起下一个」；`route/set.rs` 的 `Ipv6Net::aggregate` 旁加注释（`u128::MAX` 处与 IPv4 同样的边界，只有止于 `ffff:…:fffe` 的 `tun-included-routes` 前缀才碰上，没处理，指延后第 1 行）；`follow` 说明里那句 DNS 等待的话重写（先去掉地址的话，从那时到交还 DNS 为止，每一个查询都要等超时，而真实系统上中间每一步都是要花些时间的程序） | 本提交 |
| 102 | 终审 | 任务 10 的评审（协调者的裁定 R30）：`docs/api/phase1.md` 第 42、43 行的端点表对 `enhanced_mode` 已经不对——加注说阶段 3 / M4a 起 `GET` 也是真实状态并另带 `dropped` 与 `stackPanics`，`POST` 同 `system_proxy`（成功 `{}`、失败 500 带原因），其余功能仍是 501。这一行覆盖延后第 11 行（它把这件事推给 M4d；`phase1.md` 是使用者查端点的地方，历来由后面的里程碑加注） | 本提交 |
| 103 | 终审 | 任务 10 的评审（协调者的裁定 R30）：`docs/api/phase3.md` 两处说 `GET` 的形状「阶段 1 就写着」不对——形状是 M3 设计定的（M3-D22，它的第 14 节），M4 设计第 3 节照用；阶段 1 到 M3 这个端点 `GET` 恒为 `{"enabled":false}`、`POST` 回 501。另：`docs/api/phase3.md` 的「没有覆盖的」与 `docs/status.md` 的「M4a」原来都说手工清单的 M4 一节像已经有了，改成「清单里还没有 M4 一节（只有 M1、M2、M3），M4d 写它，要验的东西列在设计第 16 节」；计划文件末尾补上换行 | 本提交 |
| 104 | 终审 | 协调者的裁定 R26：M4a 做完任务 10 不停下等所有者评审，由协调者做整个分支的终审与一轮修正、在本地把 `main` 快进到这个分支（不推送），接着做 M4b；它覆盖计划开头「执行方式」里的停点与 P23 的「做完任务 10 停下」，也覆盖 `CLAUDE.md` 原来的「每份做完停下交项目所有者」；依据是项目所有者 2026-10-26 的指示（「你来开始调用 Pi 来做每个步骤，然后来评审每个步骤的结果」）。`CLAUDE.md`「现在在哪里」的第三条换成新的一段（指向本行） | 本提交 |
| 105 | 终审 | 门禁与循环（一台机器上的一次运行，不是 CI）：`cargo fmt --all --check` 与 `cargo clippy --all-targets -- -D warnings` 通过；`cargo test -j 8 --workspace --no-fail-fast`（工作区的 `final-fix-gate.log`）**80 个结果行、4346 通过、0 失败、12 忽略**，`rurge-proto-wireguard` 的 lib 二进制以 `STATUS_HEAP_CORRUPTION` 退出（`CLAUDE.md`「这台开发机」说的那种），单独重跑（`final-fix-rerun-lib.log`）84 通过、1 忽略，合起来 **81 个结果行、4430 通过、0 失败、13 忽略**——第 94 行的 4422 加这一轮的 8 条（F1 三条、F2 一条、F3 一条端到端、F4 一条、F5 一条、F6 一条）。F3 的端到端与 F6 的 `rurge-tun` 用例各单独循环跑 20 次全过（`final-fix-loop-summary.log`：40 行 `ok. 1 passed`，0 次 `FAILED`） | 本提交 |

## 延后事项

| # | 事项 | 去向 |
| - | ---- | ---- |
| 1 | IPv6 的 `aggregate` 有与 IPv4 同样的毛病：`ipnet` 2.12.1 对 `u128::MAX` 不减一。只有 `tun-included-routes` 里有一段止于 `ffff:…:fffe` 的前缀时才碰上，那是组播段，基础前缀总是减去 `ff00::/8`，没有处理 | 有用户报告再说 |
| 2 | 什么也不排除时 IPv4 有 55 条路由：`255.255.255.255/32` 这个洞把 `240.0.0.0/4` 拆成 28 条。macOS 上一条路由一个 `route` 进程，打开时要多久没有量过 | M4c 与手工验收（真机上量一次）；路由集的算法是设计第 4.3 节的决定 |
| 3 | `crates/rurge-platform/src/route/{linux,macos,windows}.rs` 有三处说明不够准：`linux::remove_routes` 说自己是"打开半路失败后的撤销"，其实重载与网络变化减路由也用它；`windows` 模块的说明说"每一步都是一份 `netsh -f` 脚本"，`take_dns` 还有 `ipconfig` 一步；`macos::set_dns` 说"没有服务器时放回原来的"，只在原来没有手设时才对 | M4c 任务 2（Linux、macOS）与 M4d 任务 2（Windows），改写这几个函数时一起改 |
| 4 | `crates/rurge/src/cli/enhanced/backend.rs` 的 `Platform` 与 `Off` 两个后端几乎一样（每个方法都回一句固定的错误） | M4c / M4d 把 `Platform` 换成真的 |
| 5 | 关闭等 TUN 超过 3 秒那一行 WARN 没有用例钉住：可执行文件这个 crate 里没有 `LogCapture`（协调者的裁定 R15） | 下次动到关闭时 |
| 6 | 设备自己坏了的情形（P24）要补进设计第 9.4 节的订正 | M4d 收尾时写进设计的订正（计划 P24 已写） |
| 7 | `follow` 里 `unconfigure_ipv6` 失败之后，下一次打开 IPv6 的 `configure_ipv6` 会遇到地址还在的网卡 | M4c / M4d 的平台后端让 `configure_ipv6` 遇到已有的地址时不失败（Linux 用 `ip addr replace`，Windows 先删再加或认"已存在"），用例照真实的错误文本写 |
| 8 | `#[cfg(unix)]` 的 `file::write_temporary` 与 `the_pool_file_is_its_owners_alone` 在这台 Windows 开发机上没有编译过（第 22 行） | 项目所有者推送之后 CI 的 Linux / macOS 运行 |
| 9 | `queue_bytes` 计的是 `packet.capacity()`（P9 的口径），每个在队列里等的包还多占一个 `Ingress`（`Vec` 之外的 16 字节：`Arc` 与 `usize`），所以队列真实占用是"预算 + 16 字节 × 包数"；要不要把这 16 字节也计进预算归项目所有者（任务 1 的报告「顾虑」第 3 条） | 项目所有者 |
| 10 | `crates/rurge/src/cli/enhanced/mod.rs` 的 `filled`（M4b 的读者）与 `Opened::index`（M4d 的平台后端）仍带 `#[allow(dead_code)]`，因为它们在这一版没有用户 | M4b（`filled`）与 M4d（`index`）接上线时去掉标记与说明里的 "wired up by …" |
| 11 | `docs/api/phase1.md` 的端点表仍说 `GET /v1/features/{…}` 里 `system_proxy` 之外恒为 `{"enabled":false}`，而 `enhanced_mode` 自 M4a 起是活的（形状见 `docs/api/phase3.md` 的「M4」一节） | M4d 的文档收尾（设计第 16 节那一批文档改动里加一条） |
| 12 | `rurge check` 的 `W0036` / `W0037` 只在 Profile 解析这一层对着项判，没有对着路由集的结果判（例如两段私网被合并成一条、或 `external` 的地址与排除项重叠的情形都不另报） | 有用户报告再说 |
| 13 | `state.json` 的写入（`crates/rurge-engine/src/state.rs` 的写文件那一段，先前就有）不 `fsync`。备份现在承担「崩溃之后把 macOS 的 DNS 写回去」的职责；掉电时若改名落了盘而内容没有，下一次启动读到空文件、当作坏文件改名、用缺省值，备份就没了。设计说的「崩溃」指进程崩溃，掉电要不要管 | 项目所有者 |
| 14 | 后端减路由用 `Ignore`：减不掉的路由留在网卡里，管理器却按成功记下新集，出口表随之不再有它（与第 95 行是同一个不变量）。去向：M4c / M4d 写后端时定——减不掉时回 `Err`（管理器就留着并集），或别的办法 | M4c / M4d 写后端时 |
| 15 | `crates/rurge-dns/src/fakeip/file.rs` 的 `write_temporary`（`#[cfg(unix)]`）在 `exists()` 与 `open` 之间有空档：那之间别人建出的文件保留它自己的权限；打开之后对文件本身 `set_permissions` 更直接，也省掉 `exists()`。这台开发机编译不到 `cfg(unix)` 的代码，改了也验证不了 | 下次在能编译 Unix 的地方动这个文件时 |
