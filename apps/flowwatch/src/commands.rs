use crate::clash_config::read_clash_config;
use crate::cli::{
    AppGranularity, Cli, Command as CliCommand, ConfigCommand, InstallArgs, QueryArgs, SortBy,
};
use crate::collector::{Collector, RuntimeSettings, acquire_lock};
use crate::paths::{AGENT_LABEL, AppPaths};
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, LocalResult, NaiveDateTime, TimeZone};
use flowwatch_clash::ClashSampler;
use flowwatch_core::{AppIdentity, TrafficBackend, UNKNOWN};
use flowwatch_macos::MacOsBackend;
use flowwatch_store::{
    AppUsage, AttributionGap, Database, InterfaceUsage, SpikeUsage, day_bucket, minute_bucket,
};
use plist::{Dictionary, Value};
use serde::Serialize;
use signal_hook::consts::{SIGINT, SIGTERM};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use unicode_width::UnicodeWidthStr;

const LAUNCHCTL: &str = "/bin/launchctl";

pub fn dispatch(cli: Cli) -> Result<()> {
    let paths = AppPaths::discover(cli.database)?;
    match cli.command {
        CliCommand::Collect(args) => collect(&paths, args.run_seconds),
        CliCommand::Status => status(&paths),
        CliCommand::Apps(args) => apps(&paths, args),
        CliCommand::Interfaces(args) => interfaces(&paths, args),
        CliCommand::Spikes(args) => spikes(&paths, args),
        CliCommand::Gaps(args) => gaps(&paths, args),
        CliCommand::Doctor => doctor(&paths),
        CliCommand::Install(args) => install(&paths, args),
        CliCommand::Uninstall(args) => uninstall(&paths, args.purge_data),
        CliCommand::Config(args) => configure(&paths, args.command),
    }
}

fn collect(paths: &AppPaths, run_seconds: u64) -> Result<()> {
    let _lock = acquire_lock(&paths.lock_file)?;
    let database = Database::open(&paths.database)?;
    let settings = RuntimeSettings::load(&database)?;
    let stop = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGINT, Arc::clone(&stop))?;
    signal_hook::flag::register(SIGTERM, Arc::clone(&stop))?;
    Collector::new(database, settings)?.run(run_seconds, stop)
}

fn status(paths: &AppPaths) -> Result<()> {
    let database = Database::open(&paths.database)?;
    let meta = database.meta()?;
    let now = Local::now().timestamp();
    let start = day_bucket(now);
    let attribution_start = attribution_window_start(&meta, start);
    let apps = database.query_apps(attribution_start, now + 1)?;
    let interfaces = database.query_interfaces(start, now + 1)?;
    let attribution_interfaces = database.query_interfaces(attribution_start, now + 1)?;
    let proxy = database.proxy_totals(attribution_start, now + 1)?;
    let clash_actor_start = meta
        .get("clash_actor_started_at")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(now.saturating_add(1))
        .max(attribution_start);
    let classified_proxy = if clash_actor_start <= now {
        Some(database.proxy_totals(clash_actor_start, now + 1)?)
    } else {
        None
    };
    let anomalies = database.direct_attribution_anomalies(attribution_start, now + 1)?;
    let physical = sum_interfaces(&interfaces);
    let attribution_physical = sum_interfaces(&attribution_interfaces);
    let attributed =
        apps.iter()
            .filter(|usage| usage.app.is_known())
            .fold((0u64, 0u64), |total, app| {
                (
                    total.0.saturating_add(app.upload()),
                    total.1.saturating_add(app.download()),
                )
            });
    let unknown_clash =
        apps.iter()
            .filter(|usage| !usage.app.is_known())
            .fold((0u64, 0u64), |total, app| {
                (
                    total.0.saturating_add(app.clash_upload),
                    total.1.saturating_add(app.clash_download),
                )
            });
    let pid = meta
        .get("collector_pid")
        .and_then(|value| value.parse().ok());
    let running = pid.is_some_and(process_is_running);

    println!("FlowWatch 状态");
    println!(
        "  采集服务：{}{}",
        if running { "运行中" } else { "未运行" },
        pid.map_or_else(String::new, |pid| format!("（进程号 {pid}）"))
    );
    println!(
        "  最近保存：{}",
        meta_timestamp(&meta, "last_flush_at", now)
    );
    println!(
        "  数据库：{}（{}）",
        paths.database.display(),
        human_bytes(database.size_bytes())
    );
    let integrity = database.integrity_check()?;
    println!(
        "  数据库检查：{}",
        if integrity == "ok" {
            "正常"
        } else {
            &integrity
        }
    );
    if let Some(engine) = meta.get("collector_engine") {
        println!("  采集方式：{}", collector_engine_label(engine));
    }
    println!(
        "  应用明细：{}",
        app_granularity_label(app_bucket_seconds(&database)?)
    );
    println!();
    println!("今天");
    println!(
        "  实际总量：上传 {}  下载 {}",
        human_bytes(physical.0),
        human_bytes(physical.1)
    );
    if attribution_start > start {
        println!(
            "  应用识别从 {} 开始统计",
            format_timestamp(attribution_start)
        );
    }
    println!(
        "  已识别应用：上传 {}  下载 {}（{}）",
        human_bytes(attributed.0),
        human_bytes(attributed.1),
        coverage(attributed, attribution_physical)
    );
    if proxy.upload > 0 || proxy.download > 0 {
        let unattributed = (
            proxy.upload.saturating_sub(proxy.attributed_upload),
            proxy.download.saturating_sub(proxy.attributed_download),
        );
        println!("  Clash 流量：");
        println!(
            "    总量：上传 {}  下载 {}",
            human_bytes(proxy.upload),
            human_bytes(proxy.download),
        );
        println!(
            "    已识别应用：上传 {}  下载 {}",
            human_bytes(proxy.attributed_upload),
            human_bytes(proxy.attributed_download),
        );
        println!(
            "    未识别：上传 {}  下载 {}",
            human_bytes(unattributed.0),
            human_bytes(unattributed.1),
        );
        if unknown_clash.0 > 0 || unknown_clash.1 > 0 {
            println!(
                "    未知应用：上传 {}  下载 {}（已计入未识别）",
                human_bytes(unknown_clash.0),
                human_bytes(unknown_clash.1),
            );
        }
        println!(
            "    应用识别：{}",
            coverage(
                (proxy.attributed_upload, proxy.attributed_download),
                (proxy.upload, proxy.download),
            )
        );
        match classified_proxy.filter(|value| value.actor_bytes_known) {
            Some(classified) => {
                let non_actor = (
                    classified.upload.saturating_sub(classified.actor_upload),
                    classified
                        .download
                        .saturating_sub(classified.actor_download),
                );
                println!(
                    "    详细分类从 {} 开始统计：",
                    format_timestamp(clash_actor_start)
                );
                println!(
                    "      已观察到来源的连接：上传 {}  下载 {}",
                    human_bytes(classified.actor_upload),
                    human_bytes(classified.actor_download),
                );
                println!(
                    "      已识别到应用的连接：上传 {}  下载 {}（{}）",
                    human_bytes(classified.attributed_upload),
                    human_bytes(classified.attributed_download),
                    coverage(
                        (classified.attributed_upload, classified.attributed_download),
                        (classified.actor_upload, classified.actor_download),
                    )
                );
                println!(
                    "      内部或未观察到的连接：上传 {}  下载 {}",
                    human_bytes(non_actor.0),
                    human_bytes(non_actor.1),
                );
            }
            _ => println!("    详细分类将在采满一个完整分钟后显示。"),
        }
        println!("    说明：很短的连接可能在两次采样之间结束，因此只能计入总量。");
    }
    let active_clash = meta_number(&meta, "active_clash_flows");
    let actor_clash = meta_number(&meta, "actor_clash_flows");
    let identifiable_clash = meta_number(&meta, "identifiable_clash_flows");
    let metadata_clash = meta_number(&meta, "metadata_identifiable_clash_flows");
    let fallback_clash = meta_number(&meta, "fallback_identifiable_clash_flows");
    if active_clash > 0 {
        println!(
            "  Clash 连接：活跃 {active_clash} 个，带来源信息 {actor_clash} 个，已识别应用 {identifiable_clash} 个"
        );
        if fallback_clash > 0 {
            println!(
                "              其中控制器提供 {metadata_clash} 个，本机连接匹配 {fallback_clash} 个"
            );
        }
    }
    if !anomalies.is_empty() {
        println!(
            "  数据警告：有 {} 个已完成的五分钟直连记录超过网卡实际流量",
            anomalies.len()
        );
    }
    print_collector_errors(&meta);
    if !running {
        println!();
        println!("下一步：运行 flowwatch doctor 查看原因，或运行 flowwatch install 启动服务。");
    }
    Ok(())
}

