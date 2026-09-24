# 阶段 2 / M3「策略组、订阅与连通性测试」细化设计

- 日期：2026-09-23
- 依据：阶段 2 总设计 `docs/superpowers/specs/2026-09-19-phase2-outbound-groups-design.md`（第 1.4 节 M3 行、第 7、10、11、13、14 节）；需求文档 FR-OUT-03（`test-url` `test-timeout`）/ 10、FR-GRP-01、03 ～ 07；兼容性清单第 5 节。
- 手册核对：`policy-groups/{overview,parameters,policy-including,select,url-test,fallback,load-balance,smart}`、`tools/testing`、`tools/http-api`、`policies/parameters`，2026-09-23 取。本项目的基线是 Surge Mac 6.9 / iOS 5.22；手册里标注更新版本才有的行为，在文中逐条注明取舍。
- 本文件与总设计文字不同之处，以本文件为准；需要回写总设计的地方列在第 13 节。

## 1. 目标与范围

### 1.1 目标

让 `[Proxy Group]` 从"只有 `select` 真正可用"变成 Surge 的完整策略组：组可以从订阅（`policy-path`）、其它组和全部代理里装配成员；`url-test` / `fallback` / `load-balance` / `smart` 按连通性测试与真实连接质量自动选择；组级 `underlying-proxy` 让整组节点经中继出站；订阅更新时只重建策略注册表，不打断无关的连接。

### 1.2 范围内

| 需求 | 内容 | 计划 |
| ---- | ---- | ---- |
| FR-GRP-04 | `policy-path`（策略行列表与含 `[Proxy]` 的完整配置）、`update-interval`、`include-all-proxies`、`include-other-group`、`policy-regex-filter`、`external-policy-name-prefix`、`external-policy-modifier`、装配顺序与去重 | M3a |
| FR-GRP-05 | 嵌套组、运行期环检测（`E0009` 退役，改为告警 + 运行期 REJECT）、空组兜底 | M3a |
| FR-GRP-07 | 组级 `underlying-proxy`，派生 `Name (via Relay)` 策略 | M3a |
| FR-GRP-03 | 全部组参数的解析与校验；装配类参数在 M3a 生效，测速类参数在 M3b 生效，`policy-priority` 在 M3c 生效 | M3a / b / c |
| FR-OUT-03（部分）、FR-OUT-10 | 策略级 `test-url` `test-timeout` 生效；连通性测试（两次 HEAD）、结果缓存、按需重测 | M3b |
| FR-GRP-01 | `url-test` `fallback` `load-balance`（M3b）、`smart`（M3c）的选择算法 | M3b / M3c |
| FR-GRP-06 | 自动组的临时覆盖与清除（`select` 的持久化 M1 已完成） | M3b |
| 总设计第 10 节 | `POST /v1/policies/test`、`GET /v1/policy_groups/test_results`、`POST /v1/policy_groups/test` | M3b |

### 1.3 范围外

- `subnet` 组（FR-GRP-02，阶段 3），它的 `W0008` 保留到阶段 3。
- UDP 测试 `test-udp` / `proxy-test-udp` 与 `smart` 的 UDP 信号（M5）。
- 选择变更通知与 `no-alert`（FR-GRP-08，阶段 6）；`icon-url`、`category` 只影响界面。
- "网络已变化"的来源（阶段 3 有网卡 / 路由监视后接上）；M3 只留入口。
- `[Testing]` 节的吞吐测试（Mac 6.4.4+，界面功能）。
- 非 Surge 格式的订阅（Clash YAML、base64 链接列表），见 M3-D2。

### 1.4 三份计划

| 计划 | 内容 | 完成后可用的 |
| ---- | ---- | ------------ |
| **M3a 成员装配与订阅** | `GroupSpec`、订阅解析、装配、组级 `underlying-proxy`、运行期环与空组兜底、`--empty-group-reject`、注册表接受装配结果、订阅更新只重建注册表、拨号与视图改读 `EngineShared.cell` | 订阅节点经 `select` 组日常使用 |
| **M3b 测速与自动组** | 连通性测试探针、`TestBook`、`url-test` / `fallback` / `load-balance` 与测速类参数、临时覆盖、三个测试 API、能力表翻转（`W0008` 只剩 `subnet`） | 自动选最快 / 第一个可用的节点 |
| **M3c `smart`** | 会话质量回报、请求记录两列耗时、打分、按站点记忆、拨号阶段的重试、5 分钟测速与抽样、能力表翻转 `smart` | `smart` 组 |

顺序依据 M3-D1：订阅优先。

## 2. 已确认的决定

| 编号 | 事项 | 决定 |
| ---- | ---- | ---- |
| M3-D1 | 先做哪一块 | 订阅优先：M3a → M3b → M3c（项目所有者，2026-09-23） |
| M3-D2 | `policy-path` 接受的格式 | 只接受 Surge 格式（策略行列表，或含 `[Proxy]` 节的完整配置），与 Surge 一致；其它格式的内容解析不出任何策略时给一条 WARN，建议换成 Surge 格式的链接。不做 Clash / base64 转换（项目所有者，2026-09-23） |
| M3-D3 | 组内没有成员时 | 默认回退 DIRECT 并对每个组 WARN 一次，与 Surge 一致；命令行 `--empty-group-reject`（或环境变量 `RURGE_EMPTY_GROUP_REJECT=1`）改为 REJECT。rurge 专有选项只走命令行与环境变量（FR-CFG-17），不写进配置、不写进 `state.json`（项目所有者，2026-09-23） |
| M3-D4 | 订阅更新后怎样生效 | 只重建策略注册表这一代并原子切换，规则与 DNS 不动；按 M2b 的指纹复用出站（项目所有者，2026-09-23，"方案 B"） |
| M3-D5 | 启动时是否等订阅下载 | 不等：NFR-02 规定外部资源下载不阻塞启动，`rurge run` 对规则集也不等。首次启动没有缓存时组先是空的（按 M3-D3 兜底），下载完成后热重建。**已有磁盘缓存时，每一代构建注册表前先同步载入缓存**（本地读取、不联网），避免每次启动或重载都出现一段空组窗口 |
| M3-D6 | 订阅内容有问题时 | 只告警、不阻断：坏行、重名、与主配置同名、`underlying-proxy` 指向未知或成环的导入行，逐条跳过并 WARN；绝不让配置加载、`reload` 或注册表重建失败 |
| M3-D7 | 订阅链接与订阅内容里的凭据 | `policy-path` 的值常带口令（`?token=…`）：加入脱敏名单；日志只写组名，不写 URL。订阅行含口令：日志只报行号与原因，永不打印行文本；导入策略的 `policies/detail` 与 `lineHash` 走同一套脱敏 |
| M3-D8 | 连通性测试的实现 | 不用 `rurge-net` 的池化 `HttpClient`（连接池会让"第一次 HEAD 建新连接、第二次复用"失去控制）；探针经该策略的 `Outbound::connect_tcp` 拿到流，HTTPS 时自己套 TLS，再在这条连接上用 `hyper` 的 HTTP/1 客户端连接层发两次 HEAD。订正总设计 7.3 |
| M3-D9 | `E0009` | 退役。加载期发现的组环改报新告警 `W0030`，环上的组运行期表现为 REJECT；运行期（装配后）再检测一次 |
| M3-D10 | 临时覆盖的 API | 沿用 `POST /v1/policy_groups/select`：对自动组调用即设置临时覆盖，`policy` 为空字符串即清除；不新增端点。`select` 组的行为不变 |
| M3-D11 | `smart` 与 Surge 的差异 | 没有 TCP 重传率，用失败率近似；只在拨号阶段按重试列表换成员，"3 秒无响应"只计入打分、不在已建立的连接上重试；UDP 信号等 M5（项目所有者，2026-09-23） |
| M3-D12 | 互操作层 | M3 不需要：组算法与订阅格式都是 rurge 内部行为 |
| M3-D13 | `smart` 的常数 | Surge 未公开，取第 7 节的值，写在一处常量里便于以后调整 |

