use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "flowwatch",
    version,
    about = "Lightweight, auditable per-app network accounting"
)]
pub struct Cli {
    #[arg(long, global = true, value_name = "PATH")]
    pub database: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the background collector.
    Collect(CollectArgs),
    /// Show collector health and attribution coverage.
    Status,
    /// Rank applications by upload or download usage.
    Apps(QueryArgs),
    /// Show authoritative physical-interface totals.
    Interfaces(QueryArgs),
    /// Show the highest-traffic physical minutes.
    Spikes(QueryArgs),
    /// Rank time buckets by unattributed physical traffic.
    Gaps(QueryArgs),
    /// Check collectors and storage without changing settings.
    Doctor,
    /// Install and start the per-user LaunchAgent.
    Install(InstallArgs),
    /// Remove the LaunchAgent and installed binary.
    Uninstall(UninstallArgs),
    /// Manage optional traffic providers.
    Config(ConfigArgs),
}

#[derive(Debug, Args)]
pub struct CollectArgs {
    /// Stop after this many seconds; zero runs until signalled.
    #[arg(long, default_value_t = 0)]
    pub run_seconds: u64,
}

#[derive(Debug, Clone, Args)]
pub struct QueryArgs {
    /// Named range: today, yesterday, 24h, 7d, 30d, or all.
    #[arg(
        long,
        default_value = "today",
        conflicts_with_all = ["from", "to"]
    )]
    pub period: String,

    /// Exact local start time, Unix timestamp, or RFC 3339 time (inclusive).
    #[arg(long, value_name = "DATETIME", requires = "to")]
    pub from: Option<String>,

    /// Exact local end time, Unix timestamp, or RFC 3339 time (exclusive).
    #[arg(long, value_name = "DATETIME", requires = "from")]
    pub to: Option<String>,

    #[arg(long, value_enum, default_value_t = SortBy::Total)]
    pub sort: SortBy,

    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    #[arg(long)]
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
    /// Override the poll interval (3 seconds on first install).
    #[arg(long)]
    pub poll_seconds: Option<u64>,

    /// Override the database flush interval (60 seconds on first install).
    #[arg(long)]
    pub flush_seconds: Option<u64>,

    /// Override detailed-data retention (30 days on first install).
    #[arg(long)]
    pub detail_days: Option<i64>,

    /// Override daily-data retention (365 days on first install).
    #[arg(long)]
    pub daily_days: Option<i64>,

    /// Override application detail: 5m is the first-install default, 1m is finer.
    #[arg(long, value_enum)]
    pub app_granularity: Option<AppGranularity>,

    /// Import a Clash/Mihomo config.yaml during installation.
    #[arg(long, value_name = "PATH")]
    pub clash_config: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct UninstallArgs {
    #[arg(long)]
    pub purge_data: bool,
}

#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Import controller address and secret from a Mihomo config.yaml.
    ImportClash { path: PathBuf },
    /// Disable the Clash provider without deleting its configuration.
    DisableClash,
    /// Change application detail granularity and restart the collector.
    SetAppGranularity { granularity: AppGranularity },
    /// Display configuration with credentials redacted.
    Show,
}
