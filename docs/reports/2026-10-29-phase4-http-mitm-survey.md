# 阶段 4 前置调研：HTTP 引擎与 HTTPS MITM 的现成做法

日期 2026-10-29。只读调研，没有改动仓库任何文件，没有安装、下载或运行第三方代码。本文只给事实与权衡；凡是我自己的判断都标（推断）。

本文是阶段 4（HTTP 引擎）设计（`docs/superpowers/specs/2026-10-29-phase4-http-engine-design.md`）的输入，由只读的助手写成，文中的“我”是写报告的助手；收进仓库时只把本机的路径换成了通用的写法，其余照原样。设计怎样取舍见设计第 3 节。


## 来源可靠性

**标记约定**

- 【源码】直接读本机 cargo registry（`~/.cargo/registry/src/index.crates.io-…/`）里的 crate 源码。这些版本与 仓库的 `Cargo.lock` 完全一致：hyper 1.11.1、hyper-util 0.1.20、h2 0.4.19、http 1.5.0、http-body 1.1.0、http-body-util 0.1.5、httparse 1.10.1、rustls 0.23.43、tokio-rustls 0.26.4、rcgen 0.14.7、tokio-tungstenite / tungstenite 0.28.0、x509-parser 0.18.1、lru 0.18.4。引用写作 `crate/路径:行号`。这是本文最可靠的一类。
- 【W】WebFetch 返回的是模型改写的摘要，不是原文。凡是我拿来下判断的细节，我都换一个问题让它"逐字引用"再核对了一遍（hudsucker 的 `"GET "` 与 `\x16\x03`、`signed_by(self.issuer.key()…)`、证书常量，mitmproxy 的 `starts_like_tls_record`、`probably_no_http`、options 默认值，HAR 1.2 引文等）。逐字引用仍是摘要工具转述，没有与原页面逐字节比对。没有复核过的标【W·未复核】。
- （搜索摘要）只在 WebSearch 的结果摘要里见到。
- （推断）我的判断。
- 未见：没能确认。

**需要提醒的几点**

1. crates.io 的版本号、发布日期、下载量、许可证、MSRV 来自 `https://crates.io/api/v1/crates/<名>`，经 WebFetch 转述，数字未二次核实。本机 Cargo.toml 里读到的许可证与 MSRV 是【源码】。
2. crates.io 上的最新版比 Cargo.lock 新：hyper 1.12.0（2026-10-06）、hyper-util 0.1.21、h2 0.4.20、tokio-rustls 0.26.6、rustls 0.23.46、tokio-tungstenite 0.30.0。第 1、4 节的行为结论只对 Cargo.lock 那一版成立。hyper 的 CHANGELOG（【W】）显示 1.12.0 改了 Content-Length/Transfer-Encoding 互相取舍、`Connection: close` 优先级、新增 `max_header_size`，没有提头大小写或头顺序的变化。
3. 打不开或打不开原文的页面：softwareishard.com（HAR 1.2 原始规范，证书过期）、web.archive.org（工具禁止）、whistle 官网部分页面（返回空）、sing-box 源码路径（404）、rama 的 `HttpPeekRouter` 源码（搜不到）。相应位置写"未见"，HAR 1.2 的引文取自 GitHub 镜像仓库 `ahmadnassri/har-spec`，不是原站。
4. 本文涉及的项目许可证都不是 GPL（见第 2、候选 crate 两节）。Squid 据我所知是 GPL（推断），只在第 3 节用文字描述它的一个配置项，没有引用任何代码。
5. 本文没有逐项核对 Surge 手册，Surge 自身行为（比如头顺序是否保真）一律未见。

---

## 1. hyper 1.x 作为 MITM 代理的 HTTP/1.1 引擎

### 1.1 能力矩阵（【源码】hyper 1.11.1）

| 项目 | 服务端 `server::conn::http1::Builder` | 客户端 `client::conn::http1::Builder` | 结论 |
|---|---|---|---|
| 保留头名大小写 | `preserve_header_case` | `preserve_header_case` | 能"原样转发"，不能"读取或修改拼写"（映射类型是 `pub(crate)`） |
| Title-Case 输出 | `title_case_headers` | `title_case_headers` | 与上一项同开时，保留下来的拼写优先，其余头用 Title-Case |
| 保留头顺序 | 无此选项 | `preserve_header_order`，仅在 `ffi` 特性下存在 | 做不到。写出顺序 = `HeaderMap` 遍历顺序 |
| 请求里的 obs-fold | 一律拒绝（400），无开关 | 不适用 | 无法原样转发折行头 |
| 响应里的 obs-fold | 不适用 | `allow_obsolete_multiline_headers_in_responses`：把换行换成空格 | 规范化，不是原样 |
| 畸形头行 | `ignore_invalid_headers` 悄悄丢弃 | `ignore_invalid_headers_in_responses` 悄悄丢弃 | 只能丢，不能原样转发 |
| 头名后的空格 | 拒绝 | `allow_spaces_after_header_name_in_responses` | 规范化 |
| 非 UTF-8 头值 | 接受（字节） | 接受（字节） | `HeaderValue::to_str()` 会失败，要走 `as_bytes()` |
| reason phrase | 响应扩展里有 `ReasonPhrase` 就写出去 | 与标准不同的 reason 存入 `ReasonPhrase` 扩展 | 非 ASCII reason 会变成空串（1.4） |
| 1xx | service 不能返回 1xx（会变 500）；`100 Continue` 自动发 | 1xx 被跳过；`ext::on_informational` 只读回调 | 无法把上游的 1xx 转发给下游 |
| upgrade | `serve_connection(..).with_upgrades()` + `hyper::upgrade::on(&mut req)` | `conn.with_upgrades()` + `hyper::upgrade::on(res)` | 两侧都支持 |
| trailers | 响应 trailers 需请求带 `TE: trailers` 且响应头声明 `Trailer` | 请求头声明 `Trailer` 才写 | 有条件 |
| 流水线 | 严格串行，一次一个在途请求 | 一条连接一次一个请求 | 见 1.7 |
| 头读取超时 | `header_read_timeout`，必须装 timer 才生效 | 无 | rurge 现有入站已注意到这点 |

### 1.2 头名大小写

- 两侧 builder 都有 `preserve_header_case` 与 `title_case_headers`：`server/conn/http1.rs:282,320`、`client/conn/http1.rs:464,480`。hyper-util 的 `legacy::Client::builder()` 有 `http1_title_case_headers`、`http1_preserve_header_case`（`client/legacy/client.rs:1255,1275`），`server::conn::auto::Builder` 与它的 `Http1Builder` 有同名方法（`auto/mod.rs:183,207,757,787`；hyper-util CHANGELOG 0.1.15 "Add header casing options to `auto::Builder`"）。
- 机制：解析时把每个小写的 `HeaderName` 对应到"按出现顺序排列的原始拼写列表"，存成私有类型 `HeaderCaseMap(HeaderMap<Bytes>)`，放进消息的 `http::Extensions`（`ext/mod.rs:138-199`；服务端解析 `proto/h1/role.rs:245-249,334-336,357-359`；客户端解析 `role.rs:1105-1109,1136-1138,1150-1152`）。
- 写出：服务端在响应的扩展里找 `HeaderCaseMap`，找不到而 `title_case_headers` 开着时用一个空映射（全部 Title-Case）；对每个头名取该名字的拼写迭代器的下一个，用完或没有就回落到小写（或 Title-Case）（`role.rs:461-467,583-650`）。客户端同理（`role.rs:1228-1239,1619-1652`），另外空值写成 `Name:\r\n`（无尾随空格）。
- 文档自己的说法："Since the relevant extension is still private, there is no way to interact with the original cases. The only effect this can have now is to forward the cases in a proxy-like fashion."（`server/conn/http1.rs:309-319`）
- 对代理的含义：
  - 要贯通，入站 server builder 和出站 client builder 都要开，并且改写时必须把 `Request`/`Response` 的 `Extensions` 整体带到另一侧。用 `Request::builder()` 另起一个请求会丢掉它。
  - 新增的头一律小写（或 Title-Case）。删除或改值不影响其他头的拼写。同名多值头若删掉前面某个值，后面的值会拿到错位的拼写（推断：由"按序消费"推出）。
  - 读不到原始拼写，所以抓包、日志、HAR 里的头名只能是小写，做不到"显示原始大小写"。H2 与 H1 互转没有大小写信息可保。
- 历史：hyper issue #2313（2020-10-27 提出，代理作者要求保留响应头的大小写；2021-04-21 以 commit `dbea771` 关闭）【W】。

### 1.3 头顺序

- `http::HeaderMap` 的文档明说遍历顺序"arbitrary … users must not rely on any incidental order"（http 1.5.0 `header/map.rs:34-41`）。实现上是：`Iter` 先按 `entries` 向量下标走，每个名字后面跟它的额外值链（`map.rs:2323-2356`），`remove` 用 `swap_remove`（`map.rs:1609-1614`）。所以写出顺序是"名字按首次出现、同名值聚在一起"，一旦删过头就可能被打乱。交错的 `Set-Cookie: a / X-Foo / Set-Cookie: b` 会变成 `Set-Cookie: a / Set-Cookie: b / X-Foo`。另有硬上限：一个 `HeaderMap` 最多 32,768 项（`map.rs:53-54`）。
- hyper 编码器：服务端用 `msg.head.headers.drain()`（`role.rs:698`），客户端用 `headers.keys()` 再对每个名字 `get_all`（`role.rs:1630-1634`）。
- `OriginalHeaderOrder`（`ext/mod.rs:201-298`）是 `pub(crate)`，整个类型 `#[cfg(feature = "ffi")]`。它只在解析阶段被记录（`role.rs:251-256,338-341,1111-1116,1140-1143`），读取只发生在 `ffi/http_types.rs:399,459`，编码器里没有引用。客户端的 `preserve_header_order` 同样只在 `ffi` 下存在（`client/conn/http1.rs:503-514`），服务端 builder 没有。`ffi` 需要 `--cfg hyper_unstable_ffi`（`lib.rs:141-143`、`Cargo.toml:48-57`）；工作区启用的 hyper 特性是 client/server/http1/http2，没有 ffi。
- 结论：hyper 保不住头顺序，也没有稳定的公开办法。PR #2798 引入它的动机是 curl 的测试失败（搜索摘要）。
- 别处的做法：Pingora 有独立的 `pingora-http`，crate 描述为 "HTTP header objects that preserve http header cases"，"… in order to build a transparent proxy"（【W】，docs.rs，版本 0.9.0，Apache-2.0）；mitmproxy 的 `Headers.fields` 暴露原始 `(name, value)` 字节元组（【W】，文档只说 "raw access"，顺序保真是推断）。

### 1.4 obs-fold、非法头、非 UTF-8 值、reason phrase

