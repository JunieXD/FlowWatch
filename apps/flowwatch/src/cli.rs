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
  flowwatch app \"ChatGPT\"        查看某个应用的详细用量
  flowwatch chart --period 24h   查看过去 24 小时的流量趋势

常见查询：
  flowwatch chart --period 6h
  flowwatch chart --date 2026-08-18
  flowwatch report --period 24h --compare
  flowwatch apps --period 24h --sort download --limit 10
  flowwatch apps --period 24h --details
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
  查看其他时间请使用 chart、apps、interfaces、spikes 或 gaps。

示例：
  flowwatch status
  flowwatch apps --period 昨天";
const CHART_AFTER_HELP: &str = "\
说明：
  纵轴表示每个时间段内使用的流量，不是累计值；横轴表示时间。
  默认根据时间跨度和终端宽度自动选择间隔，上传、下载和合计使用不同颜色与符号。

示例：
  flowwatch chart --period 6h
  flowwatch chart --app \"ChatGPT\" --period 24h
  flowwatch chart --period 24h
  flowwatch chart --date 2026-08-18
  flowwatch chart --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"
  flowwatch chart --period 24h --interval 15m --height 16 --width 120

时间范围：
  --date 用于查看某个自然日；--from 和 --to 用于任意起止时间。
  --period、--date 和 --from/--to 三种写法不能同时使用。";
const APP_AFTER_HELP: &str = "\
说明：
  应用可以使用显示名称或完整应用 ID。名称匹配到多个应用时会列出候选项。

示例：
  flowwatch app \"ChatGPT\" --period 24h
  flowwatch app \"bundle:com.openai.codex\" --period 7d
  flowwatch chart --app \"ChatGPT\" --period 24h

时间范围：
  --date 用于查看某个自然日；--from 和 --to 用于任意起止时间。";
const EXPLAIN_AFTER_HELP: &str = "\
说明：
  使用 --at 可直接分析某一分钟；程序会自动采用当时的应用明细精度。
  使用时间范围时，会先找到范围内流量最高的时段，再分析主要应用和未识别流量。

示例：
  flowwatch explain --at \"2026-08-18 18:37\"
  flowwatch explain --period 24h
  flowwatch explain --date 2026-08-18
  flowwatch explain --from \"2026-08-18 18:30\" --to \"2026-08-18 18:45\"

时间范围：
  --at、--period、--date 和 --from/--to 四种写法不能同时使用。";
const REPORT_AFTER_HELP: &str = "\
说明：
  报告汇总实际流量、应用识别完整度、主要应用、最高时段和未识别流量。
  增加 --compare 可与紧邻的上一段等长时间比较。

示例：
  flowwatch report --period 24h
  flowwatch report --date 2026-08-18
  flowwatch report --period 7d --compare
  flowwatch report --period 24h --json";
const INVESTIGATE_AFTER_HELP: &str = "\
调查模式会临时使用每秒采样和每分钟应用明细，到期后自动恢复原设置。

示例：
  flowwatch investigate start --duration 30m
  flowwatch investigate status
  flowwatch investigate stop

时长可使用 5m、30m、1h、6h 或 24h，范围为 5 分钟到 24 小时。";
const ALERTS_AFTER_HELP: &str = "\
提醒规则保存在本机数据库中。达到限额的 80% 和 100% 时各提醒一次。

示例：
  flowwatch alerts add --daily 10GiB
  flowwatch alerts add --monthly 100GiB
  flowwatch alerts add --app \"ChatGPT\" --daily 2GiB
  flowwatch alerts list
  flowwatch alerts disable 1
  flowwatch alerts enable 1
  flowwatch alerts remove 1
  flowwatch alerts test