## 3. crate 改动一览

```text
rurge-config   + spec/group.rs（GroupSpec 与读取、校验）；诊断码 W0030（组环）；E0009 退役；
                 [General] test-timeout 区分"未设置"（M3b）；SECRET_PARAMS 加 policy-path
rurge-policy   + subscription（订阅文本 → 策略行）、assemble（装配，纯函数）、derive（组级中继派生）；
                 registry 接受装配结果：导入 / 派生条目、运行期环、空组兜底；
                 M3b：probe（两次 HEAD）、TestBook、GroupState（临时覆盖）、三种算法、测试观察者 trait；
                 M3c：SmartBook、会话回报 trait、选择上下文 SelectCtx（目标主机名）
rurge-engine   拨号与视图改读 EngineShared.cell；Runtime 保存工厂与订阅句柄；订阅热重建任务与代际锁；
                 EngineShared 带 empty_group 策略；M3b：测试 API 的引擎方法、测试会话进请求记录；
                 M3c：会话耗时回报、拨号重试、请求记录两列
rurge-api      M3b：三个测试端点；POST /v1/policy_groups/select 对自动组即临时覆盖
rurge (bin)    --empty-group-reject / RURGE_EMPTY_GROUP_REJECT；能力表翻转（M3b：url-test fallback load-balance；M3c：smart）
rurge-net      M3-D5 的"同步载入磁盘缓存"入口（位置在计划期定，见 V3）
```

依赖方向不变：`rurge-policy` 不依赖 `rurge-engine`，测试会话与 `smart` 的回报都经 trait 注入；`rurge-policy` 仍不依赖任何具体协议实现（探针只用 `Outbound` 与 `rurge-net` 的 TLS 根证书）。`hyper` 已在工作区。

## 4. 配置层（`rurge-config`）

### 4.1 数据模型

```rust
pub struct GroupSpec {
    pub name: String,
    pub kind: GroupKind,
    /// 组行上显式写出的成员，按声明顺序。
    pub members: Vec<String>,
    pub import: ImportOpts,
    /// 组级 underlying-proxy（FR-GRP-07）；DIRECT 等于没写。
    pub underlying_proxy: Option<String>,
    pub test: TestOpts,          // M3b 生效
    pub priority: Vec<(String /* 正则 */, f64)>,  // policy-priority，M3c 生效
    pub hidden: bool,
    pub span: Span,
}

pub struct ImportOpts {
    pub policy_path: Option<PolicyPath>,       // Url(Url) | File(PathBuf，已按主配置目录解析)
    pub update_interval: Option<u64>,          // 秒；仅对 Url 有意义
    pub regex_filter: Option<String>,          // 加载期已校验可编译
    pub name_prefix: Option<String>,
    pub modifier: Vec<(String, String)>,       // external-policy-modifier 的 key=value 列表
    pub include_all_proxies: bool,
    pub include_other_groups: Vec<String>,
}

pub struct TestOpts {
    pub interval: Duration,          // 默认 600 秒
    pub tolerance: Duration,         // 默认 100 毫秒；显式 0 生效
    pub timeout: Option<Duration>,   // 可用性门槛，无默认
    pub evaluate_before_use: bool,
    pub persistent: bool,
}
```

`Config` 增加 `group_specs: Vec<GroupSpec>`（与 M1 的 `specs` 对称），`PolicyGroup` 保留原样（视图的 `definition` / `lineHash` 仍用它）。`GroupSpec` 派生 `Clone + Debug + PartialEq`，跨代比较"组定义变没变"时去掉 `span` 再比。

### 4.2 校验（沿用现有诊断码，只新增 `W0030`）

| 情况 | 处理 |
| ---- | ---- |
| `policy-path` 为空、URL 不是 `http` / `https`、或解析不了 | `E0018`（文本不回显取值：它可能带口令） |
| `update-interval` 不是正整数 | `E0018` |
| `policy-regex-filter` 编译失败 | `E0018`（回显取值；正则不是凭据） |
| `external-policy-name-prefix` 含 `=` | `E0018` |
| `external-policy-modifier` 不是带引号的 `key=value` 列表 | `E0018` |
| `include-other-group` 引用未知的组 | `E0008`（与显式成员引用未知策略同一条诊断码） |
| 组级 `underlying-proxy` 引用未知策略 | `E0007`；指向内置 REJECT 族 → `E0018`；经它绕回本组 → `E0019` |
| `interval` 不是正整数，`tolerance` `timeout` 不是非负整数 | `E0018` |
| `policy-priority` 不是 `正则:因子;…`，正则编译失败，或因子 ≤ 0 | `E0018`（手册：Mac 6.8 起拒绝非正数） |
| 参数对该组类型没有作用：`tolerance` 不在 `url-test` 上；`persistent` 不在 `load-balance` 上；`policy-priority` 不在 `smart` 上；`interval` 写在 `smart` 上（手册：无效）；测速类参数写在 `select` 上；装配类参数与 `underlying-proxy` 写在 `subnet` 上 | `W0028` |
| 组行上的旧参数 `url=`（手册：当前版本无效） | `W0006` |
| `no-alert` `icon-url` `category` | 接受，不报诊断（只影响界面，路由行为不受影响） |
| 其它未知参数 | `W0001` |
| 加载期发现的组环（成员与 `include-other-group` 两种边） | `W0030`：`` policy groups `A` and `B` form a cycle; they behave as REJECT ``（取代 `E0009`） |