- **头值字节**：httparse 接受 `\t`、`0x20-0x7e`、`0x80-0xff`，拒绝 DEL 与其他控制字符（httparse 1.10.1 `lib.rs:90-92`）。hyper 用 `from_maybe_shared_unchecked` 直接装成 `HeaderValue`（`role.rs:46-59,266`）。`http::HeaderValue` 的合法性是 `b >= 32 && b != 127 || b == b'\t'`（http 1.5.0 `header/value.rs:563-565`），所以 0x80+ 的 obs-text 能进出。`to_str()` 对它们返回错误。（推断：workspace 里的 `fancy-regex` 只吃 `&str`，对这类头值要么 lossy 转换要么限定只匹配 ASCII。）
- **请求里的 obs-fold**：服务端用的 httparse 配置里 `allow_obsolete_multiline_headers: false` 写死（httparse `lib.rs:507-510`），折行头直接解析失败，hyper 自动回 400 并关连接（`role.rs:484-501`）。
- **响应里的 obs-fold**：客户端 builder 的 `allow_obsolete_multiline_headers_in_responses(true)` 会把折行处的换行替换成空格，再把头值重新对齐（`role.rs:1458-1515`）。这是 RFC 要求的代理行为，但意味着转发出去的字节与原始字节不同。
- **畸形头行**：`ignore_invalid_headers`（服务端）与 `ignore_invalid_headers_in_responses`（客户端）是悄悄丢弃，不是原样保留。头名必须是 token（`TOKEN_MAP`，httparse `lib.rs:78-82`）。
- **reason phrase**：
  - 客户端解析时，只有当 reason 与 `StatusCode::canonical_reason()` 不同才存成 `ReasonPhrase` 扩展（`role.rs:1057-1065,1159-1164`）。
  - 服务端写响应时，扩展里有 `ReasonPhrase` 就用它，否则写 canonical 或 `<none>`（`role.rs:421,442-454`）。`ReasonPhrase::try_from` 会拒绝控制字符（`ext/h1_reason_phrase.rs`）。
  - 缺口：httparse 的 `parse_reason` 在 reason 含 obs-text（0x80+）时返回空字符串（httparse `lib.rs:839-884`），于是上游的非 ASCII reason 会被当成"空 reason"转发。
  - `StatusCode` 接受 100..=999（http 1.5.0 `status.rs:73-80`）。Cloudflare 在 Pingora 文章里把"hyper 曾限制在 100-599"当作自研理由之一，现在已不成立（【W】）。
- **请求行**：`httparse` 的 URI 字符集宽松（`lib.rs:66-76`），随后 `http::Uri::from_maybe_shared` 做严格校验（`role.rs:221-225`）。http 1.5.0 的 `PathAndQuery` 容忍 `"`、`{`、`}`，接受 0x80+ 字节但要求整串是合法 UTF-8（`uri/path.rs:32-33,427-435`），所以 URL 里夹非 UTF-8 的原始字节会被 400。`MAX_URI_LEN` 为 65534，超出回 414（`role.rs:33-34,185-187`）。方法接受任意 token（`Method::from_bytes`），版本只认 HTTP/1.0、1.1。解析错误 hyper 自动回 400/414/431 然后关闭，service 无权定制（`role.rs:484-501`）。

### 1.5 upgrade 与 WebSocket 透传

- **服务端**：`serve_connection(..).with_upgrades()`（`server/conn/http1.rs:199-204`，要求 `I: Send`）。请求是 `CONNECT`，或带 `Upgrade` 头且为 HTTP/1.1 时，hyper 在请求扩展里放入 `OnUpgrade`（`role.rs:243,326-329`；`dispatch.rs:322-330`）。取用：`hyper::upgrade::on(&mut req)` 会把它从扩展里 `remove` 掉（`upgrade.rs:339-353`），所以必须在拆解、转发请求之前取出。
- 服务端写出 101（或对 CONNECT 的 2xx）后，hyper 不加 Content-Length/Transfer-Encoding，也不检查 `Sec-WebSocket-*`（`role.rs:396-402`），随后连接 future 以 `Dispatched::Upgrade` 结束，把 IO 交给 `OnUpgrade`（`dispatch.rs:152-155`）。
- **客户端**：`conn.with_upgrades()`（`client/conn/http1.rs:302-304`）；响应为 101 或对 CONNECT 的 2xx 时 `wants_upgrade`，该连接不再复用（`role.rs:1172-1181,1276-1293`）。在响应上取 `hyper::upgrade::on(res)`。
- `Upgraded` 内部是 `Rewind<Box<dyn Io + Send>>`，带着握手后已经读入缓冲的字节（`upgrade.rs:66-68,84-96`）；必须读它或 `downcast` 取回 `Parts{io, read_buf}`，否则握手后立刻到达的 WebSocket 帧会丢。用 hyper-util 的 `auto::Builder` 时要用 `serve_connection_with_upgrades`，`downcast` 要用 `auto::upgrade::downcast`（`auto/mod.rs:247-255`）。
- 透传的做法（推断，依 API 语义；没有读 hyper 仓库的 `examples/upgrades.rs`）：
  1. 收到升级请求，先 `let client_up = hyper::upgrade::on(&mut req)`。
  2. 转发到上游，上游连接必须 `with_upgrades()` 且 `Connection` future 被 spawn。
  3. 上游回 101，则 `let server_up = hyper::upgrade::on(&mut res)`，把 101 响应（含 `Sec-WebSocket-Accept/Extensions/Protocol`）原样回给客户端。
  4. spawn 一个任务等两个 `Upgraded`，用 `TokioIo` 包装后 `tokio::io::copy_bidirectional`。
  5. 上游不是 101 就当普通响应转发。
- H1 的 `Connection`/`Upgrade` 头 hyper 不剥（只合并 `Connection` 的多个值，`role.rs:838-855`）；H2 层会剥（`proto/h2/mod.rs:30-92`），所以 H1↔H2 之间的 WebSocket 要手工做 RFC 6455 与 RFC 8441 的互转（见第 4 节）。

### 1.6 `Expect: 100-continue` 与 1xx

- 服务端：请求头 `Expect` 的值（大小写不敏感）恰好等于 `100-continue`、HTTP/1.1 以上、body 非空，状态进入 `Reading::Continue`（`role.rs:320-325`；`conn.rs:303-318`）。第一次有人 poll 请求 body 且还没开始写响应时，hyper 自动写出 `HTTP/1.1 100 Continue\r\n\r\n`（`conn.rs:405-416`）。service 不碰 body 就直接回最终响应，则跳过 100（`conn.rs:858-864`）。其他 `Expect` 取值无特殊处理，也不会自动回 417。
- 在流式代理里，上游连接一开始写请求 body 就会 poll 下游的 `Incoming`，于是下游马上收到 100，而上游此时可能还没表态。上游若随后回 4xx/417，下游已被邀请发送 body（推断）。
- 客户端：没有等 100 的逻辑，body 紧跟请求头写出。1xx（101 除外）在 `Client::decoder` 里被跳过（`role.rs:1279-1282`、1184-1188 行回调），只有挂在请求扩展里的 `hyper::ext::on_informational` 能看到，且给的是只读外观（`ext/informational.rs`）。
- 服务端发不出任何 1xx（`role.rs:392-409`：返回 1xx 会被换成 500 并报 `unsupported_status_code`），所以 `103 Early Hints`、转发上游的 `100` 都不可能经 hyper 的 H1 server 做到。
- hyper 的 H2 路径也没有暴露 1xx（在 `proto/h2` 与 `client/conn/http2.rs` 里没有搜到 "informational"），尽管 h2 0.4.13 起支持（`SendResponse::send_informational`、`ResponseFuture::poll_informational`）。

### 1.7 流水线与并发

- 服务端 `Dispatch::poll_ready` 在有在途 service future 时返回 Pending（`dispatch.rs:634-644`）；连接只有在 `Reading::KeepAlive` 与 `Writing::KeepAlive` 同时成立后才回到 idle 并解析下一个请求头（`conn.rs:1082-1101`）。所以流水线请求会留在读缓冲里，按序串行处理，没有并发。`pipeline_flush(true)` 只是合并 flush，文档标注 "Experimental, may have bugs"（`server/conn/http1.rs:400-408`）。
- 响应可以在请求 body 还没读完时开始写（读写两个方向各有状态机），但下一个请求要等两边都结束。
- 客户端：`Client::should_poll` 为 `callback.is_none()`（`dispatch.rs:764-766`），一条 H1 连接同一时刻只有一个在途请求，不做流水线。`http1::SendRequest` 不是 `Clone`，`http2::SendRequest` 是（`client/conn/http2.rs:28`）。

### 1.8 body：流式、背压、重新分帧、trailers

- **流式**：H1 的 `Incoming` 是 `mpsc::channel(0)` 加一个 `watch` 的 "want" 信号（`body/incoming.rs:114-137`），任何时刻最多一块在途，消费者不 poll 就不再读 socket，所以是真正的背压，没有无界缓冲。H2 的 `Incoming` 在 poll 到数据时才 `release_capacity`（`incoming.rs:240-245`），背压会传到对端的窗口。
- **丢弃 body**：`Incoming` 被 drop 后，hyper 尝试排空缓冲里已有的字节，排不完就关闭读端（`dispatch.rs:239-243`、`conn.rs:858-875`），响应写完后连接关闭。Map Local、reject 这类在上传途中短路的响应会因此把连接关掉（推断：客户端可能在读到响应前就看到 RST）。
- **重新分帧**：请求/响应的 `Content-Length`/`chunked` 都是重新编码的。`size_hint().exact()` 有值就走 `BodyLength::Known` 写 `content-length`，否则 `Unknown` 走 `chunked`（HTTP/1.1）或 close-delimited（HTTP/1.0）（`role.rs:919-962`，客户端 `1325-1456`）。chunk 边界等于上游 `Incoming` 吐出的数据帧边界，不保留原始 chunk 大小；chunk 扩展没有公开 API 保留（未读 `decode.rs`，按丢弃处理，推断）。
- **长度校验**：`Encoder::Length` 对超过声明长度的数据截断，不足则报错（`encode.rs:143-151`；`NotEof`）。所以改写 body 必须同步改或删 `Content-Length`（或改 chunked）。
- **CL 与 TE 并存**：请求侧先出现 TE 再出现 CL 时丢 CL，CL 在前 TE 在后则删 CL，两种情况都把 keep-alive 关掉（`role.rs:279-281,289-293,351-353`）；请求 TE 不以 chunked 结尾回 400（`346-349`）；HTTP/1.0 带 TE 拒绝（`274-277`）。响应侧有 TE 就按 TE 解码，否则按 CL，都没有则读到连接关闭（`role.rs:1301-1323`）。hyper 1.12.0 CHANGELOG 另有两条："always remove content-length if transfer-encoding is sent (#4223)"、"remove content-length if transfer-encoding is auto-added (#4224)"（【W】）。
- **trailers**：解码侧能读出 `Frame::trailers`（`conn.rs:391-393`）。编码侧只写在消息头里用 `Trailer` 声明过的名字，并且丢弃一批禁用字段（Authorization、Cache-Control、Content-Encoding、Content-Length、Content-Range、Content-Type、Host、Max-Forwards、Set-Cookie、Trailer、Transfer-Encoding、TE）（`encode.rs:163-217,264-280`）；服务端写响应 trailers 还要求请求带 `TE: trailers`（`conn.rs:328,739-743`）。透传 trailers 的代理要同时保住 `Trailer` 头和 `TE: trailers`。
- **压缩**：hyper 不处理 `Content-Encoding`。Body Rewrite 需要自己解压、改写、重压并修 `Content-Length`（候选库见末表）。