fn apps(paths: &AppPaths, args: QueryArgs) -> Result<()> {
    validate_query_args(&args)?;
    let database = Database::open(&paths.database)?;
    let mut range = parse_query_range(&args)?;
    let meta = database.meta()?;
    let requested_start = range.start;
    let has_attribution_data = apply_attribution_window(&mut range, &meta);
    let rows = if has_attribution_data {
        database.query_apps(range.start, range.end)?
    } else {
        Vec::new()
    };
    let mut rows = Database::group_apps_for_display(rows);
    sort_apps(&mut rows, args.sort);
    rows.truncate(args.limit);

    if args.json {
        let output: Vec<AppOutput<'_>> = rows.iter().map(AppOutput::from).collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }
    println!("应用流量（{}）", range.label);
    if range.start > requested_start {
        println!(
            "应用流量记录从 {} 开始；所选范围中更早的流量不计入本表。",
            format_timestamp(range.start)
        );
    }
    if range.exact {
        println!(
            "统计精度：当前{}保存一次；较早的记录可能每五分钟保存一次。",
            app_granularity_label(app_bucket_seconds(&database)?)
        );
    }
    println!(
        "{} {} {} {}  应用",
        table_left("序号", 4),
        table_right("上传", 11),
        table_right("下载", 11),
        table_right("合计", 11),
    );
    for (index, row) in rows.iter().enumerate() {
        let identity_summary = if row.identity_count > 1 || row.executable_paths.len() > 1 {
            format!(
                "（合并显示 {} 个同名程序，来自 {} 个路径）",
                row.identity_count,
                row.executable_paths.len(),
            )
        } else {
            String::new()
        };
        let rank = (index + 1).to_string();
        let upload = human_bytes(row.upload());
        let download = human_bytes(row.download());
        let total = human_bytes(row.upload().saturating_add(row.download()));
        println!(
            "{} {} {} {}  {}{} [{}]",
            table_left(&rank, 4),
            table_right(&upload, 11),
            table_right(&download, 11),
            table_right(&total, 11),
            display_app_name(&row.app.name),
            identity_summary,
            sources(row),
        );
    }
    if rows.is_empty() {
        println!("所选时间内没有应用流量记录。");
        println!("可运行 flowwatch status 确认采集服务和数据更新时间。");
    }
    Ok(())
}

fn interfaces(paths: &AppPaths, args: QueryArgs) -> Result<()> {
    validate_query_args(&args)?;
    let database = Database::open(&paths.database)?;
    let range = parse_query_range(&args)?;
    let mut rows = database.query_interfaces(range.start, range.end)?;
    sort_interfaces(&mut rows, args.sort);
    rows.truncate(args.limit);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!("网卡实际流量（{}）", range.label);
    if range.exact {
        println!("统计精度：显示与所选范围有重叠的每分钟记录。");
    }
    println!(
        "{} {} {} {}",
        table_left("网卡", 12),
        table_right("上传", 12),
        table_right("下载", 12),
        table_right("合计", 12),
    );
    for row in &rows {
        let upload = human_bytes(row.upload);
        let download = human_bytes(row.download);
        let total = human_bytes(row.upload.saturating_add(row.download));
        println!(
            "{} {} {} {}",
            table_left(&row.interface, 12),
            table_right(&upload, 12),
            table_right(&download, 12),
            table_right(&total, 12),
        );
    }
    if rows.is_empty() {
        println!("所选时间内没有网卡流量记录。");
    }
    Ok(())
}

fn spikes(paths: &AppPaths, args: QueryArgs) -> Result<()> {
    validate_query_args(&args)?;
    let database = Database::open(&paths.database)?;
    let range = parse_query_range(&args)?;
    let mut rows = database.query_spikes(range.start, range.end)?;
    sort_spikes(&mut rows, args.sort);
    rows.truncate(args.limit);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!("流量最高的分钟（{}）", range.label);
    if range.exact {
        println!("统计精度：显示与所选范围有重叠的每分钟记录。");
    }
    println!(
        "{} {} {} {}",
        table_left("时间", 20),
        table_right("上传", 12),
        table_right("下载", 12),
        table_right("合计", 12),
    );
    for row in &rows {
        let timestamp = format_timestamp(row.bucket);
        let upload = human_bytes(row.upload);
        let download = human_bytes(row.download);
        let total = human_bytes(row.upload.saturating_add(row.download));
        println!(
            "{} {} {} {}",
            table_left(&timestamp, 20),
            table_right(&upload, 12),
            table_right(&download, 12),
            table_right(&total, 12),
        );
    }
    if rows.is_empty() {
        println!("所选时间内没有分钟记录；分钟明细只保留有限天数。");
    }
    Ok(())
}

fn gaps(paths: &AppPaths, args: QueryArgs) -> Result<()> {
    validate_query_args(&args)?;
    let database = Database::open(&paths.database)?;
    let mut range = parse_query_range(&args)?;
    let meta = database.meta()?;
    let requested_start = range.start;
    let has_attribution_data = apply_attribution_window(&mut range, &meta);
    let bucket_seconds = gap_bucket_seconds(&database, &meta, range.start)?;
    let mut rows = if has_attribution_data {
        database.query_attribution_gaps(range.start, range.end, bucket_seconds)?
    } else {
        Vec::new()
    };
    sort_gaps(&mut rows, args.sort);
    rows.truncate(args.limit);

    if args.json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "未识别流量（{}；{}一组）",
        range.label,
        app_granularity_label(bucket_seconds)
    );
    if range.start > requested_start {
        println!(
            "应用流量记录从 {} 开始；所选范围中更早的流量不计入本表。",
            format_timestamp(range.start)
        );
    }
    println!(
        "{} {} {} {} {}",
        table_left("时间段", 20),
        table_right("实际总量", 12),
        table_right("已识别", 12),
        table_right("未识别", 12),
        table_right("Clash未识别", 12),
    );
    for row in &rows {
        let physical = row.physical_upload.saturating_add(row.physical_download);
        let attributed = row
            .attributed_upload
            .saturating_add(row.attributed_download);
        let gap = row.gap_upload.saturating_add(row.gap_download);
        let clash_gap = row
            .clash_upload
            .saturating_add(row.clash_download)
            .saturating_sub(
                row.clash_attributed_upload
                    .saturating_add(row.clash_attributed_download),
            );
        let timestamp = format_timestamp(row.bucket);
        let physical = human_bytes(physical);
        let attributed = human_bytes(attributed);
        let gap = human_bytes(gap);
        let clash_gap = human_bytes(clash_gap);
        println!(
            "{} {} {} {} {}",
            table_left(&timestamp, 20),
            table_right(&physical, 12),
            table_right(&attributed, 12),
            table_right(&gap, 12),
            table_right(&clash_gap, 12),
        );
    }
    if rows.is_empty() {
        println!("所选时间内没有可用于比较的明细记录。");
    }
    Ok(())
}

