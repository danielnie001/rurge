# 阶段 3 / M4 的 L1 核对

2026-10-27 做的（M4d 计划的任务 7）。这一份回答一个问题：M4 设计第 14 节列在 L1 里的每一项，以及 M4a ～ M4d 四份计划各任务的「用例」一栏点了名的每一条，是不是真的有用例在、它在哪里、在哪些平台上编译与运行。

**怎么核的**：把设计与四份计划的「用例」一栏里每个反引号括起来的名字，逐个用 `git grep -n "fn <名字>"` 在 `crates/` 与 `tests/` 里找；找到的再读它的属性（`#[test]` / `#[tokio::test]` / `#[cfg(...)]` / `#[cfg_attr(...)]`），以及它所在的模块有没有 `cfg`。计划末尾「执行期修正记录」改过的名字以记录为准（见第三节）。设备、执行器、网卡这些用例一条也没有跑过真东西——这是 P19，见第四节。

**「本机跑过」是什么意思**：这一份里说「本机跑过」的，是这台 Windows 11 x86-64 开发机上的一次全量运行（任务 7 的门禁：86 个结果行、4637 通过、0 失败、13 忽略）——**不是 CI**。M4（M4a ～ M4d）的提交 CI 一个也没有见过（`CLAUDE.md`：CI 最近一次运行在 `ac0a98a`）。

**2026-10-28 的订正**（M4 推送之后的 CI）：运行 40611702392（提交 `3a76b80`）三个平台的 `check` 都绿。下文的“M4 的提交 CI 还没有见过”“CI 还没有见过”以这一段为准：标「三平台」的用例在 CI 的三个平台上、标「只在 Windows」的在 CI 的 Windows 上编译并通过；标「只在非 Windows」的那 1 条（`the_wintun_dll_option_is_a_usage_error_elsewhere`）在 Linux 与 macOS 上运行并通过；L2 的 3 条在 Linux 上编译了，CI 没设 `RURGE_TEST_PRIVILEGED`，它们打印原因就返回，仍是任何地方都没有运行过。文末两条限制里，`a_real_wintun_dll_passes` 在 CI 上同样只是返回（`RURGE_TEST_WINTUN_DLL` 不设），`pings_the_loopback` 在 CI 的各平台上真的收发了还是跳过了，没有读到。这些都由测试一步的成功推出：任务日志要登录才能读，各平台逐条的结果没有读到。「本机跑过」的意思不变。

**三种标记**（「编译与运行」一栏）：

- **三平台（本机跑过）**：文件不带 `cfg`（或只带 `#[cfg(test)]`），三个平台都编译、都跑；这台 Windows 11 x86-64 开发机上跑过。CI 的三个平台任务会编译、运行它们；M4 的提交 CI 还没有见过。
- **只在 Windows（本机跑过）**：带 `#[cfg(windows)]`，只在 Windows 编译、运行；这台开发机上跑过。CI 的 Windows 任务会编译、运行它们；M4 的提交 CI 还没有见过。Linux / macOS 上不编译。
- **只在非 Windows（CI 还没有见过）**：带 `#[cfg_attr(not(windows), test)]` 一类，Windows 上只编译、不运行，Linux / macOS 上才跑；这台开发机只编译、不运行，第一次运行是 CI。
- **L2（缺省跳过）**：`tests/privileged`，带 `#[cfg(target_os = "linux")]`，没有 `RURGE_TEST_PRIVILEGED` 时跳过并打印原因；只在 Linux 编译，任何地方都没有运行过。
- **缺**：名单上有、代码里没有的。

**这一份不说什么**：有没有用例不等于用例真的钉住了那一句需求（那是逐句对照要做的，`docs/acceptance/phase*-acceptance.md`）；也不等于那些用例在真机上是对的（手工清单是 `docs/acceptance/phase3-manual.md`）。

## 一、设计的 L1 四条（`docs/superpowers/specs/2026-10-26-phase3-m4-platform-design.md` 第 14 节）

### 1. `rurge-platform`

