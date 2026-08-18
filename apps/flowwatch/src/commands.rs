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

    println!("FlowWatch status");
    println!(
        "  Collector: {}{}",
        if running { "running" } else { "not running" },
        pid.map_or_else(String::new, |pid| format!(" (pid {pid})"))
    );
    println!(
        "  Last flush: {}",
        meta_timestamp(&meta, "last_flush_at", now)
    );
    println!(
        "  Database:   {} ({})",
        paths.database.display(),
        human_bytes(database.size_bytes())
    );
    println!("  Integrity:  {}", database.integrity_check()?);
    if let Some(engine) = meta.get("collector_engine") {
        println!("  Engine:     {engine}");
    }
    println!(
        "  App detail: {}",
        app_granularity_label(app_bucket_seconds(&database)?)
    );
    println!();
    println!("Today");
    println!(
        "  Physical:   up {}  down {}",
        human_bytes(physical.0),
        human_bytes(physical.1)
    );
    if attribution_start > start {
        println!(
            "  Attribution since {}",
            format_timestamp(attribution_start)
        );
    }
    println!(
        "  Attributed: up {}  down {}  ({})",
        human_bytes(attributed.0),
        human_bytes(attributed.1),
        coverage(attributed, attribution_physical)
    );
    if proxy.upload > 0 || proxy.download > 0 {
        let unattributed = (
            proxy.upload.saturating_sub(proxy.attributed_upload),
            proxy.download.saturating_sub(proxy.attributed_download),
        );
        println!("  Clash:");
        println!(
            "    Total:        up {}  down {}",
            human_bytes(proxy.upload),
            human_bytes(proxy.download),
        );
        println!(
            "    Attributed:   up {}  down {}",
            human_bytes(proxy.attributed_upload),
            human_bytes(proxy.attributed_download),
        );
        println!(
            "    Unattributed: up {}  down {}",
            human_bytes(unattributed.0),
            human_bytes(unattributed.1),
        );
        if unknown_clash.0 > 0 || unknown_clash.1 > 0 {
            println!(
                "    Unknown app:  up {}  down {} (included in unattributed)",
                human_bytes(unknown_clash.0),
                human_bytes(unknown_clash.1),
            );
        }
        println!(
            "    Coverage:     {}",
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
                    "    Classification since {}:",
                    format_timestamp(clash_actor_start)
                );
                println!(
                    "      Observed actor:       up {}  down {}",
                    human_bytes(classified.actor_upload),
                    human_bytes(classified.actor_download),
                );
                println!(
                    "      App-attributed actor: up {}  down {}  ({})",
                    human_bytes(classified.attributed_upload),
                    human_bytes(classified.attributed_download),
                    coverage(
                        (classified.attributed_upload, classified.attributed_download),
                        (classified.actor_upload, classified.actor_download),
                    )
                );
                println!(
                    "      Non-actor/unobserved:  up {}  down {}",
                    human_bytes(non_actor.0),
                    human_bytes(non_actor.1),
                );
            }
            _ => println!("    Actor-byte classification is waiting for a complete new minute."),
        }
        println!("    Note: unobserved includes short flows missed between controller samples.");
    }
    let active_clash = meta_number(&meta, "active_clash_flows");
    let actor_clash = meta_number(&meta, "actor_clash_flows");
    let identifiable_clash = meta_number(&meta, "identifiable_clash_flows");
    let metadata_clash = meta_number(&meta, "metadata_identifiable_clash_flows");
    let fallback_clash = meta_number(&meta, "fallback_identifiable_clash_flows");
    if active_clash > 0 {
        println!(
            "  Clash flows: {active_clash} active, {actor_clash} actor, {identifiable_clash} app-identifiable"
        );
        if fallback_clash > 0 {
            println!(
                "               {metadata_clash} from controller, {fallback_clash} from local sockets"
            );
        }
    }
    if !anomalies.is_empty() {
        println!(
            "  Quality:    warning: {} completed 5-minute direct bucket(s) exceed physical bounds",
            anomalies.len()
        );
    }
    print_collector_errors(&meta);
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
    println!("Application traffic ({})", range.label);
    if range.start > requested_start {
        println!(
            "Attribution data starts at {}; earlier bytes in the requested range are excluded.",
            format_timestamp(range.start)
        );
    }
    if range.exact {
        println!(
            "Resolution: current app detail is {}; retained older data may use five-minute buckets.",
            app_granularity_label(app_bucket_seconds(&database)?)
        );
    }
    println!(
        "{:<3} {:>11} {:>11} {:>11}  Application",
        "#", "Upload", "Download", "Total"
    );
    for (index, row) in rows.iter().enumerate() {
        let identity_summary = if row.identity_count > 1 || row.executable_paths.len() > 1 {
            format!(
                " ({} {}, {} {})",
                row.identity_count,
                if row.identity_count == 1 {
                    "identity"
                } else {
                    "identities"
                },
                row.executable_paths.len(),
                if row.executable_paths.len() == 1 {
                    "path"
                } else {
                    "paths"
                }
            )
        } else {
            String::new()
        };
        println!(
            "{:<3} {:>11} {:>11} {:>11}  {}{} [{}]",
            index + 1,
            human_bytes(row.upload()),
            human_bytes(row.download()),
            human_bytes(row.upload().saturating_add(row.download())),
            row.app.name,
            identity_summary,
            sources(row),
        );
    }
    if rows.is_empty() {
        println!("No application samples in this period.");
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
    println!("Physical interface traffic ({})", range.label);
    if range.exact {
        println!("Resolution: one-minute buckets that overlap the requested range.");
    }
    println!(
        "{:<12} {:>12} {:>12} {:>12}",
        "Interface", "Upload", "Download", "Total"
    );
    for row in &rows {
        println!(
            "{:<12} {:>12} {:>12} {:>12}",
            row.interface,
            human_bytes(row.upload),
            human_bytes(row.download),
            human_bytes(row.upload.saturating_add(row.download)),
        );
    }
    if rows.is_empty() {
        println!("No physical-interface samples in this period.");
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
    println!("Highest-traffic minutes ({})", range.label);
    if range.exact {
        println!("Resolution: one-minute buckets that overlap the requested range.");
    }
    println!(
        "{:<20} {:>12} {:>12} {:>12}",
        "Minute", "Upload", "Download", "Total"
    );
    for row in &rows {
        println!(
            "{:<20} {:>12} {:>12} {:>12}",
            format_timestamp(row.bucket),
            human_bytes(row.upload),
            human_bytes(row.download),
            human_bytes(row.upload.saturating_add(row.download)),
        );
    }
    if rows.is_empty() {
        println!("No minute samples in this period (fine detail is retained for a limited time).");
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
        "Attribution gaps ({}; {} buckets)",
        range.label,
        app_granularity_label(bucket_seconds)
    );
    if range.start > requested_start {
        println!(
            "Attribution data starts at {}; earlier bytes in the requested range are excluded.",
            format_timestamp(range.start)
        );
    }
    println!(
        "{:<20} {:>12} {:>12} {:>12} {:>12}",
        "Bucket", "Physical", "Attributed", "Gap", "Clash gap"
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
        println!(
            "{:<20} {:>12} {:>12} {:>12} {:>12}",
            format_timestamp(row.bucket),
            human_bytes(physical),
            human_bytes(attributed),
            human_bytes(gap),
            human_bytes(clash_gap),
        );
    }
    if rows.is_empty() {
        println!("No fine-detail attribution buckets in this period.");
    }
    Ok(())
}

fn doctor(paths: &AppPaths) -> Result<()> {
    let mut failures = Vec::new();
    println!("FlowWatch diagnostics");
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
            bail!("integrity check returned {integrity}");
        }
        Ok((database, integrity))
    }) {
        Ok((database, integrity)) => {
            println!("  [ok] SQLite integrity: {integrity}");
            check_database_permissions(database.path());
            let now = Local::now().timestamp();
            let anomalies = database.direct_attribution_anomalies(now - 86_400, now + 1)?;
            if anomalies.is_empty() {
                println!(
                    "  [ok] Direct attribution: completed 5-minute buckets fit physical bounds"
                );
            } else {
                println!(
                    "  [warn] Direct attribution: {} completed 5-minute bucket(s) exceed physical bounds",
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
                            "  [warn] Clash controller: {} active connections, none identify an application",
                            sample.active_connections
                        );
                        println!("         Consider setting find-process-mode: strict in Mihomo.");
                    }
                    Ok(sample) => {
                        println!(
                            "  [ok] Clash controller: {} active, {} actor, {} app-identifiable",
                            sample.active_connections,
                            sample.actor_connections,
                            sample.identifiable_connections
                        );
                        if sample.fallback_identifiable_connections > 0 {
                            println!(
                                "       {} from controller, {} from local sockets",
                                sample.metadata_identifiable_connections,
                                sample.fallback_identifiable_connections
                            );
                        }
                    }
                    Err(error) => println!("  [warn] Clash controller: {error}"),
                }
            } else {
                println!("  [skip] Clash provider is disabled");
            }
        }
        Err(error) => {
            println!("  [fail] SQLite: {error:#}");
            failures.push("SQLite");
        }
    }

    match backend.interface_counters() {
        Ok(counters) if !counters.is_empty() => {
            println!("  [ok] Physical counters: {} interface(s)", counters.len())
        }
        Ok(_) => {
            println!("  [fail] Physical counters: no hardware interfaces");
            failures.push("physical counters");
        }
        Err(error) => {
            println!("  [fail] Physical counters: {error:#}");
            failures.push("physical counters");
        }
    }
    match process_probe {
        Ok(sample) => println!(
            "  [ok] nettop snapshot: {} active flows, {} tracked, {} socket owner(s)",
            sample.active_flows,
            sample.tracked_flows,
            sample.socket_owners.len()
        ),
        Err(error) => {
            println!("  [fail] nettop process sampler: {error:#}");
            failures.push("nettop");
        }
    }

    if launch_agent_loaded() {
        println!("  [ok] LaunchAgent is loaded");
    } else if paths.launch_agent.exists() {
        println!("  [warn] LaunchAgent plist exists but is not loaded");
    } else {
        println!("  [skip] LaunchAgent is not installed");
    }

    if failures.is_empty() {
        println!("Diagnostics passed.");
        Ok(())
    } else {
        bail!("diagnostics failed: {}", failures.join(", "))
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
        println!("Imported Clash controller configuration (secret redacted).");
    }
    drop(database);

    install_binary(&paths.installed_binary, Some(0o700))?;
    install_binary(&paths.command_binary, Some(0o755))?;
    write_launch_agent(paths)?;
    bootout_launch_agent()?;
    bootstrap_launch_agent(&paths.launch_agent)?;
    println!("Installed and started FlowWatch.");
    println!("  Binary:   {}", paths.installed_binary.display());
    println!("  Command:  {}", paths.command_binary.display());
    println!("  Database: {}", paths.database.display());
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
        println!("Removed FlowWatch service, binary, and traffic database.");
    } else {
        println!("Removed FlowWatch service and binary. Traffic history was preserved.");
    }
    Ok(())
}

