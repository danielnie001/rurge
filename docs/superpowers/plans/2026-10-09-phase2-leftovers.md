# 阶段 2 遗留事项总表

阶段 2 的 20 份计划（M1a ～ M7c）各有一张「延后事项」表，共 403 行。本表给每一行写明最后的结局和依据，让事情不会在里程碑之间丢掉：一行被送给"M5"，要看 M5 是不是真的接了。读法：「小计」给各结局的数目，「阶段 2 结束时仍开着的事项」按去向列出所有 `移交` 的行，「逐份计划」是逐行的明细（`#` 沿用各计划自己的行号；M1a、M1b、M2a、M2b 的表原来没有编号，按出现顺序编号）。本表收的是这 20 份计划的「延后事项」记下的事，不是阶段 1 与阶段 2 没做完的一切；没有任何一份计划记下的不在这里——例如阶段 1 的托管配置更新与远程 `#!include` 从没做过，M8c 的需求审计（逐句对照代码）又查出一批——，它们的行在 M8c 计划「延后事项」的第 100 ～ 105 行。

各行的结局先核对到提交 `28340f3`（M8c 的代码冻结点：交给整分支终审的那一版；任务 12 的文档提交在它之上）；终审之后的最终修正波（九个提交，到 `087f399`）碰到的几行，由任务 12 A 部分的补充补核，写在下面「终审之后」一段。三件事要先知道。第一，没有推送过：`origin/main` 仍是 `0154e73`，那次 CI 运行之后的所有提交都没有推送，CI 没有在 M8 的任何提交上跑过；M8a 任务 6（CI 首跑的修正）要等推送之后读 CI 的失败清单，还没有开始，所以结局是 `移交：CI 首跑` 的行都还开着。第二，互操作用例在本机没有参照服务端，全部跳过，从未对真实二进制跑过；依据里写「M8c 做掉」的互操作夹具改动也只能这样说。第三，M8c 自己的「延后事项」不在本表里，在 M8c 计划（`docs/superpowers/plans/2026-10-09-phase2-m8c-leftovers-acceptance-plan.md`）的同名一节里。

草稿写成时，依赖任务 4、任务 6 的修正轮和任务 8 ～ 13 的行还不能确认（任务 5 ～ 7 已经执行）；核对时改了三行的结局：M1a #20（移交：其它 → M8 做掉：两条计时类用例都已改掉）、M3c #1（移交：有用户报告再说 → 关闭：登记为已知限制，不改）、M5a #20（移交：下次动到时 → M8 做掉：任务 10 的修正轮把 TCP 客户端自己的地址也归成 IPv4）。其余依赖 M8c 任务的行结局不变，依据补上了提交与名字。

终审之后：最终修正波补核了它碰到的行，结局改了一行、依据改了几行。结局：M5c #9（按来源过滤回包时精确比较 `Target`）由 `移交：有用户报告再说` 改为 `M8 做掉`——`packet_datagram` 的接收任务与 `udp_probe` 比较来源时两侧先归一，`from_relay` 随后同样（`8e581e7`、`b14b45a`）——；小计与开着的清单随之重数：M8 做掉 63 → 64，移交 187 → 186，有用户报告再说 98 → 97。依据改了的：M2b #1（看门狗没有对真实服务端验证过）、M3a #20（又有 `cf8df9a`）、M3a #33（URL 任务每分钟的醒来恢复了）、M3b #20（`generation_lock` 的说法）、M5a #20 与 M7b #8（归一的地方，「唯一的实现」的措辞）、M3a #23（资源管理器的显示）、M8a #3（`UnknownKey` 的 `Debug`）、M2b #29、M4a #11、M7c #15（评审指出的措辞与数字）。

## 结局的写法

每一行有且只有一个结局：

- `已做掉`：在 M8 之前就了结——在它自己里程碑的终审修正里，或被后面某个里程碑（M1b ～ M7c）接手做完，或只是"登记进文档"而文档里已有。
- `M8 做掉`：由 M8a、M8b 或 M8c 做掉，依据里写任务号；M8 设计 3.5 以查明来源了结的偶发异常退出也算在内（没有代码改动）。
- `关闭`：有意不做，也没有人在等它：该行自己的去向写着不处理 / 保持现状 / 接受，或 M8 设计第 7 节的关闭表点了名。
- `移交`：阶段 2 结束后仍开着，冒号后的标签说交给谁、等什么：
  - `阶段 3`：等阶段 3；写了更后的阶段时照写（`阶段 3 之后`、`阶段 4`）。
  - `有用户报告再说`：已知的限制，有人被它咬到再处理。
  - `项目所有者`：等项目所有者表态或提供。
  - `CI 首跑`：等第一次转绿的 CI 运行（M8a 任务 6）。
  - `依赖升级时`：等某个依赖或工具链升级时重新评估。
  - `手工验收`：写在 `docs/acceptance/phase2-manual.md` 里，由人在真实环境验收。
  - `下次动到时`：代码里留着一处小整理，下一次动那个文件的人顺手做。
  - `其它`：冒号后写明条件。

M8 设计第 7 节用"动到那些文件时再整理"作理由关闭了五行（M4b #26、#27，M7c #18、#19、#21），意思与 `移交：下次动到时` 相同；表里照设计的写法保持为 `关闭`。另一个例外是 M3c #1（组环检测把 `smart` 组里写的嵌套组当作边）：它原来的去向写的是"有用户报告再说"，按上面的写法该是 `移交：有用户报告再说`；M8c 执行中裁定它登记为已知限制、不改（有绕开的办法：把那个本来就不起作用的嵌套组从 `smart` 组里删掉），所以表里记为 `关闭`。小计里的关闭 73 / 移交 186 因此比按上面的写法数多一行关闭、少一行移交。

## 小计

| 计划 | 已做掉 | M8 做掉 | 关闭 | 移交 | 合计 |
| ---- | ------ | ------- | ---- | ---- | ---- |
| M1a | 17 | 1 | 7 | 6 | 31 |
| M1b | 11 | 1 | 12 | 9 | 33 |
| M2a | 4 | 1 | 8 | 1 | 14 |
| M2b | 12 | 6 | 9 | 5 | 32 |
| M2c | 1 | 4 | 0 | 10 | 15 |
| M3a | 10 | 9 | 3 | 12 | 34 |
| M3b | 3 | 6 | 0 | 14 | 23 |
| M3c | 1 | 2 | 1 | 8 | 12 |
| M4a | 2 | 3 | 0 | 10 | 15 |
| M4b | 3 | 6 | 4 | 18 | 31 |
| M4c | 0 | 0 | 0 | 9 | 9 |
| M5a | 2 | 1 | 0 | 18 | 21 |
| M5b | 2 | 0 | 2 | 10 | 14 |
| M5c | 1 | 2 | 2 | 14 | 19 |
| M6a | 2 | 2 | 2 | 9 | 15 |
| M6b | 0 | 0 | 3 | 6 | 9 |
| M6c | 1 | 1 | 2 | 7 | 11 |
| M7a | 5 | 2 | 2 | 4 | 13 |
| M7b | 2 | 2 | 6 | 6 | 16 |
| M7c | 1 | 15 | 10 | 10 | 36 |
| 合计 | 80 | 64 | 73 | 186 | 403 |

## 阶段 2 结束时仍开着的事项

20 份计划里结局是 `移交` 的 186 行，按标签分组，一行一项；括号里是标签后面的补充。

### 项目所有者（2）

- M1b #3 — `/v1/policies/detail`、`/v1/policy_groups` 的真实响应形状（M1 设计 O2）
- M6b #1 — `version` 1–3 与 6、v5 的 QUIC Proxy Mode

### CI 首跑（20）

- M1a #3 — Linux / macOS 的 `bind_interface`、`set_tclass_v6` 分支本机编不了
- M1b #1 — 互操作用例与 CI 里装 sing-box 的步骤本机跑不了
- M1b #9 — Linux / macOS 平台分支（含网卡表缓存）本机编不了
- M1b #26 — sing-box 1.14.1 是否接受夹具配置；三个 SHA-256；CI 步骤在真实 runner 上
- M2a #11 — sing-box 的 trojan 互操作本机跑不了
- M2b #25 — xray 夹具客户端只有 `id`，真实 xray 是否接受未确认
- M2b #31 — "anytls 鉴权必须一次写出"的唯一守卫是 CI 运行
- M2c #2 — 首次推送后盯互操作 job 的 Shadow TLS 用例（三项）
- M4a #4 — 对 `sshd` 的互操作只在 CI 上运行
- M4a #13 — CI 里装 `openssh-server` 的 apt 无重试
- M4b #4 — 对 sing-box WireGuard 端点的互操作只在 CI 上跑
- M4b #30 — Linux 上 UDP 缓冲封顶在 `rmem_max`
- M4c #4 — Unix 分支只在 CI 上编译与运行
- M6a #13 — 手工验收的时钟偏差文本；`ssserver` 互操作配置只在 CI 上验证
- M6b #4 — 互操作数不了复用的连接数；snell-server 是否默认开 UDP
- M6b #9 — `reuse=false` 时应用半关闭后还发 FIN
- M6c #3 — `h2-connect` 对 endpoint 不发 `user-agent` 是否可行
- M7b #6 — tuic 0.8.5 的命令行、配置键、就绪日志未核对
- M7b #7 — v5 线上细节只对回环假服务端验证
- M7c #1 — 新的互操作用例没有对真实二进制跑过

### 手工验收（8）

- M1b #2 — `socks5-tls` 没有参考实现可对测
- M3a #24 — `include-other-group` 取未派生成员，绕过中继
- M6a #2 — obfs 没有参考服务端，只能手工验收
- M6a #14 — `obfs=tls` 的 ClientHello 至多约 16 KiB
- M6c #2 — CONNECT-UDP over HTTP/2 没有参考服务端
- M7a #6 — `gecko-password` 是否等于公开规范里的 Gecko
- M7a #7 — 端口跳跃对真实服务端的互操作
- M7b #2 — `quic` 模式没有参考服务端

### 阶段 3（8）

- M1b #30 — `SessionInfo::url` 作"明文 HTTP 转发"信号的契约（阶段 4）
- M2b #2 — （计划期）Unicode 写的代理服务器名不可用（P15）（阶段 3 之后）
- M3b #5 — "网络已变化"没有来源
- M3b #11 — `invalidate_all` 管不到正在跑的测试
- M4b #3 — "网络已变化"的探测器
- M4b #19 — `network_changed()` 用 `try_lock`，事件会被丢
- M4c #5 — `external` 的 `udp-relay`（M5）、`addresses` 与「外部进程的流量走 DIRECT」（阶段 3）
- M5a #19 — 网卡绑定的载体在地址变化后不自愈

### 依赖升级时（3）

- M1a #1 — `tfo` 三平台不生效
- M2c #10 — 自检 3、4 没有失败路径的测试
- M4b #1 — smoltcp 0.12 的 TCP 限制

### 有用户报告再说（97）

- M1a #16 — `decode_p12` 的无私钥 / 链为空 / 无填充 Base64 分支没有用例
- M1a #30 — `BootstrapConnector` 不能表达 `prefer_v6`
- M2c #3 — `vmess`（无 TLS）+ Shadow TLS v2 首帧等 100 ms
- M2c #4 — HelloRetryRequest：伪装握手只提供 X25519
- M2c #7 — 挂起的写与重试之间插入 flush 会让帧发两遍
- M2c #8 — 无用例覆盖零长度的 v3 ApplicationData 记录
- M2c #11 — 握手期部分验证通过又被当伪装流量的 v3 握手直接失败
- M3a #1 — `[Host]` 豁免名单不含订阅节点的主机名
- M3a #6 — 每次重建把装配告警逐条再 WARN
- M3a #7 — 每次重建重新解析全部订阅
- M3a #10 — 两组导入同名策略时命名空间先于校验裁决
- M3a #15 — 订阅重建占住工作线程
- M3a #16 — `W0030` 只点名回边两端
- M3a #17 — 坏的 `external-policy-modifier` 每行一条 `W0023`
- M3a #18 — `derive` 线性查名；派生名撞车
- M3a #21 — 1 秒内多次更新只重建一次，无用例
- M3a #32 — 单行长度与本地文件大小无上限
- M3a #34 — 导入行的 `W0023` 无数量上限
- M3b #1 — 测试会话不计字节
- M3b #2 — `load-balance` 的 `persistent` 哈希用 `DefaultHasher`
- M3b #3 — `evaluate-before-use` 等待上限不计别组占用
- M3b #4 — 链中间跳是 `load-balance` 时 `persistent` 退化
- M3b #8 — `policy_names` 无数量上限
- M3b #12 — `dial_internal` 被取消时会话句柄不结束
- M3b #13 — "服务端不保持连接"只告警一次的分支无用例
- M3b #15 — `TestBook` 不清理已消失策略的结果
- M3b #16 — 已移除的组仍记下一轮测试时间
- M3b #17 — 嵌套 `load-balance` 外层打分不看覆盖
- M3b #22 — 策略 `test-url` 只按前缀校验
- M3c #2 — 改 `test-url` / `test-timeout` 清零 `SmartBook` 记录
- M3c #3 — DNS 会话不判断"上游先断"
- M3c #5 — 站点按目标主机名原样区分
- M3c #7 — 重载后迟到的回报仍写进 `SmartBook`
- M3c #8 — 改定义后站点记忆仍是旧结果
- M3c #9 — 拨号路径的开销（记录锁、重试列表克隆）（或性能数据）
- M3c #12 — 上传中上游先重置，不计入成员
- M4a #2 — 无 `server-fingerprint` 的告警按出站对象计一次
- M4a #3 — 老服务器的 `ssh-rsa`；DSA / 带口令私钥
- M4a #6 — russh 应答队列随被拒的通道增长
- M4a #8 — 无 RST 的断线要等保活判定
- M4a #9 — 口令只走 `password` 方法
- M4a #10 — `server-fingerprint` 一项后带注释是 `E0018`
- M4a #12 — `smart` 给首个成员的时间与 SSH 握手
- M4b #5 — 每包开销以 UDP 系统调用为主；无批量收发
- M4b #9 — 同名 `[WireGuard <name>]`：首节有错时后一个也被丢
- M4b #10 — 连接错误一律报 `cannot be connected to`，端口 0 同
- M4b #11 — 最后一个连接丢弃后 FIN / RST 随设备任务结束而发不出
- M4b #13 — 启动要等每个 peer 的载体拨完
- M4b #16 — 缺用例：`set_tos` 失败后不再标记、`recv_any` 轮换
- M4b #17 — SERVFAIL / REFUSED 的回答要等满一轮
- M4b #18 — 缺用例：P4 响应码规则、`system` 失败后继续、IPv6 服务器
- M4b #21 — 缺用例：P13 的日志规则
- M4b #22 — 共用隧道以启动它的策略名记日志
- M4b #25 — bin 的 `Targets` 过滤把日志最大级别抬到 TRACE
- M4b #29 — 缺引擎用例：原生测速策略带 `url` 的 `POST /v1/policies/test`
- M4b #31 — `established()` 不看隧道是否被接替；只等任一 peer 的握手
- M4c #1 — Unix 上 rurge 崩溃后留下外部程序
- M4c #3 — Unix 上进程组号被复用时可能误发信号
- M4c #6 — 持续崩溃的程序每 2 秒被拉起一次，无更长的退避
- M4c #7 — 端口登记的两个少见窗口
- M4c #8 — 最后的 `stop_external_programs()` 期间再按 Ctrl-C 不立即退出
- M5a #1 — UDP 流的 STUN 与 DNS 嗅探
- M5a #4 — UDP 流不计入 REJECT 的自动升级
- M5a #5 — `DirectPacket::recv_from` 先轮询 IPv4 再 IPv6
- M5a #6 — 声明端口的严格程度；BND 丢 IPv6 scope id
- M5a #10 — 持续使用的 DNS 流每 10 秒回收一次
- M5a #11 — 入站关联端口的接收缓冲用系统默认值
- M5a #16 — DIRECT 的 UDP 对 IP 字面量也套用 `ip-version`
- M5a #18 — 端口 53 的流是载体上唯一的流时，回收连带关掉载体
- M5b #1 — vmess 的 XUDP（全锥）
- M5b #3 — vmess 请求记录的 `connectMs` 记的是载体就绪的时刻
- M5b #4 — Shadow TLS 上的 UDP 没有单独的用例
- M5b #6 — `StreamUdp::recv_from` 每个数据报分配小缓冲
- M5b #7 — anytls 的 UDP 载体：三处用例与假服务端的缺口
- M5b #8 — vmess 连接没回应就结束时下一个数据报重新拨号
- M5b #9 — 互操作用例的 `send_to` 没套超时；kill 打断不了首包的拨号
- M5b #12 — `LazyHead` 读写两半在两个任务里轮询会丢唤醒
- M5c #1 — `dns-follow-interface` 不跟随加密 DNS
- M5c #3 — UDP 测试的结果不保存、不进 `test_results`、不进请求记录
- M5c #4 — 隧道里 UDP socket 的缓冲固定 64 包 / 256 KiB
- M5c #11 — 换载体：`connect_udp` 超过 10 秒时可能同时两次拨号；换过载体后连不上的 peer 每 10 秒重拨
- M5c #13 — UDP 测试：无 `test_case` 的策略没有 UDP 结果等三处
- M5c #14 — `smart` 的 UDP：载体打不开的任何错误都算成员失败等
- M5c #16 — 经组中继的 UDP 载体不跟随组的切换（只有 `wireguard` 跟随）
- M5c #18 — WireGuard 设备一批收 256 个包，隧道里 UDP socket 只有 64 个包的缓冲
- M6a #11 — SS 2022 UDP 的淘汰、派生、来源过滤
- M6b #3 — `obfs-host` 缺省、命令字与 Surge 的差异；`udp-port` 只解析
- M6c #5 — 挑中连接后、发出前连接死掉不重试
- M6c #7 — 通告 `MAX_CONCURRENT_STREAMS = 0` 的服务端
- M6c #8 — `h2` 0.4.19 的 `data_frame_budget` 与 ENHANCE_YOUR_CALM
- M7a #3 — 认证或握手反复失败没有退避
- M7a #12 — 被淘汰的连接最后一个租约放手时立刻关闭
- M7b #5 — 认证或握手反复失败没有退避
- M7c #3 — 服务端不收 QUIC 数据报时 MASQUE 的 UDP 失败
- M7c #16 — CONNECT-UDP 流与 TCP 隧道共用连接的流额
- M7c #17 — 被放弃的内部 DNS 拨号不留记录、不向 `smart` 回报
- M7c #26 — 握手之后载体失效时的错误文本是固定的一句

### 下次动到时（37）

- M1a #18 — 两个假上游各有一份 accept 循环
- M1a #25 — `http` 出站 CONNECT 写无 EOF / reset 归一；两出站的测试辅助函数近乎相同
- M1b #8 — 出站测试辅助函数重复；假上游 accept 循环；`http` CONNECT 写无归一
- M1b #18 — 拒绝文本在 engine 与 proto 各写一遍；隧道用例注释；丢头日志
- M1b #25 — 互操作层的防抖小项
- M2b #19 — `environment()` 是人工维护的约定
- M2b #21 — 证书指纹转十六进制的五行出现两次
- M2c #5 — M2b 延后事项里不属于 M2c 的条目继续有效
- M2c #12 — 假服务端 `upwards` 提前返回漏 `stop.notify_one()`
- M2c #14 — 测试专用 `FlushGated::poll_shutdown` 不先写出挂起的缓冲
- M3b #23 — 探针、选法、引擎、API 的测试缺口
- M3c #10 — 只靠读代码核对、没有用例的行为
- M4a #14 — 测试覆盖缺口
- M4c #9 — 任务评审记下的其余 Minor（`local-port=+80` 被接受等五条）
- M5a #7 — 刚交出的载体随即死掉时新流失败（其余已解决）
- M5a #8 — 载体表 `Carriers` 的条目在关联存续期间不删
- M5a #12 — `ChainConnector::open_udp` 与 `connect_tcp` 前导重复；几个分支缺单元测试
- M5a #13 — 用例的缺口（嗅探反例等五处）
- M5a #14 — `tests/interop/README.md` 条目顺序、`socks-helper` 头注释、回显任务没 abort
- M5a #15 — `socks5` 经不载 UDP 的 `underlying-proxy` 时先握手再报错、每 5 秒重来
- M5a #17 — 被 kill 的 UDP 流记成「完成」；只丢一个包的原因只记第一个
- M5a #21 — SOCKS 地址编解码写了两遍；`ConnectionReset` 吞法散在 6 处；限频日志三套写法
- M5b #11 — trojan `connect_tcp` / `open_udp` 里重复的「栈上 open 外套超时」
- M5b #14 — `stream_udp` 里 7 处按封装分支；`vmess/udp.rs` 与 `h2connect/udp.rs` 逻辑重复
- M5c #8 — 隧道里 UDP 载体：`recv_from` 先看 IPv4 等三处
- M5c #10 — 链式载体：收包任务遇任何收包错误都结束；`BrokenPipe` 不算 `gone()`
- M5c #12 — 告警限频表 `UNREACHABLE_SAID` / `ENDED_SAID` 从不清理
- M5c #15 — `dns-follow-interface`：上游与缓存只按网卡名区分；socket 用 7 MiB 缓冲
- M5c #17 — `dns-follow-interface` 把 AAAA 迟到的部分结果当完整结果缓存；`lookup_on` 复制了主路径
- M5c #19 — `udp_probe` 的错误文本带查询名
- M6a #9 — `AeadStream` 三处细节
- M6a #12 — `ss.rs` 的脆弱断言、假 obfs 服务端的 EOF
- M6b #6 — 配置、记录流、池、UDP、测试的小项
- M6c #11 — 小项（超长检查的次序、`te: trailers`、两处近乎重复的载体）
- M7b #10 — `tuic/mod.rs` 约 2200 行
- M7c #5 — CONNECT-UDP 畸形 2xx 的取消码不合 RFC
- M7c #20 — 两份单飞会话的规则已分叉