fn doctor(paths: &AppPaths) -> Result<()> {
    let mut failures = Vec::new();
    println!("FlowWatch 检查");
    let mut backend = MacOsBackend::with_poll_seconds(1);
    let process_probe = backend.process_traffic();
    let socket_owners: HashMap<_, _> = process_probe
        .as_ref()
        .map(|sample| {
            sample
                .socket_owners
                .iter()
                .map(|owner| (owner.endpoint.clone(), owner.app.clone()))
                .collect()
        })
        .unwrap_or_default();

    match Database::open(&paths.database).and_then(|database| {
        let integrity = database.integrity_check()?;
        if integrity != "ok" {
            bail!("数据库完整性检查返回异常结果：{integrity}");
        }
        Ok((database, integrity))
    }) {
        Ok((database, _integrity)) => {
            println!("  [正常] SQLite 数据库完整性");
            check_database_permissions(database.path());
            let now = Local::now().timestamp();
            let anomalies = database.direct_attribution_anomalies(now - 86_400, now + 1)?;
            if anomalies.is_empty() {
                println!("  [正常] 应用流量：完整的五分钟记录未超过网卡实际总量");
            } else {
                println!(
                    "  [警告] 应用流量：有 {} 个完整的五分钟记录超过网卡实际总量",
                    anomalies.len()
                );
            }
            if let Some(config) = database.clash_config()?.filter(|config| config.enabled) {
                let mut sampler = ClashSampler::new(config, None);
                match sampler.sample(Local::now().timestamp(), |name, path, endpoint| {
                    if !name.trim().is_empty() || !path.trim().is_empty() {
                        backend.resolve_external_identity(name, path)
                    } else {
                        endpoint
                            .and_then(|value| socket_owners.get(value))
                            .cloned()
                            .unwrap_or_else(|| AppIdentity::process(UNKNOWN, ""))
                    }
                }) {
                    Ok(sample)
                        if sample.active_connections > 0
                            && sample.identifiable_connections == 0 =>
                    {
                        println!(
                            "  [警告] Clash 控制器：有 {} 个活跃连接，但没有识别到应用",
                            sample.active_connections
                        );
                        println!("           建议在 Mihomo 中设置 find-process-mode: strict。");
                    }
                    Ok(sample) => {
                        println!(
                            "  [正常] Clash 控制器：活跃 {} 个，带来源信息 {} 个，已识别应用 {} 个",
                            sample.active_connections,
                            sample.actor_connections,
                            sample.identifiable_connections
                        );
                        if sample.fallback_identifiable_connections > 0 {
                            println!(
                                "           其中控制器提供 {} 个，本机连接匹配 {} 个",
                                sample.metadata_identifiable_connections,
                                sample.fallback_identifiable_connections
                            );
                        }
                    }
                    Err(error) => println!("  [警告] Clash 控制器：{error}"),
                }
            } else {
                println!("  [跳过] Clash 数据来源未启用");
            }
        }
        Err(error) => {
            println!("  [失败] SQLite 数据库：{error:#}");
            failures.push("SQLite 数据库");
        }
    }

    match backend.interface_counters() {
        Ok(counters) if !counters.is_empty() => {
            println!("  [正常] 网卡计数：检测到 {} 个物理网卡", counters.len())
        }
        Ok(_) => {
            println!("  [失败] 网卡计数：没有检测到物理网卡");
            failures.push("网卡计数");
        }
        Err(error) => {
            println!("  [失败] 网卡计数：{error:#}");
            failures.push("网卡计数");
        }
    }
    match process_probe {
        Ok(sample) => println!(
            "  [正常] 应用采样：活跃连接 {} 个，持续跟踪 {} 个，已匹配本机连接 {} 个",
            sample.active_flows,
            sample.tracked_flows,
            sample.socket_owners.len()
        ),
        Err(error) => {
            println!("  [失败] 应用采样：{error:#}");
            failures.push("应用采样");
        }
    }

    if launch_agent_loaded() {
        println!("  [正常] 登录自启服务正在运行");
    } else if paths.launch_agent.exists() {
        println!("  [警告] 登录自启配置存在，但服务未运行");
    } else {
        println!("  [跳过] 未安装登录自启服务");
    }

    if failures.is_empty() {
        println!("所有检查均通过。");
        Ok(())
    } else {
        bail!("检查未通过：{}", failures.join("、"))
    }
}

