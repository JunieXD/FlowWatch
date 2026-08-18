use clap::error::{ContextKind, ErrorKind};
use clap::{
    ArgAction, Args, Command as ClapCommand, CommandFactory, FromArgMatches, Parser, Subcommand,
    ValueEnum,
};
use std::path::PathBuf;

const HELP_TEMPLATE: &str =
    "{before-help}{name} {version}\n{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";
const SUBCOMMAND_HELP_TEMPLATE: &str =
    "{before-help}{about-with-newline}\n用法：{usage}\n\n{all-args}{after-help}";
const ROOT_AFTER_HELP: &str = "\
第一次使用：
  flowwatch install              安装并启动登录自启服务
  flowwatch doctor               检查采集是否正常
  flowwatch status               查看今天的流量概况
  flowwatch apps                 查看今天的应用流量排行

常见查询：
  flowwatch apps --period 24h --sort download --limit 10
  flowwatch apps --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"
  flowwatch spikes --period 7d
  flowwatch gaps --period 24h

查看详细用法：
  flowwatch <命令> --help
  flowwatch help <命令>";
const COLLECT_AFTER_HELP: &str = "\
说明：
  通常不需要手动运行此命令。flowwatch install 会安装并管理后台采集服务。

示例：
  flowwatch collect --run-seconds 30    采集 30 秒后退出
  flowwatch collect                     持续采集，按 Ctrl-C 停止";
const STATUS_AFTER_HELP: &str = "\
说明：
  此命令固定显示今天的概况，不接受时间范围参数。
  查看其他时间请使用 apps、interfaces、spikes 或 gaps。

示例：
  flowwatch status
  flowwatch apps --period 昨天";
const APPS_AFTER_HELP: &str = "\
示例：
  flowwatch apps
  flowwatch apps --period 昨天
  flowwatch apps --period 24h --sort download --limit 10
  flowwatch apps --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --from 和 --to 必须一起使用，且不能同时使用 --period。
  开始时间包含在结果内，结束时间不包含在结果内。
  本地时间格式为 YYYY-MM-DD HH:MM[:SS]；也支持 Unix 时间戳和 RFC 3339。";
const INTERFACES_AFTER_HELP: &str = "\
示例：
  flowwatch interfaces
  flowwatch interfaces --period 7d
  flowwatch interfaces --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --from 和 --to 必须一起使用，且不能同时使用 --period。
  开始时间包含在结果内，结束时间不包含在结果内。
  本地时间格式为 YYYY-MM-DD HH:MM[:SS]；也支持 Unix 时间戳和 RFC 3339。";
const SPIKES_AFTER_HELP: &str = "\
示例：
  flowwatch spikes
  flowwatch spikes --period 24h --sort upload --limit 20
  flowwatch spikes --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --from 和 --to 必须一起使用，且不能同时使用 --period。
  开始时间包含在结果内，结束时间不包含在结果内。
  本地时间格式为 YYYY-MM-DD HH:MM[:SS]；也支持 Unix 时间戳和 RFC 3339。";
const GAPS_AFTER_HELP: &str = "\
说明：
  “未识别”是实际网卡流量中没能对应到具体应用的部分。

示例：
  flowwatch gaps
  flowwatch gaps --period 24h --limit 20
  flowwatch gaps --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --from 和 --to 必须一起使用，且不能同时使用 --period。
  开始时间包含在结果内，结束时间不包含在结果内。
  本地时间格式为 YYYY-MM-DD HH:MM[:SS]；也支持 Unix 时间戳和 RFC 3339。";
const DOCTOR_AFTER_HELP: &str = "\
示例：
  flowwatch doctor

检查失败时，先阅读每个 [失败] 或 [警告] 项；此命令不会修改设置或删除数据。";
const INSTALL_AFTER_HELP: &str = "\
示例：
  flowwatch install
  flowwatch install --app-granularity 1m
  flowwatch install --clash-config \"/path/to/config.yaml\"