`include-other-group` 的值按带引号的逗号列表解析（手册：`"g1,g2"`，可带引号）；`external-policy-modifier` 的切分规则见 V5。

### 4.3 其它配置层改动

- **规则不能引用导入的策略名**：导入名在加载期不存在，规则里写导入名仍报 `E0007`（规则应引用组；未与 Surge 核对，登记进清单）。
- **`SECRET_PARAMS` 加 `policy-path`**（M3-D7）：本地路径也会被抹掉，偏安全一侧，符合脱敏模块的既定取向。
- **`[General] test-timeout`**（M3b）：现在直接存成默认 5 秒，分不出"没写"与"写了 5"；直连类策略的默认值是 10 秒，只在全局没写时适用。改为 `Option<Duration>`，默认值在使用处决定（V1）。
- **`W0029` 退役**：`test-url` `test-timeout` 在 M3b 生效，从"解析但未生效"名单里去掉；`test-udp` 继续 `W0029` 到 M5（订正清单里"M3 生效"的写法）。

## 5. 订阅与成员装配（M3a）

### 5.1 获取

每个带 `policy-path` 的组向本代 `ResourceManager` 登记一个资源：URL 按 `update-interval`（默认 86400 秒）后台条件刷新；本地文件（相对路径按主配置所在目录解析，与规则集一致）由文件监视触发。同一来源被多个组引用时只下载一份（`ResourceManager` 按来源去重）。

下载经 `rurge-net` 自己的直连连接器，与规则集相同，不经代理（未与 Surge 核对，登记进清单）。单个资源的大小上限沿用 `ResourceOptions.max_size`（64 MiB）。

M3-D5：构建一代注册表时，每个订阅先取磁盘缓存作为初始快照（同步的本地读取），没有缓存才是空的。

### 5.2 解析（`rurge_policy::subscription`，纯函数）

输入订阅文本，输出 `Vec<ProxyPolicy>`（沿用 `rurge-config` 的 `parse_policy`）：

1. 文本里有 `[Proxy]` 节（按配置解析器的节识别规则）→ 只取该节的行；否则整个文本逐行当作策略行。
2. 空行、`#` 与 `//` 开头的行跳过。
3. 每行 `名字 = 定义` 经 `parse_policy`；失败的行跳过，记下行号与原因。
4. 同一订阅内重名的，保留第一个。
5. 单个订阅最多取 10 000 条策略，超出截断并 WARN。
6. 一条策略都没解析出来时 WARN 一次，提示"内容可能不是 Surge 格式"（M3-D2）。

`to_spec` 在装配时做（5.3），因为它需要的名字查找表取决于装配结果。

### 5.3 装配（`rurge_policy::assemble`，纯函数）

输入：`Config`、每个订阅来源的当前快照（解析后的策略行）。输出：`Assembly`，包括每个组的最终成员表、全部导入策略的 `PolicySpec`、全部派生策略的 `PolicySpec`、诊断（告警列表）。

每个组的成员按手册的顺序拼接，重名保留第一个：

1. 显式成员（声明顺序）；
2. `include-other-group`：按列出的顺序，取每个被引用组**装配后**的成员（递归）；
3. `include-all-proxies`：主配置 `[Proxy]` 里全部代理策略（按声明顺序；不含 `direct` / `reject*` 别名、内置策略与组）；
4. `policy-path`：该组订阅的策略。

处理步骤：

- `policy-regex-filter` 作用于第 2、3、4 类成员的名字（第 4 类按加前缀之前的原名），不作用于显式成员。
- `external-policy-name-prefix` 与 `external-policy-modifier` 只作用于第 4 类（手册："This parameter only affects members imported via policy-path"）。顺序是过滤 → 加前缀 → 改参数。
- `external-policy-modifier` 在文本层覆盖导入行的参数（同名参数替换，没有的追加），然后才做 `to_spec`；所以它能改 `test-url`、`tfo`、`underlying-proxy` 等任何参数。
- 导入策略的 `to_spec` 用的名字查找表是：主配置的全部策略与组，加上本次装配里已经导入的策略。`underlying-proxy` 引用不到、或经它成环（沿用 `underlying_cycles` 的算法）的行，跳过并 WARN。
- **全局命名空间**：导入策略的最终名字（加前缀之后）与主配置的策略或组同名 → 跳过并 WARN，主配置优先（手册："policies whose names duplicate an existing policy are skipped with a warning"）。两个组导入了同名策略：定义相同的视为同一个策略；定义不同的，第一个组（按 `[Proxy Group]` 声明顺序）的那份生效，后面的 WARN 并跳过。
- `include-other-group` 递归时遇到环：环上的组标记为"成环"（5.6），它们的装配结果不再展开。

`assemble` 不碰网络、不读磁盘；第 1 层测试直接覆盖它。

### 5.4 组级 `underlying-proxy`（`rurge_policy::derive`）

对设了 `underlying-proxy = R` 的组 G，装配结果里每个成员 M：

- M 是代理策略（主配置或导入，有 `PolicySpec`）→ 替换为派生策略 `M (via R)`：`PolicySpec` 复制 M 的，只把 `common.underlying_proxy` 设为 `R`（M 自己的 `underlying-proxy`，包括经 `external-policy-modifier` 设上的，一律被组上的覆盖，手册如此）。派生策略由现有的 `build_one` 构建，自然走 `ChainConnector`，也自然有自己的指纹。
- M 是组、内置策略、`direct` / `reject*` 别名、或尚未实现的协议 → 原样保留。
- 同一个派生名出现两次（两个组共用中继 R 与成员 M）→ 定义必然相同，只建一个条目。

派生策略出现在 `GET /v1/policies` 与组的成员列表里，有自己的测试结果（M3b）。

### 5.5 注册表接受装配结果

`PolicyRegistry::build(cfg, assembly, factory, cell, selections, previous, empty_group)`：