| 项 | 用例（文件：名字） | 编译与运行 |
| -- | ------------------ | ---------- |
| 路由集（第 4.7 节） | `crates/rurge-platform/src/route/set.rs`：`the_base_routes_are_eight_prefixes_each_way`、`loopback_link_local_multicast_and_broadcast_never_go_in`、`one_excluded_address_leaves_a_hole_and_no_more`、`adjacent_holes_merge`、`a_hole_that_covers_a_base_prefix_takes_it_whole`、`a_hole_across_two_base_prefixes`、`included_wins_over_excluded`、`rurges_space_is_never_stripped_and_pieces_inside_it_are_dropped`、`ipv6_off_uses_no_ipv6_item`、`external_addresses_are_holes`、`diff_adds_and_removes_only_what_changed`、`nat64_is_in_and_the_rest_of_colon_colon_slash_8_is_out`（12 条） | 三平台（本机跑过） |
| 三个平台的步骤翻译（逐字） | Linux：`route/linux.rs` 的 `the_adapter_comes_up_in_one_batch`、`ipv6_is_a_batch_of_its_own_that_may_fail`、`each_family_is_one_batch_of_routes`、`a_family_with_no_routes_is_no_batch`、`linux_adds_routes_and_never_replaces_them`、`an_empty_set_is_no_step`、`removing_routes_is_one_batch_that_goes_on`、`the_system_dns_is_taken_over_after_the_resolvers_own_settings_go_off`、`without_resolvectl_the_system_dns_is_left_alone`、`giving_dns_back_is_one_step_that_goes_on`、`scripts_are_lines_ending_in_a_newline_and_no_carriage_return`（11 条）。macOS：`route/macos.rs` 的 `the_adapter_comes_up_with_its_address_and_mtu`、`ipv6_comes_and_goes_on_its_own`、`a_route_is_a_step_of_its_own`、`a_family_with_no_routes_is_no_step`、`an_empty_set_is_no_step`、`removing_routes_goes_on_with_what_remains`、`the_system_dns_points_at_the_responder`、`a_service_name_with_a_space_stays_one_argument`、`an_empty_list_of_servers_is_empty`、`the_dns_caches_are_flushed_in_two_steps`（10 条）。Windows：`route/windows.rs` 的 `the_tools_are_the_ones_in_system32`、`the_adapter_comes_up_in_one_script`、`ipv6_is_a_script_of_its_own`、`each_family_is_one_script_of_routes`、`a_family_with_no_routes_is_no_script`、`an_empty_set_is_no_step`、`removing_routes_goes_on_with_what_remains`、`the_system_dns_is_taken_over_then_the_caches_are_flushed`、`turning_ipv6_off_clears_the_ipv6_dns_list`、`scripts_are_lines_ending_in_a_newline_and_no_carriage_return`（10 条）。三个模块都不带 `cfg` | 三平台（本机跑过） |
| 输出的解析（夹具，含中文） | 路由表与地址：`route/windows.rs` 的 `routes_are_read_from_the_english_table`、`routes_are_read_whatever_the_language`、`a_prefix_with_an_embedded_ipv4_address_is_read`、`lines_without_a_prefix_and_a_number_are_skipped`、`addresses_are_read_from_the_english_output`、`addresses_are_read_whatever_the_language`、`an_address_with_a_zone_is_read_without_it`、`the_mtu_is_the_first_number_of_the_first_row`、`the_readback_commands_are_one_netsh_call_each`。读回之后的判断：`enhanced/windows.rs` 的 `configure_runs_the_script_then_reads_back_the_address_and_the_mtu`、`a_missing_address_fails_configure`、`an_mtu_that_is_not_1500_fails_configure`、`an_ipv6_address_that_is_missing_is_ok_false_and_taken_out_again`、`routes_are_read_back_by_the_adapter_s_index`、`missing_ipv4_routes_take_the_added_ones_out_and_fail`、`missing_ipv6_routes_are_ok_false`、`the_dns_is_read_back`、`a_missing_ipv6_dns_is_one_warn_and_still_ok`、`a_dns_that_did_not_change_fails_take_dns`、`netsh_s_exit_status_does_not_decide`。macOS 的输出：`enhanced/macos.rs` 的 `enabled_services_skip_the_heading_and_the_disabled`、`all_services_keep_the_disabled_without_the_asterisk`、`dns_servers_read_a_list_and_the_none_set_sentence`、`service_devices_pair_each_service_with_its_device`、`dhcp_dns_reads_domain_name_server`。`resolv.conf`：`platform/dns.rs` 的 `parses_resolv_conf_servers_and_search`、`an_unknown_option_does_not_empty_the_servers` | 三平台（本机跑过）；中文与按 GBK 读坏的输出是 `routes_are_read_whatever_the_language` 与 `addresses_are_read_whatever_the_language` 两份手写夹具（P17：不是从机器上取的） |
| 出口判定（表 × 地址的组合） | `crates/rurge-platform/src/socket/egress.rs`：`nothing_is_bound_while_the_enhanced_mode_is_off`、`loopback_unspecified_multicast_link_local_and_broadcast_are_never_bound`、`a_directly_connected_address_goes_out_of_its_interface_before_the_routes_are_asked`、`an_ipv4_address_in_its_mapped_form_is_decided_as_an_ipv4_address`、`the_longest_subnet_wins`、`an_address_in_the_routes_goes_out_of_the_primary_interface`、`without_a_primary_interface_it_is_no_route`、`an_excluded_address_is_left_to_the_system`、`subnets_of_leaves_out_loopback_link_local_the_excluded_and_a_zero_prefix`、`subnets_of_lists_each_subnet_once_longest_first`（10 条） | 三平台（本机跑过） |
| ICMP 的分派 | `crates/rurge-platform/src/icmp.rs`：`request_and_reply_round_trip_for_both_families`、`the_icmpv4_checksum_is_right`、`a_reply_after_an_ipv4_header_is_read`、`only_linux_datagram_sockets_lose_the_identifier`、`anything_but_an_echo_reply_is_none`。分派与取消：`crates/rurge/src/cli/pinger.rs` 的 `replies_go_to_the_waiter_of_their_sequence_number`、`the_identifier_matters_where_the_socket_keeps_it`、`a_dropped_echo_leaves_the_table`、`sequence_numbers_skip_the_ones_still_waited_for`、`forget_drops_only_the_carrier_that_failed`、`a_carrier_that_fails_to_send_is_made_again`；真的套接字：`pings_the_loopback` | 三平台（本机跑过）。`pings_the_loopback` 在开不了 ICMP 套接字的机器上打印原因并跳过（Windows 非管理员、Linux 的 `ping_group_range`），本机那一次是否真的收发没有单独记 |

### 2. `rurge-net` / `rurge-dns` / `rurge-engine`

