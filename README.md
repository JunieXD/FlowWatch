<div align="center">

# FlowWatch

**轻量、注重隐私的 macOS 应用流量统计工具。**

[![发布版本](https://img.shields.io/github/v/release/JunieXD/FlowWatch?display_name=tag&style=flat-square)](https://github.com/JunieXD/FlowWatch/releases/latest)
[![持续检查](https://img.shields.io/github/actions/workflow/status/JunieXD/FlowWatch/ci.yml?branch=main&label=%E6%8C%81%E7%BB%AD%E6%A3%80%E6%9F%A5&style=flat-square)](https://github.com/JunieXD/FlowWatch/actions/workflows/ci.yml)
![平台](https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-macOS%2013%2B-2f6f5e?style=flat-square)
[![许可证](https://img.shields.io/github/license/JunieXD/FlowWatch?style=flat-square)](LICENSE)

[快速开始](#快速开始) · [统计方式](#统计方式) · [准确性边界](#准确性边界) · [隐私与存储](#隐私与存储)

</div>

FlowWatch 在本机记录 Mac 的网络用量：网卡实际上传、下载多少，各应用已识别出多少，以及仍无法对应到应用的流量。它不抓取数据包，不需要管理员权限，也不上传任何统计数据。

当前版本支持 Apple Silicon 和 Intel Mac，并以当前用户的登录自启服务持续采集。安装、使用和开机自启均不需要 `sudo`、Apple Developer 账号、系统扩展或内核扩展。

## 快速开始

### 安装

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

安装器会自动选择适合 Apple Silicon 或 Intel 的版本，校验 SHA-256，安装命令并启动登录自启服务。若 `~/.local/bin` 不在 `PATH` 中，安装器会显示需要添加的命令。

### 确认服务状态

```sh
flowwatch doctor
flowwatch status
```

### 查看流量较高的应用

```sh
flowwatch apps --period 今天
flowwatch apps --period 24h --sort download --limit 50
flowwatch apps --from "2026-08-18 09:00" --to "2026-08-18 18:00"
flowwatch gaps --period 24h --limit 20
```

`--period` 同时支持 `today`、`yesterday`、`all` 和中文别名 `今天`、`昨天`、`全部`。自定义范围时，`--from` 和 `--to` 必须同时提供；开始时间包含在内，结束时间不包含。需要供脚本处理时，查询命令可增加 `--json`；JSON 字段名保持英文。

不查阅 README 也可以从终端逐步了解全部功能：运行 `flowwatch --help` 查看快速开始，运行 `flowwatch <命令> --help` 查看该命令的说明、规则和示例，也可以使用 `flowwatch help <命令>`。

状态输出示例：

```text
今天
  实际总量：上传 888.0 MiB  下载 1.9 GiB
  已识别应用：上传 782.4 MiB  下载 1.3 GiB（识别率 77.7%）
  Clash 流量：
    总量：上传 834.6 MiB  下载 1.5 GiB
    已识别应用：上传 726.4 MiB  下载 1.2 GiB
    未识别：上传 108.2 MiB  下载 307.2 MiB
```

## 统计方式

| 内容 | 数据来源 | 用途 |
| --- | --- | --- |
| 实际总量 | macOS 原生物理网卡计数器 | 确认这台 Mac 实际经过了多少上传、下载流量。 |
| 直连应用流量 | 定时读取结构化 `nettop` 数据 | 识别未经过代理的应用连接。未开系统代理时，这部分仍会正常工作。 |
| Clash 应用流量 | 可选的 Clash/Mihomo 控制器和本机连接匹配 | 在使用代理时尽可能识别是哪一个应用发起了流量。 |
| 未识别流量 | 实际总量与已识别应用流量的比较 | 找出短连接、内部连接或缺少进程信息的流量较多的时段。 |

实际总量和应用记录刻意分开保存。协议开销、短连接、睡眠、采样时间点和进程信息缺失，都可能让应用合计与网卡总量不同。FlowWatch 会保留并显示这个差额，不会把不确定的流量猜测分配给某个应用。

### 可选接入 Clash/Mihomo

确认 Clash Verge 或 Mihomo 的外部控制器监听在本机地址后，导入其 `config.yaml`：

```sh
flowwatch config import-clash \
  "$HOME/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/config.yaml"
flowwatch config show
```

FlowWatch 只读取控制器地址和密钥，不会修改代理设置。设置 `find-process-mode: strict` 通常能让 Mihomo 提供更多应用信息；若控制器未提供进程信息，FlowWatch 会短暂地按本机回环连接尝试匹配。

Clash 并非必需。不开系统代理、不使用 Clash 时，标准模式仍会统计网卡总量和直连应用流量。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `flowwatch status` | 查看采集服务、实际总量、已识别应用和 Clash 状态。 |
| `flowwatch apps` | 按上传、下载或总量查看应用排行。 |
| `flowwatch interfaces` | 查看各物理网卡的实际流量。 |
| `flowwatch spikes` | 查看流量最高的分钟。 |
| `flowwatch gaps` | 查看未识别流量较高的时间段。 |
| `flowwatch doctor` | 检查数据库、采集服务、权限和登录自启状态。 |
| `flowwatch config` | 管理 Clash 设置和应用明细精度。 |
| `flowwatch install` | 安装或更新当前用户的登录自启服务。 |
| `flowwatch uninstall` | 删除服务和程序，默认保留历史数据。 |

默认每五分钟保存一次应用明细，以减少资源和磁盘占用。排查某段异常流量时，可以暂时改为每分钟：

```sh
flowwatch config set-app-granularity 1m
flowwatch config set-app-granularity 5m
```

## 准确性边界

- 标准模式通过定时采样识别应用，不是逐个数据包的进程统计。
- 完全发生在两次采样之间的短连接，可能只会计入实际总量，无法显示在应用排行中。
- 每个连接第一次出现时只建立基线，以免把它此前累积的数据误记为新流量。
- Clash 的 TUN、内部连接、未知应用和控制器两次读取之间已结束的连接，会保留为未识别流量。
- 网卡计数器可跨采集服务重启补齐总量；睡眠、网卡重置和网络切换仍会形成统计边界。
- 完整的五分钟应用记录会与网卡实际总量对照。若出现明显超量，FlowWatch 会提示数据警告，不会静默缩放数字。

使用 `flowwatch gaps` 可以定位未识别流量集中的具体时段。更详细的实现约束见[架构说明](docs/architecture.md)。

## 隐私与存储

数据库默认位置：

```text
~/Library/Application Support/io.github.FlowWatch.FlowWatch/traffic.sqlite3
```

默认保留 30 天应用和分钟明细、365 天每日汇总。FlowWatch 不持久化原始数据包、包内容、远端域名、远端 IP、本地端口、原始 `nettop` 输出或原始 Clash 响应；本机连接和应用的短期匹配只保存在内存中。项目没有遥测、账号或云端服务。

导入 Clash 后，控制器密钥按当前设计以明文保存在 SQLite 中。数据目录权限为 `0700`，数据库文件权限为 `0600`，普通命令输出会隐藏密钥。仅接受本机 HTTP 控制器。

## 升级与卸载

再次执行安装命令即可升级。新版会替换程序并重启登录自启服务，同时保留 SQLite 数据库、Clash 设置、保存期限和应用明细精度。

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

只删除服务和命令、保留历史数据：

```sh
flowwatch uninstall
```

只有增加 `--purge-data` 才会删除流量数据库。

## 开发与贡献

开发环境需要 macOS 13 或更高版本、Xcode Command Line Tools 和 Rust 1.88 或更高版本。

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
```

贡献方式、隐私与安全说明、发布流程见[贡献指南](CONTRIBUTING.md)、[安全说明](SECURITY.md)和[发布流程](docs/releasing.md)。

## 许可证

[MIT](LICENSE) © JunieXD 与 FlowWatch 贡献者。
