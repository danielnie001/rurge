# 测试用 GeoIP 数据库

- `GeoIP2-Country-Test.mmdb`、`GeoLite2-ASN-Test.mmdb` 来自 <https://github.com/maxmind/MaxMind-DB>（`test-data/`），许可 Apache-2.0 / MIT，仅用于测试。
- 已知条目（来自仓库 `source-data/*.json`）：`2001:218::/32` → JP；`2001:220::1/128` → KR；`1.0.0.0/24` → AS15169；`1.128.0.0/11` → AS1221。
- 若上游更新导致条目变化，按 `source-data` 中的值调整 `geoip.rs` 的测试。