fn install(paths: &AppPaths, args: InstallArgs) -> Result<()> {
    let mut database = Database::open(&paths.database)?;
    let previous_granularity =
        resolve_app_granularity(None, database.setting("app_granularity")?.as_deref())?;
    let app_granularity = args.app_granularity.unwrap_or(previous_granularity);
    let settings = RuntimeSettings {
        poll_seconds: resolve_numeric_setting(
            args.poll_seconds,
            database.setting("poll_seconds")?.as_deref(),
            3,
            "poll_seconds",
        )?,
        flush_seconds: resolve_numeric_setting(
            args.flush_seconds,
            database.setting("flush_seconds")?.as_deref(),
            60,
            "flush_seconds",
        )?,
        detail_days: resolve_numeric_setting(
            args.detail_days,
            database.setting("detail_days")?.as_deref(),
            30,
            "detail_days",
        )?,
        daily_days: resolve_numeric_setting(
            args.daily_days,
            database.setting("daily_days")?.as_deref(),
            365,
            "daily_days",
        )?,
        app_bucket_seconds: app_granularity.bucket_seconds(),
    };
    settings.validate()?;

    database.set_setting("poll_seconds", &settings.poll_seconds.to_string())?;
    database.set_setting("flush_seconds", &settings.flush_seconds.to_string())?;
    database.set_setting("detail_days", &settings.detail_days.to_string())?;
    database.set_setting("daily_days", &settings.daily_days.to_string())?;
    database.set_setting("app_granularity", app_granularity.setting())?;
    if app_granularity == AppGranularity::OneMinute
        && previous_granularity != AppGranularity::OneMinute
    {
        database.set_meta(&std::collections::BTreeMap::from([(
            "app_one_minute_started_at".to_string(),
            minute_bucket(Local::now().timestamp())
                .saturating_add(60)
                .to_string(),
        )]))?;
    }
    if let Some(path) = args.clash_config {
        let config = read_clash_config(&path)?;
        database.set_clash_config(&config)?;
        println!("已导入 Clash 控制器设置；密钥内容已隐藏。");
    }
    drop(database);

    install_binary(&paths.installed_binary, Some(0o700))?;
    install_binary(&paths.command_binary, Some(0o755))?;
    write_launch_agent(paths)?;
    bootout_launch_agent()?;
    bootstrap_launch_agent(&paths.launch_agent)?;
    println!("FlowWatch 已安装并启动。");
    println!("  程序文件：{}", paths.installed_binary.display());
    println!("  命令路径：{}", paths.command_binary.display());
    println!("  数据库：{}", paths.database.display());
    println!();
    println!("接下来：");
    println!("  flowwatch doctor   检查采集是否正常");
    println!("  flowwatch status   查看今天的流量概况");
    println!("  flowwatch apps     查看今天的应用流量排行");
    Ok(())
}

fn uninstall(paths: &AppPaths, purge_data: bool) -> Result<()> {
    bootout_launch_agent()?;
    remove_file_if_exists(&paths.launch_agent)?;
    remove_file_if_exists(&paths.installed_binary)?;
    remove_file_if_exists(&paths.command_binary)?;
    if purge_data {
        remove_file_if_exists(&paths.database)?;
        remove_file_if_exists(&PathBuf::from(format!("{}-wal", paths.database.display())))?;
        remove_file_if_exists(&PathBuf::from(format!("{}-shm", paths.database.display())))?;
        remove_file_if_exists(&paths.lock_file)?;
        println!("已删除 FlowWatch 服务、程序文件和流量数据库。");
    } else {
        println!("已删除 FlowWatch 服务和程序文件；历史流量数据已保留。");
    }
    Ok(())
}