| 项 | 用例（文件：名字） | 编译与运行 |
| -- | ------------------ | ---------- |
| 出口在 `open_socket`、`DirectPacket`、QUIC 上 | `crates/rurge-net/src/connector.rs`：`open_socket_asks_egress_for_the_destination_and_binds_there`、`no_egress_no_binding`、`a_policy_interface_is_used_and_egress_is_not_asked`、`allow_other_interface_falls_back_to_egress`、`an_egress_error_fails_the_dial_and_nothing_reaches_the_server`、`a_binding_that_fails_fails_the_dial_with_no_interface_name_in_the_error`、`bind_udp_passes_the_destination_to_egress`、`direct_packet_opens_a_socket_for_each_egress_and_hears_on_all`、`direct_packet_fails_a_datagram_whose_egress_is_an_error`、`the_hook_sees_every_socket_of_a_direct_carrier`、`a_direct_connector_routes_to_the_egress_table_its_hook_names`；`crates/rurge-net/src/quic/mod.rs` 的 `quic_direct_socket_asks_egress_for_the_server` | 三平台（本机跑过） |
| DNS 上游经连接器 | `crates/rurge-dns/src/upstream/udp.rs` 的 `a_udp_upstream_asks_through_its_connector`；`crates/rurge-dns/src/resolver.rs` 的 `each_kind_of_upstream_dials_through_its_own_connector`；端到端（每一类上游、资源下载、Profile 的来源、失败时什么都没发）：`crates/rurge-engine/tests/egress.rs` 的 `every_kind_of_dns_upstream_asks_the_hook_before_it_dials`、`the_pipeline_fallback_asks_the_hook`、`resource_downloads_ask_the_hook`、`profile_sources_ask_the_hook`、`an_egress_error_stops_each_kind_before_anything_is_sent` | 三平台（本机跑过） |
| `taken_over` 的去处（第 6.2 节的表逐行） | `crates/rurge-dns/src/resolver.rs`：`server:system` / `syslib` → `in_the_enhanced_mode_system_and_syslib_ask_the_physical_servers`，没有已知的服务器时失败、不退回库 → `in_the_enhanced_mode_system_without_servers_fails_and_never_asks_the_library`；`server:force-syslib` → `force_syslib_asks_the_library_in_the_enhanced_mode_too`；单标签名字 → `a_single_label_asks_only_the_servers_in_the_enhanced_mode`、`a_single_label_asks_only_the_servers_in_the_enhanced_mode_and_fails_without_them`；`.local` → `local_names_ask_the_library_in_the_enhanced_mode_too`；没有服务器时的库与 `system` → `a_resolver_without_servers_does_not_turn_to_the_library_in_the_enhanced_mode`。不接管的那一栏：`the_three_system_values_of_host_are_all_the_library`、`a_simple_host_name_goes_to_the_system_servers_with_the_search_domain`、`a_local_name_is_asked_of_the_library_and_not_of_the_servers`、`without_a_system_server_a_simple_host_name_is_asked_of_the_library` | 三平台（本机跑过） |
| `try_update` | `crates/rurge-engine/src/state.rs`：`try_update_changes_nothing_when_the_file_cannot_be_written`、`try_update_writes_before_it_changes_the_state`（另有 `store_opens_updates_atomically_and_renames_garbage`、`concurrent_updates_are_serialized`） | 三平台（本机跑过） |
| `start_tun` 的三条 | `crates/rurge-engine/tests/tun.rs`：`start_tun_refuses_options_that_check_refuses_and_opens_nothing`、`start_tun_refuses_a_device_below_1280`、`a_second_tun_on_one_engine_is_refused_until_the_first_has_ended` | 三平台（本机跑过） |
| `rurge-tun` 的三条 | `crates/rurge-tun/src/tests/dispatch.rs`：`a_shard_queue_beyond_queue_bytes_drops_and_counts_queue_full`、`queued_bytes_are_given_back_when_the_shard_takes_the_packets`、`shards_are_chosen_with_a_key_of_each_tun` | 三平台（本机跑过） |

### 3. `rurge`（可执行文件）

| 项 | 用例（文件：名字） | 编译与运行 |
| -- | ------------------ | ---------- |
| 源码扫描（第 7.2 节） | `crates/rurge/tests/sockets.rs` 的 `every_outbound_socket_is_made_in_one_place`（名单与（文件，写法）逐处比对；另有它自己的五条规则用例：`a_pattern_is_found_only_where_a_name_does_not_continue`、`a_pattern_after_a_comment_is_not_read`、`an_item_after_the_tests_is_reported`、`a_connector_without_a_hook_is_found_however_it_is_written`、`a_test_module_in_another_file_does_not_end_the_file`） | 三平台（本机跑过） |
| 管理器的状态机（文件后端，单元测试里直接驱动） | `crates/rurge/src/cli/enhanced/mod.rs` 的 39 条，从 `a_device_that_fails_tells_the_loop_which_open_it_belongs_to` 起到 `a_crash_that_left_no_undo_action_puts_the_flag_back`；`backend.rs` 的 `undo_and_saved_map_both_ways`、`a_test_undo_on_the_platform_backend_is_dropped`、`the_off_backend_says_it_is_off`、`an_unknown_value_never_falls_back_and_the_two_known_ones_are_taken`、`rurges_space_is_one_and_the_same_everywhere`；`file.rs` 的 9 条；`wanted.rs` 的 `wanted_follows_ipv6_vif`、`the_profile_and_the_external_addresses_decide_the_routes` | 三平台（本机跑过） |
| 守护进程端到端（`rurge_process()` + `RURGE_ENHANCED_BACKEND=file:`，第 9.10 节的那张单子） | `crates/rurge/tests/run/enhanced_mode.rs` 的 22 条，从 `what_a_dead_run_left_is_undone_by_the_next_start` 起到 `the_daemon_s_own_dns_and_downloads_keep_working_while_the_enhanced_mode_is_on`；API 的形状另在 `crates/rurge-api/tests/api.rs` 的 `enhanced_mode_get_has_the_counters_and_the_shape`、`enhanced_mode_post_goes_to_the_control_and_its_error_is_a_500` | 三平台（本机跑过） |
| `rurge check` 的四个码 | `crates/rurge-config/src/general.rs` 的 `ipv6_vif_always_warns`、`ipv6_vif_auto_and_disabled_say_nothing`、`including_a_whole_private_range_warns`、`excluding_into_rurges_space_warns`；`config.rs` 的 `force_syslib_warns_and_system_and_syslib_do_not`；`spec/mod.rs` 的 `external_addresses_are_no_longer_inert`、`config.rs` 的 `external_addresses_lists_the_addresses_of_every_external_policy`；四个码都在 `codes::hint` 里有决定——`crates/rurge-config/src/diagnostic.rs` 的 `every_code_is_decided_once` | 三平台（本机跑过） |