### 其它（11）

- M4c #2 — Windows：放进 Job Object 之前启动的子进程不在 Job 里（条件：等 std 稳定 `raw_attribute`（工具链升级时））
- M5a #2 — 全锥时「收到其它来源的包」的标注（条件：等请求记录有说明字段）
- M6a #3 — 没有"口令错"的互操作用例（条件：需要时另议）
- M6a #5 — `2022-blake3-chacha20-poly1305`（条件：手册列出时）
- M6a #10 — 密钥材料不清零（条件：需要时另议）
- M6b #7 — 每条记录分配 `Vec`；Snell UDP 经 Shadow TLS 无用例（条件：需要时另议）
- M6c #9 — CONNECT-UDP 已有目标的数据报不触发存活检查（条件：需要时另议）
- M7b #12 — 假服务端对长度不对的 v4 认证命令只忽略（条件：有协议事实时再补）
- M7c #2 — MASQUE 不乐观发送（条件：有需要时再议）
- M7c #7 — 内部 UDP 会话的协议标签只按端口推断（条件：需要时另议）
- M7c #13 — sing-box 1.15 的 `http` 入站可作 MASQUE 的三平台参照（条件：sing-box 1.15 发布正式版之后）

### M8 自己留下的（18 行）

M8a 计划（`docs/superpowers/plans/2026-10-08-phase2-m8a-ci-credentials-plan.md`）6 行与 M8b 计划（`docs/superpowers/plans/2026-10-08-phase2-m8b-p2-protocols-plan.md`）12 行的「延后事项」，只列编号与一个短语，去向照那两张表写；其中 5 行由 M8c 承接：M8a #4、M8b #6、#7、#12 已做掉（任务号与提交写在行里），M8b #9 已由任务 12 的 B 部分订正（`e7d0eef`）。

- M8a #1 — `h3://` 的 DoH3 URL 带用户名口令仍被拒（`W0026`），与 `https://` 的 Basic 认证不对称 — 去向：有用户报告再说（做法同 DoH：加一个头）
- M8a #2 — 缓存目录 `meta.json` 里资源的完整 URL — 去向：不处理（与缓存的订阅正文、配置文件同在使用者的数据目录里）
- M8a #3 — `[General]` 里不认识的键的取值在 `General.unknown` 的 `Debug` 里可见 — 去向：有用户报告再说；M8c 任务 13 复查后仍不动（隐藏它们会隐藏配置的大半），在 M8c 计划的「延后事项」里列为由项目所有者决定的一条；终审之后 `UnknownKey` 的 `Debug` 显示脱敏后的那一行（`b4a5759`），未知键的取值本身仍有意显示
- M8a #4 — `General` 的 `proxy_test_url` / `internet_test_url` 在 `Debug` 里可见 — 去向：M8c 任务 13，已做掉（`eafd6be`；`redact_profile` 也隐去这两个键，修正轮 `199148c` 又收进 `geoip-maxmind-url`）
- M8a #5 — 失败摘要脚本没有自动化测试；注解长度上限没有文档依据 — 去向：第一次真正用上时核对；需要时给脚本配一组样本日志
- M8a #6 — `url_without_credentials` 对查询串里带 `@` 的 URL 过度脱敏 — 去向：接受（偏安全一侧；只出现在诊断与脱敏输出里）
- M8b #1 — mihomo 与 `ssserver` 流式方法的互操作用例没有对真实二进制跑过 — 去向：下一次 CI 运行时核对（M8a 任务 6）
- M8b #2 — `salsa20` 没有可用的参照服务端 — 去向：手工验收（有 libev 服务端的人）；出现预编译的参照时补互操作
- M8b #3 — `chacha20` / `chacha20-ietf` 每个方向 256 GiB 少 64 字节处到头 — 去向：接受（已登记进兼容性清单 4.2 `ss` 一行）
- M8b #4 — Snell v1 ～ v3 没有对官方服务端验证 — 去向：手工验收（`docs/acceptance/phase2-manual.md` 的 M8b 一节）
- M8b #5 — Snell v6 — 去向：见可行性报告；项目所有者表态之后
- M8b #6 — 互操作夹具仍一个一个地取端口 — 去向：M8c 任务 11，已做掉（`6e22fa2`；`Failures` 的 `Drop` 守卫到 `284c40a` 才做完）
- M8b #7 — 本机连着跑全量时偶发失败的两条用例 — 去向：M8c 任务 11，已做掉（`6e22fa2`、`284c40a`；另有五条同类用例一并按原因改掉）
- M8b #8 — Snell「没有应答」提示的已知限制 — 去向：接受（已登记进兼容性清单 4.2 `snell` 一行）
- M8b #9 — `snell` 的 `udp-port` 的注释与清单措辞 — 去向：M8c 任务 12 的 B 部分，已做掉（`e7d0eef`：四个源文件里讲 `udp-port` 的注释与兼容性清单 4.2、4.5 的两行改用手册的原话，把它同 v5 的 QUIC Proxy Mode 画等号的说法去掉；M6 设计第 18 节第 7 条加了旁注）
- M8b #10 — 评审留下的小项（不影响行为） — 去向：动到那些文件时再整理
- M8b #11 — 只靠读代码、没有本机用例盖住的三处 — 去向：接受
- M8b #12 — 一行已经有错误时仍多说一句的其余各处 — 去向：M8c 任务 10，已做掉（`57977af`，修正轮 `c870456`）

M8c 自己的「延后事项」（105 行）列在 M8c 计划（`docs/superpowers/plans/2026-10-09-phase2-m8c-leftovers-acceptance-plan.md`）的「延后事项」一节里；其中去向写「由项目所有者决定」的 32 行（第 26、36、43 行，第 51 ～ 77 行，第 99 行，第 105 行）是留给项目所有者的问题；第 100 ～ 103 行是阶段 1 从没做的托管配置更新与远程 `#!include`、需求审计查出的其余缺口和做了而没找到用例的几句，移交到 M8c 之后、阶段 3 开始之前补做；第 104 行（`hidden`）移交阶段 6。

## 逐份计划

列：事项与原去向是原表的缩写；结局见上；依据写打开过的东西——提交、文件与函数、文档，或接手那份计划自己的表和行；M8 的处置以 M8 设计第 7 节（`docs/superpowers/specs/2026-10-08-phase2-m8-wrapup-design.md`）为准，M8c 的任务号以 M8c 计划（含执行期新增的任务 13、14、15）为准。

### M1a — `2026-09-19-phase2-m1a-outbound-library-plan.md`（31 行；原表无编号，按出现顺序编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | `tfo` 三平台不生效 | 登记进兼容性清单 4.3；`socket2` 有安全封装后再评估 | 移交：依赖升级时 | 已登记：`docs/surge-compatibility-matrix.md` 4.3 的 `tfo` 行（解析并校验、`W0029`、三平台都不生效，等 `socket2` 的安全封装）；重新评估等该封装出现 |
| 2 | `dns-follow-interface` | M5（M1 设计 11 节 C2） | 已做掉 | → M1b #5 → M5a 承接事项 C3 → M5c 任务 5（`b15a703`，终审修正 `9741d76`）：`rurge-dns` 的 `Resolver::lookup_via`；`rurge-config/src/spec/common.rs` 对没写 `interface` 的写法报 `W0028`；清单 4.3 `dns-follow-interface` 行写"M5c 已实现" |
| 3 | Linux / macOS 的 `bind_interface`、`set_tclass_v6` 分支本机编不了 | 首次推送后由 CI 证明 | 移交：CI 首跑 | 编译已证明：M8 设计 3.1，2026-10-08 的推送（`0154e73`）上三个平台的 clippy 都通过，`.github/workflows/ci.yml` 的矩阵含 ubuntu 与 macos，跑 `cargo clippy --all-targets -- -D warnings`，两个 Unix 分支因此已编过。这些分支的运行期行为等测试一步转绿（M8 设计 3.2 的"首跑见分晓"项，M8a 任务 6） |
| 4 | 旧式（RC2-40 + 3DES）p12 读不读得了 | 已结（任务 10 Step 4 的分支） | 已做掉 | M1a 执行期修正记录 任务 10（`a6c9ab1`）：`p12-keystore` 0.2.1 两种都能解码；`rurge-proto/src/keystore.rs` 的 `files_written_by_openssl_decode_with_either_encryption` 钉着 |
| 5 | 出站按指纹复用、`RegistryCell`、工厂、接线、能力表、API、互操作夹具 | M1b | 已做掉 | M1b 任务 3（`357b14b`：`RegistryCell` 与工厂）、4（`b20973d`）、5（`bf84ed8`：引擎接线）、8（`f1cb39e`、`9877025`）、9（`d0eaa9a`：API）、10（`bf2899e`：能力表）、11（`e68d1bd`：互操作）；"按指纹跨代复用"M1b 延到 M2（M1b #4），M2b 任务 8（`09840e9`）做掉 |
| 6 | 入站只验 UTF-8，控制字符传到出站（根因） | M1b：入站拒绝；转发前 `valid_target` | 已做掉 | M1b 承接事项 → 任务 1（`d525787`：`HostName::from_wire`）、2（`26ba584`、`c8d8698`：`rurge-inbound` 的 SOCKS5 入站用它拒绝，出站用 `wire_host`）、6（`4ea463b`）：`rurge-engine` 的 `connect_through` 先 `valid_target` 再转发；`HttpForward` 的文档写着调用方的两条义务 |
| 7 | `HttpOutbound::request_headers()` 每次调用重新渲染占位 | M1b：转发调用方每条连接只调一次 | 已做掉 | M1b 任务 6（`4ea463b`）：`connect_through` 每条转发连接调一次；契约写在 `rurge-proto/src/outbound.rs` 的 `HttpForward` 文档 |
| 8 | 生产 DIRECT 仍用 `NoopSocketHook` | M1b：注入 `PlatformSockets` | 已做掉 | M1b 任务 5（`bf84ed8`，`StackOptions.socket_hook`）、10（`bf2899e`）：`rurge/src/cli/runtime.rs` 的 `PlatformSockets` 经 `socket_hook: Arc::new(PlatformSockets)` 注入 |
| 9 | 两处语义未与真实 Surge 核对（`sni` 兼作校验名、`https` 无 ALPN） | M1b 文档任务标"未核对" | 已做掉 | M1b 任务 12：清单 4.4 的 `sni` 行与 `alpn` 行各写"（未与真实 Surge 核对）" |
| 10 | `race` 的 3 秒分支被 `queue.is_empty()` 挡住 | 终审裁定保持现状 | 关闭 | 终审裁定：需首选族 13 个以上地址才出现、不会挂起，改动会动已上线的 DIRECT；`rurge-net/src/socket.rs` 的 `race` 仍是原样 |
| 11 | 没有测试钉住握手签名校验 | 已修（`4a8872e`） | 已做掉 | `4a8872e` 存在；`rurge-proto/src/transport/tls.rs` 的 `a_server_without_the_private_key_is_refused_in_every_mode` |
| 12 | `server-cert-verify-name` 只在标准校验分支解析 | M2（TLS 族） | 已做掉 | M2 设计第 9 节第 1 条 → M2a 任务 1（`bdfc38c`）：`rurge-config/src/spec/tls.rs` 配置期校验，与指纹 / `skip-cert-verify` 同写时 `W0012`；`TlsClient::build` 先解析（用例 `a_bad_verify_name_is_a_build_error_in_every_mode`）；清单 4.4 `server-cert-verify-name` 行 |
| 13 | DNS 空应答文本误导；超时文本里 IPv6 无方括号 | M1b | 已做掉 | M1b 承接事项 → 任务 2（`26ba584`、`c8d8698`）：`rurge-net/src/connector.rs` 的 `display_target` 给 IPv6 加方括号，空应答文本为 `no addresses for <名字>`；两条用例在同一文件 |
| 14 | TLS / shadow-tls 参数写在 `reject*` 别名上落到 `W0001` | 不处理 | 关闭 | 原表：结果同为"忽略并告警" |
| 15 | `[Keystore]` 空的 `base64` 不报 `E0021` | M1b：干构建报 `E0022` | 已做掉 | M1b 执行期修正记录 任务 4（`b20973d`）写明就此关闭；`rurge-engine/src/outbounds.rs` 的 `the_dry_build_reports_what_cannot_be_built_at_the_policys_own_line` 用 `base64=` 为空的条目得到带行号的 `E0022` |
| 16 | `decode_p12` 的无私钥 / 链为空 / 无填充 Base64 分支没有用例 | 有真实样本时补（M2 的双向 TLS 夹具） | 移交：有用户报告再说 | `rurge-proto/src/keystore.rs` 的用例仍是两份 OpenSSL 样本、错误口令 / 错误 Base64 / 截断、`a_decoded_identity_completes_mutual_tls`，没有这三个分支的用例；M2a / M2b / M2c 的表都未提；原去向是常驻条件（有真实样本），无人承接 |
| 17 | `http` 出站小项（16 KiB 上限近似、随机源 WARN、min ≤ max 靠约定） | 不处理 / 出现实际问题时再收紧 | 关闭 | 原表：不处理；其中 `random_string` 的 `min > max` 已由 M1a 终审 D（`4a8872e`）加 `max.max(min)` |
| 18 | 两个假上游各有一份 accept 循环 | 第三个假上游（M2）出现时抽公共循环 | 移交：下次动到时 | 第三个假上游（M2a 的 `FakeTrojan`）出现后，M2a / M2b / M2c 的表与 M2 设计第 9 节都没有接这条，此后也无计划接手；`crates/rurge-proto/src/testing/` 的 12 个文件里仍有 13 处各自的 `listener.accept()` 循环（`anytls` `h2proxy` `http_proxy` `mod` `shadowsocks` `shadow_tls`×2 `snell` `socks5` `tls` `trojan` `vmess` `ws`），没有公共循环；留给下一次动这些文件的人 |
| 19 | `echo_server` / `spawn_echo` 只返回 `SocketAddr` | 不处理 | 关闭 | 原表：每个 `#[tokio::test]` 的运行时结束时取消 |
| 20 | 两个时序用例偶发失败（`empty_answer_rules`、`watch_reloads_rules_on_change`） | 再出现一次就单独修 | M8 做掉 | 两条用例都由 M8c 改掉。`rurge/tests/cli.rs` 的 `run::watch_reloads_rules_on_change`：任务 11（`6e22fa2`，第 4 项）查明配置若写在监视器装好之前就不会再被看到，用例改为先等守护进程说 `running`，等重载生效的上限从 8 秒放到 30 秒（等的是重载，不是一个时长）。`rurge-dns/src/fanout.rs` 的 `empty_answer_rules`：任务 15 的修正轮（`dd22e04`）把它搬到暂停的时钟和内存里的上游上，断言虚拟时间的确切时刻（0 与 400 毫秒） |
| 21 | `socks5` 出站的 `expect("offered only with credentials")` | 已修（`4a8872e`） | 已做掉 | `rurge-proto/src/socks5.rs` 现为 `match (selected[1], credentials)`，文件里已无该 `expect` |
| 22 | `socks5` 应答的 ATYP=3 与未知 ATYP 无用例 | 已修（`4a8872e`） | 已做掉 | `rurge-proto/src/socks5.rs` 的 `the_bound_address_may_be_a_domain_or_ipv6_and_unknown_types_are_refused` |
| 23 | `socks5` 超长域名用例没证明"没有拨号" | 保持延后（结构上成立） | 关闭 | 原表：`connect_request` 是拿不到 connector 的自由函数，且在 connect 之前调用 |
| 24 | `socks5` 的三个版本字节不校验等 | 不处理 / 出现实际问题时再改 | 关闭 | 原表：不处理（诊断质量） |
| 25 | `http` 出站 CONNECT 写无 EOF / reset 归一；两出站的测试辅助函数近乎相同 | 第三个出站（M2）到位时对齐；辅助函数挪进 `crate::testing` | 移交：下次动到时 | 两部分。`http` 出站握手的错误归一：M8c 任务 9 做掉——`a96eabf` 让 CONNECT 请求的写、flush 与应答的读都经 `handshake_io`（`crates/rurge-proto/src/http.rs`），代理关了连接说成一句 `http proxy closed the connection during CONNECT`；修正轮 `771b94f` 让下一跳自己带话的 reset / abort / broken pipe 原样通过、EOF 仍一律算代理的（已知限制：下层的拒绝若以 EOF 表现，仍被说成上层代理关了连接）。两出站的测试辅助函数近乎相同：未做，`target` 9 份、`no_roots` 4 份、`roundtrip` 5 份仍分散在 `http.rs`、`socks5.rs`、`vmess/mod.rs` 等的测试模块里，未进 `crate::testing`；M2a / M2b / M2c 的表与 M2 设计第 9 节都没有接这部分 |
| 26 | `bind_interface` 每次连接都查网卡表 | M1b：加短 TTL 缓存 | 已做掉 | M1b P12 与任务 10（`bf2899e`）：`rurge-platform/src/socket.rs` 的 `Cached`，`INTERFACE_TABLE_TTL` 为 5 秒 |
| 27 | `skip-cert-verify=true` 没有信号 | M1b：构建时打 WARN | 已做掉 | M1b 承接事项 → 任务 4（`b20973d`，P11）：`rurge-engine/src/outbounds.rs` 构建时 `warn!` `skip-cert-verify is on: …` |
| 28 | `http` 出站拒绝非 ASCII 主机名（IDN 未转 A-label） | M1b：转 A-label；清单登记 | 已做掉 | M1b 任务 2（`26ba584`、`c8d8698`）：`rurge-proto/src/http.rs` 的 `wire_host` 转 A-label（用例 `wire_host_renders_ip_literals_a_labels_and_refuses_the_unconvertible`）；清单 4.2 `http` / `https` 行已登记 |
| 29 | 三处配置兼容性观察（空 `interface=`、`hybrid` 非法值、未读参数的 `W0001`） | M1b 文档任务：核对后决定放宽与否 | 已做掉 | M1b P10 与任务 1（`d525787`）定：`hybrid` 不再校验取值（清单 4.3 `hybrid` 行），空 `interface=` 仍 `E0018` 并标"未与真实 Surge 核对"（4.3 `interface` 行）；第三项（未读参数的 `W0001`）P10 写了要标"未核对"，清单里没有这一句 |
| 30 | `BootstrapConnector` 不能表达 `prefer_v6` | 需要时再加回 | 移交：有用户报告再说 | `rurge-dns/src/bootstrap.rs` 的 `BootstrapConnector` 仍把 `prefer_v6` 写死为 `false`（`interleave(ips, false)`）；`rurge-dns/src/upstream/session.rs` 的调用也传 `false`，没有调用方要 `true` |
| 31 | 全部失败时 `race` 报最后完成的那次失败 | 不处理 | 关闭 | 原表：不处理 |