### 1.9 hyper 会自己改的头，与不会剥的头

- 服务端响应：自动加 `Date`，可 `auto_date_header(false)` 关（`role.rs:974-983`；`server/conn/http1.rs:395`）；多个 `Connection`、`Transfer-Encoding`、`Trailer` 值合并成一行（`role.rs:806-900`）；无长度信息时补 `content-length: 0` 或 `transfer-encoding: chunked`（`role.rs:919-962`）；对端是 HTTP/1.0 时把响应降为 1.0 并调整 keep-alive 头，keep-alive 被关时补 `connection: close`（`conn.rs:665-712`）。
- 客户端请求：必要时补或改 CL/TE；GET/HEAD/CONNECT 加未知长度 body 时按"无 body"处理（`role.rs:1403-1405`）；请求带 `Connection: close` 则该上游连接用完即弃（`conn.rs:615-626`）。`conn::http1` 不补 `Host`，hyper-util 的 `legacy::Client` 才补，并按需转成 origin-form/absolute-form/authority-form（`client/legacy/client.rs:298-322`）。
- 不剥 H1 的 hop-by-hop 头（`Keep-Alive`、`Proxy-Connection`、`TE`、`Upgrade` 等）。hyper 只在 H2 层剥（`proto/h2/mod.rs:30-92`，注释引用 RFC 9110 §7.6.1）。（推断：H1↔H1 的代理需要自己按该节处理，rama 的 MITM 例子里有专门的"hop-by-hop 头移除层"（【W】），hudsucker 的 `normalize_request` 只删 Host 和合并 Cookie。）

### 1.10 超时与上限

- `header_read_timeout` 默认 30 秒，但只有安装了 timer 才生效：`Time::check` 在 `Dur::Default` 且没有 timer 时只打一条 warn 并返回 None，显式配置了却没有 timer 则 panic（`common/time.rs:70-85`）。rurge 的入站已经显式 `.timer(TokioTimer::new())`（`crates/rurge-inbound/src/http.rs:95-101`，同时开了 `preserve_header_case(true)` 与 `with_upgrades()`）。
- `max_headers` 默认 100，超出服务端回 431、客户端报 "message header too large"；`max_buf_size` 默认约 400KB，下限 8192；`max_headers` 一旦设置就改用堆分配，文档称约 5% 性能代价（`server/conn/http1.rs:325-341`）。1.12.0 新增 `max_header_size`（【W】）。
- H2 服务端默认值（`proto/h2/server.rs:30-75`）：连接窗口与流窗口各 1 MiB，最大帧 16 KiB，`max_concurrent_streams` 200，发送缓冲 400 KB，头列表 16 KiB，本地错误重置流上限 1024，keep-alive ping 关闭，`enable_connect_protocol` 关闭。H2 客户端默认值（`proto/h2/client.rs:44-61`）：连接窗口 5 MiB，流窗口 2 MiB，发送缓冲 1 MiB，初始可发流数 100，`enable_push(false)`。

### 1.11 H1 与 H2 之间必须手工转换的东西

- H2→H1：把 `:authority` 补成 `Host`（`conn::http1` 不会，`legacy::Client` 会）；把请求 URI 改成 origin-form；H2 允许多个 `cookie` 头，H1 只应有一个，需合并成 `a; b`（hudsucker 在 `normalize_request` 里做了，hyper 不做）【W】。
- H1→H2：URI 必须带 scheme 和 authority。h2 客户端在 `version == HTTP_2` 且 URI 缺 scheme 与 authority 时报 `MissingUriSchemeAndAuthority`，版本不是 2.0 时则默默把 scheme 设成 `http`（h2 `client.rs:1661-1687`），对 MITM 出来的 https 请求这是错的，必须先把 origin-form 补成 `https://host/path`。hyper 的 H2 层会自动去掉 connection 专用头（`proto/h2/mod.rs:30-92`）。
- H2 服务端把 `:scheme`、`:authority`、`:path` 拼成 `Uri`；没有 `:authority` 时 scheme 被丢弃（h2 `server.rs:1700-1735`），要回退用 `Host`。
- 1xx 在两个方向上都转发不了（1.6）。

### 1.12 已知局限，以及别人为什么没用或改了 hyper

- **Cloudflare Pingora**：自研 HTTP 库，原因写的是灵活性与步调自控。文中点名 hyper："Although there are some great off-the-shelf 3rd party HTTP libraries, such as hyper, we chose to build our own because we want to maximize the flexibility in how we handle HTTP traffic"，并说 "We have many cases of bizarre and non-RFC compliant HTTP traffic that we have to support."（【W】，blog.cloudflare.com，逐字复核过两次）。但 2026 年的走私漏洞文章也警告宽容的代价：Pingora "does allow edge case HTTP requests or responses that are not strictly RFC compliant"，修复在 0.8.0（CVE-2026-2833、2835、2836）；同文还说 nginx 与 hyper 会直接拒绝"HTTP/1.0 带 Transfer-Encoding"这类歧义请求（【W】）。
- **rama**：维护 hyper、h2、http、http-body、hyper-util、tower 等的"同步分叉"，README 只说要保持与上游同步，没有写理由（【W】）；`rama-http-types` 把 `http` 也 vendor 了（搜索摘要）。
- **mitmproxy**：自写 HTTP/1 栈（基于 h11），文档说 "Protocol violations are often deliberately forwarded or inserted at the proxy"，并列出 "mitmproxy currently does not support trailers with HTTP/1.x"（【W】）。
- **hudsucker 用户**：issue #142 指出 hudsucker 总是删 `Host`，维护者认为"仅在代理到服务器是 H2 时才该删"，但"用 hyper-util 的客户端无法事先判断"（【W】）。原因见第 6 节：legacy `Client` 在连接建立之后才知道 ALPN 结果。
- 以上加上 1.2、1.3、1.4、1.6、1.8 的源码事实，可以归纳 hyper 作为"透明 MITM 引擎"的缺口（推断）：①不保头顺序；②读不到原始拼写；③拿不到原始头字节与 `headersSize`；④畸形头只能丢弃或拒绝；⑤发不出 1xx；⑥请求侧 obs-fold 直接 400；⑦非 UTF-8 的 URL 直接 400；⑧trailers 要满足声明条件。这些在"不要求字节级保真"的 MITM 里都可以接受。需要字节级保真时，现成路线有三条：用 `httparse` 自写 H1 编解码、像 rama 那样维护分叉、像 Pingora 那样整套自研（并自担走私风险）。

---

## 2. 现有 MITM 代理是怎么搭的

### 2.1 hudsucker（Rust）

- 版本与许可：0.25.0（2026-07-15），MIT OR Apache-2.0，MSRV 1.86.0，累计下载 548,944（【W】，crates.io API）。GitHub 约 384 星、9 个开放 issue（【W】）。
- **拦截决定**（【W】，`src/proxy/internal.rs`，关键判断式逐字复核过）：
  - 先对每个请求（含 CONNECT）调用 `HttpHandler::handle_request`，可以直接短路回响应。
  - CONNECT 走 `process_connect`：立刻回一个空的成功响应，后台任务里 `hyper::upgrade::on` 取到升级后的流，读 4 个字节，用 `Rewind` 把这 4 字节放回去。
  - 随后 `should_intercept_connect` 返回 false 则透传（`TcpStream` 连目标加 `copy_bidirectional`）。
  - 4 字节等于 `"GET "` 才当明文 HTTP 解析；前两个字节是 `\x16\x03` 才当 TLS；其他一律打 warn "Unknown protocol" 后透传。所以明文 HTTP 的 `POST`、`PUT`、`HEAD` 等在 CONNECT 隧道里不会被拦截。
  - TLS 分支用 `LazyConfigAcceptor` 包一层 `Tee` 读 ClientHello，再问 `should_intercept_tls(ctx, client_hello)`，拒绝就把 `Tee` 记下的原始字节回放后透传。
  - 证书的主机名取 ClientHello 里的 SNI，没有 SNI 才用 CONNECT 的目标。
- **叶证书**（【W】，`rcgen_authority.rs`）：`RcgenAuthority` 内部是 `moka::future::Cache<Authority, Arc<ServerConfig>>`，容量由调用者给，TTL 为 `TTL_SECS / 2`（半年）；缓存键是完整的 `Authority`（含端口，`example.com` 与 `example.com:8443` 是两项）。叶证书：随机 u64 序列号，CN 为 host，SAN 只有一个 `DnsName(host)`，没有为 IP 主机名生成 IP 类型的 SAN（推断，源自 `SanType::DnsName` 的用法），KeyUsage 只有 digitalSignature，EKU 只有 serverAuth，带 AKI，`notBefore = now − 60s`，有效期 `TTL_SECS = 365 天`。叶证书的私钥直接用 CA 的私钥（`issuer.key().serialize_der()`，逐字复核），省去每个叶证书生成密钥的开销，代价是叶证书与 CA 共用同一把私钥。`ServerConfig` 的 ALPN 为 `h2`（仅 `http2` 特性）加 `http/1.1`。
- **HTTP/2**：客户端侧用 hyper-util 的 `auto::Builder::serve_connection_with_upgrades`，由 TLS 之后的 ALPN 或 H2 前言自动分流；ALPN 由 CA 生成的 `ServerConfig` 决定，不看上游。上游用 `hyper_util::client::legacy::Client`，连接器默认是 `hyper-rustls`（`enable_http1`，开 `http2` 特性时 `enable_http2`），ALPN 由连接器协商；`normalize_request` 把请求版本设成 HTTP/1.1、删掉 `Host`（让 hyper 重新生成）、把多个 `Cookie` 合并成一个。没有"镜像上游 ALPN"的逻辑。
- **WebSocket**：用 `hyper_tungstenite::is_upgrade_request` 识别，自己对客户端完成一次 WebSocket 握手，再用 `tokio_tungstenite::connect_async_tls_with_config` 另开一条到上游的 WebSocket（重新握手，不是转发原握手），然后两个转发任务逐条消息调用 `WebSocketHandler::handle_message`，返回 `None` 丢弃该消息。issue #133（"wss:// protocol websocket connections cannot be intercepted"，2024-12 提出，17 条评论，未解决）里提到带 `Authorization` 的握手头似乎没带到上游，维护者复现不出来，原因未明（【W】）。（推断：重新握手本身就是一类风险，Cookie、Authorization、Origin、`Sec-WebSocket-Protocol` 等握手头需要显式带过去。）
- **池**：一个全局的 `legacy::Client`（池键为 scheme + authority），`ProxyBuilder::with_client` 可替换；池参数在 builder 里没有设置，用 hyper-util 默认值（【W】）。
- **钩子**：`HttpHandler` 有 `handle_request(&mut self, ctx, Request<Body>) -> RequestOrResponse`、`handle_response`、`handle_error`（默认回 502）、`should_intercept_connect`、`should_intercept_tls`；`WebSocketHandler::handle_websocket` 默认实现逐条调 `handle_message(ctx, Message) -> Option<Message>`；`HttpContext` 只有 `client_addr` 一个字段。`Body` 是库自己的类型，实现细节未见；`decoder` 特性提供 `decode_request`、`decode_response` 做解压（推断：这两个是缓冲型的）。
- **已知问题**（【W】）：#145 客户端到代理这一跳的 HTTP/2 不支持（2025-05，维护者回复目前做不到，但应该比较容易实现）；#144 reCAPTCHA 在开关 HTTP/2 时都异常，维护者复现不出来；#142 总是删 `Host`；#125 访问本地 IP 站点时浏览器报 `ERR_CERT_COMMON_NAME_INVALID`，讨论里把原因指向代理出站不信任该站证书，没有证实是 IP-SAN 问题；#170 npm 报 `UNABLE_TO_VERIFY_LEAF_SIGNATURE`，最后疑似用户把 `cafile` 写成相对路径。