### 4. 不变的规矩

| 项 | 用例（文件：名字） | 编译与运行 |
| -- | ------------------ | ---------- |
| 只用回环 | 不是一条用例，是测试约定（`docs/testing.md`）；本节的每一项都只用回环与端口 0 | — |
| 缺省的 `cargo test` 从不执行地址、路由、DNS 的命令、从不建网卡（平台后端的执行函数没有任何用例调用） | 没有一条用例调 `TunDevice::open`（见第四节）；平台后端的每个动作只经假的执行器与临时目录测（`route/*.rs`、`enhanced/{linux,macos,windows}.rs` 的用例都建自己的 `Fake` / `FakeRunner`） | 三平台（本机跑过） |
| 有一条用例断言起 `rurge` 的辅助函数缺省是 `off` | `crates/rurge/tests/run/enhanced_mode.rs` 的 `the_helpers_keep_the_enhanced_mode_off` | 三平台（本机跑过） |

## 二、四份计划各任务点名了的用例

计划里那些名字是**生产代码的函数**、不是用例（「改哪里」一栏点了名，或者用例一栏里提到但指的是别的东西），不在下表中：`udp_bytes`、`shard_of`、`contains`、`addresses`、`addresses_in`、`routes_in`、`configure`、`configure_ipv6`、`unconfigure_ipv6`、`unicast_if`、`take_dns`、`give_dns_back`、`dns_saved`、`physical_dns`、`rp_filter`、`bind_udp`、`connect_udp`、`recv_from`、`enhanced`、`library_asked`、`network_changed`、`ipv6_usable`、`wintun_dll`、`system_dns_changed`、`on_network_change`、`reread_system_servers`、`apply_system_servers`、`host_upstreams_for`、`build_plain_udp`、`build_stack_with`、`look_up_owner`、`reload_and_refresh`、`restore_what_a_crash_left`、`recover_without_a_log`、`add_routes`、`remove_routes`、`try_update`、`expand`。

### M4a（`docs/superpowers/plans/2026-10-26-phase3-m4a-skeleton-plan.md`）

| 任务 | 点名了的用例 | 编译与运行 |
| ---- | ------------ | ---------- |
| 1（`rurge-tun` 的排队字节与散列） | `a_shard_queue_beyond_queue_bytes_drops_and_counts_queue_full`、`queued_bytes_are_given_back_when_the_shard_takes_the_packets`、`shards_are_chosen_with_a_key_of_each_tun`、`a_shard_rebuilt_twice_in_a_minute_warns_once` | 三平台（本机跑过） |
| 2（`start_tun` 先检查选项） | `start_tun_refuses_options_that_check_refuses_and_opens_nothing`、`start_tun_refuses_a_device_below_1280`、`a_second_tun_on_one_engine_is_refused_until_the_first_has_ended` | 三平台（本机跑过） |
| 3（`try_update` 与 `enhanced_backup`） | `try_update_changes_nothing_when_the_file_cannot_be_written`、`try_update_writes_before_it_changes_the_state`、`enhanced_backup_round_trips_and_an_old_file_reads_without_it` | 三平台（本机跑过） |
| 4（路由集） | 见一、1 的 12 条 | 三平台（本机跑过） |
| 5（三个平台的步骤翻译） | 见一、1；每个翻译函数一条逐字用例 | 三平台（本机跑过） |
| 6（`rurge check` 的四个码与 `external` 的 `addresses`） | `ipv6_vif_always_warns`、`ipv6_vif_auto_and_disabled_say_nothing`、`including_a_whole_private_range_warns`、`excluding_into_rurges_space_warns`、`force_syslib_warns_and_system_and_syslib_do_not`、`external_addresses_are_no_longer_inert`、`external_addresses_lists_the_addresses_of_every_external_policy`、`every_code_is_decided_once` | 三平台（本机跑过） |
| 7（管理器、后端与恢复） | 单元：`opening_goes_in_the_designed_order`、`the_backup_is_saved_before_the_step_it_undoes`、`a_backup_that_cannot_be_written_stops_the_step_before_it_is_done`、`a_step_that_fails_undoes_the_steps_before_it_in_reverse`、`closing_goes_in_the_designed_order_and_never_stops_halfway`、`closing_waits_for_the_tun_three_seconds_at_most`、`ipv6_that_cannot_be_had_is_left_off_for_this_open`、`an_unreadable_backup_is_left_alone`、`rurges_space_is_one_and_the_same_everywhere`、`wanted_follows_ipv6_vif`。端到端：`what_a_dead_run_left_is_undone_by_the_next_start`、`what_a_dead_run_left_is_undone_even_when_the_profile_does_not_load`、`an_undo_that_fails_stays_for_the_next_start`、`the_helpers_keep_the_enhanced_mode_off`、`an_unknown_enhanced_backend_ends_the_start_with_2`。计划另点了 `the_platform_backend_is_not_available_in_this_version`——M4c 任务 8 删掉了它（见第三节） | 三平台（本机跑过）；`closing_waits_for_the_tun_three_seconds_at_most` 是计时用例，提交前循环跑过二十次 |
| 8（API 与记录） | `enhanced_mode_get_has_the_counters_and_the_shape`、`enhanced_mode_post_goes_to_the_control_and_its_error_is_a_500`、`the_enhanced_mode_opens_at_start_and_closes_at_stop`、`turning_it_on_and_off_through_the_api`、`a_step_that_fails_undoes_what_was_done_and_the_start_ends_with_1`、`post_failure_is_a_500_with_the_reason`、`a_killed_run_with_the_enhanced_mode_on_is_undone_by_the_next_start`、`ipv6_vif_always_opens_with_ipv6`、`ipv6_that_fails_goes_on_without_it`、`a_device_that_fails_turns_the_enhanced_mode_off` | 三平台（本机跑过） |
| 9（网络变化、暂停、重载） | `the_enhanced_mode_waits_on_a_suspend_network_and_opens_when_it_is_left`、`a_post_during_the_pause_is_noted_and_done_when_it_ends`、`a_reload_changes_only_the_routes_that_changed`、`a_reload_turns_ipv6_on_and_off`、`ipv6_auto_follows_the_network`、`a_follow_that_fails_keeps_the_enhanced_mode_on` | 三平台（本机跑过）；等网络变化与重载的几条在提交前循环跑过二十次 |
| 10（文档） | 没有用例；判据是 `git grep` 核对文档里引用的用例名与路径 | — |