容量支持 B、KiB、MiB、GiB、TiB，也支持 KB、MB、GB、TB。应用限额只统计已识别到的应用流量。";
const APPS_AFTER_HELP: &str = "\
示例：
  flowwatch apps
  flowwatch apps --period 昨天
  flowwatch apps --date 2026-08-18
  flowwatch apps --period 24h --sort download --limit 10
  flowwatch apps --period 24h --details
  flowwatch apps --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --date 可查看某个自然日；--from 和 --to 必须一起使用。
  --period、--date 和 --from/--to 三种写法不能同时使用。
  开始时间包含在结果内，结束时间不包含在结果内。
  本地时间格式为 YYYY-MM-DD HH:MM[:SS]；也支持 Unix 时间戳和 RFC 3339。";
const INTERFACES_AFTER_HELP: &str = "\
示例：
  flowwatch interfaces
  flowwatch interfaces --period 7d
  flowwatch interfaces --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --date 可查看某个自然日；--from 和 --to 必须一起使用。
  --period、--date 和 --from/--to 三种写法不能同时使用。
  开始时间包含在结果内，结束时间不包含在结果内。
  本地时间格式为 YYYY-MM-DD HH:MM[:SS]；也支持 Unix 时间戳和 RFC 3339。";
const SPIKES_AFTER_HELP: &str = "\
示例：
  flowwatch spikes
  flowwatch spikes --period 24h --sort upload --limit 20
  flowwatch spikes --from \"2026-08-18 09:00\" --to \"2026-08-18 18:00\"

自定义时间说明：
  --date 可查看某个自然日；--from 和 --to 必须一起使用。
  --period、--date 和 --from/--to 三种写法不能同时使用。
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
  --date 可查看某个自然日；--from 和 --to 必须一起使用。
  --period、--date 和 --from/--to 三种写法不能同时使用。
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
  flowwatch config app-names list
  flowwatch config app-names set <应用ID> \"我的名称\"
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
const APP_NAMES_AFTER_HELP: &str = "\
应用 ID 可通过 flowwatch apps --details 查看。路径型程序会使用不随安装路径变化的 group: ID。

示例：
  flowwatch config app-names list
  flowwatch config app-names set \"bundle:com.example.App\" \"工作浏览器\"
  flowwatch config app-names set \"group:chrome-headless-shell:chrome-headless-shell\" \"自动化浏览器\"
  flowwatch config app-names remove \"bundle:com.example.App\"";
const DATA_AFTER_HELP: &str = "\
查看、导出和维护本机 SQLite 流量数据。

示例：
  flowwatch data info
  flowwatch data export --period 30d --format csv --output flowwatch.csv
  flowwatch data export --date 2026-08-18 --format json --output flowwatch.json
  flowwatch data retention --details 30d --daily 365d
  flowwatch data prune --before 2026-01-01 --confirm
  flowwatch data compact

导出不会覆盖已有文件。prune 会永久删除指定日期以前的流量记录，必须明确增加 --confirm。";
const UPDATE_AFTER_HELP: &str = "\
从 GitHub Release 下载适合当前 Mac 的正式版本，校验 SHA-256 和程序版本后再安装。

示例：
  flowwatch update --check
  flowwatch update
  flowwatch update --version 0.2.0

更新会保留数据库和全部设置。程序不会自动降级，也不会安装预发布版本。";
const DASHBOARD_AFTER_HELP: &str = "\
在一个交互式终端界面中查看概览、趋势、应用和异常时段。

示例：
  flowwatch dashboard
  flowwatch dashboard --period 24h
  flowwatch dashboard --date 2026-08-18