- 条目来源：主配置策略（不变）、导入策略、派生策略、组（成员表取自 `assembly`，不再取 `PolicyGroup.members`）。
- 导入与派生策略同样按指纹复用上一代的出站（M2b 7.1）；构建失败的（例如引用了不存在的 Keystore 条目）跳过并 WARN，不让整代失败（M3-D6）。主配置策略的构建失败仍是加载期错误，与现在一致。
- 注册表记住每个组的装配后成员表，视图（`groups_view`、`policy_detail`、`group_selection`）改从注册表读取，而不是从 `Config` 读取。

### 5.6 环、空组与兜底

- **环**：装配后，按"组 → 作为成员的组"的边再检测一次环。环上的组条目标记为成环，`resolve` 到它时返回 REJECT，会话记录的 `error` 为 `policy group cycle: A → B → A`，每一代每个环只 WARN 一次。不在环上、但成员里有成环组的组，只有解析到那个成员时才 REJECT。
- **空组**：装配后成员表为空的组（订阅还没下载到、过滤把成员全滤掉、`smart` 组只有被忽略的成员）→ 默认解析到 DIRECT，会话记录带说明 `policy group has no members; DIRECT substituted`；带 `--empty-group-reject` 时解析到 REJECT，说明 `policy group has no members`。每一代每个组 WARN 一次。
- 这是行为变化：现在的空组解析为 REJECT（`registry.rs` 的 "policy group has no members; treating as REJECT"）。
- "组内没有可用成员"（M3b 起，所有成员测试都失败）不走空组兜底：手册规定 `fallback` 此时仍用第一个成员、`load-balance` 把全部成员当候选；`url-test` 手册未说明，取第一个成员，与 `fallback` 一致。

### 5.7 订阅更新时的热重建（M3-D4）

- `Runtime` 保存本代的出站工厂（`Arc<dyn OutboundFactory>`）与每个订阅来源的 `ResourceHandle`。
- 每一代启动一个后台任务，监听本代全部订阅句柄的版本变化；任一变化后去抖 1 秒，然后：
  1. 取引擎的**代际锁**（`tokio::sync::Mutex`，配置重载也取同一把锁）；
  2. 若引擎的当前运行时已经不是本代（期间发生了重载），放弃——新一代有自己的任务，而且它构建时读到的是最新快照；
  3. 用本代的 `Config` 加各订阅的最新快照重新装配，以当前注册表为 `previous` 构建新注册表，经 `EngineShared.cell` 原子发布。
- 本代被替换时，任务随之中止（持有 `AbortOnDrop`）。
- 重建成功记一条 INFO：组名、新增 / 删除的成员数（不写 URL，M3-D7）；装配里的告警按 5.3 逐条 WARN。
- 选择（`select` 组）与后面 M3b 的临时覆盖都按名字保存，重建后名字还在就继续生效；名字消失就回落到第一个成员（M1 已有的行为）。

### 5.8 拨号与视图改读 `EngineShared.cell`

只换注册表的发布路径要求：拨号（`engine.rs` 里的三处 `rt.policies.resolve`）与视图都从 `EngineShared.cell.load()` 取当前注册表，而不是从 `Runtime.policies` 取。`Runtime` 仍在构建时产出第一份注册表并由 `publish_generation` 存进 cell；`Runtime.policies` 字段是否保留由计划决定（V6）。

### 5.9 `rurge check` 与 `POST /v1/profiles/check`

不联网，只读数据目录里已有的订阅缓存：有缓存就按缓存装配，并把 5.2 / 5.3 的告警一并列出；没有缓存就给一条告警 `` policy group `G`: the content of `policy-path` has not been downloaded yet; its imported members are unknown ``（`W0022`，与规则集"资源不可用"同一条诊断码）。

## 6. 测速与自动组（M3b）

### 6.1 一次测试（`rurge_policy::probe`）

1. 解析测试 URL 与超时：策略的 `test-url` → `[General]` 的 `proxy-test-url`（直连类策略用 `internet-test-url`）→ `http://bing.com/`；策略的 `test-timeout` → `[General]` 的 `test-timeout` → 5 秒（直连类 10 秒）。"直连类"指 `DIRECT` 与 `direct` 别名。总设计里 WireGuard 的"另加 10 秒 L3 初始化"随 M4 实现。
2. 经该策略的出站 `connect_tcp(URL 的主机与端口)` 拿到流；目标主机名原样交给代理（远程解析），与普通会话相同。
3. HTTPS URL：在流上套 TLS（系统根证书；测试注入 `EngineShared.roots`）。
4. 在这条连接上用 `hyper` 的 HTTP/1 客户端连接层发第一次 HEAD，等到响应头；若连接可复用（没有 `Connection: close`、连接未关闭），发第二次 HEAD，以第二次从发出到响应头的耗时计分；否则以第一次的完整耗时（从开始拨号起）计分，并对该 URL 只 WARN 一次"结果不准"。
5. 收到任何状态码的完整响应头都算成功；超时、拨号失败、连接中断算失败。整个测试受上面的超时约束。

`REJECT` 族成员恒为失败；嵌套组的分数见 6.4。

### 6.2 `TestBook`

- 按策略名保存"最近一次结果（分数或失败）+ 时间 + 测试时的指纹"，跨代保留；策略的指纹（含 `test-url` / `test-timeout`）变了，旧结果作废。
- 同一策略同时只有一个测试在跑，后来的请求等它的结果；全局并发上限 8。
- 测试会话经观察者 trait（`TestObserver`）写入请求记录，带 `test` 标记。
- 结果不写入 `state.json`。
- 预留 `invalidate_all()`（"网络已变化"入口），阶段 3 接上来源。

### 6.3 何时测

- 组被解析（`resolve`）到、且它的成员结果比组的 `interval` 旧（或没有结果）→ 后台为该组发起一轮测试，本次解析先用现有结果（没有结果时：`url-test` / `fallback` 用第一个成员，`load-balance` 在全部成员里选）。
- `evaluate-before-use`：组第一次被使用时等第一轮测完再解析；测完仍没有可用成员，本次会话以 `policy group evaluation failed` 失败。拨号入口因此变成可等待的，只对开了该参数的组等待。
- 临时覆盖期间，该组不因使用而触发测试。
- `POST /v1/policy_groups/test` 与 `POST /v1/policies/test` 立即测，不看 `interval`。

### 6.4 三种组的选法

"通过"指最近一次测试成功，且写了 `timeout` 时分数低于 `timeout`。

| 类型 | 选法 |
| ---- | ---- |
| `url-test` | 通过者里分数最低的。迟滞：当前成员仍通过、且新的最优没有比它快超过 `tolerance` 时，保持当前成员；`tolerance=0` 时每次结果变化都选最快的。没有通过者时用第一个成员 |
| `fallback` | 按成员顺序第一个通过的；没有通过者时用第一个成员 |
| `load-balance` | 在通过者里均匀随机；`persistent=true` 时对目标主机名哈希后在通过者里取模；没有通过者时全部成员都是候选 |

