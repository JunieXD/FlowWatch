# 贡献指南

FlowWatch 仍处于早期阶段。相比让更多流量看起来“已识别”，项目更重视统计正确性和清楚说明不确定性。

报告问题时，请提供 macOS 版本、Mac 架构、FlowWatch 版本、`flowwatch doctor` 输出，以及流量是否为直连、代理或 TUN 模式。请勿附上 Clash 密钥、完整配置、生产数据库、原始控制器响应、私人 IP、域名或其他敏感数据。

较大的架构调整请先创建 Issue 讨论。操作系统接口应留在各平台后端 crate 中；共享模型和 SQLite 代码要能支持未来的 Windows 后端。

提交 Pull Request 前请运行：

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
scripts/test-installer.sh
```

涉及流量统计的改动必须包含回归测试，并说明影响的是网卡总量、直连应用、Clash、存储还是安装流程。不要把未识别流量猜测分配给某个应用，也不要静默缩放异常数据来提高识别率。

请保持提交范围明确、说明简洁。当命令、兼容性、隐私、权限、保存期限或统计行为变化时，请同步更新中文文档。维护者遵循[发布流程](docs/releasing.md)。