### M1b — `2026-09-19-phase2-m1b-assembly-control-plane-plan.md`（33 行；原表无编号，按出现顺序编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | 互操作用例与 CI 里装 sing-box 的步骤本机跑不了 | 首次推送后的 CI | 移交：CI 首跑 | M8 设计 3.1：2026-10-08 的推送（`0154e73`）上三个平台的参照服务端安装步骤都通过，`cargo test` 一步失败；互操作用例的结果要等读到失败清单；M8a 任务 6（CI 首跑的修正）还没有开始，要等推送之后读 CI；至今没有推送，CI 没有在 M8 的提交上跑过 |
| 2 | `socks5-tls` 没有参考实现可对测 | 保持回环假上游覆盖；出现参考实现时再补 | 移交：手工验收 | 没有参照服务端能说 TLS 上的 SOCKS5：`tests/interop/README.md` 写明不覆盖及原因（sing-box 的 `socks` / `mixed` 入站没有 `tls`），M8 设计 6.2 把 `socks5-tls` 列入"无参照服务端、由回环假服务端覆盖"一类；`docs/acceptance/phase2-manual.md` 加入该项（M8c 任务 12 的 B 部分） |
| 3 | `/v1/policies/detail`、`/v1/policy_groups` 的真实响应形状（M1 设计 O2） | 拿到 Surge 实例的样本后对齐 | 移交：项目所有者 | `docs/api/phase2.md` 开头的"暂定"标注仍在，等真实 Surge 实例的样本；`docs/acceptance/phase2-manual.md` 没有对照样本的条目 |
| 4 | 出站按指纹跨代复用 | M2 | 已做掉 | M2b 任务 8（`09840e9`）：`rurge-policy/src/registry.rs` 的 `fingerprint`；M2 设计第 7 节 |
| 5 | `dns-follow-interface`；`udp-relay`、UDP 载体 | M5 | 已做掉 | M5a 承接事项 C1：任务 1（`561bb9f`：`PacketSocket` 与 `Connector::open_udp`）、2（`816690f`：`socks5` 的 `udp-relay`）、6（`b970034`：链式 UDP、`external`）；M5b（`a4ed8bb`：trojan 等）；`dns-follow-interface` 见 M1a #2（M5c 任务 5，`b15a703`） |
| 6 | `test-url` / `test-timeout` 的使用、计时字段、组环降级、空组回退、不可用策略 → REJECT | M3 | 已做掉 | M3a 承接事项 C1 / C2：`W0030` 取代 `E0009`（`5326d77`）、环上的组 REJECT 与 `EmptyGroup`（`911da34`）、`--empty-group-reject`（`90e58df`）、导入与派生策略构建失败只略去该条；`test-url` / `test-timeout` → M3b 任务 3（`be33e04`）；计时字段 → M3c 任务 1（`d36c95f`：`connectMs` / `firstByteMs`） |
| 7 | `race` 3 秒分支被挡住；`server-cert-verify-name` 只在标准分支解析 | 前者保持现状；后者 M2 | 关闭 | 前者同 M1a #10（`rurge-net/src/socket.rs` 的 `race` 不改）；后者 M2a 任务 1（`bdfc38c`）已做掉，同 M1a #12 |
| 8 | 出站测试辅助函数重复；假上游 accept 循环；`http` CONNECT 写无归一 | 第三个出站（M2）到位时一并处理 | 移交：下次动到时 | 同 M1a #18、#25：`http` CONNECT 写无归一的部分由 M8c 任务 9 做掉（`a96eabf`、`771b94f`，见 M1a #25）；假上游的 accept 循环（`crates/rurge-proto/src/testing/` 的 12 个文件里 13 处）与出站测试辅助函数的重复（`no_roots` / `target` / `roundtrip`）原样仍在，M2a / M2b / M2c 的表与 M2 设计第 9 节都没有接 |
| 9 | Linux / macOS 平台分支（含网卡表缓存）本机编不了 | 首次推送后的 CI | 移交：CI 首跑 | 同 M1a #3：M8 设计 3.1，`0154e73` 上 ubuntu 与 macos 的 `cargo clippy --all-targets` 通过，编译已证明；网卡表缓存在 `rurge-platform/src/socket.rs` 的 `Cached`；运行期行为等测试一步转绿；M8a 任务 6（CI 首跑的修正）还没有开始，要等推送之后读 CI；至今没有推送，CI 没有在 M8 的提交上跑过 |
| 10 | `HostName::from_wire` 放行 Unicode 格式字符 | 保持延后 | 关闭 | 终审裁定：一概拒绝会误伤合法 IDN，出站字母表已拦注入；`rurge-config/src/types.rs` 的 `from_wire` 仍只拒控制字符与空白 |
| 11 | SOCKS5 入站拒名字不打日志；非 UTF-8 名字直接断连 | debug 日志已加；非 UTF-8 保持不处理 | 关闭 | 日志部分 `ae6bbf5` 已做（`rurge-inbound/src/socks5.rs` 只记长度）；非 UTF-8 仍以 `io::Error` 断连，原表：保持不处理 |
| 12 | `socks5` 出站"超过 255 字节"提示量的是 A-label | 不处理 | 关闭 | 原表：不处理（措辞） |
| 13 | 运行期跨多跳 `ChainConnector` 不受 `MAX_DEPTH` 约束 | M3：订阅引入动态成员时加运行期深度兜底 | 关闭 | → M3a 承接事项 C3 / 延后事项 #2（静态兜住，运行期守卫留 M8）→ M8 设计第 7 节关闭表（M3a #2）：有界性由装配期的环检查保证，没有能构造出无界链的输入 |
| 14 | `cell.rs` 里 `via <name>: ` 前缀出现四处 | 不处理 | 关闭 | 原表：不处理 |
| 15 | 注册表测试重写时少带三条旧断言等 | 已修（`ae6bbf5`） | 已做掉 | `ae6bbf5` 存在（终审 B、E、G）；`rurge-policy/src/registry.rs` 的 `a_three_hop_chain_hands_each_server_to_the_hop_below`；`Direct::with_socket_opts` 在 `crates` 里已无引用 |
| 16 | 1 Hz 采样任务持有 `Arc<Engine>` | 不处理 | 关闭 | 原表：守护进程退出路径先 `stop_accepting`，测试夹具不启动采样器 |
| 17 | 转发模式下上游的 407 原样交还，会话记 Completed | 已修（`ae6bbf5`） | 已做掉 | 终审 C（`ae6bbf5`）：`rurge-inbound/src/http.rs` 把 407 记为失败会话并回 502 页面；用例在 `rurge-engine/tests/outbounds.rs`（断言 `http proxy answered 407 Proxy Authentication Required`） |
| 18 | 拒绝文本在 engine 与 proto 各写一遍；隧道用例注释；丢头日志 | 重复文本保持延后（第三个调用方时导出常量）；其余已修 | 移交：下次动到时 | 注释与丢头 debug 日志已修（`ae6bbf5`）；拒绝文本仍两处（`rurge-engine/src/engine.rs` 的 `connect_through`、`rurge-proto/src/http.rs` 的 `tunnel`），第三个调用方没有出现 |
| 19 | `dry_build` 文档注释的说法 | 已修（`ae6bbf5`） | 已做掉 | `rurge-engine/src/outbounds.rs` 的 `dry_build` 文档注释现写明：跳过 Direct 是因为 `Direct::new` 不会失败，只有 reject 别名没有自己的出站 |
| 20 | HTTP 入站"tripwire"测试钉的是 `http::Uri::from_str` | 不处理 | 关闭 | 原表：两者共用 `Uri::parse`，间接但有效 |
| 21 | `[Host]` 给同名列多个地址时总取第一个，不看 `ip-version` | 登记进兼容性清单；需要时再按 `ip-version` 挑选 | 已做掉 | 清单 2.1 的 `use-local-host-item-for-proxy` 行已写"`[Host]` 给同一个名字列了多个地址时取第一个，不看策略的 `ip-version`（未与真实 Surge 核对）"；按 `ip-version` 挑选未做（原表：需要时再做） |
| 22 | 三条新路由没有各自的 401 用例 | 不处理 | 关闭 | 原表：路由器只有一条 `.layer(require_key)` 链，兜底路由的既有用例在结构上已覆盖 |
| 23 | `Cached::get` 持锁期间调 `get_if_addrs` | 不处理 | 关闭 | 原表：不是回退——原来每条连接内联调用一次，现在每 5 秒一次 |
| 24 | `rurge run` 遇坏配置先退出 2，系统代理恢复不到 | 单独跟进 | M8 做掉 | M8c 任务 8（M8 设计 5.10；设计第 7 节「M1b 的坏配置与系统代理恢复」；M8c 承接事项「M1b」一行）：`6a473ed` 做掉主体，修正轮 `9bab75e` 补三处——数据目录锁不了时不做提前恢复、备份清不掉时说第四句、`--log-file` 打不开时同样先恢复。`rurge run` 在三个出口（配置读不了、配置有错误、日志文件打不开）退出之前先试一次系统代理恢复（`recover_without_a_log`），退出码仍是 2 |
| 25 | 互操作层的防抖小项 | 已修部分；守卫夹具缺口与 panic 回溯保持延后 | 移交：下次动到时 | 已修部分在 `ae6bbf5`（`Timeout` 分支、`try_wait` 在前、转发用例超时、`curl --retry`、README），现仍在位；其余两项（守卫夹具不渲染"不带客户端 CA 的 TLS 入站"；必需模式的探针用例打印 panic 回溯）后面各计划与 M8 都没有提到 |
| 26 | sing-box 1.14.1 是否接受夹具配置；三个 SHA-256；CI 步骤在真实 runner 上 | 首次推送后的 CI | 移交：CI 首跑 | SHA-256 与安装步骤：M8 设计 3.1 记 2026-10-08 的运行上全部参照服务端安装通过；夹具配置是否被 sing-box 接受要看互操作用例的结果，M8 设计 3.2 列为首跑核对项（M8a 任务 6 还没有开始，要等推送；sing-box 已升到 1.14.2） |
| 27 | `publish_registry` 用 `assert!` | 保持现状（M2 出现更多调用方时改 `debug_assert!`） | 关闭 | M2 设计第 9 节第 5 条：保留，M2b 扩成 `publish_generation`；`rurge-engine/src/engine.rs` 的 `publish_generation` 仍以 `assert!` 校验 |
| 28 | 缺"链式会话进行中重载"与"选择指向已消失成员"的用例 | M2 | 已做掉 | M2 设计第 9 节第 2 条 → M2a 任务 6（`ea609bd`）：`rurge-engine/tests/outbounds.rs` 的 `a_reload_leaves_a_chained_session_alone_and_moves_the_next_one`、`a_selection_whose_member_is_gone_falls_back_to_the_first_member` |
| 29 | `interface` / `tos` 与 `underlying-proxy` 同写时静默失效 | M2 设计时决定（是否 `W0028`） | 已做掉 | M2 设计第 9 节第 3 条 → M2a 任务 1（`bdfc38c`）、4（`c99388e`）：`rurge-config/src/spec/mod.rs` 加载时对 `interface`、`allow-other-interface`、`tos`、`ip-version` 各报 `W0028`（用例在同一文件）；清单 4.3 `interface` 等行写"M2a 起加载时报 W0028" |
| 30 | `SessionInfo::url` 作"明文 HTTP 转发"信号的契约 | 阶段 4（MITM）动它之前重新审视 | 移交：阶段 4 | 契约已写进 `rurge-config/src/session.rs` 的 `SessionInfo::url` 文档注释；重新审视留到阶段 4 动 MITM 时 |
| 31 | 防环回退对 `[Host]` 能回答的代理偏保守 | 关闭（M2 设计第 9 节第 4 条） | 关闭 | M2 设计第 9 节第 4 条：前提不成立（`proxy_hostnames` 把代理服务器名排除在 `[Host]` 之外）；M2a 任务 8 订正了 M1b 的表 |
| 32 | 链底的 REJECT 到不了 `dial_internal` 的旁路 | M3（策略组） | 已做掉 | M3a 延后事项 #3 → M3b 任务 9（`d176f4b`，P16）：`rurge-engine/src/engine.rs` 的 `socket_opener` 与 `REJECT_BYPASSED`；用例 `rurge-engine/tests/pipeline.rs` 的 `a_dns_session_bypasses_a_proxy_chain_that_ends_at_a_reject` |
| 33 | 链深超 `MAX_DEPTH` 时 `socket_opener` 与注册表说法不一 | M3，与运行期深度守卫一并处理 | 已做掉 | M3a 延后事项 #3 → M3b 任务 9（`d176f4b`，P16）：`socket_opener` 走出 `MAX_DEPTH` 与链止于 REJECT 同样返回 `None`、同样旁路直连，文档注释写明；一并提到的运行期守卫按 M8 设计第 7 节关闭，见 #13 |

### M2a — `2026-09-20-phase2-m2a-trojan-plan.md`（14 行；原表无编号，按出现顺序编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | `WsByteStream.queued` 与 `LazyHead.coalesced` 假设同缓冲重试 | 终审：保持延后（不可达）；`queued` 上报改 `min`（b4f1ee8） | 关闭 | 原文「保持延后」：工作区里没有放弃挂起的写、换一段缓冲再写的调用方。做掉的一半：`WsByteStream::poll_write` 上报 `min(queued, data.len())`（`crates/rurge-proto/src/transport/ws.rs`，b4f1ee8）；约定写在该函数的注释与 `vmess/stream.rs` 的文件头 |
| 2 | `a_write_reaches_the_peer_without_an_explicit_flush` 的写无上界 | 保持延后（30 秒上界的往返用例兜底） | 关闭 | 原文「保持延后」，只涉及一条用例的时限；用例仍在 `crates/rurge-proto/src/transport/ws.rs`，读有 5 秒上界，`write_all` 没有 |
| 3 | `ws_io` 的 `Capacity` 文本只说 frame | 保持现状（文本已登记进 API 文档） | 关闭 | 原文「保持现状」；文本仍是 `ws: the server sent a frame larger than the limit`，`docs/api/phase2.md` 的这一行写成「单帧或重组后的消息超过入站上限（1 MiB）」 |
| 4 | `is_managed` 在 config 与 proto 各一份 | 保持现状（出站不信任调用方，四行） | 关闭 | 原文「保持现状」；两份仍在：`crates/rurge-config/src/spec/ws.rs`、`crates/rurge-proto/src/transport/ws.rs` |
| 5 | `the_head_does_not_grow_…` 在旧实现上也通过 | 保持延后（不变式靠构造保证） | 关闭 | 原文「保持延后」，留作文档；用例仍在 `crates/rurge-proto/src/transport/lazy_head.rs` |
| 6 | `LazyHead` 的 `WriteZero` 错误不粘性 | 保持延后（无调用方出错后继续轮询） | 关闭 | 原文「保持延后」，工作区里不可达；`lazy_head.rs` 里该错误的处理未变 |
| 7 | `TrojanSpec` 派生 `Debug`、口令明文 | M2b 给凭据字段包一层不打印的类型 | 已做掉 | M2b 任务 1（e06a06b）：`TrojanSpec.password` 是 `Secret<String>`（`crates/rurge-config/src/spec/trojan.rs`），`Debug` 恒为 `Secret(***)`（`spec/secret.rs`）；M2b 计划「承接事项」表里讲凭据字段包一层的那一行 |
| 8 | "需要 server 与 port"的检查散在三处 | M2b（收进 proto 或抽公共函数） | 已做掉 | M2b 任务 9（d209feb）：`rurge_proto::build::server_of`（`crates/rurge-proto/src/build.rs`），引擎工厂的 trojan、vmess、anytls 等分支都用它；M2b 计划「承接事项」表里讲 server / port 检查的那一行。`http` / `socks5` 的 `from_spec` 仍自己解构 spec，与变体判断合在一起 |
| 9 | pipeline 用例没断言 mock DNS 被查询过 | 保持延后 | 关闭 | 原文「保持延后」，只涉及一条用例的断言强度 |
| 10 | 互操作 `roundtrip()` 读回显无上界 | 已修（终审修复，b4f1ee8） | 已做掉 | b4f1ee8（M2a 终审修复 E）：连接、写、读回显各加 10 秒 `timeout`；该函数现在在 `tests/interop/tests/common/mod.rs` |
| 11 | sing-box 的 trojan 互操作本机跑不了 | 首次推送后的 CI 证明 | 移交：CI 首跑 | 用例在：`tests/interop/tests/sing_box_tls_family.rs` 的 `trojan_with_and_without_websocket`、`a_wrong_trojan_password_is_not_relayed`；`.github/workflows/ci.yml` 在工作流一级设 `RURGE_INTEROP_REQUIRED: 1`。首跑结果还没读到：M8a 任务 6 还没有开始，要等推送（M8 设计 3.2） |
| 12 | 转发循环 `write_all` 之后不 flush | M2b（要有自己的用例并看吞吐） | 已做掉 | M2b 任务 6（b0f2d2f）：`crates/rurge-engine/src/relay.rs` 的 `copy_half` 写完即 `flush`，用例 `what_was_written_is_flushed_without_waiting_for_more`；吞吐的交替 A/B 记在 M2b 计划「执行期修正记录」。TLS / WebSocket / VMess 栈上的 flush 成本（M2b #16、#30）：M8c 任务 9 量过，结论是没有差别，每次写后 flush 保留（`6752afa`、`d309f33`；M8 设计 5.11） |
| 13 | IDN 写的代理服务器名在 TLS 层是构建错误 | M2b 连同解析器的 IDN 一起核对 | M8 做掉 | M2b 核对后不改行为（计划期决定 P15，即 M2b #2）→ M8 设计 5.9 → M8c 任务 7（`010a9ca`，修正轮 `2e4cb63`）：策略行的 `server` 进 spec 前转成 A-label。规则、`[Host]`、解析器对 Unicode 名的部分不属于本行，见 M2b #2 |
| 14 | 引号感知切分后含逗号的行里 `user` 不再被抹 | 保持现状（这样的行加载不了） | 关闭 | 原文「保持现状」：端口不是数字的行加载不了，不会成为 `profiles/current` 提供的运行中配置 |