说明：
  不提供选项时使用推荐默认值，并保留已有设置和历史数据。
  Clash/Mihomo 是可选数据来源，不使用代理时无需配置。";
const UNINSTALL_AFTER_HELP: &str = "\
示例：
  flowwatch uninstall                删除服务，保留历史数据
  flowwatch uninstall --purge-data   同时永久删除历史数据库";
const CONFIG_AFTER_HELP: &str = "\
常见操作：
  flowwatch config show
  flowwatch config import-clash \"/path/to/config.yaml\"
  flowwatch config set-app-granularity 1m
  flowwatch config disable-clash

查看某项设置的详细用法：
  flowwatch config <命令> --help";
const IMPORT_CLASH_AFTER_HELP: &str = "\
示例：
  flowwatch config import-clash \"$HOME/Library/Application Support/io.github.clash-verge-rev.clash-verge-rev/config.yaml\"

只会读取外部控制器地址和密钥，不会修改 Clash/Mihomo 配置。";
const GRANULARITY_AFTER_HELP: &str = "\
示例：
  flowwatch config set-app-granularity 1m   每分钟保存应用明细
  flowwatch config set-app-granularity 5m   每五分钟保存，资源占用更低";
const DISABLE_CLASH_AFTER_HELP: &str = "\
示例：
  flowwatch config disable-clash

已保存的控制器地址和密钥会保留，之后可重新导入配置来启用。";
const SHOW_CONFIG_AFTER_HELP: &str = "\
示例：
  flowwatch config show

Clash/Mihomo 密钥只显示为 [已隐藏]，不会输出原文。";

#[derive(Debug, Parser)]
#[command(
    name = "flowwatch",
    version,
    about = "轻量、透明的 macOS 应用流量统计工具",
    help_template = HELP_TEMPLATE,
    after_help = ROOT_AFTER_HELP,
    arg_required_else_help = true,
    disable_help_flag = true,
    disable_version_flag = true,
    subcommand_help_heading = "命令",
    next_help_heading = "选项"
)]
pub struct Cli {
    /// 使用指定的 SQLite 数据库；默认使用 FlowWatch 的用户数据库。
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
        let command = localized_command();
        let matches = command
            .try_get_matches()
            .unwrap_or_else(|error| exit_with_cli_error(error));
        Self::from_arg_matches(&matches)
            .unwrap_or_else(|error| -> Self { exit_with_cli_error(error) })
    }
}

fn localized_command() -> ClapCommand {
    let mut command = Cli::command();
    // Build first so Clap's standard `help` command is present and can be localized too.
    command.build();
    localize_help(&mut command, true, "flowwatch");
    command
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
        eprintln!("{}", localized_error_message(&error));
    }
    std::process::exit(error.exit_code());
}