嵌套组作为成员时的分数：`select` 组按它当前选择的成员计；`url-test` / `fallback` 按它当前选中的成员计；`load-balance` 取它通过者分数的平均值，没有通过者算失败。

`load-balance` 的 `persistent` 需要目标主机名：`resolve` 增加一个选择上下文参数 `SelectCtx { host }`，引擎在拨号时传入（M3c 的站点记忆也用它）。

### 6.5 临时覆盖（`GroupState`）

- 按组名保存覆盖的成员。组定义（去掉 `span` 的 `GroupSpec`）没变的重载保留覆盖；组消失或定义变了就清除；进程重启不保留。
- 覆盖的成员从装配结果里消失时（订阅更新删掉了它），覆盖自动失效，WARN 一次。
- API 见 6.6；`select` 组的选择仍走 `SelectionTable` 并持久化，与覆盖无关。

### 6.6 API（`docs/api/phase2.md` 登记，JSON 形状"暂定"）

| 端点 | 请求 | 响应 |
| ---- | ---- | ---- |
| `POST /v1/policies/test` | `{"policy_names": [...], "url": "http://…"}`（手册的请求例子；`url` 可省略，省略时用各策略自己的测试 URL） | 每个策略一个结果：成功时毫秒数，失败时错误文本；形状暂定 |
| `GET /v1/policy_groups/test_results` | 无 | 每个 `url-test` / `fallback` / `load-balance` / `smart` 组：成员名 → 最近结果与时间；形状暂定 |
| `POST /v1/policy_groups/test` | `{"group_name": "…"}` | `{"available": ["…", …]}`（手册的响应例子：本轮通过的成员） |
| `POST /v1/policy_groups/select` | `{"group_name": "…", "policy": "…"}` | 对 `select` 组不变；对自动组即临时覆盖，`policy` 为空字符串即清除（M3-D10）；原来对非 `select` 组返回的 400 取消 |

`GET /v1/policy_groups/select` 对自动组返回当前生效的成员（覆盖优先，其次是算法的结果）。

## 7. `smart`（M3c）

### 7.1 会话质量回报

- 引擎在每条经 `smart` 组选出成员的会话结束（或出错）时，把"组名、成员名、目标主机名、建连耗时、首字节耗时（从出站连接建立到收到出站返回的第一个响应字节）、结果"经 `SessionReporter` trait 回报给 `SmartBook`。
- 请求记录新增 `connect_ms`、`first_byte_ms` 两列（所有会话都有，API 与 `smart` 共用）。
- 结果分三类：正常（有首字节）；失败（出站拨号失败，或连接在收到任何数据之前就结束）；无响应（连上后 3 秒内没有收到任何响应数据，手册的 Mac 6.8 行为，按它做）。

### 7.2 打分

每个成员一个分数（毫秒），越低越好：

- 基础：首字节耗时的时间加权移动平均，半衰期 5 分钟；测速结果按同样方式喂进来。
- 失败与无响应：每次在平均值上叠加 800 毫秒的罚分（随时间同样衰减）。
- 最后乘以 `policy-priority`：按"正则:因子"列表首个命中的因子，没有命中为 1.0。
- 平均分 ≥ 3000 毫秒，或连续 3 次失败 → 标为失败；之后一次成功的会话或测试即恢复。
- 没有任何数据的成员：按"未知"处理，排在健康成员之后、失败成员之前。

### 7.3 选择与重试

- 优选集：健康成员里分数不超过最优者 1.2 倍的（至少包含最优者）；从中随机选一个。
- 重试列表：其余健康成员（按分数），然后未知成员，然后失败成员。
- 引擎拨号：选中成员拨号失败时，按重试列表依次再拨，最多再试 2 个，总时长仍受本条会话的连接超时约束；每次失败都回报。已经建立的连接不重试（M3-D11）。
- 嵌套组与内置策略直接忽略（手册）；忽略后没有成员 → 空组兜底（5.6）。

### 7.4 按站点记忆

- 按目标主机名记最近在该站点成功与失败的成员，有效期 1 小时，最多 4096 个站点（LRU 淘汰）。
- 选择时：某成员在该站点最近成功过、且分数不超过最优者 2 倍 → 直接选它；某成员最近在该站点失败过 → 从优选集移到重试列表末尾。

### 7.5 测速节奏

- 固定每 5 分钟一轮，`interval` 无效（加载期 `W0028`）。
- 成员超过 12 个时，常规轮次只测 12 个：最近最常用的 6 个，加最久没测的 6 个；手动触发（API）测全部成员。
- `evaluate-before-use` 照常生效。
- 视图里的"当前选择"是最近 10 分钟内被选中次数最多的成员（手册："the most-used one in the recent period"）。

### 7.6 常数（M3-D13）

| 常数 | 取值 |
| ---- | ---- |
| 移动平均半衰期 | 5 分钟 |
| 失败 / 无响应罚分 | 800 毫秒 |
| 标为失败 | 平均分 ≥ 3000 毫秒，或连续 3 次失败 |
| 优选集 | 最优者的 1.2 倍以内 |
| 站点记忆 | 1 小时，4096 个站点，"好成员仍可用"的上限为最优者的 2 倍 |
| 无响应判定 | 3 秒 |
| 拨号重试 | 最多再试 2 个成员 |
| 抽样 | 超过 12 个成员时每轮 12 个（6 常用 + 6 最久未测） |

## 8. 引擎、bin 与 API

- 拨号流程：`choose_policy` → `cell.load().resolve(policy, &SelectCtx)`（M3b 起可能等待 `evaluate-before-use`）→ `connect_tcp`；`smart` 的重试在引擎的拨号循环里。
- `EngineShared` 增加空组策略（`EmptyGroup::Direct` / `Reject`），由 bin 从 `--empty-group-reject` / `RURGE_EMPTY_GROUP_REJECT` 设置；它随引擎存续，重载沿用。
- bin 的能力表：M3b 翻转 `url-test` `fallback` `load-balance`，M3c 翻转 `smart`；`W0008` 此后只会因 `subnet` 出现。翻转任务里核对设计承诺的守卫都已存在（M2 设计第 8 节的教训）。
- `rurge-api`：M3b 三个端点与 `select` 的扩展；`policies/detail` 能查导入与派生策略（脱敏）；`GET /v1/policies` 的 `proxies` 包含导入与派生策略。