fn configure(paths: &AppPaths, command: ConfigCommand) -> Result<()> {
    let mut database = Database::open(&paths.database)?;
    match command {
        ConfigCommand::ImportClash { path } => {
            let config = read_clash_config(&path)?;
            database.set_clash_config(&config)?;
            println!("Clash provider enabled (secret stored in SQLite and redacted here).");
            if paths.uses_default_database {
                restart_launch_agent()?;
            }
        }
        ConfigCommand::DisableClash => {
            let Some(mut config) = database.clash_config()? else {
                println!("Clash provider is not configured.");
                return Ok(());
            };
            config.enabled = false;
            database.set_clash_config(&config)?;
            println!("Clash provider disabled.");
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
                "Application detail granularity set to {}.",
                app_granularity_label(granularity.bucket_seconds())
            );
            if paths.uses_default_database {
                restart_launch_agent()?;
            }
        }
        ConfigCommand::Show => {
            println!("Database: {}", paths.database.display());
            println!(
                "poll_seconds: {}",
                database.setting("poll_seconds")?.as_deref().unwrap_or("3")
            );
            println!(
                "flush_seconds: {}",
                database
                    .setting("flush_seconds")?
                    .as_deref()
                    .unwrap_or("60")
            );
            println!(
                "detail_days: {}",
                database.setting("detail_days")?.as_deref().unwrap_or("30")
            );
            println!(
                "daily_days: {}",
                database.setting("daily_days")?.as_deref().unwrap_or("365")
            );
            println!(
                "app_granularity: {}",
                database
                    .setting("app_granularity")?
                    .as_deref()
                    .unwrap_or("5m")
            );
            match database.clash_config()? {
                Some(config) => {
                    println!("clash_enabled: {}", config.enabled);
                    println!("clash_controller: {}", config.controller);
                    println!(
                        "clash_secret: {}",
                        if config.secret.is_empty() {
                            "not set"
                        } else {
                            "[redacted]"
                        }
                    );
                }
                None => println!("clash_enabled: false"),
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
        _ => bail!("--from and --to must be provided together"),
    }
}

fn validate_query_args(args: &QueryArgs) -> Result<()> {
    if !(1..=10_000).contains(&args.limit) {
        bail!("limit must be between 1 and 10000");
    }
    if args.from.is_some() != args.to.is_some() {
        bail!("--from and --to must be provided together");
    }
    Ok(())
}

fn parse_exact_range(from: &str, to: &str) -> Result<Period> {
    let start = parse_query_timestamp(from).with_context(|| format!("parse --from {from:?}"))?;
    let end = parse_query_timestamp(to).with_context(|| format!("parse --to {to:?}"))?;
    if start >= end {
        bail!("--from must be earlier than --to");
    }
    Ok(Period {
        start,
        end,
        label: format!(
            "{} to {} (end exclusive)",
            format_timestamp_with_seconds(start),
            format_timestamp_with_seconds(end)
        ),
        exact: true,
    })
}

fn parse_query_timestamp(raw: &str) -> Result<i64> {
    let value = raw.trim();
    if value.is_empty() {
        bail!("timestamp is empty");
    }
    if let Ok(timestamp) = value.parse::<i64>() {
        Local
            .timestamp_opt(timestamp, 0)
            .single()
            .context("Unix timestamp is outside the supported range")?;
        return Ok(timestamp);
    }
    if let Ok(timestamp) = DateTime::parse_from_rfc3339(value) {
        return Ok(timestamp.timestamp());
    }
    let naive = ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .context("use local YYYY-MM-DD HH:MM[:SS], a Unix timestamp, or an RFC 3339 timestamp")?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(timestamp) => Ok(timestamp.timestamp()),
        LocalResult::Ambiguous(_, _) => {
            bail!("local time is ambiguous due to daylight saving time")
        }
        LocalResult::None => bail!("local time does not exist due to daylight saving time"),
    }
}