fn localized_error_message(error: &clap::Error) -> String {
    let value = |kind| {
        error
            .get(kind)
            .map(ToString::to_string)
            .filter(|value| !value.trim().is_empty())
    };
    let invalid_arg = value(ContextKind::InvalidArg);
    let invalid_value = value(ContextKind::InvalidValue);
    let invalid_subcommand = value(ContextKind::InvalidSubcommand);
    let prior_arg = value(ContextKind::PriorArg);
    let valid_values = value(ContextKind::ValidValue).map(|values| values.replace(", ", "、"));

    let message = match error.kind() {
        ErrorKind::UnknownArgument => invalid_arg.as_deref().map_or_else(
            || "无法识别这个参数。".to_string(),
            |arg| format!("无法识别参数“{arg}”。"),
        ),
        ErrorKind::InvalidSubcommand => invalid_subcommand.as_deref().map_or_else(
            || "无法识别这个命令。".to_string(),
            |command| format!("无法识别命令“{command}”。"),
        ),
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => match (
            invalid_arg.as_deref(),
            invalid_value.as_deref(),
            valid_values.as_deref(),
        ) {
            (Some(arg), Some(input), Some(valid)) => {
                format!("参数“{arg}”不支持值“{input}”；可用值：{valid}。")
            }
            (Some(arg), Some(input), None) => format!("参数“{arg}”的值“{input}”无效。"),
            (Some(arg), None, _) => format!("参数“{arg}”缺少值。"),
            _ => "参数值无效。".to_string(),
        },
        ErrorKind::MissingRequiredArgument => invalid_arg.as_deref().map_or_else(
            || "缺少必需的命令或参数。".to_string(),
            |arg| format!("缺少必需内容：{arg}。"),
        ),
        ErrorKind::ArgumentConflict => match (invalid_arg.as_deref(), prior_arg.as_deref()) {
            (Some(arg), Some(prior)) => format!("参数“{arg}”不能与“{prior}”同时使用。"),
            _ => "这些参数不能同时使用。".to_string(),
        },
        ErrorKind::TooManyValues => "参数提供了过多的值。".to_string(),
        ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues | ErrorKind::NoEquals => {
            "参数缺少值或格式不正确。".to_string()
        }
        ErrorKind::MissingSubcommand => "缺少要执行的命令。".to_string(),
        _ => "命令行参数无效。".to_string(),
    };

    let suggestion = [
        ContextKind::SuggestedSubcommand,
        ContextKind::SuggestedArg,
        ContextKind::SuggestedValue,
        ContextKind::SuggestedCommand,
    ]
    .into_iter()
    .find_map(value)
    .map(|suggestion| suggestion.replace(", ", "、"));
    let usage = value(ContextKind::Usage).map(|usage| {
        usage
            .trim()
            .strip_prefix("Usage: ")
            .unwrap_or(usage.trim())
            .to_string()
    });

    let mut output = format!("错误：{message}");
    if let Some(suggestion) = suggestion {
        output.push_str(&format!("\n可能的正确写法：{suggestion}"));
    }
    if let Some(usage) = usage {
        output.push_str(&format!("\n\n用法：{usage}"));
    }
    output.push_str("\n提示：在当前命令末尾加上 --help，可查看完整说明和示例。\n");
    output
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
    let mut localized = command
        .clone()
        .help_template(if is_root {
            HELP_TEMPLATE
        } else {
            SUBCOMMAND_HELP_TEMPLATE
        })
        .subcommand_help_heading("命令")
        .next_help_heading("选项")
        .override_usage(usage);
    if command.get_name() == "help" {
        localized = localized.about("查看全部命令，或查看指定命令的详细帮助");
    }
    *command = localized;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localized_help_uses_chinese_headings_and_usage() {
        let mut command = localized_command();
        let help = command.render_help().to_string();
        assert!(help.contains("用法：flowwatch [选项] <命令>"));
        assert!(help.contains("第一次使用："));
        assert!(help.contains("help"));
        assert!(help.contains("查看全部命令，或查看指定命令的详细帮助"));
        assert!(help.contains("命令:"));
        assert!(help.contains("选项:"));
        assert!(!help.contains("Commands:"));
        assert!(!help.contains("Options:"));
    }

    #[test]
    fn localized_subcommand_help_keeps_the_full_command_path() {
        let mut command = localized_command();
        let apps = command
            .get_subcommands_mut()
            .find(|subcommand| subcommand.get_name() == "apps")
            .expect("apps subcommand must exist");
        let help = apps.render_help().to_string();
        assert!(help.contains("用法：flowwatch apps [选项]"));
        assert!(help.contains("--from 和 --to 必须一起使用"));
        assert!(help.contains("flowwatch apps --from \"2026-08-18 09:00\""));
        assert!(help.contains("选项:"));
    }

    #[test]
    fn standard_help_subcommand_shows_localized_command_help() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "apps"])
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        let help = error.to_string();
        assert!(help.contains("用法：flowwatch apps [选项]"));
        assert!(help.contains("自定义时间说明："));
    }

    #[test]
    fn argument_errors_name_the_input_and_offer_a_suggestion() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "apps", "--fromm", "2026-08-18 09:00"])
            .unwrap_err();
        let message = localized_error_message(&error);
        assert!(message.contains("无法识别参数“--fromm”"));
        assert!(message.contains("--from"));
        assert!(message.contains("用法：flowwatch apps"));
    }

    #[test]
    fn missing_option_value_names_the_option() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "apps", "--period"])
            .unwrap_err();
        let message = localized_error_message(&error);
        assert!(message.contains("参数“--period <时间范围>”缺少值"));
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 在后台持续采集流量。
    #[command(after_help = COLLECT_AFTER_HELP)]
    Collect(CollectArgs),
    /// 查看采集服务状态、实际总量和应用识别率。
    #[command(after_help = STATUS_AFTER_HELP)]
    Status,
    /// 按上传、下载或总量查看应用排行。
    #[command(after_help = APPS_AFTER_HELP)]
    Apps(QueryArgs),
    /// 查看物理网卡的实际流量总量。
    #[command(after_help = INTERFACES_AFTER_HELP)]
    Interfaces(QueryArgs),
    /// 查看流量最高的时间段。
    #[command(after_help = SPIKES_AFTER_HELP)]
    Spikes(QueryArgs),
    /// 查看未能识别到应用的流量时段。
    #[command(after_help = GAPS_AFTER_HELP)]
    Gaps(QueryArgs),
    /// 检查采集服务、数据库和权限，不修改设置。
    #[command(after_help = DOCTOR_AFTER_HELP)]
    Doctor,
    /// 为当前用户安装并启动登录自启服务。
    #[command(after_help = INSTALL_AFTER_HELP)]
    Install(InstallArgs),
    /// 删除登录自启服务和命令，默认保留历史数据。
    #[command(after_help = UNINSTALL_AFTER_HELP)]
    Uninstall(UninstallArgs),
    /// 管理可选的流量来源。
    #[command(after_help = CONFIG_AFTER_HELP)]
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
    /// 时间范围；默认 today（今天）。也可用 yesterday（昨天）、all（全部）、24h、7d、30d 等。
    #[arg(
        long,
        value_name = "时间范围",
        default_value = "today",
        hide_default_value = true,
        conflicts_with_all = ["from", "to"],
        help_heading = "选项"
    )]
    pub period: String,

    /// 自定义开始时间，包含该时间点；必须和 --to 一起使用。
    #[arg(long, value_name = "时间", requires = "to", help_heading = "选项")]
    pub from: Option<String>,

    /// 自定义结束时间，不包含该时间点；必须和 --from 一起使用。
    #[arg(long, value_name = "时间", requires = "from", help_heading = "选项")]
    pub to: Option<String>,

    /// 排序方式：upload（上传）、download（下载）或 total（合计）；默认按合计排序。
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
    #[command(after_help = IMPORT_CLASH_AFTER_HELP)]
    ImportClash {
        /// Clash/Mihomo 的 config.yaml 路径。
        #[arg(value_name = "路径", help_heading = "参数")]
        path: PathBuf,
    },
    /// 停用 Clash 数据来源，但保留已保存的配置。
    #[command(after_help = DISABLE_CLASH_AFTER_HELP)]
    DisableClash,
    /// 修改应用明细粒度并重启采集服务。
    #[command(after_help = GRANULARITY_AFTER_HELP)]
    SetAppGranularity {
        #[arg(
            value_name = "粒度",
            hide_possible_values = true,
            help = "保存间隔：1m（每分钟）或 5m（每五分钟）。",
            help_heading = "参数"
        )]
        granularity: AppGranularity,
    },
    /// 显示设置；密钥内容会隐藏。
    #[command(after_help = SHOW_CONFIG_AFTER_HELP)]
    Show,
}