### M2b — `2026-09-20-phase2-m2b-vmess-anytls-plan.md`（32 行；原表无编号，按出现顺序编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | （计划期）AnyTLS 不实现 3 秒 SYNACK 看门狗（P7） | M8 收尾时评估；清单已登记 | M8 做掉 | M8c 任务 5（设计 5.6、M8-D14；M8c「承接事项」「M2b 计划期」一行）：`cf22f19` 做掉主体，`16b54c5` 让假服务端照参考服务端的次序应答 UDP 流并补复用会话上的 UDP 载体用例。复用的会话（流号不小于 2、对端协议版本不低于 2）上新流发出后 3 秒没有它的 `cmdSYNACK` 即关闭整条会话，流上的读写以 `anytls: the session is closed` 结束；差异与限制由任务 12 的 B 部分登记进兼容性清单。终审另记：看门狗没有对真实服务端验证过——互操作用例在几毫秒内放掉每条流，UDP 用例用新会话的第一条流，第一次 CI 运行也改变不了这一点；缺的是一条在复用的会话上活过三秒的流的互操作用例（M8c 计划「延后事项」第 87 行） |
| 2 | （计划期）Unicode 写的代理服务器名不可用（P15） | M8：连同规则、`[Host]`、解析器做全面 IDN；清单已登记 | 移交：阶段 3 之后 | 做掉的一半：出站一侧（策略行 `server` 转 A-label，`ssh` 与 `wireguard` 的目标名）→ M8c 任务 7（设计 5.9；`010a9ca`，修正轮 `2e4cb63`）。未做的一半：规则、`[Host]` 的键、解析器对 Unicode 名 → M8 设计第 7 节「M2b P15 的其余部分」移交阶段 3 之后。兼容性清单「所有协议通用 / 策略行里的代理服务器主机名」一行由任务 12 的 B 部分从「须写成 ASCII」改为「出站一侧已支持，规则与 `[Host]` 须写成 `xn--` 形式」 |
| 3 | （计划期）`Secret<T>` 只包了部分凭据字段（P14） | M8 | M8 做掉 | M8a 任务 2（设计 3.3，b09b126）：`WsOpts`、`KeystoreItem` 手写 `Debug`；`HttpSpec.headers` 的值早在 6f58b74 起不进 `Debug`（`HeaderTemplate`）。判据 `crates/rurge-config/tests/secrets.rs` 的标记配置含 `ws-path`、`ws-headers` 与 `[Keystore]` |
| 4 | （计划期）VMess 旧式（非 AEAD）握手 | M8（设计 M2-D4） | M8 做掉 | M8b 任务 1（设计 4.2、M8-D8），以「不实现」结案：没写 `vmess-aead=true` 的行改用 AEAD 握手并报 `W0029`（4203eb3，断言在 `crates/rurge/tests/cli.rs`）；`NotImplemented::LegacyVmess` 已删；差异登记在清单 `vmess` 行 |
| 5 | `ChunkCipher::open_len` / `open` 的调用约定没写 | 终审修复波写进注释 | 已做掉 | 5c5a785：两个函数的文档注释写明每个分块各调一次、先 `open_len` 后 `open`（`crates/rurge-proto/src/vmess/chunk.rs`） |
| 6 | `VmessStream::poll_write` 停着的分块的重试约定 | 终审修复波写进模块注释 | 已做掉 | 5c5a785：`crates/rurge-proto/src/vmess/stream.rs` 文件头写明挂起的写须以同样的字节重试 |
| 7 | `VmessStream` 的读错误不粘性 | 终审修复波写进模块注释 | 已做掉 | 5c5a785：同一文件头写明读错误是终局、之后不得再读 |
| 8 | `FakeVmess` 缺 `len < TAG` 检查（仅测试） | 保持现状 | 关闭 | 原文「保持现状」，只影响测试用的假服务端 |
| 9 | 会话死在 `Pool::take()` 检查与 `Session::open` 之间 | 终审修复波已修复 | 已做掉 | 5c5a785：`Session::open` 发送后再查 `closed`（`crates/rurge-proto/src/anytls/session.rs`），用例 `a_session_that_closed_a_moment_ago_does_not_open_a_stream` |
| 10 | `FakeAnyTls::kick()` 用 `notify_waiters` 会丢通知 | 终审修复波已修复 | 已做掉 | 5c5a785：`kick` 改用 `watch` 代际计数（`crates/rurge-proto/src/testing/anytls.rs`） |
| 11 | "鉴权必须一次写出"没有回归守卫 | 保持现状（由 sing-box 互操作兜底） | 关闭 | 原文「保持现状」：单次 `read()` 的假服务端会抄来参考服务端的时序依赖，把守卫变成抖动。互操作一侧的首跑见 M2b #31 |
| 12 | AnyTLS 九条用例里四条各捆多个场景 | 保持现状 | 关闭 | 原文「保持现状」：共用代价高的 TLS 夹具与假服务端，每条断言带自己的说明 |
| 13 | `vmess/stream.rs` 文件头"从不调用 flush"已过时 | Task 12 改写这句注释 | 已做掉 | M2b 任务 12（e287ae0）；现文件头（`crates/rurge-proto/src/vmess/stream.rs`）已无这句 |
| 14 | 无用例钉住"一个方向的正常 EOF 不结束另一个方向" | 终审修复波补用例 | 已做掉 | 5c5a785：`a_clean_eof_in_one_direction_leaves_the_other_running`（`crates/rurge-engine/src/relay.rs`） |
| 15 | `relay_throughput` 接收循环无上界 | 保持现状 | 关闭 | 原文「保持现状」：`#[ignore]` 的手动用例 |
| 16 | 吞吐只测了 TCP，TLS / WS / VMess 栈上的 flush 成本未测 | M8 收尾时若做性能工作一并测 | M8 做掉 | M8c 任务 9（设计 5.11；设计第 7 节点名「flush 的成本」）：量过，结论是没有差别——两种 flush 时机在四种栈、四种缓冲上的 16 个比值在 96.5% ～ 102.0%，平均 100.3%——，每次写后 flush 保留（`6752afa`；修正轮 `d309f33` 去掉了时机这一维，对比可从 `6752afa` 重现）；与 #30 同题 |
| 17 | `PolicyRegistry::build` 文档没写 `previous` 须出自同一 `cell` | 终审修复波补句 | 已做掉 | 5c5a785：`crates/rurge-policy/src/registry.rs` 中 `build` 的文档注释 |
| 18 | 每个出站整代持有 `PolicySpec` 克隆与 Keystore base64 | 保持现状（P10 的取舍） | 关闭 | 原文「保持现状」，是 P10 的取舍 |
| 19 | `environment()` 是人工维护的约定 | M3 动工厂时复核 | 移交：下次动到时 | M3a 做了复核（M3a 计划「承接事项」C6：没给 `EngineFactory` 加按值字段，结论「继续跟踪」），之后没有哪份计划的「延后事项」接着记。现状：`environment()` 只含 `ipv6`（`crates/rurge-engine/src/outbounds.rs`），工厂的其余字段是进程级的；约定本身仍靠人工维护，给工厂加按值字段时要同步改 |
| 20 | 复用用例没覆盖 Keystore 口令 / 类型变化 | 评审后仍搁置（覆盖面缺口，非缺陷） | 关闭 | 原文「仍按现状搁置」，覆盖面缺口 |
| 21 | 证书指纹转十六进制的五行出现两次 | 下次改这个文件时合并 | 移交：下次动到时 | `tests/outbounds.rs` 已在 M2c 任务 6（288a529）拆分，这五行现在在 `crates/rurge-engine/tests/common/mod.rs`：`trojan_upstream` 与 `pin_of` 里仍是两份 |
| 22 | 无"改 client-cert → 出站重建"的端到端用例 | 评审后搁置（端到端链路留待后续里程碑） | 关闭 | 原文「仍按现状搁置」。单元用例覆盖：`ProtoSpec::tls()` 的两个新分支，注册表层 `a_changed_parameter_keystore_item_or_environment_rebuilds`。后续计划的「承接事项」与 `crates/rurge-engine/tests` 里没有接手这条端到端链路 |
| 23 | `outbounds` 测试二进制偶发 `STATUS_ACCESS_VIOLATION` | 保持跟踪（自 M4a 起的已知现象） | M8 做掉 | M8 设计 3.5（查明来源，没有代码改动）；M8a 计划「承接事项」"M2b"一行 |
| 24 | 守卫核对里"首包在超时之内"说过满 | 文档写成"在超时内入队" | 已做掉 | fca17f6：清单 `anytls` 行与 `docs/api/phase2.md` 的错误文本说明都写「入队」 |
| 25 | xray 夹具客户端只有 `id`，真实 xray 是否接受未确认 | 首次推送后的 CI | 移交：CI 首跑 | 夹具仍只渲染 `id`（`tests/interop/src/xray.rs`）；首跑结果还没读到：M8a 任务 6 还没有开始，要等推送 |
| 26 | 互操作 anytls 复用用例只断言往返成功 | 保持现状 | 关闭 | 原文「保持现状」：复用本身由回环假服务端的用例钉住；用例仍在 `tests/interop/tests/sing_box_tls_family.rs` |
| 27 | `sing_box.rs` 已 518 行 | M2c 动它时考虑按协议拆文件 | 已做掉 | M2c 任务 6（288a529；M2c 计划「承接事项」C2）：拆出 `sing_box_tls_family.rs` 与 `common/mod.rs`，`tests/interop/tests/sing_box.rs` 现 250 行 |
| 28 | vmess"连接在分块中间断开"文本也用于应答头读半截 | 保持延后（罕见路径） | 关闭 | 原文「保持延后」：罕见路径，改文本要动已验证的流代码。文本未变，`docs/api/phase2.md` 如实写了两种成因 |
| 29 | `crates/rurge-engine/tests/outbounds.rs` 已 1500 多行 | M2c 动它时按协议拆分 | 已做掉 | M2c 任务 6（288a529；M2c 计划「承接事项」C2）：该文件现 827 行（任务 14 的 `6c83d07` 去掉了四行），协议用例分到 `outbounds_tls_family.rs` 等文件 |
| 30 | 每 8 KiB 一次 flush 在 TLS / WS / VMess 栈上的成本 | M8 的性能工作里必须测 | M8 做掉 | M8c 任务 9（设计 5.11）：结论同 #16——每次写后 flush 在 TLS / WebSocket / VMess 栈上不花吞吐，保留（`6752afa`、`d309f33`） |
| 31 | "anytls 鉴权必须一次写出"的唯一守卫是 CI 运行 | 首次推送后盯住互操作 job | 移交：CI 首跑 | M8 设计 3.2 点名「sing-box 的 anytls 鉴权」；M8a 任务 6 还没有开始，要等推送 |
| 32 | `environment()` 第二例：`with_roots` 公开而根证书库不在指纹里 | M2c 迁 http / socks5 到 `Stack` 时复核，考虑改成派生值 | 已做掉 | M2c 任务 5、7（M2c 计划「承接事项」C3，计划期决定 P15、P16）：复核结论 `environment()` 不变；根证书库移进 `EngineShared.roots`（`crates/rurge-engine/src/shared.rs`，60943c8）。改成派生值考虑后没做，约定仍靠人工维护，见 #19 |

### M2c — `2026-09-21-phase2-m2c-shadow-tls-plan.md`（15 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | 每 8 KiB 一次 `flush` 在 TLS / WS / VMess / Shadow TLS 栈上的成本 | M8 | M8 做掉 | M8c 任务 9（设计 5.11；设计第 7 节点名 M2c #1）：结论同 M2b #16、#30——flush 的时机测不出差别，每次写后 flush 保留（`6752afa`、`d309f33`） |
| 2 | 首次推送后盯互操作 job 的 Shadow TLS 用例（三项） | 首次推送之后 | 移交：CI 首跑 | M8 设计 3.2 点名「Shadow TLS 的三项」，由 M8a 任务 6 核对；任务 6 还没有开始（要等推送后读 CI） |
| 3 | `vmess`（无 TLS）+ Shadow TLS v2 首帧等 100 ms | 有用户报告再说 | 移交：有用户报告再说 | `crates/rurge-proto/src/transport/lazy_head.rs` 里没有对 Shadow TLS v2 的特判，`HEAD_GRACE` 仍是 100 毫秒 |
| 4 | HelloRetryRequest：伪装握手只提供 X25519 | 有用户报告再说 | 移交：有用户报告再说 | 限制已登记：M2 设计附录 A，清单 `shadow-tls-version` 一行 |
| 5 | M2b 延后事项里不属于 M2c 的条目继续有效 | 见 M2b 计划 | 移交：下次动到时 | 指针行，点名三条：`ws-path` / `headers` 不是 `Secret` → M8a 任务 2（M2b #3）；偶发 `STATUS_ACCESS_VIOLATION` → M8 设计 3.5（M2b #23）；`environment()` 仍靠人工维护 → M2b #19，未了结的是这一条。「等」字涵盖的其余条目各在 M2b 小节有自己的结局 |
| 6 | 崩溃恢复顺序缺陷（坏配置先退出、后 `sysproxy.recover()`） | 单独跟进（等项目所有者点头后做小设计） | M8 做掉 | M8c 任务 8（设计 5.10、M8-D16；`6a473ed`、`9bab75e`）；M8c「承接事项」与设计第 7 节只写「M1b」，M2c #6 是同一缺陷的重复登记（原条目在 M1b 延后事项，M3a 延后事项 #9 也有一条） |
| 7 | 挂起的写与重试之间插入 flush 会让帧发两遍 | 补一句文档；有用户报告再说 | 移交：有用户报告再说 | 文档一半做掉：`crates/rurge-proto/src/transport/shadow_tls/framed.rs` 的模块文档写明重试之间不得 flush 或 shutdown。行为不变，`tokio` 与 `tokio-rustls` 不会这样调用 |
| 8 | 无用例覆盖零长度的 v3 ApplicationData 记录 | 有用户报告再说 | 移交：有用户报告再说 | `framed.rs` 的 `what_is_wrong_with_a_record_is_said_in_the_error` 只覆盖长度为 2 的记录；`crates/rurge-proto/src` 里没有零长度记录的用例 |
| 9 | `hello_tag` 在固定偏移切片，前提条件没写 | 补一句文档；有用户报告再说 | M8 做掉 | M8c 任务 12 的 B 部分在 `hello_tag` 的文档注释（`crates/rurge-proto/src/transport/shadow_tls/sign.rs`）补上前提条件一句（调用须在 `well_formed` 之后） |
| 10 | 自检 3、4 没有失败路径的测试 | rustls 升级时一并覆盖 | 移交：依赖升级时 | `sign.rs` 现有三条用例，没有自检 3、4 的失败路径 |
| 11 | 握手期部分验证通过又被当伪装流量的 v3 握手直接失败 | 有用户报告再说 | 移交：有用户报告再说 | 行为未变：以 `shadow-tls: the camouflage handshake failed: <原因>` 失败（`crates/rurge-proto/src/transport/shadow_tls/mod.rs`），`docs/api/phase2.md` 已列该文本 |
| 12 | 假服务端 `upwards` 提前返回漏 `stop.notify_one()` | 测试夹具；下次改动夹具时顺带修 | 移交：下次动到时 | `crates/rurge-proto/src/testing/shadow_tls.rs` 的 `upwards` 里仍是 `?` 先于 `stop.notify_one()`，未改 |
| 13 | Task 4 报告"三个 `allow(dead_code)`"实为四个 | 已在此更正；无需后续动作 | 已做掉 | 更正写在本行（即 M2c 计划「延后事项」里的这一行），无需后续动作 |
| 14 | 测试专用 `FlushGated::poll_shutdown` 不先写出挂起的缓冲 | 有用户报告或复用该类型时再说 | 移交：下次动到时 | 类型仍在 `crates/rurge-proto/src/transport/shadow_tls/mod.rs` 的测试模块，该路径未被使用 |
| 15 | `http` / `socks5` 握手的写在读应答前不 flush | 有用户报告或 M8 统一核对时再说 | M8 做掉 | M8c 任务 9 第 4 项（设计 5.11「顺带（M2c #15）」）：`a96eabf` 让 `crates/rurge-proto/src/http.rs` 的 CONNECT 请求与 `socks5.rs` 的问候、认证、请求写完即 flush（`socks5.rs` 的 `send` = `write_all` + `flush`），用例 `the_request_is_flushed_before_its_answer_is_awaited`、`every_step_is_flushed_before_its_answer_is_awaited`（下层是只在 flush 时转发的包装流，改动之前握手挂住） |

### M3a — `2026-09-23-phase2-m3a-subscriptions-plan.md`（34 行；原表有 # 列，按原编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | `[Host]` 豁免名单不含订阅节点的主机名 | 有用户报告再说；最迟 M8 | 移交：有用户报告再说 | M8 设计第 7 节「移交」表（M3a #1、#7、#15、#32：有用户报告或性能数据再说）；限制仍在：`ResolverConfig` 只取 `Config::proxy_hostnames()`（`crates/rurge-dns/src/resolver.rs`），订阅节点不在名单里（M3a 计划 P19） |
| 2 | `ChainConnector` 无运行期深度守卫 | M8 | 关闭 | M8 设计第 7 节「关闭」表：有界性由装配期的环检查保证，没有能构造出无界链的输入；M3b 计划承接事项 C6 同样保持不变 |
| 3 | 链底 REJECT 的 DNS 旁路；链深超限两处说法不一 | 已处理（M3b 任务 9） | 已做掉 | M3b 计划 P16 / 承接事项 C4 → 任务 9，提交 d176f4b；代码：`REJECT_BYPASSED`（`crates/rurge-engine/src/engine.rs`），用例在 `crates/rurge-engine/tests/pipeline.rs` |
| 4 | 测试二进制偶发异常退出 | 单独排查，保持跟踪 | M8 做掉 | M8 设计 3.5（查明来源，没有代码改动）；M8a 计划承接事项同列 |
| 5 | 布尔环境变量只认 `true` / `false` | 等项目所有者决定；单独小改动 | M8 做掉 | M8a 任务 4，提交 aa8837d：`crates/rurge/src/cli/mod.rs` 的 `switch()` 返回 `BoolishValueParser`，用例 `a_switchs_environment_variable_takes_the_usual_spellings`；M8 设计 3.4、M8-D6 |
| 6 | 每次重建把装配告警逐条再 WARN | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：`rebuild_registry` 每次对 `assembly.diagnostics` 逐条 `tracing::warn!`（`crates/rurge-engine/src/subscriptions.rs`）；M8 各计划与之后的计划都未点名 |
| 7 | 每次重建重新解析全部订阅 | 需要时（M8） | 移交：有用户报告再说 | M8 设计第 7 节「移交」表（有用户报告或性能数据再说）；限制仍在：`rebuild_registry` 每次对全部订阅快照重新 `assemble` |
| 8 | 兼容性清单附录的统计对不上 | 单独核对 | M8 做掉 | M8b 任务 5：计划「执行期修正记录」#20（附录整张重数；改之前写的合计 529，按每行第一个状态标记数出 538，附录自 M2a 起由各文档任务手工增减）与终审 #28，提交 71d03d0、68ba8ff；现附录合计 539 / 396 / 96 / 29 / 14 / 4（`docs/surge-compatibility-matrix.md`「附录：统计」）；M8c 任务 12 的 B 部分按脚本重数过一遍，每格不变（`e7d0eef`） |
| 9 | 坏配置时先退出、后 `sysproxy.recover()` | 单独跟进（等项目所有者点头） | M8 做掉 | M8c 任务 8（M8 设计 5.10；M8c 承接事项「M1b」一行；`6a473ed`、`9bab75e`）：`rurge run` 在配置读不了或有错误时先试一次系统代理恢复；本行与 M1b 登记的是同一缺陷，M8 设计第 7 节与 M8c 承接事项只写了 M1b，没写本行 |
| 10 | 两组导入同名策略时命名空间先于校验裁决 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：`Imports::collect` 按名字占位（`crates/rurge-policy/src/assemble.rs`，`to_spec` 的校验在它之后）；兼容性清单 5.2「成员装配顺序」一行登记了"同名时先声明的组那份生效"；M8 各计划未点名 |
| 11 | 订阅行能用主配置的 `client-cert` / 策略 | 已处理（M3b 任务 1） | 已做掉 | M3b 计划 P1 / 承接事项 C1 → 任务 1，提交 990f627、b41574f；代码：`reaches_into_profile` 对订阅行自己写的 `client-cert` 与指向主配置的 `underlying-proxy` 跳过并 `W0023`（`crates/rurge-policy/src/assemble.rs`）；兼容性清单 5.2 `policy-path` 行已登记 |
| 12 | 空组当中继时回退 DIRECT | 已处理（M3b 任务 2） | 已做掉 | M3b 计划 P2 / 承接事项 C2 → 任务 2，提交 f6253b2；代码：`PolicyRegistry::resolve_relay`（`crates/rurge-policy/src/registry.rs`）；兼容性清单 4.3 `underlying-proxy` 行已登记"一律失败、不回退 DIRECT" |
| 13 | 中继指向构建失败成员的导入条目仍在成员表 | 有用户报告再说；最迟 M8 | M8 做掉 | M8c 任务 7（M8 设计 5.8；M8c 承接事项「M3a #13」）；提交 cde2ce2，修正轮 2e4cb63 让告警至多列 20 个名字并带 `count` 字段：构建之后再过一遍导入与派生条目，中继链上有被略去者的一并略去，每组告警一条 |
| 14 | 会话先取当前代、后取注册表 | 已处理（M3b 任务 9） | 已做掉 | M3b 计划 P15 / 承接事项 C5 → 任务 9，提交 d176f4b；代码：`Engine::snapshot`（`crates/rurge-engine/src/engine.rs`）在代际锁下成对读取运行时与注册表 |
| 15 | 订阅重建占住工作线程 | 需要时（M8，与 #7 一起） | 移交：有用户报告再说 | M8 设计第 7 节「移交」表（有用户报告或性能数据再说）；限制仍在：`rebuild_registry` 同步装配并构建（`crates/rurge-engine/src/subscriptions.rs`） |
| 16 | `W0030` 只点名回边两端 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：加载期环检测的 `dfs` 只对回边告警（`crates/rurge-config/src/config.rs`）；M8 各计划未点名 |
| 17 | 坏的 `external-policy-modifier` 每行一条 `W0023` | 与 #6 一起 | 移交：有用户报告再说 | 随 #6（同一处：每次重建逐条记告警）；M8 各计划未点名 |
| 18 | `derive` 线性查名；派生名撞车 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在（`crates/rurge-policy/src/assemble.rs` 的 `derive`）；M8 各计划未点名 |
| 19 | `<random-string(…)>` 长度错误引用括号内文字 | 单独小改动 | M8 做掉 | M8a 任务 2，提交 b09b126：长度不是数字时错误不再引用括号里的文字（`crates/rurge-config/src/spec/http.rs`，用例 `a_placeholder_whose_length_is_none_is_not_quoted`）；M8 设计 3.3 |
| 20 | 计时类偶发失败；`rurge-dns` 用例空转 | 单独排查 | M8 做掉 | M8c 任务 15（`f28d67c`、`f0412ca`，修正轮 `dd22e04`）与任务 11（`6e22fa2`、`284c40a`）。空转查明并修好：`rurge_dns::fanout::resolve_name` 在循环醒来时已过查询的截止时间、还有轮次没发的情形下没有出口（`667af52` 引入），一个条件修复，用例 `a_fanout_kept_from_running_until_past_its_deadline_ends_as_a_timeout`。与原行的观察相符，但那次运行没有留下日志，不能证明就是它；没有证据表明还有第二个。四条计时类用例：`bootstrap::tests::set_upstreams_takes_effect_for_the_next_resolve` 在任务 11（改用不重发的 `once()`），`resolver::tests` 的三条（`a_partial_result_completes_aaaa_in_the_background`、`aaaa_suppression_after_five_timeouts_and_flush_resumes`、`cache_fresh_stale_negative_and_coalescing`）在任务 15（暂停的时钟与 `MockDns::upstream()`）；同一 crate 里同类的其余用例（`fanout::tests` 5 条、`resolver::tests` 6 条）在任务 15 的修正轮改掉；终审之后的 `cf8df9a` 又改了剩下的几条靠时间窗的用例（多线程用例改名为 `all_dropped_is_a_timeout_on_a_multi_thread_runtime`，TCP 等锁的用例与假服务端的 `misbehaviours` 不再靠时间窗，三条解析器用例改用不重发的选项） |
| 21 | 1 秒内多次更新只重建一次，无用例 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：去抖只由常数 `REBUILD_DEBOUNCE` 保证（`crates/rurge-engine/src/subscriptions.rs`），`crates/rurge-engine` 的用例里没有去抖的断言 |
| 22 | 订阅下载的 User-Agent | 已修（M3a 合并后的小修正） | 已做掉 | 提交 b6caa13：`build_stack_with` 把 `HttpClientConfig.user_agent` 设为 `rurge/<版本> (Surge-compatible)`（`crates/rurge-engine/src/stack.rs`）；兼容性清单 5.2 `policy-path` 行已登记 |
| 23 | 资源管理器 `Debug`；`meta.json` 里的完整 URL | 单独小改动 | 关闭 | `Debug` 一半：M8a 任务 5，提交 8fb7bf7（URL 只显示到主机与端口、正文只显示长度，用例 `the_debug_of_a_resource_shows_neither_its_url_nor_its_body`）。`meta.json` 里的完整 URL 一半不动：M8 设计 3.3「不动的」与 M8a 延后事项 #2（与缓存的订阅正文同在使用者的数据目录里），`Meta.url` 仍写盘（`crates/rurge-net/src/resource/cache.rs`）。M8 设计第 7 节把本行整行列在「M8 做掉」。终审之后的补充：`Entry::log_name` 的回退走 `ResourceSource::shown()`（`ec3bc9d`），`Display for ResourceSource` 与 `Response` 派生的 `Debug`（`final_url` 保留用户信息）今天没有东西打印（M8c 计划「延后事项」第 86 行） |
| 24 | `include-other-group` 取未派生成员，绕过中继 | 手工验收时对照 Surge 再定 | 移交：手工验收 | 现状已登记：兼容性清单 5.2 `underlying-proxy` 行（"经 `include-other-group` 取到的是未派生的成员"）；与 Surge 的对照加入 `docs/acceptance/phase2-manual.md`（M8c 任务 12 的 B 部分） |
| 25 | 订阅导入的 `external` 须一律跳过 | M4 设计必查 | 已做掉 | M4c 计划承接事项 C1 → 任务 1，提交 fe9bc16；代码：`Imports::collect` 对 `PolicyKind::External` 一律跳过并 `W0023`，`external-policy-modifier` 设什么都不例外（`crates/rurge-policy/src/assemble.rs`） |
| 26 | 漏写 `=` 的行让 `E0004` 回显口令 | 已修（M3a 合并后的小修正） | 已做掉 | 提交 b6caa13：`definition_entries` 遇到名字含逗号的行报 `E0017` 且不引用行内容（`crates/rurge-config/src/config.rs`，用例 `a_missing_equals_after_the_name_quotes_nothing_of_the_line`） |
| 27 | 恰好 100 000 行的订阅误报 `W0024` | 已修（M3a 合并后的小修正） | 已做掉 | 提交 b6caa13：`bound_lines` 只在后面还有内容时置位（`crates/rurge-policy/src/subscription.rs`，用例 `exactly_the_line_limit_is_not_cut`） |
| 28 | `[ Proxy ]` 表头不被识别 | 已修（M3a 合并后的小修正） | 已做掉 | 提交 b6caa13：`has_proxy_header` 按配置解析器的规则去掉方括号内的空白（`crates/rurge-policy/src/subscription.rs`，用例 `a_proxy_header_with_blanks_inside_is_the_proxy_section`） |
| 29 | 监视失败的告警带出被监视的目录 | 单独小改动 | 关闭 | M8 设计 3.3「不动的」：Windows 上"无法监视文件"告警里的目录路径，路径不是凭据 |
| 30 | `E0017` 引用整行、`E0001` 引用端口字段 | 已修（M3a 合并后的小修正） | 已做掉 | 提交 b6caa13：`E0017` 不再引用整行（同 #26）；端口字段只在是数字时引用（`crates/rurge-config/src/policy.rs` 的 `parse_policy`，用例 `an_invalid_port_is_quoted_only_when_it_is_a_number`） |
| 31 | `ProxyPolicy` 派生 `Debug` 带出原文 | 单独小改动 | M8 做掉 | M8a 任务 2，提交 b09b126：`ProxyPolicy` 手写 `Debug`，不打印 `positional` / `params`，`definition` 经 `redact_definition`（`crates/rurge-config/src/policy.rs`）；标记用例 `crates/rurge-config/tests/secrets.rs`；M8 设计 3.3 |
| 32 | 单行长度与本地文件大小无上限 | M8 | 移交：有用户报告再说 | M8 设计第 7 节「移交」表（有用户报告或性能数据再说） |
| 33 | 旧资源管理器的 `url_task` 睡满刷新间隔 | 单独排查 | M8 做掉 | M8c 任务 13（第 5 项；`8812e3f`，修正轮 `28340f3`）：`ResourceManager`（`crates/rurge-net/src/resource/mod.rs`）多一个 `gone` 的 `watch` 通道，管理器一没，它的 URL 任务立刻结束（正在抓取的等这次抓取结束，至多 `fetch_timeout`，缺省 30 秒），不论刷新间隔多长；自动刷新关闭的任务每分钟醒一次、只看间隔而不抓取——修正轮（`28340f3`）曾把这次醒来去掉，复评指出那是个回归（它是停着的任务看到同一 URL 的另一处引用并入的间隔的办法），最终修正波在 `8e581e7` 里恢复了它；管理器消失由 `gone` 立刻通知，不靠这次醒来。用例 `a_url_task_ends_with_its_manager`、`a_url_task_without_auto_refresh_waits_for_a_kick_and_for_its_manager` |
| 34 | 导入行的 `W0023` 无数量上限 | 与 #6 一起 | 移交：有用户报告再说 | 随 #6（同一处：每次重建逐条记告警）；M8 各计划未点名 |

