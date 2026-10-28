# 阶段 3 需求逐句对照代码

## 怎么读

### 六列（M6 设计第 4.2 节）

| 列 | 内容 |
| --- | --- |
| 需求 | 需求编号；M6 设计第 3.2 节的两张表写 `第 7 节`，M6 设计第 3.3 节的表写原来的编号与出处 |
| 这一句 | 同阶段 1 |
| 结论 | M6 设计第 4.4 节的八种之一 |
| 代码 | 真正**作用于**这件事的地方，`路径::函数`；只有解析器接受它而没有东西用它的，不算"做了" |
| 用例 | 路径与用例名；读过它的断言，断言的是这一句说的行为，不是"跑通了"；只在某个平台上编译的，写明平台 |
| 真机上 | M6 设计第 4.3 节 |

### "真机上"一列（M6-D3）

阶段 3 的大多数句子，内存设备上的用例证明了逻辑，真的网卡、路由、系统 DNS 与真实应用的反应要在真机上看——这正是 VCRM 里"演示"与"测试"分开记的理由。这一格写：

- 手工清单的编号（`W19`、`M17`、`L18`、`P29` 一类），一个平台一个或几个；
- L2 特权用例的名字（`tests/` 下环境变量门控的那一份），注明"没有运行过"；
- `—`：L1 的用例已经证明了这一句的全部，不需要真机（例如解析、诊断、纯函数）；
- `看不出`：要真机才证得了，但在真机上也没有一项手工操作看得出它（例如 `udp-priority` 只在高负载时让 UDP 先写，M5 照实没有设手工项）；括号里一句话说为什么（M6-D21）。

要真机而手工清单里没有对应项、又不是"看不出"的，这一格写"缺"，任务 6 补项（M6 设计第 7 节）。

### 八种结论（M6-D4）

阶段 1 的七种照用（把"不属于阶段 1"换成"不属于阶段 3"），定义一字不改：`做了`、`做了，补了用例`、`补上了`、`没有做完（已登记）`、`有意不同（已登记）`、`不属于阶段 3`、`需求文字有问题`。另加一种：

- `做了，本机没有能红的用例`：代码在，作用于这一句，但没有一条缺省的用例会因这一句不成立而红，在这台开发机上也补不出来。有两种（M6-D21）：
  - 它只在 Linux 或 macOS 上编译（`cfg(target_os = "linux")`、`cfg(target_os = "macos")`、`cfg(unix)`），本机编译不到；
  - 它在本机编译，但只在有特权、真设备或签名的 DLL 时才运行——直接调操作系统的 FFI（例如 Wintun 的建网卡、收发包）、真的执行系统命令的那一层——缺省的用例照规矩从不走到它。

  代码一格写明那段代码与原因（它的 `cfg`，或"要特权 / 真设备 / DLL"），用例一格写"—"或断言不到这一句的那几条用例，L2 的用例写在「真机上」一格；这些句子另列一节「本机没有能红的用例的」，给以后能在那个平台上、或以特权跑的助手（或所有者）补。

**逻辑钉住了、系统的效果没有钉住的，记 `做了`**（阶段 1 的先例：FR-IN-04 系统代理三个平台的几行）：L1 的用例把作用于这一句的逻辑一直钉到系统的边界——假执行器收到的逐字命令、路由集、恢复的计划——改坏那段逻辑会有用例红，就记 `做了`；系统真的照做没有，是"真机上"一格的事。只有作用于这一句的代码整段都没有缺省用例走到时，才用 `做了，本机没有能红的用例`。

已经有一条只在 Linux / macOS 上编译、会因这一句而红的用例的，记 `做了`，用例一格写"只在 Linux 上编译；CI 运行 40611702392 通过"（那次运行之后加的用例写"CI 没有见过"）。

（其余由任务 7 写）

## 结论

（任务 7 写）

## 第一部分：需求逐句对照代码