## 9. 错误处理、可观测性与安全

| 层 | 策略 |
| -- | ---- |
| 配置 | 4.2 的分级；主配置里的错误照旧阻止 `run`、让 `reload` 保留旧一代 |
| 订阅 | 永不阻断（M3-D6）：下载失败保留缓存并 WARN；解析与装配的问题逐条跳过并 WARN |
| 凭据 | `policy-path` 的值脱敏、日志只写组名（M3-D7）；订阅行永不进日志与错误文本；导入策略的详情与 `lineHash` 走 `redact_definition` |
| 缓冲 | 订阅 ≤ 64 MiB（资源管理器上限）、≤ 10 000 条策略；站点记忆 ≤ 4096；测试并发 ≤ 8 |
| 等待 | 每次测试受测试超时约束；`evaluate-before-use` 的等待受同一超时约束；热重建去抖 1 秒；代际锁只在构建注册表期间持有，不在锁内做网络 I/O |
| 后台任务 | 热重建任务随本代中止；测试任务随 `TestBook` 所属的引擎存续，panic 由任务边界隔离并记 ERROR |
| 会话记录 | 空组、成环、`evaluate-before-use` 失败、`smart` 重试各有固定文本（5.6、6.3、7.3） |

## 10. 测试策略

总设计第 13 节的三层里，M3 只用前两层（M3-D12）。

**第 1 层，纯函数与单元测试**：
- 订阅解析：纯策略列表、含 `[Proxy]` 的完整配置、注释、坏行、重名、10 000 条上限、非 Surge 格式的提示。
- 装配：四类来源的顺序、去重、过滤只作用于非显式成员、前缀与修饰只作用于订阅成员、修饰覆盖与追加、递归 `include-other-group`、全局重名规则、导入行的 `underlying-proxy` 未知与成环。
- 派生：`M (via R)` 的 spec、组级覆盖成员自己的中继、组与内置成员原样保留。
- 环检测与空组兜底（两种策略）。
- 三种组的选法：用假的测试结果，覆盖迟滞、`tolerance=0`、`timeout` 过滤、没有通过者、`persistent` 的哈希稳定性、嵌套分数。
- `smart`：用假的回报与可控时间，覆盖衰减、罚分、失败标记与恢复、`policy-priority`、站点记忆的过期与淘汰、优选集与重试列表、抽样。
- 需要推进时钟的用例：先完成真实 socket 上的往返，再 `tokio::time::pause()` + `advance`（M2b 的教训）。

**第 2 层，回环**：
- 订阅：由本地 `TestServer` 提供 URL 订阅；本地文件订阅的改动触发重建；两种格式的样本。
- 热重建：改变服务端内容后，新成员出现、删除的成员消失、没变的成员仍是同一个出站对象（`Arc::ptr_eq`）；与配置重载并发时不出错。
- 首次启动没有缓存：空组按两种策略兜底，下载完成后自动恢复；已有缓存时启动与重载都没有空组窗口。
- 测速：测试 URL 指向回环 `TestServer`（支持与不支持 keep-alive 两种）；经真实出站（M1 的 `http` 假上游）测试；HTTPS 测试 URL 用 `TlsFixture`。
- 引擎端到端：给假上游加可控延迟（计划期定具体做法，V7），`url-test` 选中更快的、`fallback` 跳过不可用的、`load-balance` 的 `persistent`、临时覆盖与清除、`smart` 在拨号失败时换成员。
- `rurge-api`：三个端点与 `select` 扩展的成功与失败路径。
- CLI：`--empty-group-reject` 与环境变量；`rurge check` 对有缓存 / 无缓存订阅的输出。

**安全约束不变**：测试配置里 `proxy-test-url` / `internet-test-url` 一律指向回环地址，绝不测真实的 `bing.com`；只用回环 + 端口 0 + 有界等待；不修改本机系统代理与网络设置。

## 11. 验收标准

对应总设计第 14 节第 3、4 条里属于 M3 的部分：

1. 两种订阅样本解析正确；装配顺序、过滤、前缀、修饰符合清单 5.2。
2. 订阅更新热重建：没变的成员复用出站；与配置重载并发正确。
3. 空组与环按 5.6 处理；`--empty-group-reject` 生效。
4. 组级 `underlying-proxy` 派生正确，派生成员可测、可选、可经链拨号。
5. 五种组算法（`select` 已有）的单元测试与端到端测试通过；临时覆盖与清除正确；`select` 的选择重启后保留（M1 已有，回归）。
6. 三个测试 API 与 `select` 扩展的行为正确。
7. 凭据不外泄：`policy-path` 的值、订阅行在 API 输出、日志与错误文本里都不出现。
8. fmt / clippy 零警告 / `cargo test --workspace` 全绿；`W0008` 只因 `subnet` 出现。
9. 需要真实机场订阅与真实节点的项目进 `docs/acceptance/phase2-manual.md`。

## 12. 兼容性清单需登记的差异

| 位置 | 内容 |
| ---- | ---- |
| 5.1 `url-test` `fallback` `load-balance` | M3b 已实现；没有"网络变化"触发的重测（阶段 3）；`url-test` 在没有通过者时用第一个成员（手册未说明） |
| 5.1 `smart` | 🟡：用失败率代替 TCP 重传率；只在拨号阶段重试；没有 UDP 信号（M5）；打分常数为 rurge 自定（第 7.6 节） |
| 5.1 嵌套与循环 | 循环在加载期报 `W0030`（取代 `E0009`），运行期表现为 REJECT；无成员时回退 DIRECT，`--empty-group-reject` 可改为 REJECT |
| 5.1 临时覆盖 | 经 `POST /v1/policy_groups/select` 设置与清除（rurge 的扩展，Surge 未定义自动组上的该端点） |
| 5.2 新增 `category` 一行 | 接受，不影响路由（iOS 5.23 / Mac 6.10，界面分类） |
| 5.2 `policy-path` | 只接受 Surge 格式；下载直连、不经代理（未核对）；值会被脱敏；单个订阅 ≤ 10 000 条策略；首次下载不阻塞启动 |
| 5.2 `url` 旧参数 | `W0006` |
| 5.2 成员装配 | 规则不能直接引用导入的策略名（`E0007`，未核对） |
| 4.3 `test-url` `test-timeout` | M3b 生效；HTTPS 测试 URL 在已建立的 TLS 连接上测第二次（手册 Mac 6.10 的行为） |
| 4.3 `test-udp` | 继续 `W0029`，M5 生效（订正原来的"M3 生效"） |
| 10.4 `POST /v1/policies/test` 等三个端点 | M3b 已实现，JSON 形状暂定（`docs/api/phase2.md`） |
| 10.4 `profiles/current`、`policies/detail` | 脱敏名单增加 `policy-path` |