### M3b — `2026-09-25-phase2-m3b-testing-auto-groups-plan.md`（23 行；原表有 # 列，按原编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | 测试会话不计字节 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：`crates/rurge-policy/src` 与 `crates/rurge-engine/src/auto.rs` 里没有 `Counting`（探针的流不计字节）；M8 各计划未点名 |
| 2 | `load-balance` 的 `persistent` 哈希用 `DefaultHasher` | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在（`crates/rurge-policy/src/auto.rs`）；兼容性清单 5.1 `load-balance` 行写明"哈希函数是 rurge 自己的" |
| 3 | `evaluate-before-use` 等待上限不计别组占用 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：`round_timeout` 按本组每 8 个一批计算（`crates/rurge-policy/src/registry.rs`） |
| 4 | 链中间跳是 `load-balance` 时 `persistent` 退化 | 有用户报告再说 | 移交：有用户报告再说 | M3b 计划 P10 的取舍（中间跳不等 `evaluate-before-use`、没有目标主机名）；M8 各计划未点名 |
| 5 | "网络已变化"没有来源 | 阶段 3（网卡 / 路由监视） | 移交：阶段 3 | M8 设计第 7 节「移交」表（M3b #5、#11，M4b #3、#19，M5a #19："网络已变化"的探测器，阶段 3） |
| 6 | `smart` 组的选择、测试结果、两列耗时 | M3c | 已做掉 | M3c 计划承接事项 C1 → 任务 1、4、6、7；提交 d36c95f（请求记录 `connectMs` / `firstByteMs`）、f17ae25 与 5babf5a（`smart` 的选择、重试与回报）、de46058（`automatic()`、`select` 覆盖、测试端点、抽样）、00ee28d（能力表翻转，`W0008` 只剩 `subnet`）；`docs/api/phase2.md`「`smart` 组」一节写明 `test_results` 与 `select` 接纳 `smart` |
| 7 | 测试二进制偶发 `STATUS_HEAP_CORRUPTION` | 单独排查（与 M3a #4 一起） | M8 做掉 | M8 设计 3.5（查明来源，没有代码改动）；M3c 承接事项 C4 照旧 |
| 8 | `policy_names` 无数量上限 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：`test_policies` 取全部 `policy_names`（`crates/rurge-api/src/routes/policies.rs`） |
| 9 | 订阅行带引号的整项碰上同名修饰 | 单独小改动 | M8 做掉 | M8c 任务 13（第 4 项；`8812e3f`，修正轮 `28340f3`）：`with_params`（`crates/rurge-config/src/policy.rs`）按策略行解析器的读法取键——整项带引号的 `"k=v"` 与不带引号的同名项是同一个参数；解析器当作类型、服务器与端口的三个字段不取键；追加的修饰在行尾一项留着未闭合的引号或括号时写在它前面、那一项原样不动。订阅行带引号的整项与以未闭合的引号结尾的行因此都照修饰的值导入；`client-cert` / `underlying-proxy` 的安全检查（`b41574f`）不动，它的 `` `external-policy-modifier` cannot set `<key>` on this line `` 一句经 `with_params` 已走不到，只剩兜底 |
| 10 | ssh / WireGuard 按名字用主配置材料，进订阅安全门 | M4 设计必查 | 已做掉 | M4a 计划 P11 / 承接事项 C1（`private-key`，提交 46d3dd4）与 M4b 计划承接事项 C1（`section-name`，提交 388fec6）；代码：`reaches_into_profile` 的键表 `["client-cert", "private-key", "section-name"]`（`crates/rurge-policy/src/assemble.rs`） |
| 11 | `invalidate_all` 管不到正在跑的测试 | 阶段 3（接上"网络已变化"时） | 移交：阶段 3 | M8 设计第 7 节「移交」表（同 #5） |
| 12 | `dial_internal` 被取消时会话句柄不结束 | 有用户报告再说 | 移交：有用户报告再说 | 同一缺口在 M7c #17 再次登记（"TCP 一侧是既有的做法"）；M8 设计第 7 节「移交」表：M7c #17 有用户报告再说，只影响记录与一次回报 |
| 13 | "服务端不保持连接"只告警一次的分支无用例 | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划与之后的计划都未点名；原去向不变 |
| 14 | 自动组每次拨号为每个成员构造 `TestCase` | M3c | 已做掉 | M3c 计划承接事项 C2 → 任务 2，提交 f3291ab：`TestBook::outcome`（`crates/rurge-policy/src/testbook.rs`），拨号只查表、不再为每个成员构造 `TestCase`；余下的每次拨号扫描成员见 M3c #9 |
| 15 | `TestBook` 不清理已消失策略的结果 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：`crates/rurge-policy/src/testbook.rs` 没有按策略清理结果的方法；M8 各计划未点名 |
| 16 | 已移除的组仍记下一轮测试时间 | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划与之后的计划都未点名；原去向不变 |
| 17 | 嵌套 `load-balance` 外层打分不看覆盖 | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划与之后的计划都未点名；原去向不变 |
| 18 | `override_of` 持状态锁写告警 | 单独小改动 | M8 做掉 | M8c 任务 13（第 3 项；`5cd3fc8`）：`override_of`（`crates/rurge-policy/src/auto.rs`）把要说的话带出 `state` 锁再写告警，文本、字段与「只说一次」不变；用例 `an_override_whose_member_is_gone_is_said_once` 钉住告警，改动前后都通过，它咬得住由变异证明。「写日志时锁确实已放开」靠读代码，没有用例 |
| 19 | 巨大的 `test-timeout` 让 `round_timeout` 溢出 | 单独小改动 | M8 做掉 | M8c 任务 13（第 1 项；`5cd3fc8`）：溢出点有两处——`PolicyRegistry::round_timeout` 的乘法（`evaluate-before-use` 的组多于 8 个可测成员、第一次拨号时 panic）与 `wireguard` 策略的 `test_spec` 里 `+ 10 秒` 的加法（加载配置时就 panic），都改饱和运算；用到这个取值的其余地方都经 `tokio::time::timeout`，本来就不 panic。用例 `a_round_of_tests_with_an_absurd_timeout_is_bounded_by_the_longest_duration`、`a_wireguard_policy_with_an_absurd_test_timeout_is_built`（`crates/rurge-policy/src/registry.rs`）与 `an_absurd_test_timeout_does_not_stop_a_round`（`crates/rurge-engine/tests/auto_groups.rs`） |
| 20 | `generation_lock` 中毒时 `expect` | 单独小改动 | M8 做掉 | M8c 任务 13（第 2 项；`5cd3fc8`）：`generation_lock`（`crates/rurge-engine/src/engine.rs`）改用 `PoisonError::into_inner`，锁中毒时不再 `expect`（这把锁只护着 `()`）；修正轮 `28340f3` 又把 `publish_generation` 里会失败的 `auto.retain` 挪到第一次 store 之前：已知会失败的都在第一次 store 之前；store 还会在最后一个持有者时丢掉被替换的东西（前一代的注册表与没有沿用的出站），没有已知会 panic 的 `Drop`（终审之后 `9c5ee82` 把两处注释改成只说已知的）。用例 `a_panic_under_the_generation_lock_stops_no_dial`（`crates/rurge-engine/src/subscriptions.rs`）；修正轮的顺序调整没有新用例（要让它 panic 得毒化 `rurge-policy` 里引擎够不着的锁），由 `reload`、`auto_groups`、`smart`、`subscriptions` 的现有用例守着 |
| 21 | `CommonOpts` 的 `Debug` 打印 `test_url` | 单独小改动 | M8 做掉 | M8a 任务 2，提交 b09b126：`CommonOpts` 手写 `Debug`，`test_url` 显示为 `***`（`crates/rurge-config/src/spec/common.rs`）；M8 设计 3.3；M3c 承接事项 C3 把它留在原处 |
| 22 | 策略 `test-url` 只按前缀校验 | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：策略的 `test-url` 只查 `http://` / `https://` 前缀（`crates/rurge-config/src/spec/common.rs`） |
| 23 | 探针、选法、引擎、API 的测试缺口 | 单独补测试 | 移交：下次动到时 | M8 各计划与之后的计划都未点名；原去向（单独补测试）不变 |

### M3c — `2026-09-26-phase2-m3c-smart-plan.md`（12 行；原表有 # 列，按原编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | 组环检测把 `smart` 的嵌套组当作边 | 单独小改动 | 关闭 | 登记为已知限制，不改（M8c 执行期的裁定）：去掉这条边要先证明每一处沿成员关系往下走的遍历都不会从 `smart` 组走进嵌套组，风险与收益不相称；绕开的办法是把那个本来就不起作用的嵌套组从 `smart` 组里删掉；差异由任务 12 的 B 部分登记进兼容性清单。现状：`group_edges` 只对 `subnet` 去掉 `include-other-group`，`smart` 写进去的嵌套组仍是边——加载时 `W0030`、运行期那个组 REJECT（`crates/rurge-config/src/config.rs`；M3c 计划 P20） |
| 2 | 改 `test-url` / `test-timeout` 清零 `SmartBook` 记录 | 有用户报告再说 | 移交：有用户报告再说 | M3c 计划 P2 的取舍；M8 各计划未点名 |
| 3 | DNS 会话不判断"上游先断" | 有用户报告再说 | 移交：有用户报告再说 | 限制仍在：TCP 的内部 DNS 会话经 `wrap_internal` 只计字节与首字节、不标记"上游先断"（`crates/rurge-engine/src/dns_pipeline.rs`）；M7c 加的是 QUIC 载体出错时的标记，不是这一项 |
| 4 | `CORE_VERSION` 仍报告 20 | 阶段 2 收尾（M8）时由项目所有者决定 | M8 做掉 | M8c 任务 10（M8 设计 6.3；M8c 承接事项「M3c #4」）：`CORE_VERSION` 改为 22（`f58678b`；FR-CFG-08 的方案），用例 `check_reports_core_version_22`（`crates/rurge/tests/cli.rs`） |
| 5 | 站点按目标主机名原样区分 | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划与之后的计划都未点名；原去向不变 |
| 6 | UDP 延迟与静默失败不计入 `smart` | M5 | 已做掉 | M5c 计划承接事项 C5 / P9 → 任务 4，提交 c84c18f（`crates/rurge-engine/src/udp.rs`、`smart.rs`）：载体打不开算失败、第一个回包是首字节、3 秒无回包只在 53 / 443 端口算失败；兼容性清单 5.1 `smart` 行已登记 |
| 7 | 重载后迟到的回报仍写进 `SmartBook` | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划与之后的计划都未点名；原去向不变 |
| 8 | 改定义后站点记忆仍是旧结果 | 有用户报告再说 | 移交：有用户报告再说 | M3c 计划 P14 的取舍；M8 各计划未点名 |
| 9 | 拨号路径的开销（记录锁、重试列表克隆） | 有性能数据再说 | 移交：有用户报告再说（或性能数据） | M8 各计划未点名；原去向（有性能数据再说）不变 |
| 10 | 只靠读代码核对、没有用例的行为 | 顺手补用例 | 移交：下次动到时 | M8 各计划与之后的计划都未点名；原去向（顺手补用例）不变 |
| 11 | 目标站点不通时失败计入全局分数 | 项目所有者决定 | M8 做掉 | M8c 任务 4（M8 设计 5.5、M8-D13；M8c 承接事项「M3c #11」）：`1325bbc` 与修正轮 `dd3aabc`。规则是 (b′)，不是设计写的 (b)：一次失败记下的那一刻，该站点的记忆里没有任何未过期的失败记录——不论是别的成员的还是这个成员自己上一次的——才另算成员自己的失败。本地就拒绝的目标（域名转不了 ASCII，或转出来超过 255 字节）整条会话当作不经这个组：不回报、不换成员、不记首字节与使用计数。N 个不同的失效地址仍罚 N 次（没有采用 (a) 的代价） |
| 12 | 上传中上游先重置，不计入成员 | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划与之后的计划都未点名；原去向不变 |

### M4a — `2026-09-27-phase2-m4a-ssh-plan.md`（15 行；原表有 # 列，按原编号）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | WireGuard 的 `section-name=` 进订阅安全门 | M4b | 已做掉 | M4b 计划承接事项 C1 → 任务 1，提交 388fec6；代码：`reaches_into_profile` 的键表含 `section-name`（`crates/rurge-policy/src/assemble.rs`） |
| 2 | 无 `server-fingerprint` 的告警按出站对象计一次 | 有用户报告再说 | 移交：有用户报告再说 | M4a 计划 P12 的取舍；M8 各计划未点名 |
| 3 | 老服务器的 `ssh-rsa`；DSA / 带口令私钥 | 有用户报告再说（登记为差异） | 移交：有用户报告再说 | 已登记：兼容性清单 4.6 `ssh` 行（只认 `ssh-rsa` 的老服务器用 RSA 私钥登录不了）与 1.6 `[Keystore]` 行（DSA、带口令的私钥在干构建期是 `E0022`）；其余按原去向 |
| 4 | 对 `sshd` 的互操作只在 CI 上运行 | 首次推送后看 CI | 移交：CI 首跑 | M8 设计 3.2 点名"`sshd` 与各参照服务端的夹具"；M8a 计划承接事项「各计划"首次推送后看 CI"的条目」→ 任务 6（还没有开始，要等推送） |
| 5 | Keystore 私钥的 Base64 解码与 p12 那段重复 | 阶段收尾时抽成共用函数 | M8 做掉 | M8c 任务 13（第 7 项；`eafd6be`）：`rurge_proto::keystore::decode_base64`，`decode_p12` 与 `rurge-proto-ssh` 的 `decode_private_key`（`crates/rurge-proto-ssh/src/keys.rs`）共用，报错文本不变；不带填充的 Base64 此前没有用例，补了 `base64_is_read_with_or_without_its_padding`；`rurge-proto-ssh` 的 `base64` 依赖改为随 `testing` 特性 |
| 6 | russh 应答队列随被拒的通道增长 | 向 russh 报告；有用户报告再说 | 移交：有用户报告再说 | M8 各计划未点名；没有向 russh 报告的记录 |
| 7 | 目标主机名是 IDN 时原样发给服务器 | M8（IDN） | M8 做掉 | M8c 任务 7（`010a9ca`；M8 设计 5.9；M8c 承接事项「M2b P15、M4a #7、M4b #8」）：`ssh` 在写 `direct-tcpip` 之前经 `to_ascii`，转不了以 `ssh: the host name cannot be sent to the server` 失败 |
| 8 | 无 RST 的断线要等保活判定 | 有用户报告再说（已登记） | 移交：有用户报告再说 | 已登记：兼容性清单 4.2 `ssh` 行（没有 RST 的断线要等保活判定约 2 分钟，其间新连接各自超时失败）；其余按原去向 |
| 9 | 口令只走 `password` 方法 | 有用户报告再说（已登记为差异） | 移交：有用户报告再说 | 已登记：兼容性清单 4.6 `ssh` 行（口令不走 `keyboard-interactive`）；其余按原去向 |
| 10 | `server-fingerprint` 一项后带注释是 `E0018` | 有用户报告再说 | 移交：有用户报告再说 | M8 各计划未点名；原去向不变 |
| 11 | russh 的 `log` 记录进 rurge 日志 | 日志策略由项目所有者决定 | M8 做掉 | M8c 任务 13（第 8 项；`eafd6be`）：决定（原留给项目所有者，由控制者代定）为 `russh` 只留 WARN 及以上，照 `boringtun` 与 `quinn` 的先例，`quiet_dependencies()`（`crates/rurge/src/cli/run.rs`）加一条；用例 `russh_says_only_its_warnings` |
| 12 | `smart` 给首个成员的时间与 SSH 握手 | 跨里程碑的设计问题，有用户报告再说 | 移交：有用户报告再说 | M8 各计划未点名；原去向不变 |
| 13 | CI 里装 `openssh-server` 的 apt 无重试 | 首次推送后看 CI；不稳就加 `Acquire::Retries` | 移交：CI 首跑 | M8 设计 3.2 / M8a 计划承接事项（各计划"首次推送后看 CI"的条目 → 任务 6）；`.github/workflows/ci.yml` 里该步仍无 `Acquire::Retries`，但已是 `continue-on-error: true`（M8b 终审，提交 33b38c0）；M8 设计 3.1 记着首跑时参照服务端的安装步骤都通过 |
| 14 | 测试覆盖缺口 | 以后顺手补 | 移交：下次动到时 | M8 各计划与之后的计划都未点名；原去向（以后顺手补）不变 |
| 15 | russh 会话任务在登录前空转 | 已修（合并后的修正） | 已做掉 | 提交 2ca41e5：交给 russh 的连接包一层外壳，拨号放弃时读取立即失败；密钥交换加登录合计最多 `LOGIN_LIMIT`（20 秒）（`crates/rurge-proto-ssh/src/outbound.rs`，用例 `a_handshake_whose_dial_gave_up_does_not_live_on`、`a_login_ends_at_its_limit_however_long_the_dial_may_take`）；M4a 计划「执行期修正记录」"合并后"一行 |