Tab 或左右方向键切换视图，上下方向键选择记录，Enter 查看详情，r 刷新，q 退出。";

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
    let validation_error = std::error::Error::source(error).map(ToString::to_string);

    let message = match error.kind() {
        ErrorKind::UnknownArgument => invalid_arg.as_deref().map_or_else(
            || "无法识别这个参数。".to_string(),
            |arg| format!("无法识别参数“{arg}”。"),
        ),
        ErrorKind::InvalidSubcommand => invalid_subcommand.as_deref().map_or_else(
            || "无法识别这个命令。".to_string(),
            |command| format!("无法识别命令“{command}”。"),
        ),
        ErrorKind::ValueValidation => match (invalid_arg.as_deref(), validation_error.as_deref()) {
            (Some(arg), Some(detail)) => format!("参数“{arg}”无效：{detail}。"),
            (_, Some(detail)) => format!("参数值无效：{detail}。"),
            _ => "参数值无效。".to_string(),
        },
        ErrorKind::InvalidValue => match (
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

    #[test]
    fn chart_help_explains_dates_intervals_and_visual_controls() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "chart"])
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);
        let help = error.to_string();
        assert!(help.contains("flowwatch chart --date 2026-08-18"));
        assert!(help.contains("--interval <间隔>"));
        assert!(help.contains("--app <应用>"));
        assert!(help.contains("--no-color"));
    }

    #[test]
    fn chart_dimensions_are_validated_during_argument_parsing() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "chart", "--width", "49"])
            .unwrap_err();
        let message = localized_error_message(&error);
        assert!(message.contains("图表宽度必须在 50 到 240 之间"));
    }

    #[test]
    fn explain_accepts_a_timestamp_or_a_range_but_not_both() {
        assert!(Cli::try_parse_from(["flowwatch", "explain", "--at", "2026-08-18 18:37"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "explain",
                "--at",
                "2026-08-18 18:37",
                "--period",
                "24h",
            ])
            .is_err()
        );
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "explain"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("flowwatch explain --at \"2026-08-18 18:37\""));
        assert!(help.contains("主要应用和未识别流量"));
    }

    #[test]
    fn app_help_explains_selectors_details_and_trends() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "app"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("用法：flowwatch app [选项] <应用>"));
        assert!(help.contains("flowwatch chart --app \"ChatGPT\""));

        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "apps"])
            .unwrap_err();
        assert!(error.to_string().contains("--details"));
    }

    #[test]
    fn report_help_explains_comparison_and_structured_output() {
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "report"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("flowwatch report --period 7d --compare"));
        assert!(help.contains("--json"));
        assert!(
            Cli::try_parse_from(["flowwatch", "report", "--period", "24h", "--compare"]).is_ok()
        );
    }

    #[test]
    fn investigation_duration_is_bounded_and_help_explains_auto_restore() {
        assert_eq!(parse_investigation_duration("5m").unwrap(), 300);
        assert_eq!(parse_investigation_duration("24h").unwrap(), 86_400);
        assert!(parse_investigation_duration("4m").is_err());
        assert!(parse_investigation_duration("25h").is_err());
        assert!(parse_investigation_duration("300").is_err());
        assert!(
            Cli::try_parse_from(["flowwatch", "investigate", "start", "--duration", "30m",])
                .is_ok()
        );
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "investigate"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("到期后自动恢复原设置"));
        assert!(help.contains("flowwatch investigate stop"));
    }

    #[test]
    fn alert_sizes_and_commands_are_validated_during_parsing() {
        assert_eq!(parse_byte_size("1KiB").unwrap(), 1_024);
        assert_eq!(parse_byte_size("1.5GiB").unwrap(), 1_610_612_736);
        assert_eq!(parse_byte_size("2GB").unwrap(), 2_000_000_000);
        assert!(parse_byte_size("10").is_err());
        assert!(parse_byte_size("0B").is_err());
        assert!(parse_byte_size("999999999999999999999TiB").is_err());
        assert!(Cli::try_parse_from(["flowwatch", "alerts", "add", "--daily", "10GiB"]).is_ok());
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "alerts",
                "add",
                "--app",
                "ChatGPT",
                "--monthly",
                "100GB",
            ])
            .is_ok()
        );
        assert!(Cli::try_parse_from(["flowwatch", "alerts", "add"]).is_err());
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "alerts",
                "add",
                "--daily",
                "1GiB",
                "--monthly",
                "10GiB",
            ])
            .is_err()
        );
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "alerts"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("达到限额的 80% 和 100%"));
        assert!(help.contains("flowwatch alerts add --app \"ChatGPT\""));
    }

    #[test]
    fn app_name_commands_are_discoverable_and_parse_nested_actions() {
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "config",
                "app-names",
                "set",
                "bundle:com.example.App",
                "工作浏览器",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "config",
                "app-names",
                "remove",
                "bundle:com.example.App",
            ])
            .is_ok()
        );
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "config", "app-names"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("flowwatch apps --details"));
        assert!(help.contains("group:chrome-headless-shell"));
    }

    #[test]
    fn data_commands_explain_safety_and_validate_retention() {
        assert_eq!(parse_detail_retention("30d").unwrap(), 30);
        assert_eq!(parse_daily_retention("365d").unwrap(), 365);
        assert!(parse_detail_retention("0d").is_err());
        assert!(parse_daily_retention("30").is_err());
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "data",
                "export",
                "--period",
                "30d",
                "--format",
                "csv",
                "--output",
                "flowwatch.csv",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "data",
                "retention",
                "--details",
                "30d",
                "--daily",
                "365d",
            ])
            .is_ok()
        );
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "data"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("不会覆盖已有文件"));
        assert!(help.contains("必须明确增加 --confirm"));
    }

    #[test]
    fn update_command_is_explained_without_requiring_the_readme() {
        assert!(Cli::try_parse_from(["flowwatch", "update", "--check"]).is_ok());
        assert!(Cli::try_parse_from(["flowwatch", "update", "--version", "0.2.0"]).is_ok());
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "update"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("校验 SHA-256"));
        assert!(help.contains("不会自动降级"));
        assert!(help.contains("flowwatch update --check"));
    }

    #[test]
    fn dashboard_help_lists_views_controls_and_time_ranges() {
        assert!(Cli::try_parse_from(["flowwatch", "dashboard", "--period", "24h"]).is_ok());
        assert!(Cli::try_parse_from(["flowwatch", "dashboard", "--date", "2026-08-18"]).is_ok());
        let error = localized_command()
            .try_get_matches_from(["flowwatch", "help", "dashboard"])
            .unwrap_err();
        let help = error.to_string();
        assert!(help.contains("概览、趋势、应用和异常时段"));
        assert!(help.contains("Tab 或左右方向键"));
        assert!(help.contains("flowwatch dashboard --date 2026-08-18"));
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
    /// 在终端中绘制上传、下载和合计流量趋势图。
    #[command(after_help = CHART_AFTER_HELP)]
    Chart(ChartArgs),
    /// 分析流量最高或指定的时段，找出主要应用和未识别流量。
    #[command(after_help = EXPLAIN_AFTER_HELP)]
    Explain(ExplainArgs),
    /// 生成包含总量、主要应用、高峰和数据说明的流量报告。
    #[command(after_help = REPORT_AFTER_HELP)]
    Report(ReportArgs),
    /// 临时提高采样精度，并在到期后自动恢复。
    #[command(after_help = INVESTIGATE_AFTER_HELP)]
    Investigate(InvestigateArgs),
    /// 设置每日、每月或单个应用的流量限额和本机通知。
    #[command(after_help = ALERTS_AFTER_HELP)]
    Alerts(AlertsArgs),
    /// 按上传、下载或总量查看应用排行。
    #[command(after_help = APPS_AFTER_HELP)]
    Apps(AppsArgs),
    /// 查看单个应用的用量、身份、来源和最高流量时段。
    #[command(after_help = APP_AFTER_HELP)]
    App(AppArgs),
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
    /// 查看、导出、清理和压缩本机流量数据。
    #[command(after_help = DATA_AFTER_HELP)]
    Data(DataArgs),
    /// 检查或安装经过校验的 FlowWatch 正式版本。
    #[command(after_help = UPDATE_AFTER_HELP)]
    Update(UpdateArgs),
    /// 打开包含概览、趋势、应用和异常时段的交互界面。
    #[command(after_help = DASHBOARD_AFTER_HELP)]
    Dashboard(DashboardArgs),
}

