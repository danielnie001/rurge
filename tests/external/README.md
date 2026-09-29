# rurge-external-tests

`rurge-external-tests` 是仅供测试使用的工作区成员（`publish = false`），不对外发布，也不是任何其它 crate 的依赖。它带一个极小的 SOCKS5 服务端 `socks-helper`（`src/bin/socks-helper.rs`，只监听 127.0.0.1、不认证、只做 CONNECT），用例把它当作 `external` 策略的外部程序真实拉起：按需拉起与转发、参数顺序与代理变量、程序退出后的再拉起与 2 秒间隔、连接被拒时的重试、日志、停止时连同它启动的子进程一起结束（Unix 进程组、Windows Job Object）、经引擎的端到端、检查与构建不拉起、重载时沿用或替换程序。

辅助程序只存在于测试构建里：`cargo build -p rurge` 与发行的二进制不含它。用例不访问公网，不改本机的网络与代理设置。

```bash
cargo test -p rurge-external-tests
```