### M4b（`docs/superpowers/plans/2026-10-26-phase3-m4b-loop-dns-plan.md`）

| 任务 | 点名了的用例 | 编译与运行 |
| ---- | ------------ | ---------- |
| 1（出口判定） | `nothing_is_bound_while_the_enhanced_mode_is_off`、`loopback_unspecified_multicast_link_local_and_broadcast_are_never_bound`、`a_directly_connected_address_goes_out_of_its_interface_before_the_routes_are_asked`、`the_longest_subnet_wins`、`an_address_in_the_routes_goes_out_of_the_primary_interface`、`without_a_primary_interface_it_is_no_route`、`an_excluded_address_is_left_to_the_system`、`subnets_of_leaves_out_loopback_link_local_the_excluded_and_a_zero_prefix`、`subnets_of_lists_each_subnet_once_longest_first` | 三平台（本机跑过） |
| 2（`open_socket`、`DirectPacket`、QUIC） | `open_socket_asks_egress_for_the_destination_and_binds_there`、`no_egress_no_binding`、`a_policy_interface_is_used_and_egress_is_not_asked`、`allow_other_interface_falls_back_to_egress`、`an_egress_error_fails_the_dial_and_nothing_reaches_the_server`、`a_binding_that_fails_fails_the_dial_with_no_interface_name_in_the_error`、`direct_packet_opens_a_socket_for_each_egress_and_hears_on_all`、`direct_packet_fails_a_datagram_whose_egress_is_an_error`、`bind_udp_passes_the_destination_to_egress`、`quic_direct_socket_asks_egress_for_the_server` | 三平台（本机跑过） |
| 3（接管期间的 DNS） | `in_the_enhanced_mode_system_and_syslib_ask_the_physical_servers`、`in_the_enhanced_mode_system_without_servers_fails_and_never_asks_the_library`、`force_syslib_asks_the_library_in_the_enhanced_mode_too`、`local_names_ask_the_library_in_the_enhanced_mode_too`、`a_single_label_asks_only_the_servers_in_the_enhanced_mode`、`a_resolver_without_servers_does_not_turn_to_the_library_in_the_enhanced_mode`、`reread_system_servers_takes_the_new_servers_and_keeps_the_cache`、`a_udp_upstream_asks_through_its_connector` | 三平台（本机跑过） |
| 4（引擎自己的名字解析） | `own_resolve_asks_the_library_outside_the_enhanced_mode`、`own_resolve_asks_the_generation_while_the_enhanced_mode_is_on`、`own_resolve_asks_the_library_before_any_generation_is_published`、`the_stack_s_connectors_carry_the_socket_hook`、`system_dns_changed_rereads_the_servers_and_keeps_the_cache` | 三平台（本机跑过） |
| 5（快照里的服务器） | `servers_in_rurges_space_are_left_out`、`a_snapshot_leaves_out_dns_servers_in_rurges_space` | 三平台（本机跑过） |
| 6（网络任务的排除名单、回调） | `platform_sockets_egress_follows_the_table`、`platform_system_dns_is_in_the_enhanced_mode_while_the_flag_is_up`、`opening_writes_the_egress_table_with_the_subnets_and_closing_clears_it`、`subnets_that_cannot_be_read_leave_rule_three_out_and_open_all_the_same`、`the_enhanced_flag_is_up_from_the_egress_table_until_it_is_cleared`、`the_virtual_adapter_is_left_out_of_the_network_reads_while_open`、`system_dns_changed_is_called_after_the_flag_the_takeover_and_the_giving_back`、`the_daemon_s_own_dns_and_downloads_keep_working_while_the_enhanced_mode_is_on` | 三平台（本机跑过） |
| 7（真实的 `Pinger`） | `request_and_reply_round_trip_for_both_families`、`the_icmpv4_checksum_is_right`、`a_reply_after_an_ipv4_header_is_read`、`anything_but_an_echo_reply_is_none`、`replies_go_to_the_waiter_of_their_sequence_number`、`a_dropped_echo_leaves_the_table`、`sequence_numbers_skip_the_ones_still_waited_for`、`pings_the_loopback`。计划点的 `a_raw_socket_matches_the_identifier_too` 按修正记录第 46 行改成了 `the_identifier_matters_where_the_socket_keeps_it` | 三平台（本机跑过）；`pings_the_loopback` 建不了套接字时打印原因跳过 |
| 8（外部程序） | `a_running_program_answers_runs_for_its_own_pid`、`a_stopped_program_runs_nothing`、`the_sessions_of_an_external_program_go_direct_whatever_the_rules`、`in_the_global_mode_too`、`no_wait_without_a_running_program` | 三平台（本机跑过）；`tests/external` 的三条自 M4d 第 45 行起不再带 `#[cfg(unix)]` |
| 9（漏网的检查） | `every_outbound_socket_is_made_in_one_place`、`every_kind_of_dns_upstream_asks_the_hook_before_it_dials`、`the_pipeline_fallback_asks_the_hook`、`resource_downloads_ask_the_hook`、`an_egress_error_stops_each_kind_before_anything_is_sent` | 三平台（本机跑过） |
| 10（文档） | 没有用例 | — |