fn parse_period_at(raw: &str, now: i64) -> Result<Period> {
    let normalized = raw.trim().to_ascii_lowercase();
    let (start, label) = match normalized.as_str() {
        "today" => (day_bucket(now), "today".to_string()),
        "yesterday" => {
            let today = day_bucket(now);
            (day_bucket(today - 1), "yesterday".to_string())
        }
        "all" => (0, "all retained history".to_string()),
        _ => {
            let (number, multiplier) = if let Some(value) = normalized.strip_suffix('h') {
                (value, 3_600i64)
            } else if let Some(value) = normalized.strip_suffix('d') {
                (value, 86_400i64)
            } else {
                bail!("invalid period {raw:?}; use today, yesterday, 24h, 7d, 30d, or all");
            };
            let amount: i64 = number.parse().context("parse period amount")?;
            if !(1..=3_650).contains(&amount) {
                bail!("period amount must be between 1 and 3650");
            }
            let seconds = amount
                .checked_mul(multiplier)
                .context("period is too large")?;
            (now.saturating_sub(seconds), normalized.clone())
        }
    };
    let end = if normalized == "yesterday" {
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
        values.push("direct");
    }
    if row.clash_upload > 0 || row.clash_download > 0 {
        values.push("clash");
    }
    if row.enhanced_upload > 0 || row.enhanced_download > 0 {
        values.push("enhanced");
    }
    if values.is_empty() {
        "unknown".to_string()
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
        return "coverage unavailable".to_string();
    }
    format!(
        "{:.1}% coverage",
        numerator as f64 * 100.0 / denominator as f64
    )
}