### 2.2 rama（Rust）

- 版本与许可：0.4.0（2026-08-19），MIT OR Apache-2.0，**MSRV 1.96.0**（高于 rurge 的 1.89），累计下载 129,878（【W】，crates.io API）。0.3.0（2026-07-07）到 0.4.0（2026-08-19）相隔约六周，GitHub 约 1.3k 星、61 个开放 issue。示例自己写着 "this proxy is not production ready, and is only meant to show you how one might start"（【W】）。
- 架构：Service/Layer 栈。仓库里有 `http_mitm_proxy_rustls.rs`、`http_mitm_proxy_boring.rs`、`http_mitm_relay_proxy_boring.rs`、`socks5_connect_proxy_mitm_proxy.rs`、`tls_sni_proxy_mitm.rs`、`ws_over_h2.rs`、`quic_h3_mitm_proxy.rs` 等示例（【W】）。
- `http_mitm_proxy_rustls.rs` 自称 "manual variant … not the recommended proxy"，推荐 Relay/Peek 栈：
  - CONNECT 先建好出站再回成功（`EagerHttpProxyConnector`），出入站成对交给 `BridgeIo<Ingress, Egress>`。
  - `PeekTlsClientHelloService` 窥探是否 TLS；是则走 `TlsMitmRelay`（文档注释 "The TLS relay mirrors the ingress ClientHello and target certificate"），否则 `maybe_http_relay`。
  - `HttpPeekRouter::with_known_non_http_protocol_methods()` 识别明文 HTTP，认不出的走 `IoForwardService` 原样转发。窥探与回放的实现细节、HttpPeekRouter 源码：未见。
  - SNI 路由示例按 SNI 选择性终止 TLS，其余字节原样桥接。
- H2：manual 例子里入站 `with_alpn_http_auto()` + `HttpServer::auto`，上游固定 `Version::HTTP_11`；Relay 路线里，issue #932（2026-05 提出、06-01 关闭）指出此前"对所有客户端无条件宣告 RFC 8441 扩展 CONNECT，上游不支持时回 400 且客户端不会回退到 H1 WebSocket"，修复为先对上游做 H2 握手、观察其 SETTINGS，再把 `SETTINGS_ENABLE_CONNECT_PROTOCOL` 镜像到入站（【W】）。issue #514（2025-04）里 MITM 例子因为漏设 ALPN 导致上游报 `FRAME_SIZE_ERROR` GOAWAY，是示例 bug（【W】）。
- 池与性能：出站客户端是 `EasyHttpWebClient`，manual 例子里 `without_connection_pool()` 关了池；issue #1224（2026-10）报告 TCP_NODELAY 默认关闭导致 ≥16 KiB 的响应因 delayed ACK 卡约 42 ms，以及池的 checkout 在一把锁下遍历同源全部连接，1 KiB 的 MITM 吞吐在 64 到 1024 并发间从 99k rps 掉到 16k rps（【W】）。
- 分叉：维护 hyper、h2、http 等的同步分叉（1.12 已述）；自带 WebSocket 实现（含压缩），有 WebSocket over H2 与 H3 的示例。

### 2.3 mitmproxy（Python，MIT，12.3.1，2026-10-25，Python ≥ 3.12）

来源：docs.mitmproxy.org 与 GitHub 源码（【W】，关键判断复核过）。

- **分层与"下一层"判定**：每个新连接先放一个 `NextLayer` 占位层，默认只在收到客户端数据时（`DataReceived`）才触发 `next_layer` 钩子，`ask_on_start` 参数可选（调用点未核实）；信息不够时 `next_layer` 抛 `NeedsMoreData`，下次来数据再重试。读到的 `next_layer.py` 与 `tls.py` 里没有看到超时。判定顺序：
  1. `ignore_hosts` / `allow_hosts`：对 `host:port` 字符串做 `re.search`（忽略大小写），候选串包括对端地址、Host 头、ClientHello 的 SNI；需要时才去解析 Host 头或 ClientHello，数据不全则 `NeedsMoreData`。命中忽略的 TCP 连接变成不处理的 `TCPLayer`。
  2. 反向代理或显式 HTTP 代理模式：按规格拼层；显式代理下，数据像 TLS 就先放 `ClientTLSLayer`，再放 `HttpLayer`。
  3. 数据像 TLS / DTLS / QUIC：TLS 的判断是长度大于 2、第一个字节 `0x16`、第二个字节 `0x03`、第三个字节在 `0x00..=0x03`；DTLS 是 `0x16 0xFE` 加 `0xFD..=0xFE`；QUIC 要求至少 18 字节，长头部看版本号，短头部只能看端口是否在 80/443/8443。
  4. `tcp_hosts` / `udp_hosts`：匹配目的地址或 SNI（不含端口）。
  5. ALPN 属于 HTTP 则 `HttpLayer`；端口 53/5353 则 DNS；其他 UDP；最后是 raw TCP 启发式（见第 3 节）。默认落到 `HttpLayer`。
- **TLS 与上游证书嗅探**：`ClientTLSLayer` 缓冲并解析 ClientHello，然后触发 `tls_clienthello` 钩子，钩子可以设 `establish_server_tls_first`（先和上游握手）或 `ignore_connection`（旁路）。内置 `tlsconfig` 插件的做法是 `establish_server_tls_first = (服务端是 TLS) and connection_strategy == "eager"`，而 `connection_strategy` 默认是 `eager`（"Determine when server connections should be established."）。叶证书的名字来自：`upstream_cert`（默认 True，"Connect to upstream server to look up certificate details."）为真且上游证书已拿到时，复制上游证书的 CN（当作 DNS 名或 IP）、SAN、Organization，并把第一个 CRL 分发点改成指向 mitmproxy 自己的路径；再加上客户端 SNI（没有 SNI 就用客户端本地 socket IP）和已知的服务器地址；去重后 CN 取第一个 SAN。官方文档对此的表述："we use the Common Name in the upstream certificates to generate the dummy certificate for the client"，以及 "SNI breaks our upstream certificate sniffing process" 的处理："we allow the TLS handshake to continue until just after the SNI value has been passed to us"。
- **叶证书的生成与缓存**（`mitmproxy/certs.py`）：缓存键是 `(common_name, sans)`，容量 `STORE_CAP = 100`，先进先出淘汰；叶证书有效期 `CERT_EXPIRY = 199 天`，`notBefore` 往前推 2 天；EKU 只有 serverAuth；不加 SubjectKeyIdentifier（注释写着为了避开 SChannel 的问题）；带 AKI；CRL 分发点按需加；所有叶证书共用同一个 `default_privatekey`。
- **ALPN**（`tlsconfig.py`，摘要转述，回调细节存疑）：客户端侧的 `alpn_select_callback` 大意是优先采用"服务端已协商、且客户端也提供了"的协议；服务端明确拒绝 ALPN 则对客户端也拒绝；否则按偏好表（`http2` 开则含 h2）选第一个。服务端侧：客户端的 ALPN 原样带到上游，`http2` 关闭时先去掉 `h2`；客户端没带 ALPN 则上游也不带。另有一条特例：当 mitmproxy 自己作为 HTTPS 代理（客户端到代理这一跳用 TLS）时，这一跳的 ALPN 被强制为 `http/1.1`，原因是不支持 CONNECT over HTTP/2。
- **客户端拒绝伪造证书怎么报**：`ClientTLSLayer.on_handshake_error` 以 WARNING 记 `Client TLS handshake failed. <原因>`，原因文本有几种：TLS 版本无法协商（提示 `tls_version_client_min`）、客户端不信任（unknown ca / bad certificate / certificate unknown）时 "The client does not trust the proxy's certificate for {dest} ({err})"、握手中途断开时 "The client disconnected during the handshake. If this happens consistently for {dest}…"、其他错误 "The client may not trust the proxy's certificate for {dest} ({err})"、ClientHello 解析失败 "Cannot parse ClientHello"。同时触发 `tls_failed_client` 钩子；这时还没有 HTTP flow，所以不会出现在 flow 列表里（推断：该钩子独立于 HTTP flow）。`examples/contrib/tls_passthrough.py` 示例用 `tls_clienthello` / `tls_established_client` / `tls_failed_client` 三个钩子，给每个服务端地址记最近 200 次结果，保守策略下只要有过一次失败，之后对该地址就设 `ignore_connection` 旁路。文档对证书固定（pinning）的建议是用 `ignore_hosts` 忽略（"Some traffic is protected using Certificate Pinning and mitmproxy's interception leads to errors."）。
- **`ignore_hosts` 的语义**（文档）：对 `host:port` 做正则匹配；透明模式匹配的是 IP 与 ClientHello SNI；常规与上游代理模式下 "explicit HTTP requests are never ignored"，只对 CONNECT 生效，因为一条连接可能承载多个域名。`allow_hosts` 是反向开关；`tcp_hosts` 把命中的主机按通用 TCP 处理。
- **HTTP 层与连接复用**：每个客户端连接一个 `HttpLayer`，它自己持有 `connections` 字典，复用的条件是地址、tls、via、传输协议都一致；H2 客户端连到 H1 服务端时强制新开连接；没有跨客户端连接的全局池。协议由 ALPN 决定：服务端侧 `h2` 用 `Http2Server`，`h3` 用 `Http3Server`，否则 `Http1Server`；上游侧同理。
- **大 body 与流式**：`stream_large_bodies`（默认 None，"Stream data to the client if request or response body exceeds the given threshold."）和 `body_size_limit`（默认 None，超限直接 kill）。流式时 body 不缓冲、直接转发，之后 "the message body will not be accessible within mitmproxy, and body modifications will have no effect"；必须在 `requestheaders` 或 `responseheaders` 钩子里设置 `.stream`，晚了就来不及；`.stream` 可以是逐块转换的函数；`store_streamed_bodies`（默认 False）控制是否同时存下来。不开流式时整份 body 缓冲到 `request` / `response` 钩子才触发。
- **HTTP/2 与 WebSocket**：H1 是基于 h11 的自研栈，H2 基于 hyper-h2（Python），上游不讲 H2 时 "seamlessly translates messages to HTTP/1"；官方列出的限制：忽略 PRIORITY、不通告 Push Promise、不支持 h2c、HTTP/1 不支持 trailers、不检测 STARTTLS。`validate_inbound_headers`（默认 True）与 `normalize_outbound_headers`（默认 True，"Normalize outgoing HTTP/2 header names, but emit a warning when doing so."）。WebSocket 基于 wsproto，支持消息压缩，PING/PONG 转发但不保存，未知扩展透传并告警；识别条件是 101 + `upgrade: websocket` + 版本号匹配 + `websocket` 选项开着。
- **钩子**：HTTP 有 `requestheaders`（"HTTP request headers were successfully read."）、`request`（"The full HTTP request has been read."）、`responseheaders`、`response`、`error`、`http_connect` / `http_connect_upstream` / `http_connected` / `http_connect_error`；TLS 有 `tls_clienthello`、`tls_start_client`、`tls_start_server`、`tls_established_*`、`tls_failed_*`；WebSocket 有 `websocket_start`、`websocket_message`、`websocket_end`；还有 `next_layer`。