### M4c（`docs/superpowers/plans/2026-10-26-phase3-m4c-linux-macos-plan.md`）

| 任务 | 点名了的用例 | 编译与运行 |
| ---- | ------------ | ---------- |
| 2（`exec` 与 Linux 的路由） | `a_script_is_written_to_a_file_of_its_own_and_removed_after`、`the_first_failing_step_ends_the_run`、`an_ipv6_failure_skips_the_other_ipv6_steps_and_goes_on`、`ignored_failures_go_on`、`a_script_that_cannot_be_written_fails_its_step`、`a_failing_script_leaves_its_path_out_of_the_error`、`leftover_scripts_are_removed`、`linux_adds_routes_and_never_replaces_them` | 三平台（本机跑过） |
| 3（`frame` 与 `System` 的约定） | `ifname_takes_up_to_fifteen_bytes`、`name_of_stops_at_the_first_nul`、`utun_header_follows_the_ip_version`、`utun_packet_strips_a_known_family`、`open_errors_say_what_is_missing`、`the_unsupported_system_says_so` | 三平台（本机跑过）；`open_errors_say_what_is_missing` 的说明里有 Linux 的权限提示 |
| 4（`LinuxSystem`） | `tools_are_looked_for_in_the_system_directories_in_order`、`without_ip_the_open_fails_before_the_device`、`the_stub_is_resolved_s_and_nothing_else`、`the_dns_is_taken_behind_the_stub_with_resolvectl`、`a_takeover_that_fails_partway_is_given_back`、`without_the_stub_the_dns_is_left_and_said_once`、`without_resolvectl_the_dns_is_left`、`giving_back_reverts_only_what_was_taken`、`strict_rp_filter_is_loosened_and_put_back`、`rp_filter_that_is_loose_or_off_is_left_alone`、`a_saved_rp_filter_is_not_read_again`、`an_interface_name_that_is_not_a_name_is_refused`、`rp_filter_content_that_is_not_0_1_or_2_is_refused`、`the_physical_servers_are_resolved_s_upstreams_without_rurges_space`、`without_the_takeover_the_system_list_stands`、`configure_with_ipv6_runs_two_batches_and_an_ipv6_failure_is_ok_false`、`an_ipv6_route_failure_takes_the_ipv6_routes_out_again`、`an_ipv4_route_failure_is_the_error`、`an_undo_of_another_platform_is_dropped` | 三平台（本机跑过）：`enhanced/linux.rs` 不带 `cfg`，只用假的执行器与临时目录 |
| 5（`MacSystem`） | `enabled_services_skip_the_heading_and_the_disabled`、`all_services_keep_the_disabled_without_the_asterisk`、`dns_servers_read_a_list_and_the_none_set_sentence`、`service_devices_pair_each_service_with_its_device`、`dhcp_dns_reads_domain_name_server`、`the_dns_of_each_enabled_service_is_read_before_it_is_set`、`services_saved_already_are_not_read_again_but_set_again`、`a_service_that_cannot_be_read_is_left_alone`、`no_service_to_take_is_ok_false_and_said_once`、`setting_fails_with_a_text_that_names_no_service`、`undo_writes_back_the_servers_or_empty_and_flushes`、`undo_of_a_service_that_is_gone_is_done`、`an_undo_that_cannot_list_the_services_stays`、`an_undo_of_another_platform_is_dropped`、`physical_at_open_is_resolv_conf_before_the_takeover`、`physical_after_a_change_is_the_saved_servers_or_dhcp`、`physical_without_a_primary_is_empty`（另有 `a_saved_responder_is_no_physical_server`、`the_responder_is_never_a_physical_server`、`a_list_that_is_empty_after_rurges_space_is_unknown`） | 三平台（本机跑过）：`enhanced/macos.rs` 不带 `cfg` |
| 6（Linux 的设备） | `every_unsafe_exception_is_on_the_list`（`crates/rurge-platform/tests/unsafe_functions.rs`） | 三平台（本机跑过）。设备本身没有缺省用例（P19）；`tun/linux.rs` 与 `set_iff` 这台开发机编译不到 |
| 7（macOS 的设备） | 同任务 6 | 同任务 6；`tun/macos.rs` 与 `utun_connect` 这台开发机编译不到 |
| 8（后端接上平台） | `undo_and_saved_map_both_ways`、`a_test_undo_on_the_platform_backend_is_dropped`、`the_physical_servers_are_set_after_the_takeover_and_cleared_on_close`、`a_change_of_network_reads_the_physical_servers_again`、`a_reload_does_not_read_them`、`the_file_backend_gives_its_physical_servers` | 三平台（本机跑过） |
| 9（L2 特权用例） | `tests/privileged/tests/linux_netns.rs`：`a_tcp_round_trip_and_a_ping_go_through_the_adapter_and_out_direct`、`rurges_own_dns_queries_do_not_enter_the_adapter`、`a_killed_run_is_undone_by_the_next_start` | L2（缺省跳过）：只在 Linux 编译，没有在 Linux 上运行过，**CI 还没有见过** |
| 11（文档与终审修正） | 没有新用例；修正里改到的用例见四份计划的修正记录 | — |

