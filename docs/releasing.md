# 发布流程

FlowWatch 通过 GitHub Actions 根据版本标签构建发布包。流程会发布 Apple Silicon 与 Intel 两种归档文件，以及 `SHA256SUMS` 校验文件。`0.1.x` 是未签名的 CLI 和当前用户 LaunchAgent，因此不需要 Apple Developer 证书。

## 发布准备

1. 更新 `Cargo.toml` 中 `[workspace.package].version`；必要时更新 `Cargo.lock`。
2. 新增 `docs/releases/v<版本>.md`，首行必须为 `# FlowWatch v<版本>`。
3. 命令、存储、兼容性或安全行为变化时，更新默认中文 README 和相关中文文档。
4. 运行本地发布检查：

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
scripts/check-release.sh v0.1.4 target/release/flowwatch
scripts/test-installer.sh
```

5. 提交发布准备工作，并等待 `main` 上的持续检查通过。

## 正式发布

创建与 Cargo 版本完全一致的带说明标签并推送：

```sh
git tag -a v0.1.4 -m "FlowWatch v0.1.4"
git push origin v0.1.4
```

发布工作流随后会：

1. 校验标签、Cargo 版本、二进制版本和发布说明；
2. 使用最低支持的 Rust 版本构建 `aarch64-apple-darwin` 与 `x86_64-apple-darwin`；
3. 将两个二进制、中文 README 和 MIT 许可证打包；
4. 生成 SHA-256 校验和；
5. 使用 `docs/releases/<标签>.md` 创建 GitHub Release。

公开 tap 位于 `JunieXD/homebrew-tap`。它的定时工作流每 6 小时读取最新正式 Release 和 `SHA256SUMS`，自动更新两个架构的 Formula 并运行 style、strict audit、安装与测试。发布后可立即手动触发，避免等待下一次定时任务：

```sh
gh workflow run update-flowwatch.yml --repo JunieXD/homebrew-tap
```

确认工作流通过后，实际运行：

```sh
brew update
brew install JunieXD/tap/flowwatch
brew test JunieXD/tap/flowwatch
```

使用 `workflow_dispatch` 可测试两种构建任务而不创建 GitHub Release。不要替换已经发布的标签；需要修复时发布新的补丁版本。