### 入站（FR-IN）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-IN-06 | 「增强模式：三平台 TUN 驱动」之 Windows（Wintun） | 待读（任务 4） | | | |
| FR-IN-06 | 「增强模式：三平台 TUN 驱动」之 Linux（`/dev/net/tun`） | 待读（任务 4） | | | |
| FR-IN-06 | 「增强模式：三平台 TUN 驱动」之 macOS（`utun`） | 待读（任务 4） | | | |
| FR-IN-06 | 「注册默认路由」 | 待读（任务 4） | | | |
| FR-IN-06 | 「`tun-included-routes`」 | 待读（任务 4） | | | |
| FR-IN-06 | 「`tun-excluded-routes`」 | 待读（任务 4） | | | |
| FR-IN-06 | 「`ipv6-vif`」 | 待读（任务 4） | | | |
| FR-IN-06 | 「ICMP 转发（…回显请求经本机发往真实目的地址、应答由虚拟网卡交回，不在本机伪造…）」之虚拟网卡内核里的转发 | 待读（任务 3） | | | |
| FR-IN-06 | 「ICMP 转发（…不在本机伪造…）」之本机的 ICMP 套接字（真实的 `Pinger`） | 待读（任务 4） | | | |
| FR-IN-06 | 「ICMP 转发（…）与 `icmp-forwarding`」 | 待读（任务 3） | | | |
| FR-IN-06 | 「TCP…通过用户态协议栈进入流水线」 | 待读（任务 3） | | | |
| FR-IN-06 | 「…UDP 通过用户态协议栈进入流水线」 | 待读（任务 3） | | | |
| FR-IN-06 | 「权限不足时给出明确提示」 | 待读（任务 4） | | | |
| FR-IN-06 | 「退出时恢复路由」 | 待读（任务 4） | | | |
| FR-IN-07 | 「进程识别：本机连接的进程路径与名称」 | 待读（任务 2） | | | |
| FR-IN-07 | 「Windows 通过 TCP / UDP 连接表与进程映像路径」 | 待读（任务 2） | | | |
| FR-IN-07 | 「Linux 通过 `/proc/net` 与 inode」 | 待读（任务 2） | | | |
| FR-IN-07 | 「macOS 通过 `sysctl` 的 PCB 表与 `proc_pidpath`」 | 待读（任务 2） | | | |
| FR-IN-07 | 「结果供 `PROCESS-NAME` 规则…使用」 | 待读（任务 2） | | | |
| FR-IN-07 | 「…与 `$request.processPath` 使用」 | 待读（任务 2） | | | |
| FR-IN-08 | 「UDP 会话表：NAT 映射」 | 待读（任务 3） | | | |
| FR-IN-08 | 「超时回收」 | 待读（任务 3） | | | |
| FR-IN-08 | 「每会话流量统计」 | 待读（任务 3） | | | |
| FR-IN-08 | 「`udp-priority` 尽力实现」 | 待读（任务 5） | | | |
| FR-IN-09 | 「网络环境探测：当前 SSID」 | 做了 | `crates/rurge-platform/src/netenv/windows.rs::wifi`（`netsh wlan show interfaces` 里键恰为 `SSID` 的那一行；没有位置权限时没有这一行，是"不知道"）、`crates/rurge-platform/src/netenv/linux.rs::wifi`（`nmcli`，没有它时 `iw dev <网卡> link`）、`crates/rurge-platform/src/netenv/macos.rs::wifi`（`ipconfig getsummary`，`<redacted>` 当作不知道），三种文本各有一个纯函数解析器（`crates/rurge-platform/src/netenv/parse.rs` 里的 `netsh_wlan`、`nmcli_wifi`、`iw_link`、`ipconfig_summary`）；名字进 `Snapshot.wifi.ssid`，`crates/rurge/src/cli/network.rs::facts_of` 换成 `NetworkFacts`，`SSID:` 条件与 `[SSID Setting]` 的表达式由 `crates/rurge-config/src/policy.rs::SubnetExpr::matches` 比它 | `crates/rurge-platform/src/netenv/parse.rs`：`netsh_reads_the_ssid_and_bssid_of_windows_10`（`SSID : ExampleNet` 读成 `Some("ExampleNet")`）、`netsh_ssid_may_hold_colons_spaces_and_non_ascii_letters`、`nmcli_reads_the_access_point_that_is_active`、`ipconfig_summary_reads_the_ssid_and_pads_the_bssid`；`crates/rurge-platform/src/netenv/mod.rs`：`each_platform_is_read_by_its_own_reader`（三个平台的读取器各给一台假机器，`wifi.ssid` 都是 `Some("Example Net")`）；`crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（`SSID:My Home` 命中、`SSID:my home` 不命中、通配） | W1、W2、W3、W4、W5、M1、M3、L1、L2、L4、P7、P8 |
| FR-IN-09 | 「BSSID」 | 做了 | `crates/rurge-platform/src/netenv/windows.rs::wifi`（`BSSID` 或 Windows 11 的 `AP BSSID`）、`crates/rurge-platform/src/netenv/linux.rs::wifi`、`crates/rurge-platform/src/netenv/macos.rs::wifi`；解析在 `crates/rurge-platform/src/netenv/parse.rs`（`netsh_wlan`、`nmcli_wifi` 把简洁格式里的 `\:` 还原、`ipconfig_summary` 给八位组补前导 0，M1 记录第 18 行）；`crates/rurge-config/src/policy.rs::SubnetExpr::matches` 的 `BSSID:` 不区分大小写、认通配 | `crates/rurge-platform/src/netenv/parse.rs`：`netsh_reads_the_ap_bssid_of_windows_11`、`netsh_reads_the_ssid_and_bssid_of_windows_10`、`nmcli_reads_the_access_point_that_is_active`、`ipconfig_summary_reads_the_ssid_and_pads_the_bssid`（`0:0:5e:0:53:2` 读成 `00:00:5e:00:53:02`）；`crates/rurge-platform/src/netenv/mod.rs`：`each_platform_is_read_by_its_own_reader`（三个平台读成同一个写法）；`crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（`BSSID:aa:bb:cc:dd:ee:ff` 与 `BSSID:AA:BB:CC:*` 都命中） | W2、M1、L1、P7、P8 |
| FR-IN-09 | 「默认网关」 | 做了 | `crates/rurge-platform/src/netenv/windows.rs::pick_primary`（有 IPv4 网关、度量最小的网卡；IPv6 网关同在 `routers` 里）、`crates/rurge-platform/src/netenv/linux.rs::quick`（`/proc/net/route` 与 `/proc/net/ipv6_route` 里度量最小的默认路由）、`crates/rurge-platform/src/netenv/macos.rs::quick`（`route -n get default`，IPv6 用 `-inet6`），解析在 `crates/rurge-platform/src/netenv/parse.rs`（`proc_net_route`、`proc_ipv6_route`、`route_get_default`）；网关进 `Snapshot.primary.routers`，`ROUTER:` 与裸值由 `crates/rurge-config/src/policy.rs::SubnetExpr::matches` 比它 | `crates/rurge-platform/src/netenv/windows.rs`：`the_primary_adapter_gives_its_routers_dns_servers_and_addresses`、`the_adapter_with_the_lowest_ipv4_metric_that_has_a_gateway_is_primary`、`without_an_ipv4_gateway_the_ipv6_one_decides`、`an_unspecified_gateway_is_no_gateway`；`crates/rurge-platform/src/netenv/parse.rs`：`proc_net_route_takes_the_default_route_with_the_lowest_metric`、`proc_net_route_reads_a_gateway_in_the_byte_order_of_the_machine`、`proc_ipv6_route_takes_the_default_route_with_the_lowest_metric`、`route_get_default_reads_the_interface_and_the_gateway`、`route_get_default_without_a_gateway_that_is_an_address_has_none`；`crates/rurge-platform/src/netenv/linux.rs`：`the_ipv6_gateway_joins_the_routers_only_on_the_same_interface`；`crates/rurge-platform/src/netenv/macos.rs`：`a_gateway_on_another_interface_is_not_a_router_of_the_primary_one`；`crates/rurge-platform/src/netenv/mod.rs`：`each_platform_is_read_by_its_own_reader`；`crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（`ROUTER:192.168.1.1` 与 `ROUTER:fe80::1` 命中，另一个地址不命中） | W1、M4、L1、P5 |
| FR-IN-09 | 「网络类型」 | 做了 | `crates/rurge-platform/src/netenv/windows.rs::view`（`ipconfig` 的网卡类型：802.11 是 `Wifi`、以太网类是 `Wired`，回环与隧道跳过）与 `pick_primary`、`crates/rurge-platform/src/netenv/linux.rs::kind_of`（`/sys/class/net/<网卡>/wireless` 存在是 Wi-Fi、`type` 为 1 是有线）、`crates/rurge-platform/src/netenv/macos.rs::quick`（`networksetup -listallhardwareports` 的端口名，`crates/rurge-platform/src/netenv/parse.rs` 的 `hardware_ports` 按 `Wi-Fi` 与 `Ethernet` / `LAN` 判）；进 `Snapshot.primary.kind`，`TYPE:WIFI` / `TYPE:WIRED` 由 `crates/rurge-config/src/policy.rs::SubnetExpr::matches` 比它 | `crates/rurge-platform/src/netenv/windows.rs`：`the_kind_of_the_primary_follows_the_adapter`、`what_cannot_carry_the_traffic_is_not_primary`；`crates/rurge-platform/src/netenv/linux.rs`：`the_kind_of_an_interface_comes_from_sys`；`crates/rurge-platform/src/netenv/macos.rs`：`the_kind_of_an_interface_comes_from_its_hardware_port`；`crates/rurge-platform/src/netenv/parse.rs`：`hardware_ports_tell_wifi_from_ethernet_from_the_rest`、`hardware_ports_read_the_name_by_its_words`；`crates/rurge-platform/src/netenv/mod.rs`：`each_platform_is_read_by_its_own_reader`；`crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（`TYPE:WIFI` 在有线网络上不命中） | W1、M1、M2、L1、P5 |
| FR-IN-09 | 「网络变化事件」 | 做了 | `crates/rurge-platform/src/netenv/watch.rs::hints`（Windows 与 Linux 每 2 秒比一次 `read_quick` 的指纹、有无线网卡开着时每 30 秒重读 Wi-Fi；macOS 收一个 `PF_ROUTE` 套接字，`route_message_matters` 滤掉 ARP 一类的噪声、一阵消息静下来 300 毫秒才发、每 30 秒兜底）；`crates/rurge/src/cli/network.rs::start` 与 `follow`（1 秒的首发加尾发；什么都读不出来的快照不算变化）、`crates/rurge/src/cli/run.rs::network_changed`（主循环一次一个地处理 `Command::NetworkChanged`，与重载、API 的命令不交叉） | `crates/rurge-platform/src/netenv/watch.rs`：`a_change_of_the_cheap_sources_is_a_hint_and_a_quiet_look_is_not`、`while_a_wireless_interface_is_up_the_wifi_link_is_looked_at_every_thirty_seconds`、`routes_that_come_and_go_matter_except_the_ones_of_single_hosts`、`a_burst_of_messages_is_one_hint_after_it_has_gone_quiet`、`the_file_backend_hints_when_the_file_changes_and_not_otherwise`；`crates/rurge/src/cli/network.rs`：`a_burst_of_changes_is_told_at_its_first_and_at_its_end`、`a_change_after_a_quiet_second_is_told_at_once`、`a_network_that_is_lost_and_comes_back_is_no_change`、`the_hints_of_the_platform_reach_the_task`；`crates/rurge/tests/cli.rs`：`the_network::a_network_change_moves_the_subnet_group_of_a_running_daemon`（守护进程跟着文件后端换网络）、`the_network::the_log_says_where_the_machine_is_only_at_debug`（一行 INFO `network changed`，SSID 与网关不在里面） | W1、W2、W8、M1、M6、L5、P6 |
| FR-IN-09 | 「（触发缓存清理…）」 | 做了 | `crates/rurge-engine/src/engine.rs::network_changed` 调 `crates/rurge-dns/src/resolver.rs::on_network_change`：清正在用的这一代的 DNS 缓存、解除 AAAA 抑制、重读系统的 DNS 服务器；已知限制：换网络那一刻已经在进行的查找，旧网络上的答案可能落进刚清空的缓存；一代在换网络的过程中建好时，它的解析器构造时读的系统 DNS 服务器与 IPv6 状态是旧的，要等下一次变化（M1 计划「延后事项」第 10 行） | `crates/rurge-engine/tests/network.rs`：`a_network_change_flushes_the_resolver`（缓存里的五条清空、AAAA 抑制解除、下一个查询重新问 AAAA）、`a_network_change_flushes_the_resolver_of_the_generation_in_use`（清的是正在用的那一代的解析器，不是引擎开始时的那一个） | — |
| FR-IN-09 | 「（…重测…）」 | 做了 | `crates/rurge-engine/src/engine.rs::network_changed` 调 `crates/rurge-policy/src/registry.rs::network_changed`，再由 `crates/rurge-policy/src/auto.rs::network_changed` 作废 `url-test` / `fallback` / `load-balance` 上一轮的测试时间（结果留着，测完之前照旧用）、清 `subnet` 组的临时覆盖，`smart` 不动；换网络那一刻正在跑的一轮结束时不算新网络上的一轮；已知限制：`TestBook` 对同一个策略一次只测一个，换网络后新开的一轮会与旧网络上还没结束的那一个测试共用结果（M1 计划「延后事项」第 11 行） | `crates/rurge-policy/src/auto.rs`：`a_network_change_clears_the_rounds_of_the_tested_groups_and_the_subnet_overrides`、`a_round_that_began_before_a_network_change_does_not_count`；`crates/rurge-policy/src/registry.rs`：`a_network_change_restarts_the_tests_but_keeps_the_results`、`evaluate_before_use_waits_again_after_a_network_change`；`crates/rurge-engine/tests/auto_groups.rs`：`a_dial_waits_for_a_round_that_began_on_the_new_network`（换网络那一刻的一轮不算，拨号等新的一轮，两次测速） | P2 |
| FR-IN-09 | 「（…`network-changed` 脚本…）」 | 需求文字有问题（阶段栏写 3，事件脚本在阶段 5，FR-SCR-07 与兼容性清单 8.1 节 `event-name`；现在：没有脚本引擎，网络变化不触发任何脚本） | —（代码里没有为脚本留的钩子或事件：主循环的 `Command::NetworkChanged` 只是网络变化本身） | — | — |
| FR-IN-09 | 「（…WireGuard 重建）」 | 做了 | `crates/rurge-policy/src/registry.rs::network_changed` 逐个叫注册表里的出站（`crates/rurge-proto/src/outbound.rs` 的 `network_changed`，缺省什么也不做），只有 `crates/rurge-proto-wireguard/src/outbound.rs::network_changed` 做事，转到 `crates/rurge-proto-wireguard/src/device.rs::network_changed`：每个 peer 换新的载体、重新握手 | `crates/rurge-proto-wireguard/src/outbound.rs`：`a_network_change_gives_every_peer_a_new_carrier`（第二个载体、握手两次、在途的连接照旧通过）、`a_network_change_told_through_the_outbound_trait_reaches_the_tunnel`（经 `Outbound` 那个入口一样）、`a_new_route_underneath_gives_every_peer_a_new_carrier`；`crates/rurge-policy/src/registry.rs`：`a_network_change_reaches_every_outbound`（注册表里的七个出站各被叫到一次，多一个少一个都红） | P3 |
| FR-IN-10 | 「`[SSID Setting]`」：行的读法与按当前网络选中的那一行 | 做了 | `crates/rurge-config/src/ssid_setting.rs::parse`（`parse_line` 里的 `parameters_start`：表达式写到第一个"前面是空白、写成 `<认识的键>=`"的地方为止，所以 SSID 里可以有空格；列表的下一项与下一个参数照兼容性清单 9.4 节分）与 `crates/rurge-config/src/config.rs::effective_ssid_setting`（按书写顺序第一行命中的，整行取用、不与后面的行合并）；已知限制：表达式后面直接跟逗号（`SSID:Home, suspend=true`）时逗号成了表达式的一部分，这一行永不命中，也没有诊断（兼容性清单 9.4 节第一行的"已知限制"、M1 计划「延后事项」第 9 行；拿 `rurge check` 试过，没有诊断） | `crates/rurge-config/src/ssid_setting.rs`：`ssid_setting_lines_are_read`、`the_expression_ends_in_front_of_the_first_known_key`、`lines_keep_their_order_and_their_places`、`the_dns_servers_of_a_line_are_read_as_those_of_general_are`、`an_unknown_parameter_is_said_and_never_quoted`；`crates/rurge-config/src/config.rs`：`the_first_matching_ssid_setting_line_is_in_effect`（三行，命中第一行，第二行的 `suspend` 不被合进来）、`an_ssid_setting_follows_subnet_exp_wifi_always_match` | — |
| FR-IN-10 | 「`suspend`」 | 有意不同（已登记） | `crates/rurge-config/src/ssid_setting.rs` 的 `SsidSetting` 里的 `suspend`（不写是 false）→ `crates/rurge/src/cli/run.rs::takeover_paused_here`；桌面上的 `suspend` 是"接管暂停"（总设计第 9.7 节、P3-D15）：撤掉系统代理与虚拟网卡，监听器与 API 照常。手册只写了 iOS 的"隧道不断、只是不处理"，差异登记在兼容性清单 9.4 节 `suspend` 一行（🟡）；需求的字面改不改是总设计第 16 节的 Q8，归项目所有者 | `crates/rurge-config/src/ssid_setting.rs`：`suspend_takes_the_booleans_of_general`（`true` / `TRUE` / `1` / `yes` 与 `false` / `0` / `no`）；`crates/rurge/tests/cli.rs`：`the_network::the_system_proxy_pauses_on_a_suspend_network`、`the_network::the_last_state_asked_for_during_a_pause_is_the_one_that_stands_after_it`、`the_network::a_pause_that_ends_switches_the_system_proxy_off_if_it_is_still_on_and_was_asked_off`、`the_network::a_reload_that_changes_the_ssid_setting_ends_or_begins_the_pause` | W7、W8、M5、L6 |
| FR-IN-10 | 「`dns-server` / `encrypted-dns-server` 覆盖」 | 做了 | `crates/rurge-config/src/ssid_setting.rs` 的 `add_dns_server` 与 `add_encrypted_dns_server`（`off` 是空表）；建一代时 `crates/rurge-engine/src/stack.rs` 的 `DnsOverride::for_network` 与 `apply` 把写了的列表整张替换，没写的那一项保持全局的，`[Host]` 里 `server:` 写明的服务器不受影响；网络变化让覆盖变了，`crates/rurge/src/cli/run.rs` 用内存里的配置重建一代 | `crates/rurge-config/src/ssid_setting.rs`：`the_dns_servers_of_a_line_are_read_as_those_of_general_are`、`a_list_nothing_was_read_into_is_not_the_lines`；`crates/rurge-engine/tests/network.rs`：`the_ssid_setting_of_the_network_sets_the_dns_servers`（解析器真的问覆盖后的服务器、`encrypted-dns-server=off` 去掉加密 DNS）、`the_dns_override_of_a_generation_is_known`；`crates/rurge/tests/cli.rs`：`the_network::the_ssid_setting_dns_servers_follow_the_network`（`GET /v1/dns` 的 `upstreams` 跟着网络变）、`the_network::a_rebuild_for_the_dns_of_a_network_neither_reads_the_profile_nor_forgets_a_choice`、`the_network::a_rebuild_for_the_dns_of_a_network_prints_what_the_new_generation_says` | W6、M5、L6、P4 |
| FR-IN-10 | 「`tfo-behaviour`」 | 没有做完（已登记） | `crates/rurge-config/src/ssid_setting.rs` 的 `TfoBehaviour` 与 `read_parameters`：三个值照读（不分大小写），读到就一条 `W0029`，值写错是 `W0028`。缺的是 TCP Fast Open 本身：`socket2` 0.6 没有安全封装、工作区禁 `unsafe`（兼容性清单 4.3 节 `tfo`，"有安全封装后再评估"），登记在兼容性清单 9.4 节 `tfo-behaviour` 一行（🟡）；使用者看到的是连接不受这一行影响、`rurge check` 一条 `W0029` | `crates/rurge-config/src/ssid_setting.rs`：`tfo_behaviour_takes_three_values`（三个值都读，每个一条 `W0029`；`always` 是 `W0028` 且不被引用） | — |
| FR-IN-10 | 「`suspend` 时停止接管并释放系统代理…」 | 做了 | `crates/rurge/src/cli/run.rs` 的 `Takeover`（`begin` 与 `follow`）与 `takeover_paused_here`：暂停时把开着的系统代理恢复成使用者原来的设置（同关闭系统代理），离开时想开着的重新指向 rurge；暂停只在内存里，不进 `state.json`；增强模式的那一半是本表「…与 TUN」一行，归任务 4 | `crates/rurge/tests/cli.rs`：`the_network::the_system_proxy_pauses_on_a_suspend_network`（系统代理的文件后端：暂停时文件回到原值、`GET` 说 `false`、`state.json` 里没有暂停的痕迹）、`the_network::the_last_state_asked_for_during_a_pause_is_the_one_that_stands_after_it`、`the_network::a_pause_that_ends_switches_the_system_proxy_off_if_it_is_still_on_and_was_asked_off` | W7、W8、M5、L6 |
| FR-IN-10 | 「…与 TUN」 | 待读（任务 4） | | | |
| FR-IN-10 | 「API 保持可用」 | 做了 | 暂停只改系统代理：API 服务器（`crates/rurge/src/cli/run.rs` 里的 API 任务）不经过暂停的判断，暂停期间 `crates/rurge-api/src/routes/features.rs::get_feature` 与 `set_feature` 照常答；经代理端口来的请求也照旧 | `crates/rurge/tests/cli.rs`：`the_network::the_system_proxy_pauses_on_a_suspend_network`（暂停期间 `GET` 与 `POST /v1/features/system_proxy` 都答 200）、`the_network::the_last_state_asked_for_during_a_pause_is_the_one_that_stands_after_it`（暂停期间两次 `POST` 都 200，且什么也不设） | — |