### M4d（`docs/superpowers/plans/2026-10-26-phase3-m4d-windows-wrapup-plan.md`）

| 任务 | 点名了的用例 | 编译与运行 |
| ---- | ------------ | ---------- |
| 1（`unicast_if` 与 `in_job`） | `unicast_if_on_a_loopback_socket_still_reaches_the_loopback`、`unicast_if_on_a_loopback_socket_still_reaches_the_loopback_over_ipv6`、`a_child_in_the_job_is_contained_and_this_process_is_not`、`an_exited_child_whose_handle_is_still_open_is_not_contained`、`a_process_that_is_gone_is_not_contained`；端到端 `what_the_program_started_goes_direct_too`（`tests/external/tests/engine.rs`，第 45 行起三平台都跑） | 只在 Windows（本机跑过）；端到端那条三平台 |
| 2（读回的解析与 `configure` 的翻译） | `routes_are_read_from_the_english_table`、`routes_are_read_whatever_the_language`、`a_prefix_with_an_embedded_ipv4_address_is_read`、`lines_without_a_prefix_and_a_number_are_skipped`（另有 `addresses_are_read_from_the_english_output`、`addresses_are_read_whatever_the_language`、`an_address_with_a_zone_is_read_without_it`、`the_mtu_is_the_first_number_of_the_first_row`、`the_readback_commands_are_one_netsh_call_each`、`the_adapter_comes_up_in_one_script`、`ipv6_is_a_script_of_its_own`） | 三平台（本机跑过）：`route/windows.rs` 不带 `cfg` |
| 3（`wintun.dll` 与读线程） | 读线程（三平台）：`packets_are_passed_on_in_order`、`an_empty_ring_is_waited_on`、`an_error_of_the_ring_is_passed_on_and_ends_the_loop`、`stop_ends_the_loop_within_one_wait`、`a_closed_receiver_ends_a_loop_blocked_on_a_full_channel`。加载（只在 Windows）：`an_unsigned_file_is_refused`、`a_missing_file_says_where_to_get_it`、`a_dll_without_the_wintun_entry_points_is_refused`、`a_file_that_is_not_a_dll_cannot_be_loaded`、`a_file_that_is_held_open_cannot_be_opened`、`a_real_wintun_dll_passes` | 读线程三平台（本机跑过，计时用例循环跑过二十次）；加载只在 Windows（本机跑过）。`a_real_wintun_dll_passes` 要有 `RURGE_TEST_WINTUN_DLL`，缺省打印原因、什么也不做——这台机器上没有 `wintun.dll`，所以**签名者一次也没有被读过** |
| 4（`WindowsSystem`） | `without_wintun_dll_the_open_fails_before_the_device`、`configure_runs_the_script_then_reads_back_the_address_and_the_mtu`、`a_missing_address_fails_configure`、`an_mtu_that_is_not_1500_fails_configure`、`an_ipv6_address_that_is_missing_is_ok_false_and_taken_out_again`、`routes_are_read_back_by_the_adapter_s_index`、`missing_ipv4_routes_take_the_added_ones_out_and_fail`、`missing_ipv6_routes_are_ok_false`、`the_dns_is_read_back`、`a_missing_ipv6_dns_is_one_warn_and_still_ok`、`a_dns_that_did_not_change_fails_take_dns`、`netsh_s_exit_status_does_not_decide`、`the_windows_system_never_saves_and_drops_foreign_undos`、`physical_dns_is_the_system_s_own` | 三平台（本机跑过）：`enhanced/windows.rs` 不带 `cfg` |
| 5（设备与读线程接上） | 设备本身没有缺省用例（P19）；这一个任务点的是判据，不是用例。修正第 1 轮加的 `a_packet_longer_than_wintun_takes_is_refused`（`crates/rurge-platform/src/tun/windows/wintun.rs`，第 73 行）算在这里：它只测发包长度的上限，不碰 Wintun（只在 Windows） | 设备与十个放开的函数在这台机器上编译过，没有一条用例走到它们（记录第 70 行）；`a_packet_longer_than_wintun_takes_is_refused` 只在 Windows、本机跑过；真机上第一次运行是手工清单 |
| 6（`--wintun-dll`） | `wintun_dll_is_only_for_windows`（`crates/rurge/src/cli/run.rs`，三平台）；`the_wintun_dll_option_is_a_usage_error_elsewhere`（`crates/rurge/tests/cli.rs`，Windows 上只编译、不运行） | 前者三平台（本机跑过）；后者**只在非 Windows 上运行（Windows 上编译）——这台开发机只编译、不运行，CI 还没有见过** |
| 7（这一份核对） | 没有新用例 | — |
| 8、9 | 还没有做 | — |

## 三、计划点了名、代码里却没有这个名字的五处

逐个核过，没有一处是真的缺一条用例：