#[derive(Debug, Clone, Args)]
pub struct DashboardArgs {
    #[command(flatten)]
    pub range: TimeRangeArgs,
}

#[derive(Debug, Args)]
pub struct UpdateArgs {
    /// 只检查版本，不下载或安装发布包。
    #[arg(long, help_heading = "选项")]
    pub check: bool,
    /// 检查或安装指定正式版本，例如 0.2.0。
    #[arg(long, value_name = "版本", help_heading = "选项")]
    pub version: Option<String>,
}

#[derive(Debug, Args)]
pub struct CollectArgs {
    /// 运行指定秒数后停止；填 0 表示持续运行。
    #[arg(long, value_name = "秒数", default_value_t = 0, help_heading = "选项")]
    pub run_seconds: u64,
}

#[derive(Debug, Clone, Args)]
pub struct TimeRangeArgs {
    /// 时间范围；默认 today（今天）。也可用 yesterday（昨天）、all（全部）、24h、7d、30d 等。
    #[arg(
        long,
        value_name = "时间范围",
        default_value = "today",
        hide_default_value = true,
        conflicts_with_all = ["date", "from", "to"],
        help_heading = "选项"
    )]
    pub period: String,

    /// 查看某个自然日，格式为 YYYY-MM-DD。
    #[arg(
        long,
        value_name = "日期",
        conflicts_with_all = ["from", "to"],
        help_heading = "选项"
    )]
    pub date: Option<String>,

    /// 自定义开始时间，包含该时间点；必须和 --to 一起使用。
    #[arg(
        long,
        value_name = "时间",
        requires = "to",
        conflicts_with = "date",
        help_heading = "选项"
    )]
    pub from: Option<String>,

    /// 自定义结束时间，不包含该时间点；必须和 --from 一起使用。
    #[arg(
        long,
        value_name = "时间",
        requires = "from",
        conflicts_with = "date",
        help_heading = "选项"
    )]
    pub to: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct QueryArgs {
    #[command(flatten)]
    pub range: TimeRangeArgs,

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

#[derive(Debug, Clone, Args)]
pub struct AppsArgs {
    #[command(flatten)]
    pub query: QueryArgs,

    /// 显示应用 ID、可执行路径、连接数量和出现时间。
    #[arg(long, help_heading = "选项")]
    pub details: bool,
}

#[derive(Debug, Clone, Args)]
pub struct AppArgs {
    /// 应用显示名称、完整应用 ID 或无歧义的名称片段。
    #[arg(value_name = "应用", help_heading = "参数")]
    pub selector: String,

    #[command(flatten)]
    pub range: TimeRangeArgs,

    /// 输出 JSON，字段名保持英文。
    #[arg(long, help_heading = "选项")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ExplainArgs {
    #[command(flatten)]
    pub range: TimeRangeArgs,

    /// 分析包含该时间点的应用明细时段；支持本地时间、Unix 时间戳和 RFC 3339。
    #[arg(
        long,
        value_name = "时间",
        conflicts_with_all = ["period", "date", "from", "to"],
        help_heading = "选项"
    )]
    pub at: Option<String>,

    /// 最多显示多少个主要应用；默认 5 个。
    #[arg(
        long,
        value_name = "数量",
        default_value_t = 5,
        hide_default_value = true,
        value_parser = parse_explain_limit,
        help_heading = "选项"
    )]
    pub limit: usize,

    /// 输出 JSON，字段名保持英文。
    #[arg(long, help_heading = "选项")]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ReportArgs {
    #[command(flatten)]
    pub range: TimeRangeArgs,

    /// 与紧邻的上一段等长时间比较。
    #[arg(long, help_heading = "选项")]
    pub compare: bool,

    /// 最多显示多少个主要应用；默认 5 个。
    #[arg(
        long,
        value_name = "数量",
        default_value_t = 5,
        hide_default_value = true,
        value_parser = parse_explain_limit,
        help_heading = "选项"
    )]
    pub limit: usize,

    /// 输出 JSON，字段名保持英文。
    #[arg(long, help_heading = "选项")]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct InvestigateArgs {
    #[command(subcommand)]
    pub command: InvestigateCommand,
}