### 2.4 其他

- **http-mitm-proxy**（hatoo，Rust）：0.18.0（2026-01-24），MIT，近期下载约 14k（【W】）。API 是 hyper 风格的 `service_fn(Request) -> Response`，没有内建出站客户端（可选的 `DefaultClient`）。是否拦截完全由 `server_config(host)` 是否返回 `Some` 决定，生成证书失败只记日志并静默回落为隧道；不窥探字节，不看 ALPN 之外的东西。叶证书按主机名生成，可选 `moka::sync::Cache<String, _>`，键为主机名。客户端 ALPN 是 `h2` 就用 `http2::Builder`，否则用 `http1::Builder` 并开 `preserve_header_case(true)` 与 `title_case_headers(true)`；H1 分支带 `with_upgrades()`，H2 分支没有（【W】）。
- **proxyfor**（sigoden，Rust）：0.5.0（2024-12-24），MIT OR Apache-2.0，近期下载 8，实质上停更（【W】）。依赖里有 `fancy-regex 0.14`、`moka`、`rcgen 0.13`、`hyper-rustls`、`hyper-tungstenite`、`async-compression`。CONNECT 后同样读 4 字节，`"GET "` 或 `\x16\x03` 才处理，其余透传。每次 `handle` 新建一个 `Client::builder`，因此没有跨请求的连接复用。抓包：用 `BodyWrapper` 在转发的同时把 body 写进临时目录的文件，内存里只留路径与大小；没有大小或条数上限（【W】）。
- **whistle**（Node，MIT，约 15.8k 星）：规则文本驱动。`enable://capture`（或 `enable://https`）开启 HTTPS 解密，`disable://capture` 按规则关闭，对 IP 主机名默认不解密、需要 `enable://captureIp`；`enable://socket`：开启 HTTPS 解析后，发往 80/443 端口的 TUNNEL 请求强制尝试按 HTTP/HTTPS 解析，解析失败可回落为 TUNNEL；`enable://h2`（whistle 到服务器用 HTTP2）、`enable://http2`（浏览器到 whistle 到服务器全程 HTTP2）；`enable://websocket` 用于识别非标准 WebSocket（【W】，wproxy.org 文档）。内部实现未见。
- **pingora-http**（参考）：0.9.0，Apache-2.0，更新于 2026-09-09，累计下载 1,016 万（【W】）。

### 2.5 对照表

| | hudsucker | rama | mitmproxy | http-mitm-proxy | proxyfor |
|---|---|---|---|---|---|
| 许可 | MIT OR Apache-2.0 | MIT OR Apache-2.0 | MIT | MIT | MIT OR Apache-2.0 |
| 拦截决定 | `should_intercept_connect` 再读 4 字节；`should_intercept_tls(ClientHello)` | 例子里全拦或按 SNI；Relay 栈用 Peek | `next_layer` 钩子链；`ignore_hosts` / `allow_hosts` / `tcp_hosts`；`tls_clienthello` 可旁路 | `server_config` 返回 None 即隧道 | 读 4 字节，仅 `GET ` 或 TLS |
| 叶证书 | rcgen；SAN 仅 host；缓存 moka 按 `Authority`，TTL 半年；叶私钥即 CA 私钥 | 例子里自签 CA；Relay 镜像上游证书（文档注释） | 复制上游 CN/SAN/O；缓存 `(cn,sans)` 100 条 FIFO；199 天；共用私钥 | rcgen；按主机名；可选 moka | rcgen；缓存细节未见 |
| 先连上游再握手 | 否 | Relay 路线是 | 默认是（`eager`） | 否 | 否 |
| ALPN | 固定 `h2`、`http/1.1`，不看上游 | `with_alpn_http_auto`；Relay 路线镜像上游 H2 设置 | 镜像到上游并优先上游协商结果 | 固定 `h2`、`http/1.1` | 未见 |
| H2 | 客户端侧 `auto::Builder`；出站 ALPN 由连接器定 | 双端各自选择 | 自研 H2 栈，上游不支持就降级 H1 | 客户端侧支持，H2 分支无 upgrade | 经 hyper-rustls `enable_all_versions` |
| WebSocket | 重新握手 + 消息级钩子 | 自有实现，含压缩 | wsproto，消息级，支持压缩 | 原始升级透传 | hyper-tungstenite + 新开上游连接 |
| 上游池 | 全局 `legacy::Client` | `EasyHttpWebClient` 池（例子关闭） | 每客户端连接一个，无全局池 | 无（用户提供） | 每请求新建 Client，无复用 |
| 钩子形态 | `HttpHandler` / `WebSocketHandler` trait | Layer / Service | 插件事件（headers 与 full 两档） | `service_fn` | 无 |
| body 默认 | 流式（`Body` 包装），解码需自己缓冲 | 例子里 `BodyLimitLayer` 2 MiB | 默认全缓冲，超阈值才流式 | 由 service 决定 | tee 到临时文件 |

---

## 3. 协议嗅探：不消耗字节地判断 H1 / H2 先验 / TLS / 其他

### 3.1 各家的字节模式与等待

| 来源 | 判断 TLS | 判断 HTTP/1 | 判断 H2 先验知识 | 等待与超时 | 判不出时 |
|---|---|---|---|---|---|
| hudsucker、proxyfor | 前 2 字节 `16 03` | 前 4 字节等于 `"GET "`，其他方法落入透传 | 无 | 未见超时 | 回放字节并透传（hudsucker 打 warn） |
| mitmproxy | 长度 > 2，`0x16 0x03 0x00..0x03`；DTLS 为 `0x16 0xFE 0xFD..0xFE`；QUIC 另有启发式 | 不做方法表；仅在提取 Host 时用正则 `[A-Z]{3,}.+HTTP/`；raw TCP 判定见下 | 靠 ALPN | 未见超时；信息不全 `NeedsMoreData` 等下一批数据 | `rawtcp` 开且判为非 HTTP 则 `TCPLayer`，否则默认 `HttpLayer` |
| hyper-util `auto::Builder` | 不管 | 先假定 H2、读到不符立即判 H1 | 24 字节前言 `PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n` 逐字节增量比对，首个不符或读到 EOF 就判 H1 | 本身无超时；判定为 H1 后才受 `header_read_timeout`（需 timer） | 已读字节用内部 `Rewind` 回放 |
| rama（例子） | `PeekTlsClientHelloService` | `HttpPeekRouter::with_known_non_http_protocol_methods()` | 未见 | 未见 | `IoForwardService` 原样转发 |
| sing-box | `tls` 嗅探器 | `http` 嗅探器，取 Host | 未见 | `sniff` 动作的 `timeout` 默认 300ms | 没有嗅探结果就按原目标继续 |
| Envoy | `tls_inspector`，ClientHello 超过 64KiB 记 `client_hello_too_large` | `http_inspector` 区分 http10 / http11 / http2 | `http_inspector` 的 `http2_found` | 监听器过滤器总超时 `listener_filters_timeout` 默认 15s，设 0 禁用；超时默认关闭 socket，`continue_on_listener_filters_timeout` 默认 false | 同左 |
| whistle | — | 发往 80/443 的隧道请求强制尝试按 HTTP/HTTPS 解析 | — | 未见 | `enable://socket` 允许回落为 TUNNEL |
| Squid（GPL，文字描述） | — | — | — | 配置项 `on_unsupported_protocol`，动作 `tunnel`（直连目标原样转发）或 `respond`（回错误页，默认）；ACL 里 `squid_error` 可以匹配 "request start timeout"，用来处理服务器先说话的协议；文档标注 v4 至 v7 可用，v8 没有 | 同左 |

### 3.2 mitmproxy 的 raw TCP 启发式（【W】，条件逐字复核过）

只有当 `rawtcp` 选项开着（默认开）并且下面任一成立才走 raw TCP：客户端已有数据少于 3 字节；数据里没有空格；第一个空格在第一个换行之后；前三个字节不全是字母；服务端已经先发了数据；数据以 `SSH` 开头。否则默认当 HTTP。注意：因为默认只在收到客户端数据时才问一次，纯"服务器先说话"的协议若客户端迟迟不发数据，`next_layer` 就一直没有机会决定（调用点是否用 `ask_on_start` 未核实），读到的代码里也没有超时。

### 3.3 不消耗字节的实现方式

- `tokio::net::TcpStream::peek` 只适用于裸 TCP socket。CONNECT 升级后的流（`hyper::upgrade::Upgraded`）、TLS 解密后的流、TUN 网关的虚拟 TCP 流都没有 peek。（推断：rurge 的会话来自 HTTP CONNECT、SOCKS5、TUN 多种入站，应当统一用"前缀缓冲再回放"的包装。）
- 现成做法都是自己写一个 `Rewind` / 前缀包装：hudsucker 与 proxyfor 各有 `rewind.rs`（`Rewind` 加定长 `ArrayPrefix`）；hyper-util 的 `Rewind` 是内部类型，不公开（`auto/mod.rs:33` 引用 `crate::common::rewind`）。
- TLS 的 ClientHello 窥探用 `tokio_rustls::LazyConfigAcceptor`（tokio-rustls 0.26.4 `server.rs:70-133,192-225`，【源码】）。`start.client_hello()` 返回 `rustls::server::ClientHello`，可读 `server_name()`、`alpn()`、`signature_schemes()`、`cipher_suites()`、`server_cert_types()` 等（rustls 0.23.43 `server/server_conn.rs:139-211`）。选好 `ServerConfig` 后 `into_stream(config)`。出错时可以 `take_io()` 取回 IO，但 rustls 的 `Acceptor` 已经读走的字节不会还给你，所以"不是 TLS 就回落为隧道"必须像 hudsucker 那样在 IO 外面套一层 `Tee` 记录读过的字节。`Acceptor` 在 ClientHello 不完整时返回 `Ok(None)` 等更多数据，没有内置超时。
- 同步方案是 `ResolvesServerCert::resolve(ClientHello)`（rustls `server_conn.rs:124-135`），回调内不能做异步 I/O；需要先连上游再决定证书（复制上游 SAN/CN）时只能用 `LazyConfigAcceptor`。
- ECH：rustls 的 `ClientHello` 只暴露外层 SNI（推断：浏览器启用 ECH 时拿到的是公共名，不是真实目标；本次调研没有查 ECH 的支持状况）。