| 计划里的名字 | 出处 | 实际 |
| ------------ | ---- | ---- |
| `holds_a_backup` | M4a 任务 7 第 8 条（「`holds_a_backup` 改成两种备份任一个在就为真（名字照实）」） | 计划自己写了「名字照实」：它在代码里叫 `left_to_undo`（`crates/rurge/src/cli/run.rs:1231`）与 `Left::any`（`:1225`），是生产代码不是用例。行为由 `what_a_dead_run_left_is_undone_by_the_next_start` 与 `what_a_dead_run_left_is_undone_even_when_the_profile_does_not_load` 端到端覆盖 |
| `the_platform_backend_is_not_available_in_this_version` | M4a 任务 7 的用例一栏 | M4c 任务 8 删掉了它（M4c 计划 P19 与修正记录第 30 行）：它建的是真平台的 `System` 并调 `open()`，M4c 之后那会真的开设备。取代它的是一条也不开设备的 `the_unsupported_system_says_so`（构造 `Unsupported`，不经 `system()`） |
| `outside_the_enhanced_mode_system_syslib_and_single_labels_are_as_before` | M4b 任务 3 的用例一栏第 7 条 | M4b 修正记录第 27 行：不新写，由现有的 `the_three_system_values_of_host_are_all_the_library`、`a_simple_host_name_goes_to_the_system_servers_with_the_search_domain`、`a_local_name_is_asked_of_the_library_and_not_of_the_servers` 等覆盖（见一、2 的 `taken_over` 那一行） |
| `the_network_is_read_without_the_excluded_interfaces` | M4b 任务 5 的用例一栏 | M4b 修正记录第 31 行：单元用例里观察不到（文件后端不看 `ReadOpts`），改写成管理器的 `the_virtual_adapter_is_left_out_of_the_network_reads_while_open` |
| `a_raw_socket_matches_the_identifier_too` | M4b 任务 7 的用例一栏 | M4b 修正记录第 46 行（协调者的裁定 R22）：`the_identifier_matters_where_the_socket_keeps_it` 把两种规则都测到 |

## 四、P19：`TunDevice::open` 与 `System::open` 没有被任何用例调用

```
$ git grep -n "TunDevice::open\|System::open" -- crates tests
crates/rurge-platform/src/enhanced/linux.rs:147:        TunDevice::open(&DeviceSpec {
crates/rurge-platform/src/enhanced/macos.rs:255:        TunDevice::open(&DeviceSpec {
crates/rurge-platform/src/enhanced/windows.rs:239:    /// device and its interface index come from [`TunDevice::open`].
crates/rurge-platform/src/enhanced/windows.rs:246:        TunDevice::open(&DeviceSpec {
crates/rurge-platform/src/tun/mod.rs:36://! `TunDevice::open` is what the platform's `System::open` calls, and no test
crates/rurge-platform/src/tun/mod.rs:106:    /// `System::open` calls it, and no test does (M4c plan P19).
```

六行，没有一行在用例里：三处是三个平台 `System::open` 的生产实现（Linux `:147`、macOS `:255`、Windows `:246`），一处是 Windows 的说明（`:239`），两处是 `tun/mod.rs` 的说明（`:36`、`:106`）。`tests/` 下一处也没有。

```
$ git grep -n "\.open()" -- crates/rurge-platform/src/enhanced
crates/rurge-platform/src/enhanced/linux.rs:514:        let error = match system.open() {
crates/rurge-platform/src/enhanced/mod.rs:260:        let Err(error) = system.open() else {
crates/rurge-platform/src/enhanced/windows.rs:559:        let error = match system.open() {
```

三处都在用例里，逐个看过：

- `enhanced/windows.rs:559`（`without_wintun_dll_the_open_fails_before_the_device`）：`dll` 是 `Err`，`open` 在设备之前就失败。**没有用例在 `dll` 是 `Ok` 时调 Windows 的 `open`。**
- `enhanced/mod.rs:260`（`the_unsupported_system_says_so`）：对象是 `Unsupported`，用例自己造的，不经 `system()`。
- `enhanced/linux.rs:514`（`without_ip_the_open_fails_before_the_device`）：这个**调了** `LinuxSystem::open`——与前一条的说法不完全一样，照实记在这里。它给的是一个临时目录当工具根，`ip` 找不到，`open` 回那句错误就结束；用例断言了错误文本，并在真的打开设备时 `panic!("a device was opened where ip is missing")`，还断言执行器一次也没有被调。所以走到 `TunDevice::open` 的前提是「`LinuxTools::find` 找得到 `ip`」，在临时目录里不会有。
- macOS 的 `open` 一处也没有用例调。

另外，真实平台的 `enhanced::system()` 只有生产代码调（`crates/rurge/src/cli/enhanced/backend.rs:107`），`tests/` 下没有调用；`backend.rs` 的后端用例用的是 `Unsupported`。

## 五、数

把一、与二、两节表格里每个反引号括起来的名字，对着代码里带 `#[test]` / `#[tokio::test]` / `#[cfg_attr(not(windows), test)]` 的定义去重之后数：

| | 条数 |
| - | ---- |
| 名单上的用例（去重） | **280** |
| 本机跑过——三平台的 264 条加只在 Windows 的 12 条 | **276** |
| 只在非 Windows 运行（Windows 上只编译，**CI 还没有见过**） | **1**：`the_wintun_dll_option_is_a_usage_error_elsewhere` |
| L2（只在 Linux 编译，缺省跳过，任何地方都没有运行过） | **3** |
| 缺 | **0** |

两条照实的限制：

- `a_real_wintun_dll_passes` 在这台机器上算进「本机跑过」，但它缺省只是打印一句 `RURGE_TEST_WINTUN_DLL is not set` 就返回——签名者与加载器在这台机器上一次也没有真的跑过。
- `pings_the_loopback` 同理：开不了 ICMP 套接字就打印原因跳过，本机那一次有没有真的收发没有单独记。

统计的脚本与中间结果在 git 忽略的工作区（`.superpowers/sdd/2026-10-26-phase3-m4d-windows-wrapup-plan/`）里，不随仓库走。