#[derive(Debug, Subcommand)]
pub enum InvestigateCommand {
    /// 启动有明确结束时间的高精度调查。
    Start {
        /// 调查持续时间，例如 30m、1h 或 6h。
        #[arg(
            long,
            value_name = "时长",
            default_value = "30m",
            hide_default_value = true,
            value_parser = parse_investigation_duration,
            help_heading = "选项"
        )]
        duration: u64,
    },
    /// 查看调查模式是否运行以及剩余时间。
    Status,
    /// 提前停止调查并恢复原设置。
    Stop,
}

#[derive(Debug, Args)]
pub struct AlertsArgs {
    #[command(subcommand)]
    pub command: AlertsCommand,
}

#[derive(Debug, Subcommand)]
pub enum AlertsCommand {
    /// 新增每日或每月流量限额。
    Add {
        /// 每日限额，例如 10GiB；不能与 --monthly 同时使用。
        #[arg(
            long,
            value_name = "容量",
            value_parser = parse_byte_size,
            required_unless_present = "monthly",
            conflicts_with = "monthly",
            help_heading = "选项"
        )]
        daily: Option<u64>,
        /// 每月限额，例如 100GiB；不能与 --daily 同时使用。
        #[arg(
            long,
            value_name = "容量",
            value_parser = parse_byte_size,
            help_heading = "选项"
        )]
        monthly: Option<u64>,
        /// 只统计指定应用已识别到的流量。
        #[arg(long, value_name = "应用", help_heading = "选项")]
        app: Option<String>,
    },
    /// 查看全部提醒规则。
    List,
    /// 暂停一条提醒规则，但保留设置。
    Disable {
        #[arg(value_name = "编号", help_heading = "参数")]
        id: i64,
    },
    /// 恢复一条已暂停的提醒规则。
    Enable {
        #[arg(value_name = "编号", help_heading = "参数")]
        id: i64,
    },
    /// 永久删除一条提醒规则。
    Remove {
        #[arg(value_name = "编号", help_heading = "参数")]
        id: i64,
    },
    /// 立即发送一条测试通知。
    Test,
}