### 出站（FR-OUT）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-OUT-11（阶段 2 验收第 574 行） | 「策略级 `block-quic` 与全局 `block-quic` 四种覆盖」之经虚拟网卡的 UDP 流 | 待读（任务 5） | | | |
| FR-OUT-11（阶段 2 验收第 574 行） | 「增强模式下对 UDP 443 生效」 | 待读（任务 5） | | | |

### 策略组（FR-GRP）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-GRP-02（阶段 2 验收第 26、360 行） | 「`subnet` 组」 | 做了 | `crates/rurge-config/src/policy.rs::parse_group`（`subnet` 组那一行里不是参数的 `key = value` 都是条件）、`crates/rurge-config/src/spec/group.rs::to_group_spec` 与 `crates/rurge-config/src/spec/group.rs::read_subnet`（`default` 必填）、`crates/rurge-config/src/spec/group.rs::subnet_members`（成员表是条件里的策略按书写顺序再 `default`）、`crates/rurge-policy/src/registry.rs::with_network` 与 `crates/rurge-policy/src/registry.rs::choose` 的 `GroupKind::Subnet` 一支（每次选择读一次网络：有临时覆盖用覆盖，否则第一个命中的条件的策略，都不命中用 `default`）、`crates/rurge-policy/src/registry.rs::network_changed`；网络存进所有代共用的那一格的是 `crates/rurge-engine/src/engine.rs::network_changed`，守护进程里由 `crates/rurge/src/cli/run.rs::network_changed` 调它。阶段 2 推过来的那件事：当时这种组整组代表它的 `default`、`rurge check` 给一条 `W0008`；阶段 3 / M1 起可执行文件的能力表里有这一种组，`W0008` 不再因它出现（只有没有这一项的能力表上才有那句话） | `crates/rurge-policy/src/registry.rs`：`a_subnet_group_picks_by_the_network`（Wi-Fi 上是 `A`、有线是 `B`、两个都命中取写在前面的、都不命中是 `C`，`current_member` 与 `resolve` 同）、`a_registry_without_a_network_resolves_a_subnet_group_to_its_default`；`crates/rurge-engine/tests/outbounds.rs`：`a_subnet_group_follows_the_network_the_engine_is_on`（网络不知道时走 `default`，网络换成 Wi-Fi 后下一个会话走 `A`，重载出来的新的一代看同一个网络）；`crates/rurge/tests/cli.rs`：`the_network::a_network_change_moves_the_subnet_group_of_a_running_daemon`（守护进程里按网络选，经 `GET /v1/policy_groups/select` 读）、`check_knows_the_automatic_groups`（`rurge check` 的输出里没有 `W0008`，也没有加反引号的 `subnet` 这个类型名，也没有 `W0029`） | W1、M1、L1、P10 |
| FR-GRP-02 | 「（含 `ssid` 别名）」 | 做了 | `crates/rurge-config/src/policy.rs::GroupKind::parse`（`ssid` 与 `subnet` 是同一个组类型，`legacy_keyword` 记下用的是旧写法）、`crates/rurge-config/src/policy.rs::parse_group`（两种写法的读法一字不差，裸值同样是一条条件） | `crates/rurge-config/src/spec/group.rs`：`a_subnet_group_in_its_legacy_writing_is_taken_whole`（`ssid, default = A, cellular = B, MyWifi = DIRECT`：`legacy_keyword` 为真、`MyWifi` 读成裸值条件、成员是 `["DIRECT", "A"]`、一条诊断也没有） | — |
| FR-GRP-02 | 「条件按声明顺序」 | 做了 | `crates/rurge-config/src/policy.rs::parse_group`（条件按行里写的次序进 `PolicyGroup.conditions`）、`crates/rurge-policy/src/registry.rs::choose` 的 `GroupKind::Subnet` 一支（在条件表上从头找第一个命中的） | `crates/rurge-policy/src/registry.rs`：`a_subnet_group_picks_by_the_network`（两个条件都命中当前网络时取写在前面的那个）；`crates/rurge-config/src/spec/group.rs`：`a_subnet_group_lists_its_conditions_then_its_default`（条件按写的次序留着，`default` 写在哪都排在成员表最后） | W1、M1、L1 |
| FR-GRP-02 | 「`default`」 | 做了 | `crates/rurge-config/src/spec/group.rs::read_subnet`（手册说 `default` 必填，没写或写成空就是 `E0018`，组不建）、`crates/rurge-policy/src/registry.rs::choose` 的 `GroupKind::Subnet` 一支（没有条件命中时用 `default`）；行为变化：阶段 3 / M1 起这样的组加载不了，此前加载得了、运行时按空组兜底（兼容性清单 5.2 节 `default` 一行与 5.1 节 `subnet` 一行） | `crates/rurge-config/src/spec/group.rs`：`a_subnet_group_without_default_is_an_error`（五种写法都是 `E0018`、组不建、带建议）、`a_subnet_group_stands_for_its_default`（`default` 排在成员表最后）；`crates/rurge-config/src/config.rs`：`a_subnet_group_without_default_is_a_load_error`（Profile 里只留另一组，不另外报「引用了不存在的策略」）；`crates/rurge-policy/src/registry.rs`：`a_registry_without_a_network_resolves_a_subnet_group_to_its_default` | W1、M1、L1 |
| FR-GRP-02 | 「网络变化重算」 | 做了 | `crates/rurge-engine/src/engine.rs::network_changed`（把新网络存进共用的一格，再叫注册表）→ `crates/rurge-policy/src/registry.rs::network_changed` → `crates/rurge-policy/src/auto.rs::network_changed`（`subnet` 组的临时覆盖清掉）；组自己不存状态，每次选择都读那一格（`crates/rurge-policy/src/registry.rs::choose`） | `crates/rurge-policy/src/registry.rs`：`a_subnet_group_can_be_overridden_until_the_network_changes`（覆盖压过网络，网络变了覆盖没了，回到按网络选）、`a_subnet_group_picks_by_the_network`（同一个注册表存进新网络就换选择，不必重建）；`crates/rurge-policy/src/auto.rs`：`a_network_change_clears_the_rounds_of_the_tested_groups_and_the_subnet_overrides`；`crates/rurge/tests/cli.rs`：`the_network::a_network_change_moves_the_subnet_group_of_a_running_daemon`（守护进程在两个 Wi-Fi 与一条有线之间换，读到的选择跟着变） | W1、M1、L1、P1 |
| FR-GRP-02 | 「桌面平台不可能的条件永不匹配」 | 做了 | `crates/rurge-config/src/policy.rs::SubnetExpr::matches`（`TYPE:CELLULAR` 与 `MCCMNC:` 一律不命中，桌面上没有蜂窝网络；`TYPE:WIFI` 与 `TYPE:WIRED` 只看主网卡）；`rurge check` 对这两种条件与 `SUBNET` 规则都不报「永不匹配」，拿 `rurge check` 试过 | `crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（家庭网络上 `TYPE:CELLULAR` 与 `MCCMNC:310260` 都不命中；有线网络、`kind` 为 `other` 的网络与「什么都不知道」的网络都断言 `TYPE:CELLULAR` 不命中，`MCCMNC:` 没有出现在有线那一份里；同一张表的其它条件照常命中与不命中）；`crates/rurge-rules/src/matcher.rs`：`subnet_rules_are_evaluated_on_the_network_of_the_context`（`SUBNET` 规则的 `SSID:` / `BSSID:` / `ROUTER:` / `TYPE:WIFI` / `TYPE:WIRED` 与裸值按上下文里的网络判；蜂窝的两种不在它的八条里——`SUBNET` 规则交给同一个 `SubnetExpr::matches`，这两种由 `subnet_expressions_match_the_network` 钉住） | — |

### 规则（FR-RULE）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-RULE-01（阶段 1 对照第 196 行） | 「`PROCESS-NAME` 三平台可用」 | 待读（任务 2） | | | |
| FR-RULE-01（阶段 1 对照第 197 行） | 「…文件名 / 全路径 / 前缀三种模式」 | 待读（任务 2） | | | |
| FR-RULE-01（阶段 1 对照第 198 行） | 「…Windows 路径归一化」 | 待读（任务 2） | | | |
| FR-RULE-08（阶段 1 对照第 263 行） | 「…QUIC」 | 待读（任务 5） | | | |
| FR-RULE-08（阶段 1 对照第 264 行） | 「…（含多草案的尽力识别）」 | 待读（任务 5） | | | |
| FR-RULE-08（阶段 1 对照第 265 行） | 「…STUN」 | 待读（任务 5） | | | |

### DNS（FR-DNS）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-DNS-05（阶段 1 对照第 323 行） | 「`server:`（…`force-syslib`）」 | 待读（任务 4） | | | |
| FR-DNS-08 | 「fake-IP 应答器：监听 `198.18.0.2`」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「…与 `fd00:6152::2`」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「（`198.18.0.2`–`198.18.0.9` 均视为应答器）」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「池 `198.18.1.1`–`198.19.255.254`」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「…与 IPv6 专用前缀」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「映射持久化」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「TTL 30 秒」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「地址族一致」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「非 A / AAAA 转发」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「`hijack-dns`」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「`always-real-ip`」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「（含 `[Host]` 权威应答）」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「`allow-dns-svcb`」 | 待读（任务 3） | | | |
| FR-DNS-08 | 「预匹配 REJECT 三种应答」 | 待读（任务 3） | 同「第 7 节范围里没有编号的：预匹配在 DNS 与 SYN 层」的「「DNS 阶段 `REJECT` 返回无记录（限频后不响应）」（4.1 节）」一行、同「第 7 节范围里没有编号的：预匹配在 DNS 与 SYN 层」的「「DNS 阶段…`REJECT-DROP` 不响应」（4.1 节）」一行、同「第 7 节范围里没有编号的：预匹配在 DNS 与 SYN 层」的「「DNS 阶段…`REJECT-NO-DROP` 返回 `198.18.0.244` 并对其所有 TCP 连接回 RST」（4.1 节）」一行 | | |
| FR-DNS-08 | 「Firefox canary 域 NXDOMAIN」 | 待读（任务 3） | | | |
| FR-DNS-09 | 「`[SSID Setting]` 的 DNS 覆盖」 | 做了 | 同「入站（FR-IN）」的「「`dns-server` / `encrypted-dns-server` 覆盖」」一行 | 同上 | W6、M5、L6、P4 |

### 配置与观测（FR-CFG、FR-OBS）

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-CFG-18（阶段 1 对照第 160 行） | 「子网裸值表达式」之求值 | 做了 | `crates/rurge-config/src/policy.rs::SubnetExpr::matches`（裸值：带 `*` 或 `?` 的按 `SSID:` 的通配比，否则依次比 SSID（精确、区分大小写）、BSSID（精确、不分大小写）、网关地址；同样受 `subnet-exp-wifi-always-match` 与读不到名字的限制）；解析在阶段 1（`crates/rurge-config/src/policy.rs::SubnetExpr::parse` 的裸值一支），求值在这里 | `crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（`My Home`、`AA:BB:CC:DD:EE:FF`、`192.168.1.1`、`fe80:0::1` 命中，`my home`、`aa:bb:cc:dd:ee:ee`、`192.168.1.2`、`Office` 不命中，`My*` 与 `My Hom?` 按 SSID 通配、`aa:*` 不命中）；`crates/rurge-rules/src/matcher.rs`：`subnet_rules_are_evaluated_on_the_network_of_the_context`（`SUBNET,Home` 在连着 `Home` 的网络上命中） | — |
| FR-CFG-18 | 「`ssid` 组类型别名」 | 做了 | 同「策略组（FR-GRP）」的「「（含 `ssid` 别名）」」一行 | 同上 | — |
| FR-CFG-18（阶段 2 验收第 574 行） | 「subnet 组 `cellular` 参数」 | 做了 | `crates/rurge-config/src/spec/group.rs::to_group_spec`（`cellular` 与 `default` 一样只标成「用过了」，不起作用：它不是条件，它写的策略也不进成员表；不记日志，也没有诊断）；兼容性清单 5.2 节 `cellular` 一行的备注写「读了，不起作用（桌面没有蜂窝网络）」——手册说设了它在蜂窝网络上先于条件表生效，桌面等不到那一刻 | `crates/rurge-config/src/spec/group.rs`：`a_subnet_group_in_its_legacy_writing_is_taken_whole`（`cellular = B` 没有诊断，成员是 `["DIRECT", "A"]`，`B` 不在里面）、`a_subnet_group_without_default_is_an_error`（`ssid, SSID:Home = A, cellular = B` 一样是 `E0018`） | — |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 3 / M1 加的（`subnet` 组在 `/v1/policy_groups` 与 `select` 里、接管暂停时的 `/v1/features/system_proxy`、`/v1/dns` 的 `[SSID Setting]` 覆盖） | 做了，补了用例 | `crates/rurge-api/src/routes/policy_groups.rs::groups` / `crates/rurge-api/src/routes/policy_groups.rs::selection` / `crates/rurge-api/src/routes/policy_groups.rs::select`（`subnet` 组与别的组同一个形状：成员表、当前选择、临时覆盖）；`crates/rurge-api/src/routes/features.rs::get_feature` / `crates/rurge-api/src/routes/features.rs::set_feature` 与 `crates/rurge/src/cli/run.rs` 的 `Takeover`（暂停期间 `GET` 是真实状态，`POST` 只记下想要的状态）；`crates/rurge-api/src/routes/dns.rs::dns` 的 `upstreams` 取正在用的这一代的解析器，它带着这个网络的 `[SSID Setting]` 覆盖 | `crates/rurge-api/tests/api.rs`：`select_on_a_subnet_group_overrides_it_until_cleared`（`GET /v1/policy_groups/select` 按网络给、`POST` 设置与清除、非成员 400、不写 `state.json`）、`the_api_lists_the_members_of_a_subnet_group_in_the_order_written`（`5e00564` 补：`GET /v1/policy_groups` 里 `subnet` 组的成员是条件里的策略再 `default`，每个一次）；`crates/rurge/tests/cli.rs`：`the_network::a_network_change_moves_the_subnet_group_of_a_running_daemon`（经 API 读的当前选择跟着网络变）、`the_network::the_system_proxy_pauses_on_a_suspend_network`（暂停时 `GET` 是 `false`、`POST` 返回 `{}` 而不设）、`the_network::the_ssid_setting_dns_servers_follow_the_network`（`GET /v1/dns` 的 `upstreams` 随网络覆盖变） | W6、W7、P1、P4 |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 3 / M2 加的（请求记录里进程的两个键） | 待读（任务 2） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 3 / M3 加的（`/v1/traffic` 的 `listener.tun`、请求记录里的虚拟网卡会话、计数） | 待读（任务 3） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 3 / M4 加的（`GET` / `POST /v1/features/enhanced_mode`） | 待读（任务 4） | | | |
| FR-OBS-02 | 「路径、方法、JSON 字段与 Surge 一致（清单 10.4）」之阶段 3 / M5 加的（`enhanced_mode` 多的两个数、`protocol` 的 `quic` / `stun`、内部 DNS 会话的目标） | 待读（任务 5） | | | |

