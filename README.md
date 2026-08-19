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

使用 Homebrew：

```sh
brew install JunieXD/tap/flowwatch
flowwatch install
```

或者使用带 SHA-256 校验的安装脚本：

```sh
curl -fsSL https://raw.githubusercontent.com/JunieXD/FlowWatch/main/scripts/install.sh | sh
```

两种方式都会自动选择适合 Apple Silicon 或 Intel 的版本。Homebrew 安装后需要运行一次 `flowwatch install` 来启动登录自启服务；脚本会直接完成这一步。若 `~/.local/bin` 不在 `PATH` 中，安装器会显示需要添加的命令。

### 确认服务状态

```sh
flowwatch doctor
flowwatch status
```

### 查看流量较高的应用

```sh
flowwatch apps --period 今天
flowwatch apps --date 2026-08-18
flowwatch apps --period 24h --sort download --limit 50
flowwatch apps --from "2026-08-18 09:00" --to "2026-08-18 18:00"
flowwatch explain --at "2026-08-18 18:37"
flowwatch gaps --period 24h --limit 20
```

`--period` 同时支持 `today`、`yesterday`、`all` 和中文别名 `今天`、`昨天`、`全部`。`--date YYYY-MM-DD` 用于查询某个自然日。自定义范围时，`--from` 和 `--to` 必须同时提供；开始时间包含在内，结束时间不包含。

应用排行会同时显示所选范围的实际流量、找到对应应用的流量、未找到对应应用的流量和应用识别率。这样即使应用识别不完整，也不会把排行榜误解为整台 Mac 的全部流量。需要供脚本处理时可增加 `--json`；输出包含 `range`、`summary` 和 `apps`，字段名保持英文。

查看单个应用的详情或趋势：

```sh
flowwatch app "ChatGPT" --period 7d
flowwatch chart --app "ChatGPT" --period 24h
flowwatch apps --period 24h --details
```

应用可以使用显示名称或完整应用 ID。名称匹配到多个程序时，FlowWatch 会列出候选项并要求使用完整 ID，不会猜测用户想查看哪一个。应用趋势只包含已经找到对应应用的流量，因此可能低于该应用的实际使用量。

可以为不易理解的进程设置自己的名称。先用 `--details` 查看应用 ID，再设置名称：

```sh
flowwatch apps --period 24h --details
flowwatch config app-names set "group:chrome-headless-shell:chrome-headless-shell" "自动化浏览器"
flowwatch config app-names list
flowwatch config app-names remove "group:chrome-headless-shell:chrome-headless-shell"
```

自定义名称会用于历史查询、趋势和提醒，但不会改写原始数据。详情仍会显示原始名称、底层身份和可执行路径。路径型程序使用稳定的 `group:` ID，因此升级后路径发生变化时通常不需要重新设置。

生成一份不需要组合多个查询命令的流量报告：

```sh
flowwatch report --period 24h
flowwatch report --period 7d --compare
flowwatch report --date 2026-08-18
```

报告包含实际总量、应用识别完整度、主要应用、实际流量最高时段、未找到应用最多的时段和数据质量说明。`--compare` 会与紧邻的上一段等长时间比较；`--json` 可输出相同内容的结构化结果。

问题正在发生时，可以临时提高采样精度：

```sh
flowwatch investigate start --duration 30m
flowwatch investigate status
flowwatch investigate stop
```

调查模式使用每秒采样和每分钟应用明细，允许时长为 5 分钟到 24 小时。原设置不会被覆盖；到期、手动停止、程序重启或 Mac 重启后都会恢复。高频采样会临时增加 CPU 使用，只应在排查问题时开启。

### 设置流量提醒

提醒由后台采集器在每次保存数据后检查，不会额外提高采样频率。达到限额的 80% 时预警，达到 100% 时再提醒一次；每天和每月会分别重新计算。

```sh
flowwatch alerts add --daily 10GiB
flowwatch alerts add --monthly 100GiB
flowwatch alerts add --app "ChatGPT" --daily 2GiB
flowwatch alerts list
flowwatch alerts disable 1
flowwatch alerts enable 1
flowwatch alerts remove 1
flowwatch alerts test
```

容量可写成 `B`、`KiB`、`MiB`、`GiB`、`TiB`，也支持十进制的 `KB`、`MB`、`GB`、`TB`。应用限额只统计已经识别到该应用的流量，FlowWatch 会在通知中明确提示这项限制。

### 查看流量趋势

```sh
flowwatch chart --period 6h
flowwatch chart --period 24h
flowwatch chart --date 2026-08-18
flowwatch chart --from "2026-08-18 09:00" --to "2026-08-18 18:00"
```

趋势图以网卡实际流量为准，纵轴表示每个时间段内的用量，横轴表示时间。上传、下载和合计分别使用不同颜色与符号，并在图下显示区间合计、最高流量时段和有效数据段数量。默认会根据时间跨度与终端宽度自动选择间隔；可使用 `--interval 15m`、`--height 16` 或 `--width 120` 调整，使用 `--no-color` 可生成适合保存到文本文件的输出。