#[derive(Debug, Clone, Args)]
pub struct ChartArgs {
    #[command(flatten)]
    pub range: TimeRangeArgs,

    /// 只绘制指定应用已识别到的流量；可使用显示名称或完整应用 ID。
    #[arg(long, value_name = "应用", help_heading = "选项")]
    pub app: Option<String>,

    /// 每个点代表的时间；默认自动。可用 1m、5m、10m、15m、30m、1h、3h、6h、12h、1d、7d、30d、90d 或 365d。
    #[arg(
        long,
        value_enum,
        value_name = "间隔",
        default_value_t = ChartInterval::Auto,
        hide_default_value = true,
        hide_possible_values = true,
        help_heading = "选项"
    )]
    pub interval: ChartInterval,

    /// 图表高度；默认 12 行，可设置 6 到 30 行。
    #[arg(
        long,
        value_name = "行数",
        default_value_t = 12,
        hide_default_value = true,
        value_parser = parse_chart_height,
        help_heading = "选项"
    )]
    pub height: usize,

    /// 图表总宽度；默认跟随终端，可设置 50 到 240 列。
    #[arg(
        long,
        value_name = "列数",
        value_parser = parse_chart_width,
        help_heading = "选项"
    )]
    pub width: Option<usize>,

    /// 不使用 ANSI 颜色，适合保存到文件或不支持颜色的终端。
    #[arg(long, help_heading = "选项")]
    pub no_color: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ChartInterval {
    Auto,
    #[value(name = "1m")]
    OneMinute,
    #[value(name = "5m")]
    FiveMinutes,
    #[value(name = "10m")]
    TenMinutes,
    #[value(name = "15m")]
    FifteenMinutes,
    #[value(name = "30m")]
    ThirtyMinutes,
    #[value(name = "1h")]
    OneHour,
    #[value(name = "3h")]
    ThreeHours,
    #[value(name = "6h")]
    SixHours,
    #[value(name = "12h")]
    TwelveHours,
    #[value(name = "1d")]
    OneDay,
    #[value(name = "7d")]
    SevenDays,
    #[value(name = "30d")]
    ThirtyDays,
    #[value(name = "90d")]
    NinetyDays,
    #[value(name = "365d")]
    OneYear,
}