### 第 7 节范围里没有编号的：预匹配在 DNS 与 SYN 层

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| 第 7 节（阶段 1 对照第 205 行、阶段 2 验收第 520 行） | 「在 DNS 查询…阶段提前拒绝」（3.2 节） | 待读（任务 3） | | | |
| 第 7 节（阶段 1 对照第 205 行、阶段 2 验收第 520 行） | 「在…TCP 握手阶段提前拒绝」（3.2 节） | 待读（任务 3） | | | |
| 第 7 节 | 「DNS 阶段 `REJECT` 返回无记录（限频后不响应）」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「DNS 阶段…`REJECT-DROP` 不响应」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「DNS 阶段…`REJECT-NO-DROP` 返回 `198.18.0.244` 并对其所有 TCP 连接回 RST」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「TCP…`REJECT` 回 RST（限频后丢 SYN）」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「TCP…`REJECT-DROP` 丢 SYN」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「TCP…`REJECT-NO-DROP` 回 RST」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「3 秒内 100 次 RST 后暂停回 RST 改为丢包」（4.1 节） | 待读（任务 3） | 同「阶段 1 与阶段 2 推过来的」的「「…RST 保护（3 秒 100 次）」」一行 | | |
| 第 7 节 | 「UDP…`REJECT` 回 ICMP Administratively Prohibited（…限频后丢包）」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「UDP…`REJECT-NO-DROP` 回 ICMP Administratively Prohibited」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「UDP…`REJECT-DROP` 直接丢包」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「被 pre-matching 拒绝的规则 5 分钟内只在最近请求列表出现一次」（4.1 节） | 待读（任务 3） | | | |
| 第 7 节 | 「pre-matching REJECT 在应答器层执行」（6.4 节） | 待读（任务 3） | 同「第 7 节范围里没有编号的：预匹配在 DNS 与 SYN 层」的「「在 DNS 查询…阶段提前拒绝」（3.2 节）」一行 | | |