fn configure(paths: &AppPaths, command: ConfigCommand) -> Result<()> {
    let mut database = Database::open(&paths.database)?;
    match command {
        ConfigCommand::ImportClash { path } => {
            let config = read_clash_config(&path)?;
            database.set_clash_config(&config)?;
            println!("Clash 数据来源已启用；密钥已存入 SQLite，此处不显示内容。");
            if paths.uses_default_database {
                restart_launch_agent()?;
            }
        }
        ConfigCommand::DisableClash => {
            let Some(mut config) = database.clash_config()? else {
                println!("尚未设置 Clash 数据来源。");
                return Ok(());
            };
            config.enabled = false;
            database.set_clash_config(&config)?;
            println!("Clash 数据来源已停用。");
            if paths.uses_default_database {
                restart_launch_agent()?;
            }
        }
        ConfigCommand::SetAppGranularity { granularity } => {
            let previous = database
                .setting("app_granularity")?
                .unwrap_or_else(|| "5m".to_string());
            database.set_setting("app_granularity", granularity.setting())?;
            if granularity.bucket_seconds() == 60 && previous != "1m" {
                database.set_meta(&std::collections::BTreeMap::from([(
                    "app_one_minute_started_at".to_string(),
                    minute_bucket(Local::now().timestamp())
                        .saturating_add(60)
                        .to_string(),
                )]))?;
            }
            println!(
                "应用明细已改为{}保存一次。",
                app_granularity_label(granularity.bucket_seconds())
            );
            if paths.uses_default_database {
                restart_launch_agent()?;
            }
        }
        ConfigCommand::Show => {
            println!("数据库：{}", paths.database.display());
            println!(
                "采样间隔（秒）：{}",
                database.setting("poll_seconds")?.as_deref().unwrap_or("3")
            );
            println!(
                "数据库保存间隔（秒）：{}",
                database
                    .setting("flush_seconds")?
                    .as_deref()
                    .unwrap_or("60")
            );
            println!(
                "明细保留天数：{}",
                database.setting("detail_days")?.as_deref().unwrap_or("30")
            );
            println!(
                "每日汇总保留天数：{}",
                database.setting("daily_days")?.as_deref().unwrap_or("365")
            );
            println!(
                "应用明细：{}",
                app_granularity_label(app_bucket_seconds(&database)?)
            );
            match database.clash_config()? {
                Some(config) => {
                    println!(
                        "Clash 数据来源：{}",
                        if config.enabled {
                            "已启用"
                        } else {
                            "已停用"
                        }
                    );
                    println!("Clash 控制器：{}", config.controller);
                    println!(
                        "Clash 密钥：{}",
                        if config.secret.is_empty() {
                            "未设置"
                        } else {
                            "[已隐藏]"
                        }
                    );
                }
                None => println!("Clash 数据来源：未设置"),
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct AppOutput<'a> {
    id: &'a str,
    name: &'a str,
    executable_path: &'a str,
    identity_count: u32,
    identity_ids: &'a [String],
    executable_paths: &'a [String],
    upload: u64,
    download: u64,
    total: u64,
    direct_upload: u64,
    direct_download: u64,
    clash_upload: u64,
    clash_download: u64,
    enhanced_upload: u64,
    enhanced_download: u64,
    connections: u64,
    last_seen: i64,
}

impl<'a> From<&'a AppUsage> for AppOutput<'a> {
    fn from(value: &'a AppUsage) -> Self {
        let upload = value.upload();
        let download = value.download();
        Self {
            id: &value.app.id,
            name: &value.app.name,
            executable_path: &value.app.executable_path,
            identity_count: value.identity_count,
            identity_ids: &value.identity_ids,
            executable_paths: &value.executable_paths,
            upload,
            download,
            total: upload.saturating_add(download),
            direct_upload: value.direct_upload,
            direct_download: value.direct_download,
            clash_upload: value.clash_upload,
            clash_download: value.clash_download,
            enhanced_upload: value.enhanced_upload,
            enhanced_download: value.enhanced_download,
            connections: value.connections,
            last_seen: value.last_seen,
        }
    }
}

struct Period {
    start: i64,
    end: i64,
    label: String,
    exact: bool,
}

fn parse_period(raw: &str) -> Result<Period> {
    parse_period_at(raw, Local::now().timestamp())
}

fn parse_query_range(args: &QueryArgs) -> Result<Period> {
    match (&args.from, &args.to) {
        (Some(from), Some(to)) => parse_exact_range(from, to),
        (None, None) => parse_period(&args.period),
        _ => bail!("--from 和 --to 必须同时提供"),
    }
}

fn validate_query_args(args: &QueryArgs) -> Result<()> {
    if !(1..=10_000).contains(&args.limit) {
        bail!("--limit 必须在 1 到 10000 之间");
    }
    if args.from.is_some() != args.to.is_some() {
        bail!("--from 和 --to 必须同时提供");
    }
    Ok(())
}

fn parse_exact_range(from: &str, to: &str) -> Result<Period> {
    let start = parse_query_timestamp(from).with_context(|| format!("无法解析 --from {from:?}"))?;
    let end = parse_query_timestamp(to).with_context(|| format!("无法解析 --to {to:?}"))?;
    if start >= end {
        bail!("--from 必须早于 --to");
    }
    Ok(Period {
        start,
        end,
        label: format!(
            "{} 至 {}（不含结束时间）",
            format_timestamp_with_seconds(start),
            format_timestamp_with_seconds(end)
        ),
        exact: true,
    })
}

fn parse_query_timestamp(raw: &str) -> Result<i64> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("时间不能为空");
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        Local
            .timestamp_opt(timestamp, 0)
            .single()
            .context("Unix 时间戳超出支持范围")?;
        return Ok(timestamp);
    }
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Ok(timestamp.timestamp());
    }
    let naive = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .context("请使用本地时间 YYYY-MM-DD HH:MM[:SS]、Unix 时间戳或 RFC 3339 时间")?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(timestamp) => Ok(timestamp.timestamp()),
        LocalResult::Ambiguous(_, _) => {
            bail!("该本地时间因夏令时切换而存在两个可能值，请改用 RFC 3339 时间")
        }
        LocalResult::None => bail!("该本地时间因夏令时切换而不存在，请改用 RFC 3339 时间"),
    }
}