impl ChartInterval {
    pub const fn seconds(self) -> Option<i64> {
        match self {
            Self::Auto => None,
            Self::OneMinute => Some(60),
            Self::FiveMinutes => Some(300),
            Self::TenMinutes => Some(600),
            Self::FifteenMinutes => Some(900),
            Self::ThirtyMinutes => Some(1_800),
            Self::OneHour => Some(3_600),
            Self::ThreeHours => Some(10_800),
            Self::SixHours => Some(21_600),
            Self::TwelveHours => Some(43_200),
            Self::OneDay => Some(86_400),
            Self::SevenDays => Some(604_800),
            Self::ThirtyDays => Some(2_592_000),
            Self::NinetyDays => Some(7_776_000),
            Self::OneYear => Some(31_536_000),
        }
    }
}

fn parse_chart_height(raw: &str) -> Result<usize, String> {
    parse_bounded_usize(raw, 6, 30, "图表高度")
}

fn parse_chart_width(raw: &str) -> Result<usize, String> {
    parse_bounded_usize(raw, 50, 240, "图表宽度")
}

fn parse_explain_limit(raw: &str) -> Result<usize, String> {
    parse_bounded_usize(raw, 1, 50, "应用数量")
}

fn parse_investigation_duration(raw: &str) -> Result<u64, String> {
    let value = raw.trim().to_lowercase();
    let (number, multiplier) = if let Some(number) = value.strip_suffix('m') {
        (number, 60u64)
    } else if let Some(number) = value.strip_suffix('h') {
        (number, 3_600u64)
    } else {
        return Err("调查时长必须使用 m（分钟）或 h（小时），例如 30m 或 2h".to_string());
    };
    let number = number
        .parse::<u64>()
        .map_err(|_| "调查时长必须是整数，例如 30m 或 2h".to_string())?;
    let seconds = number
        .checked_mul(multiplier)
        .ok_or_else(|| "调查时长过大".to_string())?;
    if !(300..=86_400).contains(&seconds) {
        return Err("调查时长必须在 5 分钟到 24 小时之间".to_string());
    }
    Ok(seconds)
}

fn parse_byte_size(raw: &str) -> Result<u64, String> {
    let value = raw.trim();
    if value.is_empty() {
        return Err("容量不能为空，例如 10GiB".to_string());
    }
    let split = value
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    if number.is_empty() || number.matches('.').count() > 1 {
        return Err("容量格式无效，例如 500MiB 或 10GiB".to_string());
    }
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "b" => 1u128,
        "kib" => 1u128 << 10,
        "mib" => 1u128 << 20,
        "gib" => 1u128 << 30,
        "tib" => 1u128 << 40,
        "kb" => 1_000u128,
        "mb" => 1_000_000u128,
        "gb" => 1_000_000_000u128,
        "tb" => 1_000_000_000_000u128,
        _ => return Err("容量单位必须是 B、KiB、MiB、GiB、TiB、KB、MB、GB 或 TB".to_string()),
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let whole = whole
        .parse::<u128>()
        .map_err(|_| "容量数值无效".to_string())?;
    let scale = 10u128
        .checked_pow(fraction.len() as u32)
        .ok_or_else(|| "容量小数位过多".to_string())?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .map_err(|_| "容量数值无效".to_string())?
    };
    let scaled = whole
        .checked_mul(scale)
        .and_then(|value| value.checked_add(fraction))
        .and_then(|value| value.checked_mul(multiplier))
        .ok_or_else(|| "容量过大".to_string())?;
    let bytes = scaled / scale;
    if bytes == 0 {
        return Err("容量必须大于 0 B".to_string());
    }
    if bytes > i64::MAX as u128 {
        return Err("容量过大".to_string());
    }
    Ok(bytes as u64)
}