### 第 7 节范围里没有编号的：`[IP Rewrite]`

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| 第 7 节 | 「按目的地址处理进入虚拟网卡的 IP 包」（1.2 节） | 待读（任务 5） | | | |
| 第 7 节 | 「每行 `<IP 地址或 CIDR> = <动作>`」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「IPv4 与 IPv6 都行」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「自上而下，第一条命中的生效」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「`reflect`：把包送回发送者，源地址与目的地址对调」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「`reflect`：…网关模式下局域网设备发来的包送回那台设备」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「`reject`：TCP 的连接请求回 RST」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「`reject`：…其余的包回 ICMP administratively prohibited」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「`drop`：悄悄丢弃」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「只作用于进了 Surge VIF 的流量（增强模式…）」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「（…网关模式）」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「在 TCP / UDP 协议栈、规则与策略之前，命中的包不成为一条请求」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「发给代理监听端口的请求不受影响」（9.10 节） | 待读（任务 5） | | | |
| 第 7 节 | 「包要先被路由进 VIF：…要加进 `tun-included-routes`」（9.10 节） | 待读（任务 5） | | | |

### 阶段 1 与阶段 2 推过来的

这张表只列没有并进别的表的句子；并进去的，出处写在那一行的「需求」一格里（阶段 1 对照第 160、196 ～ 198、205、263 ～ 265、323 行，阶段 2 验收第 26、360、520 行，以及同一句并进 `subnet` 组的第 574 行与并进 `block-quic` 分条的那些）。

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| FR-CFG-06（阶段 1 对照第 78 行） | 「…各键随所属功能阶段生效」之 `ipv6-vif` | 待读（任务 4） | 同「入站（FR-IN）」的「「`ipv6-vif`」」一行 | | |
| FR-CFG-06（阶段 1 对照第 78 行） | 「…各键随所属功能阶段生效」之 `tun-excluded-routes` | 待读（任务 4） | 同「入站（FR-IN）」的「「`tun-excluded-routes`」」一行 | | |
| FR-CFG-06（阶段 1 对照第 78 行） | 「…各键随所属功能阶段生效」之 `tun-included-routes` | 待读（任务 4） | 同「入站（FR-IN）」的「「`tun-included-routes`」」一行 | | |
| FR-CFG-06（阶段 1 对照第 78 行） | 「…各键随所属功能阶段生效」之 `icmp-forwarding` | 待读（任务 3） | 同「入站（FR-IN）」的「「ICMP 转发（…）与 `icmp-forwarding`」」一行 | | |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `allow-dns-svcb` | 待读（任务 3） | 同「DNS（FR-DNS）」的「「`allow-dns-svcb`」」一行 | | |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `hijack-dns` | 待读（任务 3） | 同「DNS（FR-DNS）」的「「`hijack-dns`」」一行 | | |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `always-real-ip` | 待读（任务 3） | 同「DNS（FR-DNS）」的「「`always-real-ip`」」一行 | | |
| FR-CFG-06（兼容性清单 2.1 节） | 「…各键随所属功能阶段生效」之 `udp-priority` | 待读（任务 5） | 同「入站（FR-IN）」的「「`udp-priority` 尽力实现」」一行 | | |
| FR-CFG-06（兼容性清单 2.3 节，macOS 专属参数） | 「…各键随所属功能阶段生效」之 `subnet-exp-wifi-always-match` | 做了 | `crates/rurge-config/src/general.rs::parse_general`（布尔，缺省 `true`）；三处读同一个开关：`crates/rurge-rules/src/engine.rs::build`（`SUBNET` 规则）、`crates/rurge-policy/src/registry.rs::build`（`subnet` 组）、`crates/rurge-config/src/config.rs::effective_ssid_setting`（`[SSID Setting]` 选行）；判定在 `crates/rurge-config/src/policy.rs::SubnetExpr::matches`（`true` 时无线网卡不是主网卡也看它连着的网络，`false` 时只在它就是主网卡时看；`TYPE:` 与 `ROUTER:` 不受它影响）。手册把它列在 macOS 专属键里；rurge 没有按平台分开，三个平台都读、都生效 | `crates/rurge-policy/src/registry.rs`：`a_subnet_group_reads_subnet_exp_wifi_always_match`（无线网卡连着而主网卡是有线：`true` 选 `A`、`false` 选 `B`）；`crates/rurge-rules/src/engine.rs`：`subnet_exp_wifi_always_match_reaches_the_rules`（同一份网络：`true` 时命中第一条 `SUBNET,SSID:Home`、`false` 时不命中而落到 `FINAL`；主网卡换成 Wi-Fi 后两种设置都命中）；`crates/rurge-rules/src/matcher.rs`：`the_context_carries_subnet_exp_wifi_always_match`（同一份网络、两种设置：`SSID:` / `BSSID:` 与裸值 `Home` 在 `true` 时命中、`false` 时不命中，`TYPE:WIRED` 与 `ROUTER:` 两种设置下都一样；主网卡是 Wi-Fi 时 `false` 也命中）；`crates/rurge-config/src/config.rs`：`an_ssid_setting_follows_subnet_exp_wifi_always_match`（同一份“无线网卡连着、主网卡是有线”的网络：`true` 时那一行 `[SSID Setting]` 生效、`false` 时不生效，取不到这一行；主网卡是 Wi-Fi 时两种设置都取得到）；`crates/rurge-config/src/policy.rs`：`subnet_expressions_match_the_network`（家庭、有线、「什么都不知道」几张表在两种设置下各跑一遍） | W1、M1、L1、P5（都是开关开着的那一半；把它关掉的那一半没有手工项） |
| FR-CFG-06（兼容性清单 2.4 节） | 「…各键随所属功能阶段生效」之旧键 `ipv6-vif = off` | 待读（任务 4） | | | |
| FR-CFG-06（兼容性清单 2.4 节） | 「…各键随所属功能阶段生效」之旧键 `use-default-policy-if-wifi-not-primary` | 做了 | `crates/rurge-config/src/general.rs::parse_general`（读成 `subnet-exp-wifi-always-match` 并取反，一条 `I0001`；只在内存里迁移，不回写 Profile） | `crates/rurge-config/src/general.rs`：`legacy_keys_migrate`（`use-default-policy-if-wifi-not-primary = true` 之后 `subnet_exp_wifi_always_match` 是 `false`，七个旧键各一条 `I0001`） | — |
| FR-CFG-12（阶段 1 对照第 125 行） | 「供…`always-real-ip`…共用」之 `always-real-ip` 的使用者 | 待读（任务 3） | 同「DNS（FR-DNS）」的「「`always-real-ip`」」一行 | | |
| FR-CFG-17（阶段 1 对照第 154 行） | 「…TUN 设备名」 | 待读（任务 4） | | | |
| FR-RULE-01（阶段 1 对照第 189 行） | 「…解析与匹配」之 `SUBNET` | 做了 | `crates/rurge-rules/src/matcher.rs::Matcher::eval` 的 `Matcher::Subnet` 一支（比的是求值上下文里那一份网络）、`crates/rurge-rules/src/matcher.rs::EvalCtx::with_network`、`crates/rurge-rules/src/engine.rs::with_network`（一次求值只取一次网络，网络换了之后同一个引擎的结果跟着变，不必重载）；网络存进共用那一格的是 `crates/rurge-engine/src/engine.rs::network_changed`，守护进程里由 `crates/rurge/src/cli/run.rs::network_changed` 调它；`rurge check` 不再为它报「永不匹配」（可执行文件的能力表里有这一种规则）；需求行的阶段栏仍写 1（例外只列了 PROCESS-NAME 3、SCRIPT 5、DEVICE-NAME / MAC-ADDRESS 7），兼容性清单 3.1 节把 `SUBNET` 的匹配登记在阶段 3；阶段栏怎么改由项目所有者定，见 `docs/acceptance/phase1-acceptance.md`「需求文字的问题」第一张表第 12 行 | `crates/rurge-rules/src/matcher.rs`：`subnet_rules_are_evaluated_on_the_network_of_the_context`（家庭网络上八条 `SUBNET` 规则逐条命中与不命中，有线网络上把 `TYPE:WIRED` / `TYPE:WIFI` 与两个 `ROUTER:` 再看一遍）、`a_context_without_a_network_matches_no_subnet_rule`（没人告诉它网络的上下文不命中，也不留说明）、`subnet_rules_take_part_in_logical_rules`（`AND` / `OR` / `NOT` 里一样）；`crates/rurge-rules/src/engine.rs`：`subnet_rules_follow_the_network`（同一个引擎换网络换结果，命中计数）、`a_pre_matching_subnet_rule_sees_the_network`（会话的求值与解析前的那个入口都看新网络，回到旧网络又变成不命中）、`an_evaluation_in_flight_keeps_the_network_it_started_on`；`crates/rurge-engine/tests/pipeline.rs`：`a_subnet_rule_follows_the_network_the_engine_is_on`（会话真的被拒、再被放行）；`crates/rurge/tests/cli.rs`：`check_does_not_say_a_subnet_rule_never_matches`（输出里没有 `SUBNET` 这个词，`DEVICE-NAME` 仍报一条，`0 error(s), 1 warning(s)`）、`rule_match_reads_the_network`（网络文件写 `Home` 时 `SUBNET,SSID:Home` 命中、写 `Cafe` 时不命中，不给网络文件也不命中） | W1、M1、L1、P10 |
| FR-RULE-04（阶段 1 对照第 243 行） | 「…行级参数作用于整集」之 `pre-matching` 在 DNS 与 SYN 层 | 待读（任务 3） | | | |
| FR-RULE-07（阶段 1 对照第 258 行） | 「…从 HTTP/1.1 Host…提取主机名」之经虚拟网卡来的明文 HTTP | 待读（任务 3） | | | |
| FR-DNS-01（阶段 1 对照第 293 行；阶段 2 验收第 570、574 行） | 「…网络切换清空」 | 做了 | `crates/rurge-engine/src/engine.rs::network_changed` 调 `crates/rurge-dns/src/resolver.rs::on_network_change`（清正在用的这一代的缓存、解除 AAAA 抑制、重读系统的 DNS 服务器与 IPv6 状态；网络一变就清，不必手动刷、重载或重启）；网络从 `crates/rurge/src/cli/run.rs::network_changed` 来；已知限制：换网络那一刻已经在进行的查找，旧网络上的答案可能落进刚清空的缓存；一代在换网络的过程中建好时，它构造时读的系统 DNS 与 IPv6 状态是旧的，要等下一次变化（阶段 3 / M1 计划「延后事项」第 10 行）；需求行的阶段栏仍写 1，兼容性清单 6.1 节的缓存一行写「1 / 3」，网络切换时清空这一半在阶段 3；阶段栏怎么改由项目所有者定，见 `docs/acceptance/phase1-acceptance.md`「需求文字的问题」第一张表第 27 行 | `crates/rurge-engine/tests/network.rs`：`a_network_change_flushes_the_resolver`（缓存里的五条清空、AAAA 抑制解除、下一个查询重新问 AAAA）、`a_network_change_flushes_the_resolver_of_the_generation_in_use`（清的是正在用的那一代，不是引擎开始时的那一个） | — |
| FR-OUT-01（阶段 1 对照第 392 行；阶段 2 验收第 565 行） | 「…RST 保护（3 秒 100 次）」 | 待读（任务 3） | | | |
| FR-HTTP-10（阶段 1 对照第 410 行） | 「…来源（…进程 / 设备）」之进程 | 待读（任务 2） | | | |
| FR-HTTP-10（阶段 1 对照第 410 行） | 「…来源（…进程 / 设备）」之设备 | 待读（任务 2） | | | |
| FR-OUT-10（阶段 2 验收第 574 行） | 「…网络变化失效」 | 做了 | `crates/rurge-engine/src/engine.rs::network_changed` → `crates/rurge-policy/src/registry.rs::network_changed` → `crates/rurge-policy/src/auto.rs::network_changed`（`url-test` / `fallback` / `load-balance` 上一轮的测试时间作废，下一次使用时测一轮；结果本身留着，新一轮测完之前照旧用它们选，写了 `evaluate-before-use` 的组下一次使用时等这一轮；换网络那一刻正在跑的一轮不算）；`smart` 组不动（登记在兼容性清单 5.1 节 `url-test` 与 `smart` 两行）；已知限制：同一个策略一次只测一个，换网络后新开的一轮会与旧网络上还没结束的那一个共用结果（阶段 3 / M1 计划「延后事项」第 11 行） | `crates/rurge-policy/src/registry.rs`：`a_network_change_restarts_the_tests_but_keeps_the_results`（上一轮的时间清掉、结果还在）、`evaluate_before_use_waits_again_after_a_network_change`；`crates/rurge-policy/src/auto.rs`：`a_network_change_clears_the_rounds_of_the_tested_groups_and_the_subnet_overrides`、`a_round_that_began_before_a_network_change_does_not_count`；`crates/rurge-engine/tests/auto_groups.rs`：`a_dial_waits_for_a_round_that_began_on_the_new_network`（换网络那一刻在途的一轮不算，拨号等新网络上的一轮，请求记录里 `A` 的测试有两次） | P2 |
| FR-OUT-12（阶段 2 验收第 574 行） | 「…网络变化…时重建」 | 做了 | 同「入站（FR-IN）」的「「（…WireGuard 重建）」」一行 | 同上 | P3 |
| FR-OUT-13（阶段 2 验收第 540 行） | 「…`addresses` 从 TUN 路由排除」 | 待读（任务 4） | | | |
| FR-OUT-13（阶段 2 验收第 574 行） | 「…外部进程自身流量走 DIRECT」 | 待读（任务 4） | | | |