### 3.4 握手字节细节

- TLS 记录头 5 字节：类型 `0x16`，版本两字节，长度两字节，之后第一个握手消息的类型是 `0x01`（ClientHello）。浏览器的初始 ClientHello 通常用记录版本 `0x0301`，其他记录用 `0x0303`（RFC 8446 的 legacy_record_version，凭记忆，未在本次调研里核对原文）。mitmproxy 的判断范围 `0x00..=0x03` 覆盖 SSLv3 至 TLS 1.3。
- HTTP/1 方法最长的常见字面量是 `CONNECT `、`OPTIONS `（8 字节含空格），WebDAV 的 `PROPFIND ` 等更长；用固定 4 字节只能认 `GET `（hudsucker 与 proxyfor 的缺陷）。

---

## 4. HTTP/2 MITM

### 4.1 h2 crate 的用法（【源码】h2 0.4.19）

- 服务端：`h2::server::handshake(io)`（或 `Builder::handshake`）得到 `Connection`，循环 `poll_accept` 得到 `(Request<RecvStream>, SendResponse)`；每个流就是一个请求，各自 spawn 任务（`server.rs:425`）。`SendResponse::send_response(resp, end_of_stream)` 返回 `SendStream`；`send_reset(Reason)`、`poll_reset` 用于取消。
- 客户端：`h2::client::handshake` 得到 `(SendRequest, Connection)`，`Connection` 要被 spawn；`send_request(req, end_of_stream)` 返回 `(ResponseFuture, SendStream)`。请求 URI 必须带 scheme 与 authority（1.11）。
- 流量控制完全交给调用者：入站用 `RecvStream::flow_control().release_capacity(n)`，处理完才放窗口（文档：`WINDOW_UPDATE` 批量发送）；出站先 `reserve_capacity`，`poll_capacity` 就绪再 `send_data`。文档警告："There is no bound on the amount of data that the library will buffer. If you are sending large amounts of data, you really should hook into the flow control lifecycle."，以及分配给某个流的连接级窗口"is reserved on the connection and will **not** be assigned to any other stream … this introduces the risk of deadlocking a connection."（`share.rs:56-74`）。hyper 的 `PipeToSendStream` 已经按这个模式实现，直接用 h2 的代理要自己写。
- GOAWAY：`Connection::graceful_shutdown()` 按 RFC 先发 `last_stream_id = 2^31-1`、`NO_ERROR` 的 GOAWAY，等一次 PING 往返，再发最终的 GOAWAY（`proto/connection.rs:629-651`）；`abrupt_shutdown(reason)` 立即发并关闭。h2 客户端收到 GOAWAY 后，已有流继续，新建流失败（hyper 的 `http2::SendRequest::is_closed()` 可用来在池里剔除，见第 6 节）。
- DoS 相关参数（服务端 builder）：`max_concurrent_reset_streams`（默认 50）、`max_local_error_reset_streams`（h2 默认 1024，hyper 同为 1024）、`max_pending_accept_reset_streams`（默认 20）、`data_frame_budget`（0.4.18 新增，默认连接窗口的一半，最小 25,600 字节，耗尽以 `ENHANCE_YOUR_CALM` 关闭连接）。h2 0.4.14 到 0.4.19 的 CHANGELOG 连续修复了小 DATA 帧、流控泄漏、重置竞态等问题（`CHANGELOG.md`），0.4.20 已发（2026-10-06，【W】）。
- 1xx：0.4.13 起服务端有 `SendResponse::send_informational`（`server.rs:1190`），客户端有 `ResponseFuture::poll_informational`（`client.rs:1529`）。hyper 1.11.1 没有把它们接出来。

### 4.2 hyper 的 H2 封装对代理意味着什么（【源码】）

- 每个入站流在 `poll_server` 里 `service.call(req)`，再交给执行器 spawn（`proto/h2/server.rs:263-318`），并发上限由 `max_concurrent_streams`（默认 200）控制。
- 响应头发出前会去掉 connection 专用头（`Connection`、`Keep-Alive`、`Proxy-Connection`、`Transfer-Encoding`、`Upgrade`，以及响应里的 `TE`，还会顺着 `Connection` 头里列出的名字再删），补 `Date`（`server.rs:471-483`，`auto_date_header` 可关）；body 长度已知而响应头没有 `Content-Length` 时会补上它（`server.rs:515-519`）。
- `CONNECT` 请求：不允许带非零长度 body（否则回 `INTERNAL_ERROR` 重置），成功响应走升级通道，把 `send_stream` 与 `recv_stream` 配对成 `Upgraded`（`server.rs:276-303,485-512`）。
- `adaptive_window` 会覆盖手设的窗口（`server/conn/http2.rs:183-190`）。默认窗口 1 MiB（服务端）与 2 / 5 MiB（客户端），比规范的 64 KiB 大，源码注释写明是为性能（`proto/h2/server.rs:30-35`）。

### 4.3 扩展 CONNECT（RFC 8441）与 WebSocket over H2

- hyper 服务端 `http2::Builder::enable_connect_protocol()`（`server/conn/http2.rs:258`，最终调到 `h2::server::Builder::enable_connect_protocol`，`h2 server.rs:1043`）在 SETTINGS 里宣告 `SETTINGS_ENABLE_CONNECT_PROTOCOL`。之后带 `:protocol` 的 CONNECT 请求的扩展里会有 `hyper::ext::Protocol`（`ext/mod.rs:66-113`；`proto/h2/server.rs:305-307`），响应 2xx 即进入 `Upgraded`，用 `hyper::upgrade::on(req)` 取。h2 在非 CONNECT 请求上带 `:protocol` 会判为 malformed（h2 `server.rs:1686-1694`）。
- 客户端：请求扩展里放 `hyper::ext::Protocol`（hyper 转成 `h2::ext::Protocol`，`proto/h2/client.rs:731-733`）；`SendRequest::is_extended_connect_protocol_enabled()` 可查对端是否宣告（`client/conn/http2.rs:244`）。hyper-util 0.1.11 的 CHANGELOG 有 "Fix HTTP/2 websocket requests sent through `legacy::Client`"。
- 浏览器只在服务器宣告该设置时才会用 H2 承载 WebSocket（搜索摘要，未核对一手来源；RFC 8441 原文本次未读）。rama 的 #932 说明了一个陷阱：如果 MITM 对下游无条件宣告，而上游并不支持，上游会回 400 且浏览器不会自动回退（【W】）。
- 如果 MITM 服务端不宣告，浏览器应当回到 H1 的 `Upgrade: websocket`（推断），那就走第 1.5 节的 H1 升级透传，路径更简单；代价是浏览器会为 WebSocket 另开一条 ALPN 为 http/1.1 的连接（推断）。
- H2↔H1 的 WebSocket 互转需要手工：H2 侧是 `CONNECT` + `:protocol = websocket`，没有 `Sec-WebSocket-Key` / `Sec-WebSocket-Accept`；H1 侧是 `GET` + `Upgrade` + 这两个头。

### 4.4 hudsucker 与 rama 用户报告的坑（【W】）

- 客户端到代理这一跳不能用 HTTP/2（hudsucker #145）；MITM 内层的 H2 是支持的。
- 请求头里的 `Host` 与 `:authority` 的取舍，在无法事先知道上游是 H1 还是 H2 时无解（hudsucker #142）。
- 漏设上游 ALPN（rama #514）会表现成奇怪的 `FRAME_SIZE_ERROR` GOAWAY。
- 无条件宣告扩展 CONNECT 导致 WebSocket 全挂（rama #932）。
- TCP_NODELAY 默认关闭造成 40 毫秒级的停顿（rama #1224）；大并发下池的 checkout 与等待路径成为瓶颈。
- 站点指纹：hudsucker #144 的 reCAPTCHA 现象在开关 H2 时都出现，维护者复现不出来，原因未明；（推断：MITM 之后站点看到的是 rurge 自己的 TLS 与 H2 指纹，而不是浏览器的，可能触发风控。rama 的 `TlsMitmRelay` 之所以"镜像入站 ClientHello"，大概也是为此，仅为推断。）

---

## 5. 抓包格式：HAR 1.2 与 mitmproxy 的流存储

### 5.1 HAR 1.2

来源说明：softwareishard.com 的原始规范打不开（证书过期），W3C WebPerf 的 `webperfwg.org/specs/HAR/Overview.html` 是 2012-08-14 的历史草稿，页面自己标 "DO NOT USE"、"never published … has been abandoned"，但字段清单里带 "new in 1.2" 标记；下面的引文取自 GitHub 镜像 `ahmadnassri/har-spec` 的 `versions/1.2.md`（标题为 "HTTP Archive (HAR) v1.2"，无作者与日期），均为【W】。

| 对象 | 字段 |
|---|---|
| `log` | `version`、`creator`（必填），`browser`、`pages`、`comment`（可选），`entries`（必填） |
| `creator` | `name`、`version`、`comment` |
| `entries[]` | `pageref`、`startedDateTime`、`time`、`request`、`response`、`cache`、`timings`，1.2 增加 `serverIPAddress`（"IP address of the server that was connected (result of DNS resolution)"）、`connection`（"Unique ID of the parent TCP/IP connection, can be the client or server port number."）、`comment` |
| `request` | `method`、`url`、`httpVersion`、`cookies`、`headers`、`queryString`、`postData`、`headersSize`、`bodySize` |
| `response` | `status`、`statusText`、`httpVersion`、`cookies`、`headers`、`content`、`redirectURL`、`headersSize`、`bodySize` |
| `headers[]` | `{name, value}`，规范没有说列表是否保序 |
| `postData` | `mimeType`、`params`、`text`；"`text` and `params` fields are mutually exclusive." |
| `content` | `size`、`compression`、`mimeType`、`text`、`encoding`（1.2 新增） |
| `timings` | `blocked`、`dns`、`connect`、`send`、`wait`、`receive`、`ssl`（1.2 新增） |

要点（逐字或近乎逐字）：

- 不适用的耗时用 `-1`："Tools that can provide these timings can set their values to `-1` if they don't apply."
- `ssl` 包含在 `connect` 里："If the `ssl` field is defined then the time is also included in the connect field"。
- `entries[].time` 等于各项耗时之和（`-1` 的不计）。
- `headersSize`："Total number of bytes from the start of the HTTP request message until (and including) the double `CRLF` before the body"，不可得时 `-1`。注意这个值需要原始头字节，hyper 的路线拿不到精确值（1.3）。
- `bodySize`：请求是 POST 数据负载字节数，响应是"received response body"字节数，缓存的 304 为 0。
- `content.size`："Length of the returned content in bytes. Should be equal to `response.bodySize` if there is no compression"；`content.compression` 是节省的字节数（可为负）。
- 二进制：`content.text` 是"HTTP decoded text or a encoded (e.g. "base64") representation of the response body"，配套 `encoding` 字段；历史草稿说 "Ideally, the application should be able to unencode a base64 blob and get a byte-for-byte identical resource"。
- Chrome 等扩展的 `_` 开头自定义字段：mitmproxy 用 `_webSocketMessages`、`_resourceType`、`_error`（见 5.2）。