fn parse_period_at(raw: &str, now: i64) -> Result<Period> {
    let normalized = raw.trim().to_ascii_lowercase();
    let (start, label) = match normalized.as_str() {
        "today" | "今天" => (day_bucket(now), "今天".to_string()),
        "yesterday" | "昨天" => {
            let today = day_bucket(now);
            (day_bucket(today - 1), "昨天".to_string())
        }
        "all" | "全部" => (0, "全部已保留记录".to_string()),
        _ => {
            let (number, multiplier, unit) = if let Some(value) = normalized.strip_suffix('h') {
                (value, 3_600i64, "小时")
            } else if let Some(value) = normalized.strip_suffix('d') {
                (value, 86_400i64, "天")
            } else {
                bail!("无法识别时间范围 {raw:?}；请使用 today、yesterday、24h、7d、30d 或 all");
            };
            let amount: i64 = number.parse().context("时间范围中的数字无效")?;
            if !(1..=3_650).contains(&amount) {
                bail!("时间范围中的数字必须在 1 到 3650 之间");
            }
            let seconds = amount.checked_mul(multiplier).context("时间范围过大")?;
            (now.saturating_sub(seconds), format!("最近 {amount} {unit}"))
        }
    };
    let end = if normalized == "yesterday" || normalized == "昨天" {
        day_bucket(now)
    } else {
        now.saturating_add(1)
    };
    Ok(Period {
        start,
        end,
        label,
        exact: false,
    })
}

fn sort_apps(rows: &mut [AppUsage], sort: SortBy) {
    rows.sort_by_key(|row| {
        Reverse(match sort {
            SortBy::Upload => row.upload(),
            SortBy::Download => row.download(),
            SortBy::Total => row.upload().saturating_add(row.download()),
        })
    });
}

fn sort_interfaces(rows: &mut [InterfaceUsage], sort: SortBy) {
    rows.sort_by_key(|row| {
        Reverse(match sort {
            SortBy::Upload => row.upload,
            SortBy::Download => row.download,
            SortBy::Total => row.upload.saturating_add(row.download),
        })
    });
}

fn sort_spikes(rows: &mut [SpikeUsage], sort: SortBy) {
    rows.sort_by_key(|row| {
        Reverse(match sort {
            SortBy::Upload => row.upload,
            SortBy::Download => row.download,
            SortBy::Total => row.upload.saturating_add(row.download),
        })
    });
}

fn sort_gaps(rows: &mut [AttributionGap], sort: SortBy) {
    rows.sort_by_key(|row| {
        Reverse(match sort {
            SortBy::Upload => row.gap_upload,
            SortBy::Download => row.gap_download,
            SortBy::Total => row.gap_upload.saturating_add(row.gap_download),
        })
    });
}

fn sources(row: &AppUsage) -> String {
    let mut values = Vec::new();
    if row.direct_upload > 0 || row.direct_download > 0 {
        values.push("直连");
    }
    if row.clash_upload > 0 || row.clash_download > 0 {
        values.push("Clash");
    }
    if row.enhanced_upload > 0 || row.enhanced_download > 0 {
        values.push("增强模式");
    }
    if values.is_empty() {
        "未知来源".to_string()
    } else {
        values.join("+")
    }
}

fn sum_interfaces(rows: &[InterfaceUsage]) -> (u64, u64) {
    rows.iter().fold((0u64, 0u64), |total, row| {
        (
            total.0.saturating_add(row.upload),
            total.1.saturating_add(row.download),
        )
    })
}

fn coverage(attributed: (u64, u64), total: (u64, u64)) -> String {
    let numerator = attributed.0.saturating_add(attributed.1);
    let denominator = total.0.saturating_add(total.1);
    if denominator == 0 {
        return "识别率暂无数据".to_string();
    }
    format!(
        "识别率 {:.1}%",
        numerator as f64 * 100.0 / denominator as f64
    )
}

fn app_bucket_seconds(database: &Database) -> Result<i64> {
    match database.setting("app_granularity")?.as_deref() {
        None | Some("5m") => Ok(300),
        Some("1m") => Ok(60),
        Some(value) => bail!("app_granularity 设置无效：{value:?}；请使用 1m 或 5m"),
    }
}

fn gap_bucket_seconds(
    database: &Database,
    meta: &std::collections::BTreeMap<String, String>,
    range_start: i64,
) -> Result<i64> {
    if app_bucket_seconds(database)? != 60 {
        return Ok(300);
    }
    let one_minute_start = meta
        .get("app_one_minute_started_at")
        .and_then(|value| value.parse::<i64>().ok());
    Ok(
        if one_minute_start.is_some_and(|start| range_start >= start) {
            60
        } else {
            300
        },
    )
}

fn app_granularity_label(bucket_seconds: i64) -> &'static str {
    if bucket_seconds == 60 {
        "每分钟"
    } else {
        "每五分钟"
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn table_left(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(UnicodeWidthStr::width(value)))
    )
}

fn table_right(value: &str, width: usize) -> String {
    format!(
        "{}{value}",
        " ".repeat(width.saturating_sub(UnicodeWidthStr::width(value)))
    )
}

fn format_timestamp(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn format_timestamp_with_seconds(timestamp: i64) -> String {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| timestamp.to_string())
}

fn meta_timestamp(
    meta: &std::collections::BTreeMap<String, String>,
    key: &str,
    now: i64,
) -> String {
    let Some(timestamp) = meta.get(key).and_then(|value| value.parse::<i64>().ok()) else {
        return "从未保存".to_string();
    };
    let age = now.saturating_sub(timestamp).max(0) as u64;
    format!(
        "{}（{}前）",
        format_timestamp(timestamp),
        human_duration(age)
    )
}