## 13. 对其它文档的订正

| 文档 | 订正 | 时机 |
| ---- | ---- | ---- |
| 阶段 2 总设计 1.4 | M3 行注明拆成 M3a / M3b / M3c，订阅优先 | 随本文件 |
| 阶段 2 总设计 7.2 | 补：已有缓存时每一代构建前同步载入（M3-D5）；订阅问题只告警（M3-D6）；凭据（M3-D7） | 随本文件 |
| 阶段 2 总设计 7.3 | 探针不用池化 `HttpClient`（M3-D8） | 随本文件 |
| 阶段 2 总设计 7.5 | `E0009` 退役改为 `W0030`；空组兜底可经 `--empty-group-reject` 改为 REJECT | 随本文件 |
| 阶段 2 总设计 7.7 / 第 10 节 | 临时覆盖经 `POST /v1/policy_groups/select`（M3-D10） | 随本文件 |
| `docs/surge-compatibility-matrix.md` | 第 12 节各行 | 各计划的文档任务 |
| `docs/api/phase2.md` | 三个端点与 `select` 扩展 | M3b 的文档任务 |
| `CLAUDE.md`、`README.md` / `README_en.md` | 「先读这些文档」加入本文件；状态随各计划收尾更新 | 随本文件；各计划收尾 |

## 14. 写计划时必须核对的事项

| 编号 | 事项 | 计划 |
| ---- | ---- | ---- |
| V1 | `[General] test-timeout` 改为 `Option` 时受影响的调用方与快照测试（语料库 insta 快照） | b |
| V2 | `policy-regex-filter` 与 `policy-priority` 用 `fancy-regex`（工作区已有，规则的正则已在用）还是 `regex`；与 Surge（NSRegularExpression）语法的差异、回溯上限 | a |
| V3 | "构建前同步载入磁盘缓存"放在 `ResourceManager`（规则集也能受益）还是 `rurge-policy` 自己读 `CacheDir`；`ResourceManager` 的日志是否打印资源 URL（M3-D7 要求订阅 URL 不进日志） | a |
| V4 | 配置解析器的节识别规则能否直接用来从订阅文本里取 `[Proxy]` 节；订阅行的 `Span` 怎样表示（行号 + 组名，不含 URL） | a |
| V5 | `external-policy-modifier` 带引号的 `key=value` 列表的切分：值里含逗号、引号时的规则，与 `split_list` 的关系 | a |
| V6 | `Runtime.policies` 的去留、`engine.rs` 三处 `resolve` 与视图的改法；代际锁放在 `Engine` 的位置，与 bin 侧重载流程的关系 | a |
| V7 | 端到端用例里给假上游加可控延迟的做法（`FakeHttpProxy` 的脚本字段，或包一层延迟的回环转发） | b |
| V8 | `hyper` 的 HTTP/1 客户端连接层 API（`client::conn::http1`）在工作区所用版本上的确切用法；判断"连接可复用"的依据 | b |
| V9 | `resolve` 增加 `SelectCtx` 与"可等待"之后，对 `ChainConnector`（链的中间跳也要解析组）的影响 | b |
| V10 | 首字节耗时在转发循环里的测量点；3 秒无响应的计时方式（不增加每字节开销） | c |

## 15. 任务草图

**M3a（约 9 个任务）**：① `GroupSpec` 与校验（`W0030`、`E0009` 退役、`W0028` / `W0006` 各行、`SECRET_PARAMS`）→ ② 订阅解析 → ③ 装配（含递归、过滤 / 前缀 / 修饰、全局重名、导入行的中继检查）→ ④ 组级中继派生 → ⑤ 注册表接受装配结果（导入 / 派生条目与指纹复用、环、空组兜底、视图数据）→ ⑥ 订阅资源接入与"同步载入缓存"→ ⑦ 引擎：拨号与视图改读 cell、热重建任务与代际锁、`EmptyGroup` 与 bin 的命令行 / 环境变量 → ⑧ 端到端与 CLI 用例（订阅服务器、热重建复用、首次启动空组、`rurge check`）→ ⑨ 文档（清单、README、CLAUDE.md、手工验收、本文件的实施期订正）。

**M3b（约 8 个任务）**：① `[General] test-timeout` 与测速类参数生效 → ② 探针（两次 HEAD、HTTPS、不可复用时的退化）→ ③ `TestBook` 与触发（过期、并发上限、`evaluate-before-use`、测试会话进请求记录）→ ④ 三种算法与嵌套分数、`SelectCtx` → ⑤ `GroupState` 与临时覆盖 → ⑥ 三个 API 端点与 `select` 扩展 → ⑦ 能力表翻转与端到端 → ⑧ 文档（含 `docs/api/phase2.md`）。

**M3c（约 6 个任务）**：① 请求记录两列与 `SessionReporter` → ② `SmartBook` 打分 → ③ 选择、站点记忆与重试列表 → ④ 引擎拨号重试 → ⑤ 5 分钟测速与抽样、能力表翻转、端到端 → ⑥ 文档。

## 16. M3a 实施期的订正

本节登记 M3a 计划的「计划期决定」里与本文件文字不同的地方。逐条对应实现的提交见 `docs/superpowers/plans/2026-09-23-phase2-m3a-subscriptions-plan.md` 末尾「执行期修正记录」。