### 5.2 mitmproxy 怎么存（【W】）

- **原生流文件**：`FlowWriter.add()` 对每个 flow 调 `get_state()`，用 `tnetstring.dump()` 逐个顺序写入；读取时 `FlowReader` 逐个 `tnetstring.load()`，每个字典过 `compat.migrate_flow()` 做版本迁移，再 `Flow.from_state()`。`FlowReader` 若发现文件以 `{`（或 UTF-8 BOM 加 `{`）开头，就当 HAR 文件解析并转成 flow。顶层字段与版本号字段的具体内容未核实。选项 `save_stream_file`（"Stream flows to file as they arrive. Prefix path with + to append."）边抓边写。
- **HAR 导出**（`savehar.py`，选项 `hardump`，"Save a HAR file with all flows on exit."，路径以 `.zhar` 结尾则 zlib 压缩，`-` 表示输出到 stdout）：在 `response`、`error`、`websocket_end` 钩子里把匹配 `save_stream_filter` 的 HTTP flow 追加进内存，退出时一次写出。字段映射：
  - 请求：`headers` / `cookies` / `queryString` 来自多值字典转换；`headersSize` 是 `len(str(headers))`（不是线上字节数）；`bodySize` 是原始 body 长度；`postData` 只对 POST / PUT / PATCH；CONNECT 请求的 `url` 被改成 `https://…/`。
  - 响应：`content.size` 是原始（压缩后）body 长度，`compression` 是解码后长度减原始长度；`mimeType`、`redirectURL` 来自 `Content-Type`、`Location`；内容"大部分是二进制"（`is_mostly_bin`）则 `text` 为 base64 且 `encoding = "base64"`，否则用 `get_text(strict=False)`；解码抛 `ValueError` 就回用原始字节；无响应时 `status` 为 0，`_error` 放错误信息。
  - 耗时：只产出 `connect`、`ssl`、`send`、`wait`、`receive`；`connect` 与 `ssl` 对本次导出中已经出现过的服务器连接记为 `-1`；`time` 是非负耗时之和；`blocked`、`dns` 没有；`cache` 为空对象，`pages` 为空数组；`serverIPAddress` 取自服务端对端地址。
  - WebSocket flow：`_resourceType = "websocket"`，`_webSocketMessages` 每条含 `type`（`send` / `receive`）、`time`、`opcode`、`data`（文本帧为文本，二进制帧为 base64）。
- **Headers**：`fields` 暴露原始 `(name, value)` 字节元组；文档说"折叠"多值时按 RFC 7230 合并，`get_all` 返回未折叠的值。
- **每个流在内存里保留的**（mitmproxy `HTTPFlow`）：`request`、`response`、`error`、`websocket`、`client_conn`、`server_conn`、元数据（标记、注释）；`Message` 上有 `raw_content`（原样，可压缩，流式时为 None）与 `content`（按 `Content-Encoding` 解码，设置时会重新编码并更新 `Content-Length`，除非有 transfer-encoding）；`timestamp_start` 是"Headers received."，`timestamp_end` 是"Last byte received."。

### 5.3 proxyfor 的做法（【W】）

- 每个 flow 的结构 `Traffic`：`gid`（全局自增）、请求的 `uri`、`method`、`req_headers`、`req_body_file`、响应的 `status`、`http_version`、`res_headers`、`res_body_file`、`res_body_size`、`start_time`、`end_time`、`error`、`websocket_id`。头列表的 `size` 按"键长 + 值长 + 12，再加 7"估算；非 UTF-8 头值被写成空串。body 不进内存，边转发边写入 `temp_dir/proxyfor-<pid>` 下的文件；压缩过的响应原样落盘，文件名里记编码，导出时解压。
- 导出 HAR 时把 `timings` 的 `connect`、`ssl`、`send`、`receive`、`wait` 全部填 `-1`，`statusText` 恒为空，`time` 为结束减开始。没有任何大小或条数上限。

### 5.4 每个 flow 通常保存什么，上限怎么设

- 典型内容：请求行（方法、URL、版本）、有序头列表（含原始大小写）、请求与响应 body、状态与 reason、各阶段耗时、连接信息（客户端地址、服务端 IP、TLS 版本 / 套件 / SNI / ALPN）、错误、WebSocket 消息、用户标记与备注。
- 已见的上限做法：
  - mitmproxy 的视图层是无界 `OrderedDict`，只能手动清除（`view.py`，【W】）；单个 body 靠 `stream_large_bodies` 转为不保存，靠 `body_size_limit` 直接 kill；`store_streamed_bodies` 默认关。
  - Charles：在 Recording Settings 里可限制"最大记录大小"，到达上限后只是停止记录，不影响浏览器；内存过大会提示；该页没有单个 body 或会话条数上限（【W】，charlesproxy.com）。
  - proxyfor：body 落盘，无上限。
  - rama 的 MITM 例子在外层用 `BodyLimitLayer::symmetric(2 MiB)` 限制两个方向的 body（【W】）。
  - HTTP Toolkit、Proxyman、Fiddler 的上限做法：未见。
- 可选的预算机制（推断）：总字节预算 + 条数预算，满了淘汰最旧的流；单个 body 超过阈值只保留头部若干 KiB 并打"已截断"标记；超大 body 走旁路统计只记长度；落盘是另一种选择。以上是设计空间，不是现成结论。
- 规矩冲突（推断）：CLAUDE.md 规定凭据不进日志、错误文本、API 输出与 `Debug`，抓包里的 `Authorization`、`Cookie`、`Set-Cookie`、请求体里的口令按此规矩需要有脱敏策略，这会影响"捕获存储里存什么、API 回什么"。

---

## 6. 上游连接复用（MITM 之后）

### 6.1 hyper-util 的 `legacy::Client`（【源码】0.1.20）

- 池键是 `(Scheme, Authority)`（`client/legacy/client.rs:92`），不含 TLS 配置、出站策略、绑定网卡、代理链等；这些只能靠"每组配置一个 `Client`"或把信息塞进 URI 来区分（推断）。
- 默认值：`pool_idle_timeout` 90 秒（需要同时给 `pool_timer(TokioTimer::new())` 才真正生效，源码注释 "A `Timer` is required for this to take effect"）、`pool_max_idle_per_host` 为 `usize::MAX`、`retry_canceled_requests` 开、`set_host` 开（`client.rs:1034-1036`；`pool.rs`）。
- H1：每条连接独占取用，用完且空闲才放回池。H2：同一键只允许一个"正在连接"任务（`Pool::connecting` 对 `Ver::Http2` 去重，`pool.rs:173-199`），连接做成"共享预约"（`Reservation::Shared`），多请求复用同一连接。
- 取连接的方式是"池 checkout 与新建连接赛跑"：池里有空闲连接就直接用；否则开始建连，同时 checkout 继续等别的请求释放出空闲连接，先到先得，输掉的那条新连接会在后台建完并放回池里（`client.rs:403-449`）。
- `Ver::Auto` 下，H1 还是 H2 要等连接器建好连接、读到它回报的 ALPN 之后才定（`client.rs:519-541`）。ALPN 为 h2 时，只有一个并发建连能拿到该键的 H2 "connecting 锁"，其余已建好的连接被丢弃（"ALPN upgraded to HTTP/2"，`client.rs:523-535`）。因此在连接建立之前，你没法知道这条路最终是 H1 还是 H2（hudsucker #142 的根源）。H1 连接的后台任务一律带 `with_upgrades()`（`client.rs:585`），所以经 `legacy::Client` 也能做 `Upgrade` 透传。
- 请求被改写：补 `Host`（缺失时），URI 按连接类型转成 origin-form、absolute-form 或 authority-form（`client.rs:298-322`）。无关扩展（例如 `HeaderCaseMap`）原样通过。
- 池的实现是一把 `Mutex<PoolInner>`，含 `idle`、`waiters`、`connecting` 三张表，另有定时器周期清理空闲项。
- 没有提供按 `Connected` 以外的信息（例如策略名）分池的钩子；`Connected::extra`、`CaptureConnection` 只用于把连接信息带回响应。

### 6.2 hyper-util 0.1.19 起的可组合池（`client-pool` 特性，工作区未启用）

`client::pool` 提供 `cache`（空闲连接列表，丢弃时尝试放回）、`singleton`（只保一条连接，所有并发调用得到它的克隆，文档称"fits the HTTP/2 case well"）、`map`（按请求提取键并为每个键生成服务，示例键就是 `(scheme, authority)`）、`negotiate`（"original use case is support ALPN upgrades to HTTP/2, with a fallback to HTTP/1"）。模块文档里的示例都是 `rust,ignore`，类型多为"unnameable"（推断：API 还不成熟，工作区未启用该特性，且我没读过它的使用实例）。

### 6.3 手写池

- H1：`hyper::client::conn::http1::handshake(io)` 得到 `(SendRequest, Connection)`，`Connection` 要 spawn，升级场景用 `.with_upgrades()`；`SendRequest::ready().await` 等到空闲，`is_closed()` 判断是否还能用；一条连接同时只有一个在途请求，`SendRequest` 不可克隆。
- H2：`http2::handshake`，`SendRequest` 可克隆，可多个任务共享；`is_ready()`、`is_closed()`、`current_max_send_streams()`、`current_max_recv_streams()` 可以做容量与剔除判断（`client/conn/http2.rs:125-131,254-264`）。
- 好处：可以自己选 ALPN，从而提前知道 H1 还是 H2，并且池键可以包含出站策略、目标地址、SNI、TLS 校验模式、绑定网卡、代理链；坏处：空闲回收、健康检查、并发建连去重、H1 的 `Connection: close`、H2 的 GOAWAY 都要自己做。

### 6.4 各项目

- hudsucker：全局 `legacy::Client`（【W】）。proxyfor：每次请求新建 `Client`，等于没有池（【W】）。http-mitm-proxy：不内建，`DefaultClient` 实现未见。mitmproxy：每个客户端连接一个 `HttpLayer`，按"地址、tls、via、传输协议"一致复用，不跨客户端连接（【W】）。rama：`EasyHttpWebClient` 带池，MITM 例子关闭，issue #1224 披露池的 checkout 在 O(连接数) 的锁下执行（【W】）。

### 6.5 对 rurge 的相关观察（推断）

- 池键至少需要：出站策略或链、目标 host:port、协商出的 ALPN、TLS 配置（SNI、校验模式、是否用客户端证书）、本地绑定的网卡。CLAUDE.md 规定出站套接字只由 `rurge-net` 的 `new_socket` / `open_socket` 建，所以连接器必须是 rurge 自己的；`legacy::Client` 需要的是一个返回 `TokioIo<…>` 且实现 `Connection` 的 `tower::Service<Uri>`。
- 出站 socket 要设 `TCP_NODELAY`（rama #1224 的教训）。
- 池里的 H1 连接若上一个请求的 body 没读完或带 `Connection: close`，不能放回（hyper 已在 `conn.rs:615-626` 处理请求侧 close）。

---

## 候选 crate 表

版本栏分"本仓库锁定（Cargo.lock，【源码】）"与"crates.io 最新（【W】）"。许可证与 MSRV 栏凡本机有源码的取自 Cargo.toml（【源码】），其余为【W】。

