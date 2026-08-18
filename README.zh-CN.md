<div align="center">

# FlowWatch

**不用抓包和管理员权限，也能看清 Mac 的流量去了哪里。**

[![Release](https://img.shields.io/github/v/release/JunieXD/FlowWatch?display_name=tag&style=flat-square)](https://github.com/JunieXD/FlowWatch/releases/latest)
[![CI](https://img.shields.io/github/actions/workflow/status/JunieXD/FlowWatch/ci.yml?branch=main&label=CI&style=flat-square)](https://github.com/JunieXD/FlowWatch/actions/workflows/ci.yml)
![Platform](https://img.shields.io/badge/platform-macOS%2013%2B-2f6f5e?style=flat-square)
[![License](https://img.shields.io/github/license/JunieXD/FlowWatch?style=flat-square)](LICENSE)

[English](README.md) · [快速开始](#快速开始) · [统计原理](#统计原理) · [准确性边界](#准确性边界)

</div>

FlowWatch 是一个轻量、本地优先的 macOS 流量统计工具。它用物理网卡计数器维护真实总量，尽可能把流量归因到应用，并把无法解释的余量明确展示出来，而不是把每个字节强行猜给某个进程。

当前 `0.1.0` 是无需签名的 CLI 和用户级 LaunchAgent，支持 Apple Silicon 与 Intel Mac。安装和开机自启不需要 `sudo`、抓包、系统扩展或 Apple Developer 账号。

## 为什么使用 FlowWatch

- **真实总账：** macOS 原生 64 位网卡计数器回答这台 Mac 实际上传、下载了多少。
- **应用归因：** 周期性的结构化 `nettop` 快照识别直连应用，不解析面向人类的显示文本。
- **代理感知：** 可选接入 Clash/Mihomo，在排除代理载体重复计数的同时恢复代理后的应用身份。
- **诚实的覆盖率：** 物理总量、应用归因、observed actor、内部流量和归因缺口分开统计。
- **占用很小：** 只保存 SQLite 聚合数据，不保存包内容、域名或远端 IP；当前实测常驻内存约 10 MiB。
- **方便追查：** 提供应用排行、网卡总量、流量尖峰、缺口排行、精确时间范围和 JSON 输出。

## 快速开始

### 1. 安装

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

安装器会自动选择 Apple Silicon 或 Intel 版本，校验 SHA-256，把命令安装到 `~/.local/bin`，然后启动用户级 LaunchAgent。如果该目录不在 `PATH` 中，安装器会输出需要添加的准确命令。

### 2. 检查采集器

```sh
flowwatch doctor
flowwatch status
```

### 3. 查看哪些应用使用了流量

```sh
flowwatch apps --period today
flowwatch apps --period 24h --sort download --limit 50
flowwatch gaps --period 24h --limit 20
```

健康的状态输出会把不同统计口径拆开：

```text
Today
  Physical:   up 888.0 MiB  down 1.9 GiB
  Attributed: up 782.4 MiB  down 1.3 GiB  (77.7% coverage)
  Clash:
    Total:        up 834.6 MiB  down 1.5 GiB
    Attributed:   up 726.4 MiB  down 1.2 GiB
    Classification:
      Observed actor:       up 2.9 MiB  down 33.3 MiB
      App-attributed actor: up 2.9 MiB  down 33.3 MiB  (100.0% coverage)
      Non-actor/unobserved: up 28.1 KiB  down 2.9 MiB
```

## 统计原理

| 账本 | 数据来源 | 回答的问题 |
| --- | --- | --- |
| 物理流量 | macOS 原生网卡计数器 | 硬件 `enN` 网卡实际经过了多少字节？ |
| 直连应用 | 周期性的结构化 `nettop` 快照 | 哪些应用直接使用了物理连接？ |
| 代理应用 | 可选 Clash/Mihomo controller 与本地 socket 匹配 | 哪些应用产生了由代理承载的流量？ |
| 覆盖率与缺口 | 独立账本之间的比较 | 有多少流量成功归因，哪些时段归因不足？ |

物理账本与应用账本有意保持独立。协议开销、短连接、进程信息缺失、睡眠和采样边界都会使应用合计不等于物理总量。FlowWatch 会显示这个差值，不会偷偷缩放或分摊。

数据库约束、去重规则与未来平台后端设计见[架构文档](docs/architecture.md)。

## 可选的 Clash/Mihomo 集成

确认 Clash Verge 或 Mihomo controller 监听本机地址后导入配置：

```sh
flowwatch config import-clash \
  "$HOME/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/config.yaml"
flowwatch config show
```

FlowWatch 只读取 controller 地址和密钥，不会修改代理配置。`find-process-mode: strict` 通常能改善 controller 返回的进程身份；字段缺失时，FlowWatch 还会短暂地用 loopback 源端口匹配本地应用。

按照当前设计，密钥以明文存进 SQLite。数据目录权限为 `0700`，数据库文件为 `0600`，普通命令输出会脱敏，而且只允许本机 HTTP controller。

## 常用命令

| 命令 | 用途 |
| --- | --- |
| `flowwatch status` | 采集健康、总量、覆盖率与当前 Clash 分类 |
| `flowwatch apps` | 按上传、下载或总量排列应用 |
| `flowwatch interfaces` | 查询权威的物理网卡流量 |
| `flowwatch spikes` | 查看流量最高的物理分钟 |
| `flowwatch gaps` | 按未归因流量排列时间桶 |
| `flowwatch doctor` | 检查数据库、采集器、权限与 LaunchAgent |
| `flowwatch config` | 管理 Clash 和应用明细粒度 |
| `flowwatch install` | 安装或刷新当前用户服务 |
| `flowwatch uninstall` | 删除服务，默认保留历史数据 |

查询支持 `today`、`yesterday`、`24h`、`7d`、`30d`、`all`、其他正整数 `h`/`d` 时间段，也支持明确的 `--from` 和 `--to`。查询命令均支持 `--json`。

默认使用更轻量的五分钟应用明细。排查流量尖峰时可以临时切换到一分钟：

```sh
flowwatch config set-app-granularity 1m
flowwatch config set-app-granularity 5m
```

## 存储与隐私

数据库位置：

```text
~/Library/Application Support/io.github.FlowWatch.FlowWatch/traffic.sqlite3
```

默认保留 30 天应用/分钟聚合和 365 天日汇总。不会持久化原始数据包、内容、远端域名、远端 IP、本地端口、原始 `nettop` 输出或原始 controller 响应；socket 所有者映射只在内存中保留约 15 秒。FlowWatch 没有遥测或云端服务。

## 准确性边界

- 普通模式是尽力而为的应用归因，不是逐包级进程统计。
- 完全发生在两次快照之间的连接可能进入物理账本，却没有应用身份。
- socket 第一次出现时只建立基线；直接记录已有累计值会制造虚假尖峰。
- Clash TUN/INNER、非 actor、未知应用和 controller 采样缺口会保留为未归因。
- 物理计数器可以跨采集器重启补总量，但网卡重置、睡眠或网络切换仍会形成边界。
- 系统会检查已完成的直连归因桶；异常超量会被报告，而不会被静默缩放。

使用 `flowwatch gaps` 可以找出归因最弱的具体时段。完整边界见[架构文档](docs/architecture.md)。

## 升级与卸载

重复执行安装命令即可升级。校验过的新版会替换程序并重启 LaunchAgent，同时保留 SQLite 数据库、Clash 配置、保留期和应用明细粒度。

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

只删除服务和命令、保留历史：

```sh
flowwatch uninstall
```

只有明确增加 `--purge-data` 才会同时删除流量数据库。

## 开发

开发环境需要 macOS 13 或更高版本、Xcode Command Line Tools，以及 Rust 1.88 或更高版本。

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
cargo build --locked --release --workspace
```

修改统计逻辑时应增加回归测试，并说明影响物理、直连应用、Clash、存储还是安装账本。更多信息见[贡献指南](CONTRIBUTING.md)、[安全说明](SECURITY.md)和[发布流程](docs/releasing.md)。

## 许可证

[MIT](LICENSE) © JunieXD 与 FlowWatch contributors。