### 非功能需求

| 需求 | 这一句 | 结论 | 代码 | 用例 | 真机上 |
| --- | --- | --- | --- | --- | --- |
| NFR-01 | 「目标值（阶段 1 与阶段 3 建立基准后校准）」 | 待读（任务 5） | | | |
| NFR-01 | 「fake-IP 应答 < 0.2 ms」 | 待读（任务 5） | | | |
| NFR-05 | 「网络切换后自动恢复（重建 socket…）」 | 做了 | rurge 自建的出站套接字只有一处建法（`CLAUDE.md`「出站套接字只有一处建法」）：`crates/rurge-net/src/connector.rs::open_socket` 建好之后问出口、按回答绑定（`new_socket` 与 `bind_egress`；`DirectPacket` 每个数据报自己走这两步）；活得比一次会话长的套接字在出口表变了之后靠 `Connector::route()` 重建：`crates/rurge-proto-quic/src/session.rs` 的换表连接不收新请求、下一个请求重新拨，`crates/rurge-proto-wireguard/src/device.rs::rerouted` 换载体 | `crates/rurge-net/src/connector.rs`：`open_socket_asks_egress_for_the_destination_and_binds_there`、`bind_udp_passes_the_destination_to_egress`、`direct_packet_opens_a_socket_for_each_egress_and_hears_on_all`、`a_direct_connector_routes_to_the_egress_table_its_hook_names`；`crates/rurge-proto-quic/src/session.rs`：`a_connection_dialled_under_another_egress_table_takes_no_new_requests`（换表之后旧连接不收新请求、下一个请求在新表上拨、握手两次、日志里说的是出口变了）；`crates/rurge-proto-wireguard/src/outbound.rs`：`a_new_route_underneath_gives_every_peer_a_new_carrier`（出口表变了才换载体，同一个表不换）、`a_network_change_gives_every_peer_a_new_carrier` | P3、P5 |
| NFR-05 | 「网络切换后自动恢复（…清缓存…）」 | 做了 | 同「入站（FR-IN）」的「「（触发缓存清理…）」」一行 | 同上 | — |
| NFR-05 | 「网络切换后自动恢复（…重测…）」 | 做了 | 同「入站（FR-IN）」的「「（…重测…）」」一行 | 同上 | P2 |
| NFR-05 | 「网络切换后自动恢复（…重建 WireGuard）」 | 做了 | 同「入站（FR-IN）」的「「（…WireGuard 重建）」」一行 | 同上 | P3 |
| NFR-07 | 「纯 Rust 实现优先」 | 待读（任务 4） | | | |
| NFR-07 | 「平台特定代码隔离在 `rurge-platform` 与 `rurge-tun`」 | 待读（任务 4） | | | |
| NFR-07 | 「CI 在三平台构建与测试」 | 待读（任务 4） | | | |
| NFR-11 | 「Wintun 等驱动的再分发条款在阶段 3 确认」 | 待读（任务 4） | | | |