fn meta_number(meta: &std::collections::BTreeMap<String, String>, key: &str) -> u64 {
    meta.get(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

fn attribution_window_start(
    meta: &std::collections::BTreeMap<String, String>,
    day_start: i64,
) -> i64 {
    meta.get("attribution_started_at")
        .and_then(|value| value.parse().ok())
        .unwrap_or(day_start)
        .max(day_start)
}

fn apply_attribution_window(
    range: &mut Period,
    meta: &std::collections::BTreeMap<String, String>,
) -> bool {
    range.start = attribution_window_start(meta, range.start);
    range.start < range.end
}

fn human_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} 秒")
    } else if seconds < 3_600 {
        format!("{} 分钟", seconds / 60)
    } else if seconds < 86_400 {
        format!("{} 小时", seconds / 3_600)
    } else {
        format!("{} 天", seconds / 86_400)
    }
}

fn print_collector_errors(meta: &std::collections::BTreeMap<String, String>) {
    for (label, key) in [
        ("网卡计数", "interface_error"),
        ("应用采样", "process_error"),
        ("Clash 数据来源", "clash_error"),
    ] {
        if let Some(error) = meta.get(key).filter(|error| !error.is_empty()) {
            println!("  警告：{label}：{error}");
        }
    }
}

fn process_is_running(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill with signal zero only checks whether this user's process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn install_binary(destination: &Path, parent_mode: Option<u32>) -> Result<()> {
    let source = std::env::current_exe().context("无法确定当前程序路径")?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
        if let Some(mode) = parent_mode {
            restrict_mode(parent, mode)?;
        }
    }
    if source == destination {
        return Ok(());
    }
    let temporary = destination.with_extension(format!("new.{}", std::process::id()));
    fs::copy(&source, &temporary)
        .with_context(|| format!("无法将 {} 复制到 {}", source.display(), temporary.display()))?;
    restrict_mode(&temporary, 0o755)?;
    fs::rename(&temporary, destination)
        .with_context(|| format!("无法将程序安装到 {}", destination.display()))?;
    Ok(())
}

fn resolve_numeric_setting<T>(
    requested: Option<T>,
    stored: Option<&str>,
    default: T,
    name: &str,
) -> Result<T>
where
    T: Copy + std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match (requested, stored) {
        (Some(value), _) => Ok(value),
        (None, Some(value)) => value
            .parse()
            .map_err(|error| anyhow::anyhow!("已保存的 {name} 设置 {value:?} 无效：{error}")),
        (None, None) => Ok(default),
    }
}

fn resolve_app_granularity(
    requested: Option<AppGranularity>,
    stored: Option<&str>,
) -> Result<AppGranularity> {
    if let Some(value) = requested {
        return Ok(value);
    }
    match stored {
        None | Some("5m") => Ok(AppGranularity::FiveMinutes),
        Some("1m") => Ok(AppGranularity::OneMinute),
        Some(value) => bail!("已保存的 app_granularity 设置 {value:?} 无效；请使用 1m 或 5m"),
    }
}

fn write_launch_agent(paths: &AppPaths) -> Result<()> {
    let parent = paths
        .launch_agent
        .parent()
        .context("LaunchAgents 路径缺少上级目录")?;
    fs::create_dir_all(parent)?;
    let mut dictionary = Dictionary::new();
    dictionary.insert("Label".into(), AGENT_LABEL.into());
    dictionary.insert(
        "ProgramArguments".into(),
        Value::Array(vec![
            path_string(&paths.installed_binary)?.into(),
            "--database".into(),
            path_string(&paths.database)?.into(),
            "collect".into(),
        ]),
    );
    dictionary.insert("RunAtLoad".into(), true.into());
    dictionary.insert("KeepAlive".into(), true.into());
    dictionary.insert("ProcessType".into(), "Background".into());
    dictionary.insert("LowPriorityIO".into(), true.into());
    dictionary.insert("Nice".into(), 10i64.into());
    dictionary.insert("ThrottleInterval".into(), 10i64.into());
    dictionary.insert("StandardOutPath".into(), "/dev/null".into());
    dictionary.insert("StandardErrorPath".into(), "/dev/null".into());

    let temporary = paths
        .launch_agent
        .with_extension(format!("plist.new.{}", std::process::id()));
    Value::Dictionary(dictionary)
        .to_file_xml(&temporary)
        .with_context(|| format!("无法写入 {}", temporary.display()))?;
    restrict_mode(&temporary, 0o600)?;
    fs::rename(&temporary, &paths.launch_agent)
        .with_context(|| format!("无法安装 {}", paths.launch_agent.display()))?;
    Ok(())
}

fn launch_domain() -> String {
    // SAFETY: getuid has no preconditions.
    format!("gui/{}", unsafe { libc::getuid() })
}

fn launch_service() -> String {
    format!("{}/{AGENT_LABEL}", launch_domain())
}

fn launch_agent_loaded() -> bool {
    Command::new(LAUNCHCTL)
        .args(["print", &launch_service()])
        .output()
        .is_ok_and(|output| output.status.success())
}

fn bootout_launch_agent() -> Result<()> {
    if launch_agent_loaded() {
        run_launchctl(&["bootout", &launch_service()])?;
    }
    Ok(())
}

fn restart_launch_agent() -> Result<()> {
    if !launch_agent_loaded() {
        return Ok(());
    }
    run_launchctl(&["kill", "SIGTERM", &launch_service()])?;
    println!("已通知采集服务重启。");
    Ok(())
}

