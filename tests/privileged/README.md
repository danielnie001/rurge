# rurge-privileged-tests

`rurge-privileged-tests` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它装的是增强模式的 L2 用例（设计第 14 节、M4-D27；M4c 计划 P17）：真的虚拟网卡、真的路由、真的 `rp_filter`。

## 它们做什么

`tests/linux_netns.rs` 每个用例先自己建两个网络命名空间：`rl2-a-<pid>` 当作"本机"，`rl2-b-<pid>` 当作"互联网"。两个命名空间之间是一对 veth，`10.231.0.2/30` 在 a 这一端、`10.231.0.1/30` 在 b 那一端；a 的默认路由经 `10.231.0.1`。b 的回环上另有 `10.231.1.1/32`，它就是"互联网上的服务器"：它不在 a 的任何直连路由里，但在 rurge 装进虚拟网卡的基础前缀（`8.0.0.0/5` 等）里，所以应用到它的流量必然进虚拟网卡，而 rurge 自己的连接只要绑在 veth 上就绕得开。`rurge` 经 `ip netns exec rl2-a-<pid>` 起在 a 里，`--data-dir` 是临时目录，`http-api` 在 a 的回环上。

三条用例：

- `a_tcp_round_trip_and_a_ping_go_through_the_adapter_and_out_direct`：a 里一次 TCP 往返与一次 `ping` 到 b，都进虚拟网卡、以 `DIRECT` 出去；请求记录里有一条到那个服务端、策略是 `DIRECT` 的会话。
- `rurges_own_dns_queries_do_not_enter_the_adapter`：a 里问应答器（`198.18.0.2`）一个名字，答案来自 b 上的 DNS 服务端；b 看到的来源是 a 的 veth 地址（`10.231.0.2`），请求记录里没有到那个服务端的 UDP 会话——rurge 自己的查询经 veth 出去，没有绕回虚拟网卡。
- `a_killed_run_is_undone_by_the_next_start`：a 里把 `net.ipv4.conf.all.rp_filter` 设成 1，增强模式打开之后读到 2；`kill -9` 之后 a 里不再有 `rurge` 网卡；下一次不带 `--enhanced-mode` 的启动说它把上一次留下的改动撤销了，`rp_filter` 回到 1，`state.json` 里没有 `enhanced_backup`。

`l2-helper`（`src/bin/l2-helper.rs`）是用例在两个命名空间里起的小程序，有五种用法：`echo <地址>`（TCP 回显）、`dns <地址> <名字> <IPv4>`（只答一个名字的 A 记录，并把每个查询的来源写到标准输出）、`connect <地址>`、`query <服务器> <名字>`、`get <URL> <密钥>`。它只用 std；DNS 报文的编码与解码在这个包的库里（`src/dns.rs`），在三个平台上都有单元用例。

## 怎么跑

只在一次性的 Linux 虚拟机里、以 root 跑。需要 `iproute2`、`ping`、`/dev/net/tun`，以及一个能用的账号（不用 systemd-resolved）：

```bash
cargo build -p rurge
RURGE_TEST_PRIVILEGED=1 cargo test -p rurge-privileged-tests
```

`rurge` 二进制先取 `RURGE_TEST_RURGE`；没有时取测试二进制旁边的那一个（`target/<profile>/rurge`），所以先 `cargo build -p rurge`。没有 `RURGE_TEST_PRIVILEGED=1` 时每条用例打印一行 `skipped: set RURGE_TEST_PRIVILEGED=1 in a disposable Linux VM, as root` 并返回，什么也不建。

## 它们改什么

只改用例自己建的两个命名空间里的东西：命名空间、veth、地址、路由、`rp_filter`、`rurge` 网卡。宿主上看不到其中任何一样。

两处例外，都在 a 的 `/etc/` 与宿主之间：

- `Lab` 在起 `rurge` 之前写 `/etc/netns/rl2-a-<pid>/resolv.conf`（一行 `nameserver 10.231.1.1`），用例结束时删掉 `/etc/netns/rl2-a-<pid>/` 整个目录（用例结束的清理里做，失败只打印到标准错误）。`ip netns exec` 会把 `/etc/netns/<名字>/` 下的文件绑到 `/etc/` 上，所以 a 里的 `rurge` 看到的是这份 `resolv.conf`，不是宿主的那份。不写它的话，在 resolved 的存根是缺省设置的虚拟机里，a 里的 `rurge` 会经 D-Bus 去对**宿主**的 resolved 执行 `resolvectl dns rurge …`，宿主看不到 a 里的网卡，接管失败、增强模式打不开。
- 这两个命名空间本身（`ip netns del` 删掉）。

用例从不改宿主的网卡、路由、DNS 或 `rp_filter`；也不需要宿主有网。

## 状态

**这些用例写好之后没有在任何地方运行过。** 在 CI 上启用、macOS 与 Windows 的冒烟是项目所有者的事（总设计 Q4）。开发机不是 Linux，`tests/linux_netns.rs` 在它上面是空的（0 条用例），`l2-helper` 能编译；`src/dns.rs` 的六条单元用例在三个平台上都跑。