| 名称 | 锁定版本 | crates.io 最新 | 许可证 | 用途 | 维护信号 |
|---|---|---|---|---|---|
| hyper | 1.11.1 | 1.12.0（2026-10-06） | MIT | H1 server/client conn、H2 封装、upgrade | 近期下载约 2.55 亿；约每月一发；MSRV 1.63 |
| hyper-util | 0.1.20 | 0.1.21（2026-09-24） | MIT | `auto::Builder`、`legacy::Client` 池、`TokioIo`、可组合池（特性 `client-pool`） | 近期下载约 1.89 亿；MSRV 1.64（锁定版）/ 1.85（最新） |
| h2 | 0.4.19 | 0.4.20（2026-10-06） | MIT | 直接控制 H2：1xx、流控、SETTINGS | 2026-08 连发 0.4.16 至 0.4.19，多为 DoS 与流控修复 |
| http | 1.5.0 | 未取 | MIT OR Apache-2.0 | 通用类型，`HeaderMap` 无序保证 | MSRV 1.57 |
| http-body-util | 0.1.5 | 0.1.5（2026-08-12） | MIT | `BodyExt::{frame, map_frame, inspect_frame, collect, boxed}`、`Limited`、`StreamBody` | 稳定 |
| httparse | 1.10.1 | 1.10.1（2025-03-03） | MIT OR Apache-2.0 | 自写 H1 编解码的底座（hyper 自己也用） | 近一年半无新版，稳定；无 MSRV 声明 |
| rustls | 0.23.43 | 0.23.46（2026-10-14），另有 0.24.0-dev.2 | Apache-2.0 OR ISC OR MIT | TLS，`server::Acceptor`、`ClientHello` | 活跃；MSRV 1.71 |
| tokio-rustls | 0.26.4 | 0.26.6（2026-09-28） | MIT OR Apache-2.0 | `LazyConfigAcceptor`、`TlsConnector` | 活跃；MSRV 1.71 |
| rcgen | 0.14.7 | 0.14.10（2026-08-28） | MIT OR Apache-2.0 | 签发叶证书（`CertificateParams::signed_by(&KeyPair, &Issuer)`） | 近期下载约 4750 万；默认特性 `crypto,pem,ring`；ring 后端不能生成 RSA 密钥（只有 aws-lc-rs 能），默认生成 ECDSA P-256；MSRV 1.71（锁定版）/ 1.88（最新，【W】） |
| x509-parser | 0.18.1 | 0.18.1（2026-02-05） | MIT OR Apache-2.0 | 解析上游证书的 SAN/CN（复制到叶证书） | 稳定；MSRV 1.67.1 |
| tokio-tungstenite / tungstenite | 0.28.0 | tokio-tungstenite 0.30.0（2026-07-11） | MIT / MIT OR Apache-2.0 | WebSocket 帧级解析。`WebSocketStream::from_raw_socket`、`from_partially_read` 可套在升级后的流上 | README 明说无 permessage-deflate（tungstenite 0.28.0 `README.md:75`） |
| hyper-tungstenite | 未锁定 | 0.30.0（2026-07-13） | BSD-2-Clause | 在 hyper 服务端完成 WS 握手（hudsucker、proxyfor 用） | 近期下载约 108 万 |
| fastwebsockets | 未锁定 | 0.10.0（API 摘要里最近发布为 0.8.1，2026-01-01，次序存疑） | Apache-2.0 | 另一个 WS 帧库 | 近期下载约 185 万 |
| moka | 未锁定 | 0.12.16（2026-08-09） | (MIT OR Apache-2.0) AND Apache-2.0 | 叶证书缓存（hudsucker、http-mitm-proxy、proxyfor 都用） | 近期下载约 4477 万；MSRV 1.71.1 |
| lru | 0.18.4 | 未取 | MIT | 已在锁里，可做简单容量缓存 | MSRV 1.85 |
| async-compression | 未锁定 | 0.4.51（2026-10-17） | MIT OR Apache-2.0 | gzip / br / zstd / deflate 的异步适配（proxyfor 用） | 近期下载约 6800 万 |
| flate2 | 1.1.10 | 未取 | MIT OR Apache-2.0 | gzip / deflate | 已在锁里 |
| brotli | 未锁定 | 9.1.1（2026-10-27） | BSD-3-Clause AND MIT | br 解压缩 | 近期下载约 8100 万 |
| p12-keystore | 未锁定 | 0.4.1（2026-10-13） | MIT/Apache-2.0 | 读 PKCS#12（Surge 的 `ca-p12` 是 base64 的 PKCS#12，推断；本次没有核对手册） | 近期下载约 234 万；MSRV 1.85 |
| pkcs12（RustCrypto） | 未锁定 | 0.2.0-pre.0（2026-01-12） | Apache-2.0 OR MIT | 同上的纯 Rust 备选 | 仍是 pre 版 |
| har | 未锁定 | 0.9.0（2026-03-22） | MIT | HAR 序列化类型 | 近期下载约 6.3 万；MSRV 1.88 |
| hudsucker | 未锁定 | 0.25.0（2026-07-15） | MIT OR Apache-2.0 | 整套 MITM 框架（参考或直接依赖） | 累计下载 54.9 万；MSRV 1.86.0；开放 issue 9 |
| rama | 未锁定 | 0.4.0（2026-08-19） | MIT OR Apache-2.0 | 整套框架（参考） | MSRV 1.96.0 高于项目 1.89；自述示例未到生产水平 |
| http-mitm-proxy | 未锁定 | 0.18.0（2026-01-24） | MIT | 轻量 MITM 库（参考） | 近期下载约 1.4 万 |
| proxyfor | 未锁定 | 0.5.0（2024-12-24） | MIT OR Apache-2.0 | 参考（抓包落盘与 HAR 导出） | 近期下载 8，实质停更 |
| pingora-http | 未锁定 | 0.9.0（2026-09-09） | Apache-2.0 | 参考：保留头大小写的头类型 | 累计下载约 1016 万 |
| mitmproxy（非 crate） | — | 12.3.1（2026-10-25，PyPI） | MIT | 参考：分层、旁路策略、流存储 | 约 4.65 万星 |

---

## 给设计者的开放问题

1. **H1 引擎选型。** hyper 的代价是：不保头顺序、读不到原始头名拼写与原始头字节（`headersSize` 只能估）、畸形头只能丢或 400、发不出 1xx、请求侧不收 obs-fold、非 UTF-8 URL 直接 400。要不要字节级保真？这取决于 Surge 兼容性是否要求头顺序、以及抓包是否要显示原始大小写。三条路：接受 hyper 的限制（最省力）；在 hyper 之上加自己的解析层；用 `httparse` 自写 H1（Pingora 路线，需自担请求走私风险，Cloudflare 2026 年的三个 CVE 是反例）。
2. **解析失败时是否回落为原样隧道。** Squid 的 `on_unsupported_protocol`、whistle 的 `enable://socket`、mitmproxy 的旁路都有这个机制，前提是有"字节回放"的基础设施（`Tee` / `Rewind`）。`force-http-engine-hosts` 与 `always-raw-tcp-hosts` 如何映射到这些机制？
3. **嗅探的字节数、方法集与超时。** 固定 4 字节 `GET ` 的做法不可取；服务器先说话的协议（SMTP、FTP、MySQL 等）在客户端不发数据时怎么办？可参照的数字：sing-box 300ms、Envoy 15s（超时默认关连接）。
4. **叶证书策略。**
   - 是否复制上游 SAN / CN（需要先连上游，用 `LazyConfigAcceptor` 暂停握手）；
   - IP 主机名要不要生成 IP 类型 SAN（hudsucker 只发 DNS 名）；
   - 每个叶证书独立密钥还是共用密钥（hudsucker 与 mitmproxy 都共用）；
   - 有效期（hudsucker 365 天，mitmproxy 199 天）与缓存键、容量；
   - 链里是否带 CA；
   - CA 从 PKCS#12 载入用哪个库。
5. **客户端拒绝伪造证书时怎么报、要不要自动旁路。** mitmproxy 的做法是 WARNING 日志加 `tls_failed_client` 钩子，示例插件一次失败后旁路。
6. **ALPN 策略。** 先连上游再决定客户端 ALPN（mitmproxy 默认 `eager`，代价是首包延迟与上游不可达时的行为），还是客户端侧固定 `h2` + `http/1.1` 再降级。后者要处理 `SETTINGS_ENABLE_CONNECT_PROTOCOL` 的镜像（rama #932）。
7. **H2 用 hyper 还是 h2 直接写。** hyper 省去流控与升级的样板，但没有 1xx、不能控制 `Date` 之外的响应头改写；h2 直接写可以转发 103 / 100，但流控与 GOAWAY 要自己做。H2 到 H1 的降级项（Host、Cookie 合并、URI 形式）与 H1 到 H2 的绝对 URI 要求要有明确的责任方。
8. **WebSocket。** 只做字节级透传（两侧 `upgrade::on` 加 `copy_bidirectional`），还是做消息级检查？消息级要用 tungstenite，而它不支持 permessage-deflate，意味着要剥掉 `Sec-WebSocket-Extensions` 或自己实现。H2 上的 WebSocket 是否宣告扩展 CONNECT？
9. **body 策略。** 何时流式、何时缓冲（mitmproxy 要求在 headers 钩子里决定，之后才能改写）；缓冲的上限；压缩编码的处理与 `Content-Length` 的修正；`fancy-regex` 只吃 `&str`，非 UTF-8 的 body 与头值如何处理；Map Local 与 reject 在上传途中短路会关连接。
10. **`Expect: 100-continue`。** hyper 会替代理先发 `100 Continue`，要不要接受这个语义偏差？
11. **抓包存储。** 每流保存什么、内存与条数预算、单个 body 的截断阈值、是否落盘、HAR 导出的字段映射（`-1` 的约定、`headersSize` 精度）、WebSocket 消息的存放。与"凭据不进日志与 API 输出"的规矩如何协调。
12. **上游池。** 键怎么定（策略或链、host:port、ALPN 结果、TLS 配置、绑定网卡）；用 `legacy::Client` 加"每组配置一个 Client"，还是手写；idle 回收需要 timer；H2 共享连接与 GOAWAY；`TCP_NODELAY`。
13. **指纹与 ECH。** MITM 会改变站点看到的 TLS 与 H2 指纹，hudsucker #144 的现象可能相关（未证实）；浏览器启用 ECH 时 SNI 只剩外层公共名（推断，未核实），需要与 DNS 的 HTTPS 记录处理一起考虑。
14. **依赖与 MSRV。** rama 要求 1.96，不能直接依赖；hyper、h2 的升级节奏很快（h2 在 2026-08 一个月内发了 4 个版本），Cargo.lock 版本与本文阅读的源码一致，升级前需重核头相关行为。
15. **安全面。** 请求走私（Content-Length 与 Transfer-Encoding 的取舍，hyper 1.12.0 又改了）、H2 的 Rapid Reset 与小帧 DoS 参数（`max_concurrent_streams`、`max_local_error_reset_streams`、`data_frame_budget`）、日志脱敏。