fn parse_bounded_usize(
    raw: &str,
    minimum: usize,
    maximum: usize,
    label: &str,
) -> Result<usize, String> {
    let value = raw
        .parse::<usize>()
        .map_err(|_| format!("{label}必须是整数"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{label}必须在 {minimum} 到 {maximum} 之间"));
    }
    Ok(value)
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
    /// 管理应用的自定义显示名称。
    #[command(after_help = APP_NAMES_AFTER_HELP)]
    AppNames(AppNamesArgs),
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

#[derive(Debug, Args)]
pub struct AppNamesArgs {
    #[command(subcommand)]
    pub command: AppNamesCommand,
}

#[derive(Debug, Subcommand)]
pub enum AppNamesCommand {
    /// 查看全部自定义应用名称。
    List,
    /// 设置或覆盖一个应用的显示名称。
    Set {
        #[arg(value_name = "应用ID", help_heading = "参数")]
        app_id: String,
        #[arg(value_name = "名称", help_heading = "参数")]
        display_name: String,
    },
    /// 删除一个应用的自定义名称。
    Remove {
        #[arg(value_name = "应用ID", help_heading = "参数")]
        app_id: String,
    },
}

#[derive(Debug, Args)]
pub struct DataArgs {
    #[command(subcommand)]
    pub command: DataCommand,
}

#[derive(Debug, Subcommand)]
pub enum DataCommand {
    /// 查看数据库位置、大小、保留期限和记录范围。
    Info,
    /// 将所选范围导出为 CSV 或 JSON 文件。
    Export {
        #[command(flatten)]
        range: TimeRangeArgs,
        /// 导出格式：csv 或 json。
        #[arg(long, value_enum, value_name = "格式", help_heading = "选项")]
        format: DataFormat,
        /// 输出文件；为避免误操作，不会覆盖已有文件。
        #[arg(long, value_name = "文件", help_heading = "选项")]
        output: PathBuf,
    },
    /// 修改明细和每日汇总的保存时间，并立即应用。
    Retention {
        /// 明细保存时间，例如 30d；范围 1d 到 365d。
        #[arg(long, value_name = "天数", value_parser = parse_detail_retention, help_heading = "选项")]
        details: Option<i64>,
        /// 每日汇总保存时间，例如 365d；范围 7d 到 3650d。
        #[arg(long, value_name = "天数", value_parser = parse_daily_retention, help_heading = "选项")]
        daily: Option<i64>,
    },
    /// 永久删除指定日期以前的流量记录。
    Prune {
        /// 删除这个本地自然日以前的记录，格式为 YYYY-MM-DD。
        #[arg(long, value_name = "日期", help_heading = "选项")]
        before: String,
        /// 确认永久删除；缺少时只显示将要执行的操作。
        #[arg(long, help_heading = "选项")]
        confirm: bool,
    },
    /// 回收已删除记录占用的空间并优化数据库。
    Compact,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DataFormat {
    Csv,
    Json,
}

fn parse_detail_retention(raw: &str) -> Result<i64, String> {
    parse_retention_days(raw, 1, 365, "明细保存时间")
}

fn parse_daily_retention(raw: &str) -> Result<i64, String> {
    parse_retention_days(raw, 7, 3_650, "每日汇总保存时间")
}

fn parse_retention_days(raw: &str, minimum: i64, maximum: i64, label: &str) -> Result<i64, String> {
    let value = raw
        .trim()
        .to_ascii_lowercase()
        .strip_suffix('d')
        .ok_or_else(|| format!("{label}必须使用 d（天），例如 30d"))?
        .parse::<i64>()
        .map_err(|_| format!("{label}必须是整数天数，例如 30d"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{label}必须在 {minimum}d 到 {maximum}d 之间"));
    }
    Ok(value)
}