| 编号 | 设计原文 | 订正 |
| ---- | -------- | ---- |
| P2 | 3、V3："同步载入磁盘缓存"的入口位置待定；5.1 未提资源管理器的日志 | `ResourceManager::get` 本来就在返回前同步载入 URL 的磁盘缓存与本地文件，无需新入口；它的日志原先带资源 URL，改为 `get_labelled` 登记的标签（`policy-path of` 加上组名）。`rurge check` 用新增的 `rurge_net::resource::cached` 离线读缓存 |
| P3 | 5.2："空行、`#` 与 `//` 开头的行跳过" | 没有 `[Proxy]` 节时，`;` 开头的行也跳过（与配置解析器的注释规则一致）；跳过的行只报行号与原因，原因不引用行内任何片段（`parse_policy` 自己的消息会引用类型与端口，在 `vmess://` 链接这种非策略行里那是凭据的一部分） |
| P5 | 5.7：代际锁是 `tokio::sync::Mutex`，重建期间持有 | 锁是 `std::sync::Mutex`，只包住"核对当前代 + 发布"与重载的"发布 + 切换"两步；构建在锁外进行——构建期间读到的 `previous` 可能过时，代价只是少复用几个出站 |
| P6 | 1.3：`subnet` 组在阶段 3 之前保持现状 | 空组兜底（M3-D3）会让原本 REJECT 的 `subnet` 组悄悄改走 DIRECT，所以阶段 3 之前 `subnet` 组代表它的 `default`；`category` 等界面参数在 `subnet` 组上不再被当成网络条件 |
| P7 | 4.3 / M3-D7：脱敏名单加 `policy-path` | 同时加 `external-policy-modifier`：它能给导入行设任何参数，口令也在内，而 `password` 只在参数边界处匹配，找不到引号后面的那一个 |
| P8 | 5.9："把 5.2 / 5.3 的告警一并列出"（未定诊断码） | 订阅与装配的告警沿用集合的两个码：跳过的行、重名、派生名被占用、导入行的错误与成环一律 `W0023`，超过 10 000 条 `W0024`；没有内容 `W0022`；导入了未实现的协议 `W0007`（每种一次） |
| P9 | 5.9：`W0022` 文本 "has not been downloaded yet" | 改为 `` `policy-path` has no content yet (never downloaded, or the file cannot be read); its imported members are unknown ``——本地文件读不到时同一条告警也要说得对 |
| P10 | 5.3 / 5.4：未写明 `include-other-group` 取派生前还是派生后的成员 | 取派生前的：全部组先装配，最后才对有中继的组派生，被引用组的中继不传给引用它的组 |
| P11 | 5.4：未写派生名与已有名字冲突 | 派生名已被占用时略去该成员并 `W0023`，绝不改用不经中继的原成员 |
| P14 | 5.9：`rurge check` 读数据目录里的缓存（未写怎样找到数据目录） | `rurge check` 新增 `--data-dir`（环境变量 `RURGE_DATA_DIR`，默认平台数据目录）；`POST /v1/profiles/check` 用守护进程自己的数据目录；两者都经 `rurge_engine::check_profile`，配置本身有错时不做订阅检查 |
| P15 | M3-D3：环境变量 `RURGE_EMPTY_GROUP_REJECT=1` | `RURGE_EMPTY_GROUP_REJECT=true`：rurge 的布尔环境变量（`RURGE_WATCH`、`RURGE_NO_NETWORK` 等）都经 clap 的布尔解析器，只接受 `true` / `false`，`1` 会报 invalid value |
| P16 | 15：M3a 约 9 个任务 | 10 个：原第 ⑦ 项拆成"拨号与视图改读注册表、`EmptyGroup`"与"订阅热重建与代际锁"两个任务 |
| P17 | 5.3：导入行 `to_spec` 的诊断未细分 | 只有错误让该行被跳过（`W0023`）；导入行上的未知参数、无效参数等警告不输出——一个上万行的订阅会把日志淹没 |
| P21 | （无对应文字） | 全局策略（`proxy` 模式）与拨号一样按运行中的策略表校验，导入的策略名也可以当全局策略 |
| 任务 1（执行期） | 4.2 "组级 `underlying-proxy` 引用未知策略 \| `E0007`；指向内置 REJECT 族 → `E0018`；经它绕回本组 → `E0019`"；图的范围见 P13（组装配后的成员、`include-other-group`、组自己的中继） | 图另加 `include-all-proxies` 收进的代理（非别名、经 `policy-regex-filter` 放行的 `[Proxy]` 策略）为边；新增 `ImportOpts::admits` 作为过滤规则（P1）的唯一实现，装配也用它；`subnet` 组的 `include-other-group` 不再成边。原因：`Exit = …, underlying-proxy=Hop` + `Hop = select, include-all-proxies=true` 这样的配置能加载，到装配后拨号 Exit 会无界递归；`subnet` 组被忽略的参数曾产生误报的 `W0030` / `E0019` |
| 任务 3（执行期） | 5.6："**环**：装配后，按'组 → 作为成员的组'的边再检测一次环"（未写具体算法） | 改用迭代版 Tarjan 求强连通分量，不是深度优先只在遇到栈上的组时记环：环上的组一个不漏（与成员声明顺序无关），每个环各取一条最短环、从先声明的组写起、去重后列出；`include-other-group` 的"成环的组不展开给别人"与导入行的中继成环检查也改用同一个函数（后者从递归改成线性）。原因：深度优先版本经横叉边回到环上的组会漏列，且环上被判 REJECT 的组集合会随成员书写顺序变化 |
| 任务 3（执行期） | 5.3："导入策略的 `to_spec` 用的名字查找表是……`underlying-proxy` 引用不到、或经它成环……的行，跳过并 WARN"（未写 `W0023` 的原因文本来自哪里） | 原因只写固定说法加诊断码（如 `` policy `N` has a parameter whose value cannot be used (E0018) ``、`` … has an `underlying-proxy` that names no policy (E0007) ``、`` … names a `[Keystore]` item that is missing or of another kind (E0020) ``，其余 `` … cannot be used (<码>) ``），不引用 `to_spec` 自己的消息。原因：M3-D7——`to_spec` 的消息会引用取值本身（如 `` invalid value `999` for `tos` ``、整条请求头），修饰值会因此进日志 |
| 任务 3（执行期） | 5.3 / M3-D6："订阅内容有问题时……逐条跳过并 WARN"（未写导入行的中继指向另一条已被跳过的导入行时怎么处理） | 这类导入行一并略去（`W0023`，"names a policy that was left out"），沿反向中继边一次遍历找出它们。原因：不这样处理它会留在成员表里、每次拨号都失败；逐轮剔除在长链上是平方复杂度（1 万行要 4–10 秒） |
| 任务 5（执行期） | 5.6："空组……解析到 DIRECT，会话记录带说明 `policy group has no members; DIRECT substituted`" | 空组代以 DIRECT 而拨号本身失败时，请求记录的 `error` 写"说明; 失败原因"两部分，不是只写说明。原因：只写说明会把失败原因盖住 |
| 任务（终审） | 5.2：跳过的订阅行逐条按行号报警，没有上限 | `Subscription.skipped` 只列前 20 条（`MAX_SKIPPED_LISTED`），其余合计一条 `W0023`；订阅文本本身只读前 100 000 行（`MAX_LINES`），超出的部分连内容都不解析。原因：一份百万行的本地文件会让 `rurge check` 打印百万条告警、峰值内存约 150 倍 |

实施中发现的新出入由各任务追加。