空白位置表示该时间段没有采集记录，不会被当作零流量连接起来。超过明细保留期限的历史数据会使用每日汇总，因此会自动切换为按天显示。

### 交互式 Dashboard

不想组合命令时，可以在一个终端界面中查看概览、趋势、应用排行和异常时段：

```sh
flowwatch dashboard --period 24h
flowwatch dashboard --date 2026-08-18
```

`Tab` 或左右方向键切换视图，上下方向键选择应用或时段，`Enter` 打开详情，`Esc` 关闭详情，`r` 刷新，`q` 退出。进入 Dashboard 后，底部也会持续显示这些操作提示。界面每 5 秒自动读取一次最新数据，只在主动运行时占用资源。终端过小时会显示所需尺寸；重定向输出等非交互环境会拒绝启动并给出等价的普通查询命令。设置 `NO_COLOR=1` 可停用颜色。

不查阅 README 也可以从终端逐步了解全部功能：运行 `flowwatch --help` 查看快速开始，运行 `flowwatch <命令> --help` 查看该命令的说明、规则和示例，也可以使用 `flowwatch help <命令>`。

状态输出示例：

```text
今天（2026-08-19 00:00 至 18:00）
  实际流量：上传 888.0 MiB  下载 1.9 GiB  合计 2.7 GiB
  已识别应用：上传 782.4 MiB  下载 1.3 GiB  合计 2.1 GiB（识别率 77.7%）
  未找到应用：上传 105.6 MiB  下载 614.4 MiB  合计 720.0 MiB
  Clash：总量上传 834.6 MiB  下载 1.5 GiB；已找到应用上传 726.4 MiB  下载 1.2 GiB（识别率 86.8%）
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
| `flowwatch chart` | 在终端绘制上传、下载和合计流量趋势图。 |
| `flowwatch dashboard` | 打开概览、趋势、应用和异常时段的交互界面。 |
| `flowwatch explain` | 分析指定或最高流量时段的主要应用和未识别流量。 |
| `flowwatch report` | 一次生成总量、主要应用、高峰和数据说明。 |
| `flowwatch investigate` | 临时提高采样精度，并按时自动恢复。 |
| `flowwatch alerts` | 设置、查看、暂停和测试流量限额提醒。 |
| `flowwatch apps` | 按上传、下载或总量查看应用排行。 |
| `flowwatch app` | 查看单个应用的流量、身份、路径和最高时段。 |
| `flowwatch interfaces` | 查看各物理网卡的实际流量。 |
| `flowwatch spikes` | 查看流量最高的分钟。 |
| `flowwatch gaps` | 查看未识别流量较高的时间段。 |
| `flowwatch doctor` | 检查数据库、采集服务、权限和登录自启状态。 |
| `flowwatch config` | 管理应用名称、Clash 设置和应用明细精度。 |
| `flowwatch data` | 查看数据库状态，导出、清理和压缩本机数据。 |
| `flowwatch update` | 检查并安装经过 SHA-256 校验的正式版本。 |
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

查看数据库大小、记录范围和保存期限：

```sh
flowwatch data info
```

导出 CSV 或 JSON：

```sh
flowwatch data export --period 30d --format csv --output flowwatch.csv
flowwatch data export --date 2026-08-18 --format json --output flowwatch.json
```

导出内容包括网卡实际流量时间序列、各网卡汇总、应用分来源用量和必要的时间字段，不会包含 FlowWatch 本来就不保存的远端信息。输出使用同目录临时文件完成后再原子写入，并拒绝覆盖已有文件。

修改保存期限或清理旧数据：

```sh
flowwatch data retention --details 30d --daily 365d
flowwatch data prune --before 2026-01-01 --confirm
flowwatch data compact
```

`retention` 会立即汇总和应用新期限。`prune` 只有明确增加 `--confirm` 才会永久删除指定日期以前的记录；`compact` 可在删除后回收 SQLite 文件空间。

导入 Clash 后，控制器密钥按当前设计以明文保存在 SQLite 中。数据目录权限为 `0700`，数据库文件权限为 `0600`，普通命令输出会隐藏密钥。仅接受本机 HTTP 控制器。

## 升级与卸载

可以直接检查和安装 GitHub 上的最新正式版本：

```sh
flowwatch update --check
flowwatch update
flowwatch update --version 0.2.1
```

更新器只接受 `MAJOR.MINOR.PATCH` 正式版本，自动选择 Apple Silicon 或 Intel 发布包，同时校验发布清单中的 SHA-256 和下载后程序报告的版本。校验全部通过后才由新程序执行安装。自动更新不会降级，也不会修改 SQLite 数据库、Clash 设置、提醒、应用名称或保存期限。

也可以再次执行安装脚本升级：

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
