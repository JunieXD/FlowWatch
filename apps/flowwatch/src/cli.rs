use clap::error::ErrorKind;
use clap::{
    ArgAction, Args, Command as ClapCommand, CommandFactory, FromArgMatches, Parser, Subcommand,
    ValueEnum,
};
use std::path::PathBuf;

const HELP_TEMPLATE: &str =
    "{before-help}{name} {version}\n{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";
const SUBCOMMAND_HELP_TEMPLATE: &str =
    "{before-help}{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";

#[derive(Debug, Parser)]
#[command(
    name = "flowwatch",
    version,
    about = "轻量、透明的 macOS 应用流量统计工具",
    help_template = HELP_TEMPLATE,
    disable_help_flag = true,
    disable_help_subcommand = true,
    disable_version_flag = true,
    subcommand_help_heading = "命令",
    next_help_heading = "选项"
)]
pub struct Cli {
    #[arg(long, global = true, value_name = "路径")]
    pub database: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,

    /// 显示帮助。
    #[arg(short = 'h', long = "help", global = true, action = ArgAction::Help)]
    pub help: Option<bool>,

    /// 显示版本。
    #[arg(short = 'V', long = "version", action = ArgAction::Version)]
    pub version: Option<bool>,
}

impl Cli {
    pub fn parse_localized() -> Self {
        let mut command = Self::command();
        localize_help(&mut command, true, "flowwatch");
        let matches = command
            .try_get_matches()
            .unwrap_or_else(|error| exit_with_cli_error(error));
        Self::from_arg_matches(&matches)
            .unwrap_or_else(|error| -> Self { exit_with_cli_error(error) })
    }
}

fn exit_with_cli_error(error: clap::Error) -> ! {
    let kind = error.kind();
    if matches!(
        kind,
        ErrorKind::DisplayHelp
            | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            | ErrorKind::DisplayVersion
    ) {
        let _ = error.print();
    } else {
        let message = match kind {
            ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => "命令或参数无法识别。",
            ErrorKind::InvalidValue | ErrorKind::ValueValidation => "参数值无效。",
            ErrorKind::MissingRequiredArgument => "缺少必需的命令或参数。",
            ErrorKind::ArgumentConflict => "这些参数不能同时使用。",
            ErrorKind::TooManyValues => "参数提供了过多的值。",
            ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues | ErrorKind::NoEquals => {
                "参数缺少值或格式不正确。"
            }
            ErrorKind::MissingSubcommand => "缺少要执行的命令。",
            _ => "命令行参数无效。",
        };
        eprintln!("错误：{message}");
        eprintln!("使用 --help 查看用法。\n");
    }
    std::process::exit(error.exit_code());
}