fn bootstrap_launch_agent(path: &Path) -> Result<()> {
    let domain = launch_domain();
    let path = path_string(path)?;
    let mut last_error = None;
    for delay_ms in [100, 250, 500, 1_000, 2_000] {
        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        match run_launchctl(&["bootstrap", &domain, path]) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    let error = last_error.context("未尝试启动 launchctl 服务")?;
    Err(error).context("无法启动 FlowWatch 登录自启服务")
}

fn run_launchctl(args: &[&str]) -> Result<()> {
    let output = Command::new(LAUNCHCTL)
        .args(args)
        .output()
        .context("无法运行 launchctl")?;
    if !output.status.success() {
        bail!(
            "launchctl 执行失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("路径不是有效的 UTF-8：{}", path.display()))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("无法删除 {}", path.display())),
    }
}

#[cfg(unix)]
fn restrict_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(unix)]
fn check_database_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = path.metadata() {
        let mode = metadata.permissions().mode() & 0o777;
        if mode != 0o600 {
            println!("  [警告] 数据库权限为 {mode:o}，应为 600");
        } else {
            println!("  [正常] 数据库权限：600");
        }
    }
}

fn display_app_name(name: &str) -> &str {
    if name == UNKNOWN {
        "未知应用"
    } else {
        name
    }
}

fn collector_engine_label(engine: &str) -> &str {
    match engine {
        "nettop_snapshot_v3" => "nettop 定时采样",
        _ => engine,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn parses_relative_periods() {
        let range = parse_period_at("24h", 1_000_000).unwrap();
        assert_eq!(range.start, 913_600);
        assert_eq!(range.end, 1_000_001);
        assert_eq!(parse_period_at("今天", 1_000_000).unwrap().label, "今天");
        assert_eq!(parse_period_at("昨天", 1_000_000).unwrap().label, "昨天");
        assert_eq!(
            parse_period_at("全部", 1_000_000).unwrap().label,
            "全部已保留记录"
        );
        assert!(parse_period_at("0d", 1_000_000).is_err());
        assert!(parse_period_at("week", 1_000_000).is_err());
    }

    #[test]
    fn parses_exact_local_rfc3339_and_unix_ranges() {
        let range = parse_exact_range("2026-08-18T10:00:00+08:00", "1787018460").unwrap();
        assert_eq!(range.start, 1_787_018_400);
        assert_eq!(range.end, 1_787_018_460);
        assert!(range.exact);
        assert!(parse_exact_range("100", "100").is_err());
        assert!(parse_query_timestamp("2026/08/18 10:00").is_err());

        let local = parse_query_timestamp("2026-08-18 10:00").unwrap();
        assert_eq!(format_timestamp(local), "2026-08-18 10:00");
    }

    #[test]
    fn exact_range_flags_must_be_paired_and_exclude_period() {
        assert!(Cli::try_parse_from(["flowwatch", "apps", "--from", "2026-08-18 10:00",]).is_err());
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "apps",
                "--from",
                "2026-08-18 10:00",
                "--to",
                "2026-08-18 11:00",
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "flowwatch",
                "apps",
                "--period",
                "24h",
                "--from",
                "2026-08-18 10:00",
                "--to",
                "2026-08-18 11:00",
            ])
            .is_err()
        );
    }

    #[test]
    fn formats_byte_units() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1_536), "1.5 KiB");
    }

    #[test]
    fn table_cells_use_terminal_display_width() {
        assert_eq!(UnicodeWidthStr::width(table_left("序号", 4).as_str()), 4);
        assert_eq!(UnicodeWidthStr::width(table_right("上传", 11).as_str()), 11);
        assert_eq!(
            UnicodeWidthStr::width(table_left("FlowWatch", 12).as_str()),
            12
        );
        assert_eq!(
            UnicodeWidthStr::width(table_left("网络测速", 10).as_str()),
            10
        );
    }

    #[test]
    fn reports_coverage_without_hiding_overcount() {
        assert_eq!(coverage((110, 0), (100, 0)), "识别率 110.0%");
        assert_eq!(coverage((0, 0), (0, 0)), "识别率暂无数据");
    }

    #[test]
    fn install_settings_preserve_stored_values_unless_overridden() {
        assert_eq!(
            resolve_numeric_setting(None, Some("17"), 3, "poll_seconds").unwrap(),
            17u64
        );
        assert_eq!(
            resolve_numeric_setting(Some(5), Some("17"), 3, "poll_seconds").unwrap(),
            5u64
        );
        assert!(resolve_numeric_setting::<u64>(None, Some("bad"), 3, "poll_seconds").is_err());
        assert_eq!(
            resolve_app_granularity(None, Some("1m")).unwrap(),
            AppGranularity::OneMinute
        );
        assert_eq!(
            resolve_app_granularity(Some(AppGranularity::FiveMinutes), Some("1m")).unwrap(),
            AppGranularity::FiveMinutes
        );
        assert!(resolve_app_granularity(None, Some("10m")).is_err());
    }

    #[test]
    fn limits_attribution_to_current_algorithm_window() {
        let mut meta = std::collections::BTreeMap::new();
        assert_eq!(attribution_window_start(&meta, 1_000), 1_000);

        meta.insert("attribution_started_at".into(), "900".into());
        assert_eq!(attribution_window_start(&meta, 1_000), 1_000);

        meta.insert("attribution_started_at".into(), "1100".into());
        assert_eq!(attribution_window_start(&meta, 1_000), 1_100);

        meta.insert("attribution_started_at".into(), "invalid".into());
        assert_eq!(attribution_window_start(&meta, 1_000), 1_000);
    }

    #[test]
    fn clamps_or_empties_queries_at_the_attribution_boundary() {
        let meta = std::collections::BTreeMap::from([(
            "attribution_started_at".to_string(),
            "1500".to_string(),
        )]);
        let mut crossing = Period {
            start: 1_000,
            end: 2_000,
            label: String::new(),
            exact: false,
        };
        assert!(apply_attribution_window(&mut crossing, &meta));
        assert_eq!(crossing.start, 1_500);

        let mut historical = Period {
            start: 1_000,
            end: 1_500,
            label: String::new(),
            exact: false,
        };
        assert!(!apply_attribution_window(&mut historical, &meta));
        assert_eq!(historical.start, historical.end);
    }
}