### M4b — `2026-09-28-phase2-m4b-wireguard-plan.md`（31 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | smoltcp 0.12 的 TCP 限制 | MSRV 升到 1.91 时换 smoltcp 0.13（所有者决定）；已登记差异 | 移交：依赖升级时 | 第 7 节「移交」表（M4b #1：MSRV 升到 1.91 时换 smoltcp 0.13 并逐条核对）；已登记：matrix 的 `wireguard` 行有「已知的 smoltcp 0.12 限制」一段 |
| 2 | WireGuard 的 UDP、`underlying-proxy`、`ecn` | M5 | 关闭 | 前两项已做掉：M5c 承接事项 C2，任务 1（69dc92a，`TunnelUdp`）与任务 2（2ab61e1，`ChainConnector::connect_udp`）。留着的是 `ecn`：M5c 承接事项 C2 与延后 #5 照 M5-D6 只解析（`W0029`），M7 设计第 1 节表与 3.4 明确不含 `wireguard`，matrix 的 `ecn` 行写「`wireguard` 上仍只解析」，`rurge-config` 的 spec 用例钉住 `assert_eq!(o.inert, ["ecn"])`；第 7 节与三份 M8 计划没有再提，按保持现状关闭 |
| 3 | "网络已变化"的探测器 | 阶段 3 | 移交：阶段 3 | 第 7 节「移交」表（M3b #5、#11，M4b #3、#19，M5a #19 → 阶段 3）；`network_changed` 在 `rurge-proto-wireguard` 之外没有调用者（grep）；matrix 的 `wireguard` 行写「阶段 2 没有触发它的探测器」；探测器要接的入口共三个，产品代码里都还没有调用者（只有用例调用）：`TestBook::invalidate_all`（`crates/rurge-policy/src/testbook.rs`，M3b #11）、`WireGuardOutbound::network_changed`（`crates/rurge-proto-wireguard/src/outbound.rs`，M4b #19）、`Resolver::on_network_change`（`crates/rurge-dns/src/resolver.rs`；需求审计查出，M8c 计划「延后事项」第 105 行） |
| 4 | 对 sing-box WireGuard 端点的互操作只在 CI 上跑 | 首次推送后看 CI | 移交：CI 首跑 | 用例在 `tests/interop/tests/sing_box_wireguard.rs`；M8a 计划承接事项「各计划『首次推送后看 CI』的条目 → 任务 6」，任务 6 还没有开始，要等推送；0154e73 的 CI 三平台 `cargo test` 失败、失败清单还没读到（M8 设计 3.1） |
| 5 | 每包开销以 UDP 系统调用为主；无批量收发 | 有吞吐问题报告、或阶段 3 做 TUN 时再看 | 移交：有用户报告再说 | 原去向未变，第 7 节与三份 M8 计划未认领；`rurge-proto-wireguard/src` 与 `crates/rurge-net/src/connector.rs` 里仍没有 `recvmmsg` / `sendmmsg` / GRO / GSO；M5c 延后 #4、#18 同题 |
| 6 | （已取消）隧道逐个启动 `STARTING` | — | 已做掉 | 行内已写取消；执行期修正记录「任务 6」一行（c1addb4，提交存在）去掉了全局启动锁；`rurge-proto-wireguard/src` 里已无 `STARTING`；matrix 的 `wireguard` 行写「隧道之间的启动不排队」 |
| 7 | 重载进行时旧配置的拨号得到 `newer configuration` | 不处理 | 关闭 | 原去向「不处理」（P10：有意为之，不来回争抢） |
| 8 | 隧道内 DNS 不查非 ASCII 的名字 | M8（IDN） | M8 做掉 | M8c 任务 7（`010a9ca`，修正轮 `2e4cb63`；M8 设计 5.9；M8c 承接事项「M2b P15、M4a #7、M4b #8 → 7」）：`wireguard` 隧道内 DNS 发问之前名字经 `to_ascii`（`crates/rurge-proto-wireguard/src/dns.rs`、`outbound.rs`）；隧道里的服务器问不了的名字被跳过、之后的 `system` 仍照问，搜索在这样一次跳过之后没有应答时才以 `wireguard: the host name cannot be looked up` 失败（只剩 `system` 时仍是 `dns lookup of … failed`）。用例 `the_tunnel_dns_is_asked_for_the_a_labels_of_a_name_in_unicode`、`a_name_no_question_can_carry_fails_the_lookup_at_once`（`outbound.rs`） |
| 9 | 同名 `[WireGuard <name>]`：首节有错时后一个也被丢 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #9，「同名的 `[WireGuard]` 节」） |
| 10 | 连接错误一律报 `cannot be connected to`，端口 0 同 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #10，「端口 0 的报错文字」） |
| 11 | 最后一个连接丢弃后 FIN / RST 随设备任务结束而发不出 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #11，「最后一个连接的 FIN」） |
| 12 | `FakeWgPeer` 沉默时若已有截止时间会空转 | 有用例这样用它时（最迟 M8） | M8 做掉 | M8c 任务 6 的修正轮（`996667e`）：`FakeWgPeer`（`crates/rurge-proto-wireguard/src/testing/peer.rs`）沉默时若已有截止时间，`serve` 的 `stack_due` 分支把 `due` 清掉再 `continue`，不再空转（修复前 300 毫秒里走了 212,345 次）；用例 `a_silent_fake_peer_waits_instead_of_spinning`，`FakeWgPeer::due_while_silent()` 交出计数 |
| 13 | 启动要等每个 peer 的载体拨完 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #13，「启动等每个 peer 的载体」） |
| 14 | 设备任务死掉（锁中毒）后不被替换 | M8 | M8 做掉 | M8c 任务 6（M8 设计 5.7）：a24b26d 把隧道的结束收成 `Shared::end`（被接替、设备任务结束含 panic、锁中毒三个入口），`is_closed()` 为真时出站的下一个连接换新隧道，持锁处不再 `expect`（`git grep 'expect("the tunnel")'` 无结果）；修正轮 `996667e` 让载体的两把小锁中毒也不 panic、隧道意外结束后在下一个连接到来时记一条 warn（`wireguard: the tunnel ended unexpectedly; the next connection starts a new one`）；用例 `a_tunnel_whose_task_died_is_replaced_by_the_next_connection`、`a_poisoned_stack_fails_the_call_and_the_next_connection_starts_a_new_tunnel`、`nothing_panics_on_a_poisoned_stack` |
| 15 | 「peer cannot be reached」告警不限频 | M5 | 已做掉 | M5c 承接事项 C3、计划期决定 P5（任务 2，2ab61e1）：`rurge-proto-wireguard/src/device.rs` 的 `may_say_unreachable`（每个策略的每个 peer 5 分钟至多一条 `warn!`）及其用例；matrix 的 `wireguard` 行写了这一句 |
| 16 | 缺用例：`set_tos` 失败后不再标记、`recv_any` 轮换 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表的「与只缺用例的几条」（5.7 开头也把「几条缺的用例」留在遗留事项里）；第 7 节没有逐条点名，这里把行文以「没有用例」起头的读作 #16、#18、#21、#29；已提交的 `outbound.rs` 里 `set_tos` 的测试替身恒返回 `Ok`，`recv_any` 没有用例（`git grep`） |
| 17 | SERVFAIL / REFUSED 的回答要等满一轮 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #17，「SERVFAIL 等满一轮」） |
| 18 | 缺用例：P4 响应码规则、`system` 失败后继续、IPv6 服务器 | M8 | 移交：有用户报告再说 | 同 #16（「只缺用例的几条」）；`dns::answer` 对 `NoError` / `NXDomain` 以外返回 `None` 的分支仍没有用例（已提交的 `rurge-proto-wireguard` 里没有 `ServFail`） |
| 19 | `network_changed()` 用 `try_lock`，事件会被丢 | 阶段 3，必须与探测器一起修 | 移交：阶段 3 | 第 7 节「移交」表（M4b #3、#19 → 阶段 3）；`outbound.rs` 的 `network_changed` 仍用 `try_lock`，引擎没有调用者 |
| 20 | `Device::connect` / `query` 不看 `is_closed()` | M8 | M8 做掉 | M8c 任务 6（M8 设计 5.7）：a24b26d，`connect` / `query` 经 `running()?` 在隧道结束时立即返回；用例 `a_dial_waiting_to_connect_ends_when_a_later_configuration_takes_over`、`a_lookup_waiting_for_its_answer_ends_when_a_later_configuration_takes_over` |
| 21 | 缺用例：P13 的日志规则 | M8 | 移交：有用户报告再说 | 同 #16；`wireguard: handshake completed` 与 `did not answer the handshake` 两句只出现在 `device.rs`，没有用例断言（`git grep`） |
| 22 | 共用隧道以启动它的策略名记日志 | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #22，「共用隧道的日志名」） |
| 23 | 共用从不提升隧道的代次 | M8 | M8 做掉 | M8c 任务 6（M8 设计 5.7）：a24b26d，`Device::share` 用 `fetch_max` 取较大的代次；用例 `a_shared_tunnel_belongs_to_the_latest_configuration_that_shares_it`、`a_tunnel_shared_after_dialling_belongs_to_the_later_configuration_too` |
| 24 | 拨号期 `Unsupported` 用 `set_error` 覆盖 `smart` 的「tried …」 | M5 | 已做掉 | M5c 承接事项 C4、计划期决定 P6、M5 设计第 21 节第 4 条：已无对象。现状核对：`rurge-proto` 里 `OutboundError::Unsupported` 的产生处（`external.rs`、`shadowsocks/mod.rs`、`snell/mod.rs`、`socks5.rs`、`h2connect/mod.rs`、`outbound.rs` 的默认实现）都在 `open_udp` 里，`rurge-proto-wireguard` 没有；TCP 拨号不再产生它 |
| 25 | bin 的 `Targets` 过滤把日志最大级别抬到 TRACE | M8 | 移交：有用户报告再说 | 第 7 节「移交」表（M4b #25，「日志级别提示」）；`crates/rurge/src/cli/run.rs` 的 `quiet_dependencies()` 仍是默认 TRACE 的 `Targets` |
| 26 | 忽略的吞吐基准没有截止时间 | M8 | 关闭 | 第 7 节「关闭」表（M4b #26：只影响测试代码的整洁，动到那些文件时再整理） |
| 27 | `outbounds_wireguard.rs` 两条用例的重复断言 | M8 | 关闭 | 第 7 节「关闭」表（M4b #27，「重复的断言」，理由同上） |
| 28 | `close()` 不唤醒 `handshake()` 的等待者 | M8 | M8 做掉 | M8c 任务 6（M8 设计 5.7）：a24b26d，`Shared::end` 唤醒 `handshaken` 的全部等待者，`handshake()` 在隧道结束时返回错误；用例 `a_native_test_waiting_for_the_handshake_ends_when_a_later_configuration_takes_over`、`a_native_test_waiting_for_the_handshake_ends_when_the_tunnel_closes` |
| 29 | 缺引擎用例：原生测速策略带 `url` 的 `POST /v1/policies/test` | M8 | 移交：有用户报告再说 | 同 #16；`crates/rurge-engine/tests` 里没有原生测速的用例，`rurge-api` 的 `policies_are_tested_on_request` 带 `url` 时只测 `DIRECT` |
| 30 | Linux 上 UDP 缓冲封顶在 `rmem_max` | 首次推送后看 CI | 移交：CI 首跑 | M8 设计 3.2 点名「Linux 上 UDP 缓冲封顶在 `rmem_max`」；M8a 计划承接事项 → 任务 6（还没有开始，要等推送）；留意的用例 `a_large_transfer_goes_through_whole` 在 `outbound.rs` |
| 31 | `established()` 不看隧道是否被接替；只等任一 peer 的握手 | 与 #20、#28 同类，有用户报告再说 | 移交：有用户报告再说 | 前半（隧道被接替时要等满拨号时限）M8c 任务 6 做掉：a24b26d，`Device::established` 在隧道结束时立即返回错误，用例 `a_lookup_waiting_for_the_first_handshake_ends_when_a_later_configuration_takes_over`。后半（多个 peer 时，DNS 服务器所在 peer 的发起包丢了而别的 peer 已握手，查询仍可能 2 秒后失败）未动：`established` 仍取任一 peer 的 `last_handshake()`，M8 设计 5.7 与任务 6 只写了结束时唤醒；第 7 节把 #31 列在「M8 做掉」里，只盖住前半 |

### M4c — `2026-09-29-phase2-m4c-external-plan.md`（9 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | Unix 上 rurge 崩溃后留下外部程序 | 有用户报告再说；已登记差异 | 移交：有用户报告再说 | 原去向未变，M8 未认领；已登记：matrix 的 `external` 行写「Unix 上 rurge 崩溃后留下的进程不处理」 |
| 2 | Windows：放进 Job Object 之前启动的子进程不在 Job 里 | 等 std 的 `PROC_THREAD_ATTRIBUTE_JOB_LIST` 稳定后改 | 移交：其它：等 std 稳定 `raw_attribute`（工具链升级时） | 原去向未变，M8 未认领；已登记为已知限制：M4 设计第 21 节（7.3 一行，P1） |
| 3 | Unix 上进程组号被复用时可能误发信号 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 4 | Unix 分支只在 CI 上编译与运行 | 首次推送后看 CI | 移交：CI 首跑 | M8 设计 3.2 点名「`rurge-platform` 与 `ExternalOutbound` 的 Unix 分支」；M8a 计划承接事项 → 任务 6（还没有开始，要等推送） |
| 5 | `external` 的 `udp-relay`（M5）、`addresses` 与「外部进程的流量走 DIRECT」（阶段 3） | M5 / 阶段 3 | 移交：阶段 3 | 第 7 节「移交」表（M4c #5 的其余部分 → 阶段 3，要有 TUN）。`udp-relay` 已做掉：M5a 承接事项 C2（P14，任务 6），`crates/rurge-proto/src/external.rs` 的 `open_udp`，用例 `tests/external/tests/outbound.rs` 的 `udp_goes_through_the_program`；matrix 的 `external` 行写了 `udp-relay` 与「`addresses`（阶段 3）解析但暂不生效」 |
| 6 | 持续崩溃的程序每 2 秒被拉起一次，无更长的退避 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 7 | 端口登记的两个少见窗口 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 8 | 最后的 `stop_external_programs()` 期间再按 Ctrl-C 不立即退出 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 9 | 任务评审记下的其余 Minor（`local-port=+80` 被接受等五条） | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领；五条里两条核对仍在：`crates/rurge-config/src/spec/external.rs` 用 `parse::<u16>()` 读 `local-port`（接受 `+80`），`spec/mod.rs` 的 Shadow TLS 错误文本仍是「a `external` policy」 |

### M5a — `2026-09-29-phase2-m5a-udp-foundation-plan.md`（21 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | UDP 流的 STUN 与 DNS 嗅探 | 需要时另议 | 移交：有用户报告再说 | 原去向未变；已登记：matrix 的 `PROTOCOL` 行写「UDP 流只识别 QUIC … STUN 与 DNS 的嗅探未做」 |
| 2 | 全锥时「收到其它来源的包」的标注 | 请求记录有说明字段时 | 移交：其它：等请求记录有说明字段 | 原去向未变；`crates/rurge-engine/src/observe.rs` 的 `RequestRecord` 仍只有 `error` 一个文字字段 |
| 3 | `ChainConnector::connect_udp` | M5c（`wireguard` 经 `underlying-proxy`） | 已做掉 | M5c 承接事项 C6、计划期决定 P3，任务 2（2ab61e1）：`crates/rurge-policy/src/cell.rs` 的 `ChainConnector::connect_udp`，`crates/rurge-net/src/packet_datagram.rs` 的 `packet_datagram` |
| 4 | UDP 流不计入 REJECT 的自动升级 | 需要时另议 | 移交：有用户报告再说 | 原去向未变；已登记：matrix 的「UDP 行为」行写「UDP 流的 REJECT 不计入 REJECT 的自动升级」 |
| 5 | `DirectPacket::recv_from` 先轮询 IPv4 再 IPv6 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变；`crates/rurge-net/src/connector.rs` 的 `recv_from` 仍按 `[&self.v4, &self.v6]` 的顺序轮询 |
| 6 | 声明端口的严格程度；BND 丢 IPv6 scope id | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变；`crates/rurge-inbound/src` 与 `crates/rurge-engine/src/udp.rs` 里没有 scope id 的处理（grep） |
| 7 | 刚交出的载体随即死掉时新流失败（其余已解决） | 以后顺手改 | 移交：下次动到时 | 划掉的部分已解决：M5a 执行期修正记录 #16、#17；余下一句的原去向未变，M8 未认领 |
| 8 | 载体表 `Carriers` 的条目在关联存续期间不删 | 以后顺手改 | 移交：下次动到时 | 原去向未变；`crates/rurge-engine/src/udp.rs` 的 `Carriers` 仍没有删条目的代码 |
| 9 | `Routes::flow_for` 按 `Target` 精确比较 | 已解决（余下的接受） | 已做掉 | M5a 执行期修正记录 #20（M5 设计第 23 节第 6 条）：`crates/rurge-engine/src/udp.rs` 的 `flow_for` 先按端口找最早的名字流；用例 `answers_for_a_name_find_a_flow_on_their_port`、`tests/udp.rs` 的 `answers_for_names_count_for_their_flows`；余下一条（同一载体上几条名字流共用一个端口时归最早的）行内写「接受」 |
| 10 | 持续使用的 DNS 流每 10 秒回收一次 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 11 | 入站关联端口的接收缓冲用系统默认值 | 有吞吐问题时 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 12 | `ChainConnector::open_udp` 与 `connect_tcp` 前导重复；几个分支缺单元测试 | M5c 动 `ChainConnector` 时 | 移交：下次动到时 | M5c 承接事项 C7「不做：`connect_udp` 复用 `open_udp`，没有第三份前导；照旧延后」，M5c 的「延后事项」表没有接这一行；`crates/rurge-policy/src/cell.rs` 的 `connect` 与 `open_udp` 仍各有一份「取注册表 / 已移除 / REJECT 带说明」的前导 |
| 13 | 用例的缺口（嗅探反例等五处） | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 14 | `tests/interop/README.md` 条目顺序、`socks-helper` 头注释、回显任务没 abort | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 15 | `socks5` 经不载 UDP 的 `underlying-proxy` 时先握手再报错、每 5 秒重来 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 16 | DIRECT 的 UDP 对 IP 字面量也套用 `ip-version` | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变；`crates/rurge-net/src/connector.rs` 的 `DirectPacket::send_to` 对 IP 字面量仍只在 `socket_for` 里按族拒绝 |
| 17 | 被 kill 的 UDP 流记成「完成」；只丢一个包的原因只记第一个 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 18 | 端口 53 的流是载体上唯一的流时，回收连带关掉载体 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 19 | 网卡绑定的载体在地址变化后不自愈 | 随阶段 3 的网络变化探测 | 移交：阶段 3 | 第 7 节「移交」表（M3b #5、#11，M4b #3、#19，M5a #19 → 阶段 3） |
| 20 | 双栈监听上 `SRC-IP` 规则匹配不到 IPv4 客户端 | 阶段 1 入站，以后顺手改 | M8 做掉 | M8c 任务 10 的修正轮（`0d7dac0`）：接受循环在交给 HTTP / SOCKS5 的处理之前把客户端的 `::ffff:a.b.c.d` 归成 IPv4（`canonical_peer`，`crates/rurge-inbound/src/listener.rs`），双栈监听上 `SRC-IP` 的 IPv4 网段命中 IPv4 客户端，请求记录里显示 IPv4；`HostName::canonical`（`rurge-config`）是目标地址与出站报回的来源归一的唯一实现（入站里另有几处各自归一的老代码：`udp.rs` 的 `canonical`、`restrict.rs`、`socks5.rs` 的 `reply_bound`，`rurge-proto` 的 `from_relay` 直接用 `IpAddr::to_canonical`）。用例 `an_ipv4_client_of_a_dual_stack_listener_is_named_by_its_ipv4_address`；接受循环里那个调用点只靠读代码核对，监听层面没有用例（Windows 上产品不设那个 socket 选项，造不出收 IPv4 客户端的 IPv6 监听） |
| 21 | SOCKS 地址编解码写了两遍；`ConnectionReset` 吞法散在 6 处；限频日志三套写法 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领；M8c 任务 2（`e6f65b7`、`df814a6`）又在 `crates/rurge-net/src/quic/carrier.rs`、`crates/rurge-engine/src/dns_pipeline.rs` 与 `crates/rurge-net/src/packet_datagram.rs` 各加了一处对 `ConnectionReset` 的特别处理，没有合并 |