fn app_bucket_seconds(database: &Database) -> Result<i64> {
    match database.setting("app_granularity")?.as_deref() {
        None | Some("5m") => Ok(300),
        Some("1m") => Ok(60),
        Some(value) => bail!("invalid app_granularity setting {value:?}; use 1m or 5m"),
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
        "one-minute"
    } else {
        "five-minute"
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
        return "never".to_string();
    };
    let age = now.saturating_sub(timestamp).max(0) as u64;
    format!(
        "{} ({} ago)",
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
        format!("{seconds}s")
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
    }
}

fn print_collector_errors(meta: &std::collections::BTreeMap<String, String>) {
    for (label, key) in [
        ("Physical collector", "interface_error"),
        ("Process collector", "process_error"),
        ("Clash provider", "clash_error"),
    ] {
        if let Some(error) = meta.get(key).filter(|error| !error.is_empty()) {
            println!("  Warning: {label}: {error}");
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
    let source = std::env::current_exe().context("resolve current executable")?;
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
        .with_context(|| format!("copy {} to {}", source.display(), temporary.display()))?;
    restrict_mode(&temporary, 0o755)?;
    fs::rename(&temporary, destination)
        .with_context(|| format!("install binary to {}", destination.display()))?;
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
            .map_err(|error| anyhow::anyhow!("invalid stored {name} setting {value:?}: {error}")),
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
        Some(value) => bail!("invalid stored app_granularity setting {value:?}; use 1m or 5m"),
    }
}

fn write_launch_agent(paths: &AppPaths) -> Result<()> {
    let parent = paths
        .launch_agent
        .parent()
        .context("LaunchAgents path has no parent")?;
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
        .with_context(|| format!("write {}", temporary.display()))?;
    restrict_mode(&temporary, 0o600)?;
    fs::rename(&temporary, &paths.launch_agent)
        .with_context(|| format!("install {}", paths.launch_agent.display()))?;
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
    println!("Restart signal sent to the installed collector.");
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
    let error = last_error.context("launchctl bootstrap was not attempted")?;
    Err(error).context("load FlowWatch LaunchAgent")
}

fn run_launchctl(args: &[&str]) -> Result<()> {
    let output = Command::new(LAUNCHCTL)
        .args(args)
        .output()
        .context("run launchctl")?;
    if !output.status.success() {
        bail!(
            "launchctl failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn path_string(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not valid UTF-8: {}", path.display()))
}

fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
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
            println!("  [warn] Database permissions are {mode:o}; expected 600");
        } else {
            println!("  [ok] Database permissions: 600");
        }
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
    fn reports_coverage_without_hiding_overcount() {
        assert_eq!(coverage((110, 0), (100, 0)), "110.0% coverage");
        assert_eq!(coverage((0, 0), (0, 0)), "coverage unavailable");
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
