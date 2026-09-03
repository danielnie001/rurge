# 兼容性语料库

`valid/` 中的配置必须能被 rurge 加载且没有错误级诊断；每个文件对应一份快照（`crates/rurge-config/tests/snapshots/`），记录解析结果摘要与全部诊断。
`invalid/` 中的配置故意包含错误，`<name>.expect` 列出必须出现的诊断代码（每行一个）。

来源与许可：
- `quick-start.conf`、`format-examples.conf`、`requirement.conf`、`legacy-keys.conf`：改写自 Surge 官方手册中的示例（<https://manual.nssurge.com/>），仅保留语法结构。
- `kitchen-sink.conf`、`include-*`：本项目自写，覆盖全部节与规则类型，服务器地址均为示例地址。

添加真实配置前请脱敏：删除密码、PSK、UUID、私钥、订阅地址与内网地址。
