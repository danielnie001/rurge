# 测试用 GeoIP 数据库

- `GeoIP2-Country-Test.mmdb`、`GeoLite2-ASN-Test.mmdb` 来自 <https://github.com/maxmind/MaxMind-DB>（`test-data/`），许可 Apache-2.0 / MIT，仅用于测试。
- 已知条目（来自仓库 `source-data/*.json`）：`2001:218::/32` → JP；`2001:220::1/128` → KR；`1.0.0.0/24` → AS15169；`1.128.0.0/11` → AS1221。`GeoDb::describe` 与 `GET /v1/geoip` 的用例还依赖这几个地址的回答（国家库与 ASN 库各自给的，用例断言的就是这些）：`89.160.20.112` → 国家库 SE、ASN 库 AS29518（组织名 `Bredband2 AB`）；`1.0.0.1`（在 `1.0.0.0/24` 里）的组织名是 `Google Inc.`，国家库里没有它；`2001:218::1`（在 `2001:218::/32` 里）→ JP，ASN 库里没有；`127.0.0.1` 两个库里都没有。
- 若上游更新导致条目变化，按 `source-data` 中的值调整 `geoip.rs` 的测试。