fn localize_help(command: &mut ClapCommand, is_root: bool, path: &str) {
    for subcommand in command.get_subcommands_mut() {
        let subpath = format!("{path} {}", subcommand.get_name());
        localize_help(subcommand, false, &subpath);
    }
    let has_subcommands = command.get_subcommands().next().is_some();
    let positional = command
        .get_positionals()
        .filter_map(|argument| argument.get_value_names()?.first())
        .map(|name| format!(" <{name}>"))
        .collect::<String>();
    let usage = format!(
        "{path} [选项]{}{positional}",
        if has_subcommands { " <命令>" } else { "" }
    );
    *command = command
        .clone()
        .help_template(if is_root {
            HELP_TEMPLATE
        } else {
            SUBCOMMAND_HELP_TEMPLATE
        })
        .subcommand_help_heading("命令")
        .next_help_heading("选项")
        .disable_help_subcommand(true)
        .override_usage(usage);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_help_uses_chinese_headings_and_usage() {
        let mut command = Cli::command();
        localize_help(&mut command, true, "flowwatch");
        let help = command.render_help().to_string();
        assert!(help.contains("用法：flowwatch [选项] <命令>"));
        assert!(help.contains("命令:"));
        assert!(help.contains("选项:"));
        assert!(!help.contains("Commands:"));
        assert!(!help.contains("Options:"));
    }

    #[test]
    fn localized_subcommand_help_keeps_the_full_command_path() {
        let mut command = Cli::command();
        localize_help(&mut command, true, "flowwatch");
        let apps = command
            .get_subcommands_mut()
            .find(|subcommand| subcommand.get_name() == "apps")
            .expect("apps subcommand must exist");
        let help = apps.render_help().to_string();
        assert!(help.contains("用法：flowwatch apps [选项]"));
        assert!(help.contains("选项:"));
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 在后台持续采集流量。
    Collect(CollectArgs),
    /// 查看采集服务状态、实际总量和应用识别率。
    Status,
    /// 按上传、下载或总量查看应用排行。
    Apps(QueryArgs),
    /// 查看物理网卡的实际流量总量。
    Interfaces(QueryArgs),
    /// 查看流量最高的时间段。
    Spikes(QueryArgs),
    /// 查看未能识别到应用的流量时段。
    Gaps(QueryArgs),
    /// 检查采集服务、数据库和权限，不修改设置。
    Doctor,
    /// 为当前用户安装并启动登录自启服务。
    Install(InstallArgs),
    /// 删除登录自启服务和命令，默认保留历史数据。
    Uninstall(UninstallArgs),
    /// 管理可选的流量来源。
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct CollectArgs {
    /// 运行指定秒数后停止；填 0 表示持续运行。
    #[arg(long, value_name = "秒数", default_value_t = 0, help_heading = "选项")]
    pub run_seconds: u64,
}

#[derive(Debug, Clone, Args)]
pub struct QueryArgs {
    /// 时间范围：today（今天）、yesterday（昨天）、24h、7d、30d 或 all（全部）。
    #[arg(
        long,
        value_name = "时间范围",
        default_value = "today",
        hide_default_value = true,
        conflicts_with_all = ["from", "to"],
        help_heading = "选项"
    )]
    pub period: String,

    /// 明确的开始时间，包含该时间点；支持本地时间、Unix 时间戳或 RFC 3339。
    #[arg(long, value_name = "时间", requires = "to", help_heading = "选项")]
    pub from: Option<String>,

    /// 明确的结束时间，不包含该时间点；支持本地时间、Unix 时间戳或 RFC 3339。
    #[arg(long, value_name = "时间", requires = "from", help_heading = "选项")]
    pub to: Option<String>,

    /// 排序方式：upload、download 或 total；默认按总量排序。
    #[arg(
        long,
        value_enum,
        value_name = "排序方式",
        default_value_t = SortBy::Total,
        hide_default_value = true,
        hide_possible_values = true,
        help_heading = "选项"
    )]
    pub sort: SortBy,

    /// 最多显示多少条记录；默认 20 条。
    #[arg(
        long,
        value_name = "条数",
        default_value_t = 20,
        hide_default_value = true,
        help_heading = "选项"
    )]
    pub limit: usize,

    /// 输出 JSON，字段名保持英文。
    #[arg(long, help_heading = "选项")]
    pub json: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SortBy {
    Upload,
    Download,
    Total,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AppGranularity {
    #[value(name = "5m")]
    FiveMinutes,
    #[value(name = "1m")]
    OneMinute,
}

impl AppGranularity {
    pub const fn setting(self) -> &'static str {
        match self {
            Self::FiveMinutes => "5m",
            Self::OneMinute => "1m",
        }
    }

    pub const fn bucket_seconds(self) -> i64 {
        match self {
            Self::FiveMinutes => 300,
            Self::OneMinute => 60,
        }
    }
}

#[derive(Debug, Args)]
pub struct InstallArgs {
    /// 覆盖采样间隔；首次安装默认 3 秒。
    #[arg(long, value_name = "秒数", help_heading = "选项")]
    pub poll_seconds: Option<u64>,

    /// 覆盖数据库保存间隔；首次安装默认 60 秒。
    #[arg(long, value_name = "秒数", help_heading = "选项")]
    pub flush_seconds: Option<u64>,

    /// 覆盖明细保留天数；首次安装默认 30 天。
    #[arg(long, value_name = "天数", help_heading = "选项")]
    pub detail_days: Option<i64>,

    /// 覆盖每日汇总保留天数；首次安装默认 365 天。
    #[arg(long, value_name = "天数", help_heading = "选项")]
    pub daily_days: Option<i64>,

    /// 覆盖应用明细粒度；首次安装默认每 5 分钟，也可改为每分钟。
    #[arg(
        long,
        value_name = "粒度",
        value_enum,
        hide_possible_values = true,
        help_heading = "选项"
    )]
    pub app_granularity: Option<AppGranularity>,

    /// 安装时导入 Clash/Mihomo 的 config.yaml。
    #[arg(long, value_name = "路径", help_heading = "选项")]
    pub clash_config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct UninstallArgs {
    /// 同时删除 SQLite 历史数据库。
    #[arg(long, help_heading = "选项")]
    pub purge_data: bool,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// 从 Mihomo config.yaml 导入控制器地址和密钥。
    ImportClash {
        #[arg(value_name = "路径", help_heading = "参数")]
        path: PathBuf,
    },
    /// 停用 Clash 数据来源，但保留已保存的配置。
    DisableClash,
    /// 修改应用明细粒度并重启采集服务。
    SetAppGranularity {
        #[arg(
            value_name = "粒度",
            hide_possible_values = true,
            help_heading = "参数"
        )]
        granularity: AppGranularity,
    },
    /// 显示设置；密钥内容会隐藏。
    Show,
}