### M5b — `2026-09-29-phase2-m5b-udp-tls-family-plan.md`（14 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | vmess 的 XUDP（全锥） | 需要时另议 | 移交：有用户报告再说 | 原去向未变；已登记：matrix 的 `vmess` 行写「对称型 … 没有 XUDP，M5-D5」 |
| 2 | vmess 的每目标连接随载体存续（已解决） | 已解决 | 已做掉 | M5b 执行期修正记录 #1：`crates/rurge-proto/src/vmess/udp.rs` 的 `IDLE`（60 秒）；matrix 的 `vmess` 行写「每条连接 60 秒既无读也无写就关闭并从表里摘掉」 |
| 3 | vmess 请求记录的 `connectMs` 记的是载体就绪的时刻 | 需要时另议 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 4 | Shadow TLS 上的 UDP 没有单独的用例 | 需要时另议 | 移交：有用户报告再说 | 原去向未变；`crates/rurge-engine/tests/udp_tls_family.rs`、`outbounds_shadow_tls.rs` 与 `tests/interop/tests/sing_box_shadow_tls.rs` 里没有 UDP 用例（grep） |
| 5 | 半途被丢弃的 `send_to` 会把流写乱（已解决） | 已解决 | 已做掉 | M5a 执行期修正记录 #18（合并后）：`crates/rurge-engine/src/udp.rs` 的 `SEND_TIMEOUT`，发送与关联结束赛跑、超时作废载体；用例 `a_send_that_hangs_fails_the_flow_and_its_carrier`、`the_end_of_the_association_cuts_a_send_short` |
| 6 | `StreamUdp::recv_from` 每个数据报分配小缓冲 | 有吞吐数据表明需要时 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 7 | anytls 的 UDP 载体：三处用例与假服务端的缺口 | 需要时另议 | 移交：有用户报告再说 | 原去向未变；M8c 任务 5 的 16b54c5（复用会话上的 UDP 载体）是看门狗的用例，不涉及这三条 |
| 8 | vmess 连接没回应就结束时下一个数据报重新拨号 | 需要时另议 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 9 | 互操作用例的 `send_to` 没套超时；kill 打断不了首包的拨号 | 需要时另议 | 移交：有用户报告再说 | 原去向未变；M8c 任务 11 第 2 项补的是 `hysteria2` 用例的时限（M7b #16），不含 vmess 的互操作用例 |
| 10 | 数据报与空闲到期赛跑可能丢回包；零长度数据报静默丢弃 | 接受 | 关闭 | 原去向「接受」；零长度一条已登记：matrix 的 `vmess` 行写「零长度的数据报不发送」 |
| 11 | trojan `connect_tcp` / `open_udp` 里重复的「栈上 open 外套超时」 | 有第三处时再抽出 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 12 | `LazyHead` 读写两半在两个任务里轮询会丢唤醒 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变；`crates/rurge-proto/src/transport/lazy_head.rs` 仍只保存读者的 waker（字段 `reader`），没有写者的 |
| 13 | 参考实现的限制（xray 的 8192 字节、sing-vmess 的 15000 字节）已登记 | 接受 | 关闭 | 原去向「接受」；已登记：matrix 的 `trojan` 行（xray 收到超过 8192 字节的数据报关整条连接）与 `vmess` 行（sing-vmess 把超过 15000 字节的回包拆成几个分块） |
| 14 | `stream_udp` 里 7 处按封装分支；`vmess/udp.rs` 与 `h2connect/udp.rs` 逻辑重复 | 以后顺手改 | 移交：下次动到时 | 原去向未变（M6c 计划延后 #11 同记），M8 未认领；两个文件都还在 |

### M5c — `2026-09-30-phase2-m5c-udp-wireguard-rest-plan.md`（19 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | `dns-follow-interface` 不跟随加密 DNS | 有用户要求时另建一套经网卡的加密上游 | 移交：有用户报告再说 | 原去向未变；已登记：matrix 的 `dns-follow-interface` 行写「配了 `encrypted-dns-server` 时不跟随」 |
| 2 | 规则匹配时的解析不跟随任何策略的网卡 | 接受 | 关闭 | 原去向「接受」（与手册一致）；matrix 的 `dns-follow-interface` 行写「规则匹配时的解析发生在选定策略之前，仍走全局」 |
| 3 | UDP 测试的结果不保存、不进 `test_results`、不进请求记录 | 有展示需求时另议 | 移交：有用户报告再说 | 原去向未变；已登记：`docs/api/phase2.md` 的 UDP 测试一段写「UDP 测试的结果不保存、不影响任何组」，matrix 的「UDP 测试」行同 |
| 4 | 隧道里 UDP socket 的缓冲固定 64 包 / 256 KiB | 有吞吐或内存问题时 | 移交：有用户报告再说 | 原去向未变，M8 未认领；M4b #5、M5c #18 同题 |
| 5 | `ecn` 仍只解析 | 随 QUIC 族 | 已做掉 | M7a / M7b / M7c 让 `ecn` 在 `hysteria2`、`tuic` / `tuic-v5`、`masque` 与 `h3=true` 的 `trust-tunnel` 上生效（matrix 的 `ecn` 行）；`wireguard` 上仍只解析，M7 设计第 1 节与 3.4 明确不含，归 M4b #2 的 `ecn` 部分（关闭） |
| 6 | `udp::a_closed_port_does_not_break_the_carrier` 偶发失败 | 以后顺手改 | M8 做掉 | M8c 任务 11（`6e22fa2`）：`a_closed_port_does_not_break_the_carrier`（`crates/rurge-engine/tests/udp.rs`）原来先绑定再释放端口当关着的端口，并行时该号可被旁边的用例占用；改用 `crates/rurge-engine/tests/common/mod.rs` 的 `closed_udp_port()`（取自 20000–29999、现场试绑，不取自系统分配的区间）。让端口一直被占着不行：在 Windows 上被占用的端口不产生 ICMP「端口不可达」，那样改了这条用例测的东西 |
| 7 | `packet_datagram` 发送失败只记 `trace!`；名字目标的回包不按来源过滤 | 接受 | 关闭 | 原去向「接受」；其后 M7c 的 4f9bbbc 已把载体发送失败的原因交给下一次接收（`lost_alone` 除外），名字目标不过滤的一半不变（`packet_datagram.rs` 的 `from_peer`） |
| 8 | 隧道里 UDP 载体：`recv_from` 先看 IPv4 等三处 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 9 | 按来源过滤回包时精确比较 `Target` | 有用户报告再说 | M8 做掉 | 终审之后的最终修正波（`8e581e7`、`b14b45a`）：`packet_datagram`（`crates/rurge-net/src/packet_datagram.rs`）的接收任务把对端与每个收到的来源先取 `HostName::canonical` 再比较，`udp_probe`（`crates/rurge-policy/src/udp_probe.rs`）同样，`rurge-proto` 的 `from_relay`（SOCKS5 UDP 与 Shadowsocks UDP 共用）用 `IpAddr::to_canonical` 比较；中继或服务器从 IPv4 映射的 IPv6 地址回包不再被丢掉。用例 `a_peer_is_heard_from_its_ipv4_address_in_ipv6_form`、`an_answer_from_the_other_form_of_the_servers_address_is_its_answer`、`the_relay_is_the_relay_in_either_form_of_its_address`、`the_relay_is_heard_from_its_address_in_either_form`。名字目标的回包不按来源过滤的一半不变（`from_peer` 只对地址目标设） |
| 10 | 链式载体：收包任务遇任何收包错误都结束；`BrokenPipe` 不算 `gone()` | 以后顺手改 | 移交：下次动到时 | 原去向未变；`packet_datagram.rs` 的收包任务仍 `Err(e) => break e`（M8c 任务 2 只改了它交给接收方的结束原因），`device.rs` 的 `gone()` 仍不含 `BrokenPipe` |
| 11 | 换载体：`connect_udp` 超过 10 秒时可能同时两次拨号；换过载体后连不上的 peer 每 10 秒重拨 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 12 | 告警限频表 `UNREACHABLE_SAID` / `ENDED_SAID` 从不清理 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 13 | UDP 测试：无 `test_case` 的策略没有 UDP 结果等三处 | 需要时另议 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 14 | `smart` 的 UDP：载体打不开的任何错误都算成员失败等 | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 15 | `dns-follow-interface`：上游与缓存只按网卡名区分；socket 用 7 MiB 缓冲 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 16 | 经组中继的 UDP 载体不跟随组的切换（只有 `wireguard` 跟随） | 有用户报告再说 | 移交：有用户报告再说 | 原去向未变，M8 未认领 |
| 17 | `dns-follow-interface` 把 AAAA 迟到的部分结果当完整结果缓存；`lookup_on` 复制了主路径 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |
| 18 | WireGuard 设备一批收 256 个包，隧道里 UDP socket 只有 64 个包的缓冲 | 有吞吐问题时 | 移交：有用户报告再说 | 原去向未变，M8 未认领；M4b #5、M5c #4 同题 |
| 19 | `udp_probe` 的错误文本带查询名 | 以后顺手改 | 移交：下次动到时 | 原去向未变，M8 未认领 |

### M6a — `2026-09-30-phase2-m6a-shadowsocks-plan.md`（15 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | 流式旧方法（P5） | M8 | M8 做掉 | M8b 任务 2（3ce73f9、d6d3835），设计 4.1：`crates/rurge-config/src/spec/ss.rs` 的 `SsMethod` 有手册列出的 11 个流式取值，`crates/rurge-proto/src/shadowsocks/stream.rs` 的 `Keystream` / `CipherStream`，加载告警 `W0031`；手册没列的 9 个名字仍是 `W0007` + REJECT |
| 2 | obfs 没有参考服务端，只能手工验收 | 手工验收；有参考服务端时补互操作 | 移交：手工验收 | 互操作部分已做：M8b 任务 4（a521b65）用 mihomo 的监听端覆盖 simple-obfs `http` / `tls`（`tests/interop/tests/mihomo.rs` 的 `ss_behind_simple_obfs_against_mihomo`、`snell_behind_simple_obfs_against_mihomo`），M8 设计第 12 节第 8 条；用例没有在真实二进制上跑过，等 CI 首跑。真实 `obfs-server` 节点一项仍在 `docs/acceptance/phase2-manual.md` 的 M6a 一节（obfs 项未勾） |
| 3 | 没有"口令错"的互操作用例 | 需要时另议 | 移交：其它：需要时另议 | `tests/interop/tests/shadowsocks.rs` 没有口令错的用例；M8b 对流式方法同样不写（M8b 执行期修正 #16：参照服务端按解出的乱码去连任意地址，或一直读下去）；M8 设计第 7 节没有点名 |
| 4 | 假服务端多用户只有一层、salt 不过期 | 接受（测试设施） | 关闭 | 原去向"接受"：只是测试设施的简化 |
| 5 | `2022-blake3-chacha20-poly1305` | 手册列出时 | 移交：其它：手册列出时 | 兼容性清单 4.6 `ss` 行写明它不在 Surge 的方法表里、不接受；`SsMethod` 没有这个取值；M8 设计没有再提 |
| 6 | README 路线图停在 M4b；总设计 Snell 写 v1 ～ v4 | M6b 的文档任务订正 | 已做掉 | M6b 任务 6 的文档提交 15e2d33：README 阶段 2 路线图一行从停在 M4b（55868cc 时的样子）改为写到 M6b；总设计第 1.4 节 M6 行与第 2 节 Snell 行现写 v4 / v5（`2026-09-19-phase2-outbound-groups-design.md` 第 55、78 行），`v1 ～ v4` 已无出现 |
| 7 | `udp-port` 无 `udp-relay` 时静默；`ObfsOpts` 的 `Debug` 打印 `obfs-host` | 以后顺手改 | M8 做掉 | 两部分都做：`udp-port` 没有 `udp-relay=true` 报 `W0028` → M8b 任务 1（4203eb3，`spec/ss.rs`，设计 4.5）；`ObfsOpts` 手写 `Debug`，`host` 与 `uri` 显示 `***` → M8a 任务 2（b09b126，`spec/obfs.rs`，设计 3.3） |
| 8 | 写路径"同一缓冲重试"约定 | 接受 | 关闭 | 原去向"接受"：与 `WsByteStream` / `VmessStream` 是同一约定 |
| 9 | `AeadStream` 三处细节 | 以后顺手改 | 移交：下次动到时 | 原去向"以后顺手改"；M8 设计第 7 节没有点名 |
| 10 | 密钥材料不清零 | 需要时另议 | 移交：其它：需要时另议 | 工作区没有 crate 使用 `zeroize`（`Cargo.lock` 里只是传递依赖），`Secret<T>` 没有 `Drop`；M8 设计第 7 节没有点名 |
| 11 | SS 2022 UDP 的淘汰、派生、来源过滤 | 有用户报告再说 | 移交：有用户报告再说 | 仍是已知限制；M8 设计第 7 节没有点名 |
| 12 | `ss.rs` 的脆弱断言、假 obfs 服务端的 EOF | 以后顺手改 | 移交：下次动到时 | 原去向"以后顺手改"（测试设施）；M8 设计第 7 节没有点名 |
| 13 | 手工验收的时钟偏差文本；`ssserver` 互操作配置只在 CI 上验证 | 以后顺手改；CI 首跑时核对 | 移交：CI 首跑 | `ssserver` 夹具（M8b 又加了九种流式方法）没有在真实二进制上跑过：M8 设计 3.2 的首跑核对，M8a 任务 6 等推送后的 CI 结果；时钟偏差一项：手工验收清单 M6a 一节「时钟偏差」一项已写成"同上一项的 2022 文本"，没有采用该事项提的 `clock differs` 文本 |
| 14 | `obfs=tls` 的 ClientHello 至多约 16 KiB | 手工验收时留意；需要时另议 | 移交：手工验收 | 兼容性清单 `ss` 行登记 16384 字节的上限；代码没改（线上行为的改动，没有参考服务端可验）；`docs/acceptance/phase2-manual.md` 手工验收 M6a 一节的 obfs 项加入这条留意事项（M8c 任务 12 的 B 部分） |
| 15 | `obfs-uri` 不在脱敏名单 | 已加进名单 | 已做掉 | db86f0c：`crates/rurge-config/src/redact.rs` 的脱敏名单有 `obfs-uri`，用例 `an_obfs_line_loses_its_host_and_path` |

### M6b — `2026-09-30-phase2-m6b-snell-plan.md`（9 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | `version` 1–3 与 6、v5 的 QUIC Proxy Mode | M8（需要时） | 移交：项目所有者 | 三部分：v1–v3 做掉——M8b 任务 3（23af242、b106a60），设计 4.3，`SnellVersion::V1`–`V3`、`crates/rurge-proto/src/snell/chunked.rs`；v5 的 QUIC Proxy Mode 关闭——M8 设计 6.1，第 7 节关闭表；v6 待项目所有者表态——可行性报告 `docs/reports/2026-10-08-phase2-feasibility-snell-v6-quic-tailscale.md` 第 1 节，第 7 节移交表；`version=6` 仍是 `W0007` + REJECT |
| 2 | v5 的动态记录大小（P4） | 接受（只影响发送方） | 关闭 | 原去向"接受"：只影响发送方 |
| 3 | `obfs-host` 缺省、命令字与 Surge 的差异；`udp-port` 只解析 | 有真实 Surge 抓包或用户报告时对齐 | 移交：有用户报告再说 | 两处差异已登记（兼容性清单 `snell` 行："Surge 的缺省据报是 `bing.com`"、"据报 Surge 总是发 `0x05`，未核对"）；`udp-port` 仍只解析（`W0029`）；M6b 的"v5 的 QUIC Proxy Mode 实现时再让它生效"把它同该模式画等号，手册没有这样说，注释与清单里的这句话已由任务 12 的 B 部分（`e7d0eef`；M8b 延后 #9）换成手册的原话；rurge 不跟随 `udp-port`，因为连到它再照样叠 Shadow TLS，在 Shadow TLS 占着 TCP 端口的部署里必然握手失败，而 Surge 往那个端口发什么不知道 |
| 4 | 互操作数不了复用的连接数；snell-server 是否默认开 UDP | CI 首跑；手工验收 | 移交：CI 首跑 | 复用已在 mihomo 的日志里数（M8b 任务 4，`snell_v2_requests_share_a_connection_against_mihomo`）；官方 snell-server 的 `udp_against_snell_server`（`tests/interop/tests/snell.rs`）没有在真实二进制上跑过：M8 设计 3.2，M8a 任务 6 |
| 5 | 陈旧连接的重试：至多 64 KiB，只在 1 秒内，会重发 | 接受 | 关闭 | 原去向"接受"，已登记（兼容性清单 `snell` 行："同一个请求可能到达目标两次"） |
| 6 | 配置、记录流、池、UDP、测试的小项 | 以后顺手改 | 移交：下次动到时 | 原去向"以后顺手改"；M8 设计第 7 节没有点名 |
| 7 | 每条记录分配 `Vec`；Snell UDP 经 Shadow TLS 无用例 | 需要时另议 | 移交：其它：需要时另议 | 原去向"需要时另议"；M8 设计第 7 节没有点名 |
| 8 | 隧道假定同一任务轮询（已划掉）及另外三处 | 接受 | 关闭 | 划掉的一项合并后已修（执行期修正 #10，`snell/tunnel.rs` 的 `Waiters`，用例 `a_redial_wakes_both_halves_of_the_tunnel`）；其余三处原去向"接受" |
| 9 | `reuse=false` 时应用半关闭后还发 FIN | 互操作首跑时核对；手工验收 | 移交：CI 首跑 | M8 设计 3.2 点名"Snell `reuse=false` 的半关闭"；M8a 任务 6 等推送后的 CI 结果；手工验收 M6b 一节的 `reuse` 项会走到 `reuse=false` |

### M6c — `2026-10-01-phase2-m6c-http2-plan.md`（11 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | `trust-tunnel` 的 `h3=true` | M7 | 已做掉 | M7c 承接事项 C8 → 任务 1、5、8：`TrustTunnelSpec.h3`（`crates/rurge-config/src/spec/h2.rs`，4dc2dec）、`TrustTunnelH3Outbound`（`crates/rurge-proto-quic/src/trust_tunnel.rs`，9c3ec6e）、工厂分支（`crates/rurge-engine/src/outbounds.rs`，edc1795）、`crates/rurge-engine/tests/outbounds_http2.rs` 的 h3 用例 |
| 2 | CONNECT-UDP over HTTP/2 没有参考服务端 | 手工验收；有参考服务端时补互操作 | 移交：手工验收 | `docs/acceptance/phase2-manual.md` M6c 一节「`h2-connect` 的 UDP」一项；`tests/interop/tests/http2.rs` 仍只测 TCP；M7c 的 masque-server 只是 HTTP/3 的参照 |
| 3 | `h2-connect` 对 endpoint 不发 `user-agent` 是否可行 | CI 首跑 | 移交：CI 首跑 | `h2_connect_against_the_endpoint`（`tests/interop/tests/http2.rs`）没有在真实二进制上跑过：M8 设计 3.2，M8a 任务 6 |
| 4 | ALPN 不与 `h2` 重合时是 rustls 原文 | 接受 | 关闭 | 原去向"接受" |
| 5 | 挑中连接后、发出前连接死掉不重试 | 有用户报告再说 | 移交：有用户报告再说 | 原去向照旧；M8 设计第 7 节没有点名 |
| 6 | 互操作数不了 endpoint 那边的连接数 | 接受 | 关闭 | 原去向"接受"：连接数由 `FakeH2Proxy` 覆盖 |
| 7 | 通告 `MAX_CONCURRENT_STREAMS = 0` 的服务端 | 有用户报告再说 | 移交：有用户报告再说 | 原去向照旧；M8 设计第 7 节没有点名 |
| 8 | `h2` 0.4.19 的 `data_frame_budget` 与 ENHANCE_YOUR_CALM | 有用户报告再说 | 移交：有用户报告再说 | 原去向照旧；M8 设计第 7 节没有点名 |
| 9 | CONNECT-UDP 已有目标的数据报不触发存活检查 | 需要时另议 | 移交：其它：需要时另议 | 原去向照旧；M8 设计第 7 节没有点名 |
| 10 | 重开流用例缺一条断言；`vmess/udp.rs` 的同类循环未修 | 需要时另议 | M8 做掉 | M8c 任务 13（第 6 项；`8812e3f`，修正轮 `28340f3`）：`crates/rurge-proto/src/vmess/udp.rs` 的 `send_to` 取到已结束的连接就重拨的循环抽成 `VmessUdp::connection`（拨号作参数），同 `h2connect/udp.rs` 一样只重开一次，第二次仍是已结束的连接时以 `vmess: the server ended the UDP connection`（`ConnectionAborted`）失败。两处的上界各有确定性的用例 `a_connection_that_has_ended_is_dialled_again_only_once`、`a_stream_that_has_ended_is_opened_again_only_once`；修正轮让两条用例在错误之后再调一次同一个函数，钉住上界按每次调用计、不跨调用累计 |
| 11 | 小项（超长检查的次序、`te: trailers`、两处近乎重复的载体） | 以后顺手改 | 移交：下次动到时 | 原去向"以后顺手改"；M8 设计第 7 节没有点名 |