### 数字

（任务 7 写）

### 没有做完的

| # | 需求（优先级） | 这一句 | 缺什么、使用者今天看到什么 | 去向 |
| --- | --- | --- | --- | --- |
| | FR-IN-10（P1） | 「`tfo-behaviour`」 | 缺的是 TCP Fast Open 本身：`socket2` 0.6 没有安全封装、工作区禁 `unsafe`（兼容性清单 4.3 节 `tfo`，"有安全封装后再评估"），登记在兼容性清单 9.4 节 `tfo-behaviour` 一行（🟡）。三个值照读，每个一条 `W0029`；连接不受这一行影响 | 项目所有者（M6 设计第 13 节第 4 条：放进哪个阶段） |

### 需求文字的问题

| # | 需求（优先级） | 这一句 | 问题 | 去向 |
| --- | --- | --- | --- | --- |
| | FR-IN-09（P0） | 「（…`network-changed` 脚本…）」 | 阶段栏写 3；事件脚本由 FR-SCR-07 登记在阶段 5（兼容性清单 8.1 节 `event-name` 一行、阶段 3 总设计第 1.3 节「范围外」的表、阶段 3 / M1 计划「延后事项」第 14 行），今天网络变化不触发任何脚本 | 项目所有者（M6 设计第 13 节第 4 条） |

### 有意不同的

| 需求 | 这一句 | 登记在 |
| --- | --- | --- |
| FR-IN-10 | 「`suspend`」 | 兼容性清单 9.4 节 `suspend` 一行（🟡）与阶段 3 总设计第 9.7 节、P3-D15：桌面上的 `suspend` 是接管暂停——撤掉系统代理与虚拟网卡，监听器与 API 照常；手册只写了 iOS 的"隧道不断、只是不处理"。需求的字面改不改是总设计第 16 节的 Q8，归项目所有者 |

### 本机没有能红的用例的

| 需求 | 这一句 | 代码与为什么本机没有能红的用例（它的 cfg，或要特权 / 真设备 / DLL） | 要补的用例（或 L2 的哪一条） |
| --- | --- | --- | --- |

## 第二部分：总设计第 14 节八条验收标准

（任务 7 写）

## 靠手工验收的

（任务 7 写）

## 靠 CI 的

（任务 7 写）

## 留给项目所有者的

（任务 7 写）