### M7a — `2026-10-02-phase2-m7a-quic-hysteria2-plan.md`（13 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | HTTP/3 应答 `Content-Length` 比对；SETTINGS 0x08 / 0x33 取值校验 | M7c | 已做掉 | M7c 承接事项 C2 → 任务 2（3ba28be）：`crates/rurge-net/src/quic/h3/frame.rs` 对大于 1 的取值报 `H3_SETTINGS_ERROR`，`h3/stream.rs` 核对 DATA 总长，用例 `content_that_its_content_length_does_not_match_is_malformed`（`h3/mod.rs`） |
| 2 | quinn 没有应用层 PING | 接受；M8 再评估 | 关闭 | M8 设计第 7 节关闭表（与 M7b #13 同列）：保活 10 秒加空闲超时 30 秒是 Hysteria 与 quinn 的常用取值；已登记（兼容性清单 `hysteria2` 行："quinn 0.11 没有应用层的 PING"） |
| 3 | 认证或握手反复失败没有退避 | 有用户报告再说 | 移交：有用户报告再说 | `crates/rurge-proto-quic/src/` 里没有退避；兼容性清单 `tuic` / `masque` 行写"不退避"；M8 设计第 7 节没有点名 |
| 4 | Hysteria 2 UDP 相邻分片包的号相同时误拼 | 接受 | 关闭 | 原去向"接受"：同参考实现 |
| 5 | `h3=true` 的 `trust-tunnel` 与 Shadow TLS 的 `E0018` | M7c | 已做掉 | M7c 承接事项 C3 → 任务 1（4dc2dec）：`crates/rurge-config/src/spec/mod.rs` 对 `TrustTunnel` 且 `h3` 调 `shadow_tls::refuse`，报 `E0018` |
| 6 | `gecko-password` 是否等于公开规范里的 Gecko | 手工验收 | 移交：手工验收 | `docs/acceptance/phase2-manual.md` M7a 一节的 Gecko 一项（M7-D11） |
| 7 | 端口跳跃对真实服务端的互操作 | 手工验收 | 移交：手工验收 | 清单 M7a 一节的「端口跳跃」一项 |
| 8 | `ecn` 在纯 TCP 协议上是 `W0029` | M8 | M8 做掉 | M8b 任务 1（4203eb3），设计 4.5：`crates/rurge-config/src/spec/mod.rs` 对不适用的策略报 `W0028`（`` `ecn` does not apply to `<kind>` policies ``） |
| 9 | CONNECT 的 2xx 之后再来 HEADERS | M7c | 已做掉 | M7c 承接事项 C4 → 任务 2（3ba28be）：`crates/rurge-net/src/quic/h3/stream.rs`：隧道上收到 HEADERS 即 `H3_FRAME_UNEXPECTED` |
| 10 | `:authority` 保留了 userinfo | M7c | 已做掉 | M7c 承接事项 C5 → 任务 2、6：`crates/rurge-net/src/quic/h3/client.rs` 去掉 userinfo（3ba28be）；`crates/rurge-dns/src/upstream/mod.rs` 带用户名口令的 `h3://` / `quic://` 是 `W0026`（c737591、7f51cc7） |
| 11 | HTTP/3 隧道要先 `flush` 再等对端 | M7c | 已做掉 | M7c 承接事项 C6 → 任务 2、4、6：`quic/h3/stream.rs` 的模块文档写明半帧只随写、flush、shutdown 推进；`crates/rurge-proto-quic/src/connect.rs` 的 `Tunnel::poll_read` 读时先 `poll_flush`（531243b） |
| 12 | 被淘汰的连接最后一个租约放手时立刻关闭 | 有用户报告再说（或 M8） | 移交：有用户报告再说 | M8 设计第 7 节移交表 |
| 13 | 混淆口令短于 4 字节无加载期告警 | M8 再议 | M8 做掉 | M8b 任务 1（4203eb3），设计 4.5：`crates/rurge-config/src/spec/hysteria2.rs` 报 `W0012`（不引用取值，照常使用），用例 `a_short_obfuscation_password_is_reported_and_used`；兼容性清单 `hysteria2` 参数行已写 |

### M7b — `2026-10-03-phase2-m7b-tuic-plan.md`（16 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | tuic 0.8.5 的 SHA-256 | 项目所有者 | 已做掉 | M7c 任务 9（执行期修正 #10，0154e73）：`.github/workflows/ci.yml` 钉住 `e4b5d5dc…`，取自发布自带的 `.sha256sum` |
| 2 | `quic` 模式没有参考服务端 | 手工验收 | 移交：手工验收 | `docs/acceptance/phase2-manual.md` M7b 一节"不接受 QUIC 数据报的服务端"一项 |
| 3 | v5 的"口令错"只能推断 | 接受，已登记 | 关闭 | 原去向"接受"，已登记（兼容性清单 `tuic` 行："这是推断（登记的差异）"） |
| 4 | `quic` 模式下引擎的发送时限不再起作用 | 接受，已登记 | 关闭 | 原去向"接受"，已登记（`tuic` 行："`SEND_TIMEOUT` 在这里不再起作用"） |
| 5 | 认证或握手反复失败没有退避 | 有用户报告再说 | 移交：有用户报告再说 | 同 M7a #3 |
| 6 | tuic 0.8.5 的命令行、配置键、就绪日志未核对 | 第一次 Linux CI 运行时核对 | 移交：CI 首跑 | SHA-256 已钉住（见 #1），核对仍等第一次 Linux CI：M8 设计 3.2，M8a 任务 6 |
| 7 | v5 线上细节只对回环假服务端验证 | 第一次 CI 的 sing-box 互操作 | 移交：CI 首跑 | `v5_tcp_against_sing_box`、`v5_udp_against_sing_box`（`tests/interop/tests/tuic.rs`）没有在真实 sing-box 上跑过：M8 设计 3.2，M8a 任务 6 |
| 8 | SOCKS5 对 IPv4 映射地址的回包归流 | M8 | M8 做掉 | M8c 任务 10（`21ecb85`，修正轮 `0d7dac0`；设计 5.12）：SOCKS5 入站把 `::ffff:a.b.c.d` 的目标（地址类型 4 或写成名字的文本）归成 IPv4，UDP 回包头里的来源写回地址类型 1；出站报回的来源在引擎的 `receive()` 里同样先归一再认流（`HostName::canonical`），回包不再记到载体上最老的流名下。比较报告来源与目标的地方共四处——引擎的 `receive()`、`packet_datagram` 的接收任务、`udp_probe`、`from_relay`——，后三处是终审之后补的（`8e581e7`、`b14b45a`，见 M5c #9）。用例 `an_ipv4_mapped_target_is_the_ipv4_address`（`crates/rurge-engine/tests/udp.rs`）、`an_answer_from_a_source_in_ipv6_form_finds_its_flow`（`crates/rurge-engine/src/udp.rs`） |
| 9 | `tuic` 与 `hysteria2` 的载体辅助几乎相同 | M7c | 已做掉 | 链：M7c 承接事项 C7（原定"不抽"）→ M7c 延后 #11 → M7c 执行期修正 #5（5d040ad）做掉：`crates/rurge-proto-quic/src/carrier.rs`（`RouteClock`、`Inbox`、`bounded`）由 `hysteria2/udp.rs`、`tuic/udp.rs`、`masque/udp.rs` 共用 |
| 10 | `tuic/mod.rs` 约 2200 行 | 有空时整理 | 移交：下次动到时 | 约 2200 行，没有整理；M8 设计第 7 节没有点名 |
| 11 | `native` 模式的 `Dissociate` 也经 64 个的写入队列 | 接受 | 关闭 | 原去向"接受" |
| 12 | 假服务端对长度不对的 v4 认证命令只忽略 | 有协议事实时再补 | 移交：其它：有协议事实时再补 | 原去向照旧；M8 设计第 7 节没有点名 |
| 13 | 空闲超时不超过 10 秒的服务端上保活赶不上 | 接受（已登记）；M8 再评估 | 关闭 | M8 设计第 7 节关闭表（与 M7a #2 同列）；已登记（兼容性清单 `tuic` 行：有效空闲超时取两端较小值，10 秒的保活赶不上） |
| 14 | v5 TCP `ConnectionLost` 转 `Lost` 的分支没有用例 | 接受 | 关闭 | 原去向"接受"：回环上造不出确定的时序 |
| 15 | TUIC 缺中继组略去、底层换成员、`ecn` 连线的用例 | 可选 | 关闭 | 原去向"可选"：与协议无关，`hysteria2` 的用例覆盖同一条路径（行内说明） |
| 16 | `hysteria2` 用例里没加时限的 `carrier.send_to` | 顺手时补上 | M8 做掉 | M8c 任务 11（`6e22fa2`；设计 5.12）：`hysteria2` 的用例里十二处没加时限的 `carrier.send_to` 包进 10 秒的 `timeout`（`hysteria2/mod.rs` 的 `udp_send`、`hysteria2/udp.rs` 的 `send`） |

### M7c — `2026-10-06-phase2-m7c-h3-dns-plan.md`（36 行）

| # | 事项 | 原去向 | 结局 | 依据 |
| - | ---- | ------ | ---- | ---- |
| 1 | 新的互操作用例没有对真实二进制跑过 | 第一次 CI 运行时核对 | 移交：CI 首跑 | M8 设计 3.2（各参照服务端的夹具）；M8a 任务 6 等推送后的 CI 结果；`tests/interop/README.md` 同样写明 |
| 2 | MASQUE 不乐观发送 | 有需要时再议 | 移交：其它：有需要时再议 | 兼容性清单 `masque` 行登记"收到 2xx 之后才发包"；M8 设计第 7 节没有点名 |
| 3 | 服务端不收 QUIC 数据报时 MASQUE 的 UDP 失败 | 有用户报告再说 | 移交：有用户报告再说 | 兼容性清单 `masque` 行写明缺一样时第一个包失败；M8 设计第 7 节没有点名 |
| 4 | 服务端能力在每个目标开流时才检查 | 接受（P9，已登记） | 关闭 | 原去向"接受"，已登记（`masque` 行："Surge 在建立连接时校验，rurge 只让 UDP 失败（登记的差异）"） |
| 5 | CONNECT-UDP 畸形 2xx 的取消码不合 RFC | 以后顺手补 | 移交：下次动到时 | 原去向"以后顺手补"；M8 设计第 7 节没有点名 |
| 6 | HTTP/3 的 `trust-tunnel` 不看流数上限；无 HTTP/2 回落 | 接受 | 关闭 | 原去向"接受"，已登记（兼容性清单 `trust-tunnel` 行两处"登记的差异"） |
| 7 | 内部 UDP 会话的协议标签只按端口推断 | 需要时让上游把协议告诉连接器 | 移交：其它：需要时另议 | 已登记（兼容性清单 6.2 `encrypted-dns-follow-outbound-mode` 一行："不在 443 的 DoH3 服务器因此标成 `DOQ`"）；M8 设计第 7 节没有点名 |
| 8 | DoQ 复用前不看离空闲超时多远 | 接受 | 关闭 | 原去向"接受" |
| 9 | `https://` 的 DoH 没写路径时发往 `/` | M8 再议 | M8 做掉 | M8c 任务 3（105a6fd、aee16df），设计 5.4：`crates/rurge-dns/src/upstream/mod.rs` 的 `DEFAULT_PATH` 补 `/dns-query`，明写 `/` 的照旧 |
| 10 | DoH / DoH3 不按 `Age` 修正 TTL | M8 | M8 做掉 | M8c 任务 3（105a6fd、aee16df），设计 5.4：`crates/rurge-dns/src/upstream/doh.rs`、`doh3.rs` 按 `Age` 减 TTL |
| 11 | 载体辅助与错误映射几乎相同（已划掉） | 已解决 | 已做掉 | M7c 执行期修正 #5（5d040ad）：`crates/rurge-proto-quic/src/carrier.rs` 由三个协议共用；本行在表里已划掉 |
| 12 | 互操作的 DNS 用例开着 `encrypted-dns-skip-cert-verification` | 接受 | 关闭 | 原去向"接受"：解析器没有注入测试根证书的入口 |
| 13 | sing-box 1.15 的 `http` 入站可作 MASQUE 的三平台参照 | 发布正式版后再加 | 移交：其它：sing-box 1.15 发布正式版之后 | M8 设计第 7 节移交表 |
| 14 | 不设流数上限的 QUIC 出站的流控 | M8 | M8 做掉 | M8c 任务 1（`7222293`，修正轮 `17dd21f`），设计 5.1：`crates/rurge-net/src/quic/dial.rs` 的 `Transport::STANDARD` 连接窗口取 32 × 每流窗口 = 256 MiB；`Transport::DNS` 与 `capped_transport` 的下限各自写成 20 MiB。登记的限制是 29 条，不是设计写的 32 条：quinn 的连接级额度攒到窗口的八分之一才公布一次，没人读的数据超过窗口的八分之七（224 MiB）时其余隧道才停住。`trust-tunnel` 的 HTTP/3 的连接窗口改为 ⌈`max-streams` × 8 MiB × 8/7⌉（至少 20 MiB）：原来的 max(`max-streams` × 8 MiB, 20 MiB) 在 `max-streams` ≥ 9 时保不住「没人读的隧道占不光它」。兼容性清单 `hysteria2` 行的 20 MiB 说法由任务 12 的 B 部分改 |
| 15 | MASQUE 经链式载体的 UDP 余量 | M8 | M8 做掉 | M8c 任务 1（`7222293`），设计 5.2：经载体的 `masque` 连接从 1350 字节的包起步（`crates/rurge-proto-quic/src/masque/mod.rs` 的 `CARRIED_MTU`，经 `Transport::carried_initial_mtu` 与 `Dialer` 按路径选的配置生效，`min_mtu` 仍是 1200）；路径带不动时 quinn 的黑洞检测在**四**批只丢大包之后退回 1200，这条连接此后不再升回，回环上（debug 构建）UDP 驱动 187 ～ 226 毫秒、TCP 驱动 158 ～ 217 毫秒。缺省 `mtu`（1280）的 WireGuard 带不动 1350 字节的数据报，设计 5.2 的「缺省 MTU 的 WireGuard 也带得动」不成立；那时的行为按源码推断、没有实测。清单（`masque` 一行）与手工验收清单 M7c 一节「MASQUE 经 `underlying-proxy`」一项里「UDP 负载不到 1200 字节」的旧说法由任务 12 的 B 部分改 |
| 16 | CONNECT-UDP 流与 TCP 隧道共用连接的流额 | 有用户报告再议 | 移交：有用户报告再说 | 已登记（`masque` 行："登记的限制"）；M8 设计第 7 节没有点名 |
| 17 | 被放弃的内部 DNS 拨号不留记录、不向 `smart` 回报 | M8（TCP 与 UDP 一起改） | 移交：有用户报告再说 | M8 设计第 7 节移交表改定：只影响记录与一次回报；M8c 承接事项没有这一行 |
| 18 | 互操作夹具 `locate()` / `…_or_skip()` 各十份副本 | 有空时收成辅助函数 | 关闭 | M8 设计第 7 节关闭表：只影响测试代码的整洁，动到那些文件时再整理 |
| 19 | `quic/h3/mod.rs` 约 2200 行，多为用例 | 有空时挪到 `h3/tests.rs` | 关闭 | 同上（现 2223 行） |
| 20 | 两份单飞会话的规则已分叉 | M8（合并进 `rurge_net::quic`） | 移交：下次动到时 | M8 设计第 7 节移交表：下一次动其中一份时合并，现在合并没有行为收益；两份仍在（`crates/rurge-proto-quic/src/session.rs`、`crates/rurge-dns/src/upstream/session.rs`） |
| 21 | 三个 QUIC 假服务端的脚手架重复 | 有空时整理（同 #18） | 关闭 | M8 设计第 7 节关闭表 |
| 22 | 悄悄失效的 DNS 连接到下一次查找才恢复 | 接受（已登记） | 关闭 | 原去向"接受"，已登记（兼容性清单 6.2 `h3://` / `quic://` 一行："恢复在下一次查找"） |
| 23 | `Client::request` 在请求头没发完时失败不带流号 | 接受 | 关闭 | 原去向"接受"：要补就让请求的错误带上流号 |
| 24 | `[General]` 的 `W0012` 与 `redact_profile` 回显凭据 | M8 | M8 做掉 | M8a 任务 2（b09b126），设计 3.3：`crates/rurge-config/src/redact.rs` 的 `without_credentials` / `url_without_credentials`；判据 `crates/rurge-config/tests/secrets.rs` 的 `a_diagnostic_of_a_mistyped_general_value_holds_no_secret`、`the_redacted_profile_holds_no_secret` |
| 25 | `EncryptedDns` 的 `Debug` 带出原文 URL | M8 | M8 做掉 | M8a 任务 2（b09b126），设计 3.3：`crates/rurge-config/src/general.rs` 手写 `impl fmt::Debug for EncryptedDns` |
| 26 | 握手之后载体失效时的错误文本是固定的一句 | 有用户报告再说 | 移交：有用户报告再说 | `crates/rurge-net/src/quic/dial.rs` 握手之后的连接错误按 quinn 报的是哪一种读作 `quic: the UDP path to the server failed` 或 `quic: the connection was closed`，没有动；M8c 任务 2 只把握手期间的两种「endpoint 没了」错误统一成前一句（`df814a6`） |
| 27 | DoH3 连接一建成就离开时，晚来的等待者各再拨一次 | 接受 | 关闭 | 原去向"接受" |
| 28 | `ConnectionReset` 一类载体错误三处处理不一致 | M8 | M8 做掉 | M8c 任务 2（e6f65b7、df814a6），设计 5.3 第 5 条：`quic/carrier.rs`、`crates/rurge-engine/src/dns_pipeline.rs`、`quic/dial.rs` |
| 29 | 经 `underlying-proxy` 的载体延后报出的错误缺 `via` 前缀 | M8 | M8 做掉 | M8c 任务 2（e6f65b7），设计 5.3 第 3 条：`crates/rurge-policy/src/cell.rs` 给打开之后的错误补 `via <策略>: ` |
| 30 | `Carried::try_send` 在竞争窗口里丢包 | M8 | M8 做掉 | M8c 任务 2（e6f65b7），设计 5.3 第 1 条：`crates/rurge-net/src/quic/carrier.rs` |
| 31 | 包装载体对 `poll_send_ready` 的转发没有用例 | M8 | M8 做掉 | M8c 任务 2（e6f65b7），设计 5.3 第 2 条：三个包装载体各一条不看耗时的转发用例（`dns_pipeline.rs` 的 `FinishOnDrop`、`CountingDatagram`，`cell.rs` 的 `Relayed`） |
| 32 | `packet_datagram` 接收一侧换掉载体的原话 | M8 | M8 做掉 | M8c 任务 2（e6f65b7、df814a6），设计 5.3 第 4 条：`crates/rurge-net/src/packet_datagram.rs` |
| 33 | `rurge-proto-wireguard` 两处 `< 2 s` 墙钟断言 | 顺手时放宽到 5 秒 | M8 做掉 | M8c 任务 11（`6e22fa2`；设计 5.12）：`crates/rurge-proto-wireguard/src/outbound.rs` 里五处「< 2 秒」的墙钟断言（不是两处：文件在 M7c 之后又长了用例）放宽到 5 秒 |
| 34 | DoH URL 里的用户名口令不起认证作用 | M8 | M8 做掉 | M8a 任务 3（0178a13），M8-D5：`crates/rurge-dns/src/upstream/doh.rs` 以 `Authorization: Basic` 发出并从目标里去掉，用例 `a_urls_user_name_and_password_go_as_basic_authorization` |
| 35 | 全量测试偶发 `STATUS_HEAP_CORRUPTION` | M8 | M8 做掉 | M8 设计 3.5（查明来源，没有代码改动）；M8a 承接事项；M8-D4 |
| 36 | DNS 上游 URL 的全角 `＠` 与 `invalid port` 回显 | M8 | M8 做掉 | M8a 任务 3（0178a13），设计 3.3：`crates/rurge-dns/src/upstream/mod.rs`，用例 `a_fullwidth_at_sign_hides_no_credentials` |
