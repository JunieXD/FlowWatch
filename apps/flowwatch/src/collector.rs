use anyhow::{Context, Result, bail};
use chrono::Local;
use flowwatch_clash::ClashSampler;
use flowwatch_core::{
    AbsoluteCounterTracker, AppIdentity, ByteDelta, LocalEndpoint, TrafficBackend, UNKNOWN,
    UsageDelta, UsageSource, is_proxy_carrier,
};
use flowwatch_macos::MacOsBackend;
use flowwatch_store::{Database, FlushBatch, day_bucket, minute_bucket};
use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RuntimeSettings {
    pub poll_seconds: u64,
    pub flush_seconds: u64,
    pub detail_days: i64,
    pub daily_days: i64,
    pub app_bucket_seconds: i64,
}

impl RuntimeSettings {
    pub fn load(database: &Database) -> Result<Self> {
        let settings = Self {
            poll_seconds: parse_setting(database, "poll_seconds", 3u64)?,
            flush_seconds: parse_setting(database, "flush_seconds", 60u64)?,
            detail_days: parse_setting(database, "detail_days", 30i64)?,
            daily_days: parse_setting(database, "daily_days", 365i64)?,
            app_bucket_seconds: parse_app_bucket_seconds(database)?,
        };
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<()> {
        if !(1..=60).contains(&self.poll_seconds) {
            bail!("poll_seconds must be between 1 and 60");
        }
        if self.flush_seconds < self.poll_seconds || self.flush_seconds > 600 {
            bail!("flush_seconds must be between poll_seconds and 600");
        }
        if self.detail_days < 1 || self.daily_days < self.detail_days {
            bail!("retention must satisfy 1 <= detail_days <= daily_days");
        }
        if !matches!(self.app_bucket_seconds, 60 | 300) {
            bail!("app granularity must be 1m or 5m");
        }
        Ok(())
    }
}

pub struct Collector {
    database: Database,
    settings: RuntimeSettings,
    backend: MacOsBackend,
    interface_tracker: AbsoluteCounterTracker<String>,
    socket_owners: HashMap<LocalEndpoint, (AppIdentity, i64)>,
    clash: Option<ClashSampler>,
    pending: FlushBatch,
    started_at: i64,
    history_started_at: i64,
    attribution_started_at: i64,
    clash_actor_started_at: i64,
    app_one_minute_started_at: Option<i64>,
    last_interface_sample: i64,
    last_process_sample: i64,
    last_clash_sample: i64,
    interface_error: String,
    process_error: String,
    clash_error: String,
    active_process_flows: usize,
    tracked_process_flows: usize,
    nettop_restarts: u64,
    nettop_baselines_discarded: u64,
    socket_owner_entries: usize,
    active_clash_flows: usize,
    actor_clash_flows: usize,
    identifiable_clash_flows: usize,
    metadata_identifiable_clash_flows: usize,
    fallback_identifiable_clash_flows: usize,
    last_maintenance_day: i64,
}

impl Collector {
    pub fn new(database: Database, settings: RuntimeSettings) -> Result<Self> {
        let meta = database.meta()?;
        let interface_baseline = meta
            .get("interface_counter_baseline")
            .and_then(|raw| serde_json::from_str::<HashMap<String, ByteDelta>>(raw).ok())
            .unwrap_or_default();
        let clash_baseline = meta
            .get("clash_counter_baseline")
            .and_then(|raw| serde_json::from_str::<ByteDelta>(raw).ok());
        let clash = database
            .clash_config()?
            .filter(|config| config.enabled)
            .map(|config| ClashSampler::new(config, clash_baseline));
        let started_at = Local::now().timestamp();
        let history_started_at = meta
            .get("history_started_at")
            .and_then(|value| value.parse().ok())
            .unwrap_or(started_at);
        let attribution_started_at = meta
            .get("attribution_started_at")
            .and_then(|value| value.parse().ok())
            .unwrap_or(started_at);
        let clash_actor_started_at = meta
            .get("clash_actor_started_at")
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| minute_bucket(started_at).saturating_add(60));
        let app_one_minute_started_at = if settings.app_bucket_seconds == 60 {
            Some(
                meta.get("app_one_minute_started_at")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| minute_bucket(started_at).saturating_add(60)),
            )
        } else {
            meta.get("app_one_minute_started_at")
                .and_then(|value| value.parse().ok())
        };
        let backend = MacOsBackend::with_poll_seconds(settings.poll_seconds);
        let pending = FlushBatch {
            app_bucket_seconds: settings.app_bucket_seconds,
            ..FlushBatch::default()
        };
        Ok(Self {
            database,
            settings,
            backend,
            interface_tracker: AbsoluteCounterTracker::from_baseline(interface_baseline),
            socket_owners: HashMap::new(),
            clash,
            pending,
            started_at,
            history_started_at,
            attribution_started_at,
            clash_actor_started_at,
            app_one_minute_started_at,
            last_interface_sample: 0,
            last_process_sample: 0,
            last_clash_sample: 0,
            interface_error: String::new(),
            process_error: String::new(),
            clash_error: String::new(),
            active_process_flows: 0,
            tracked_process_flows: 0,
            nettop_restarts: 0,
            nettop_baselines_discarded: 0,
            socket_owner_entries: 0,
            active_clash_flows: 0,
            actor_clash_flows: 0,
            identifiable_clash_flows: 0,
            metadata_identifiable_clash_flows: 0,
            fallback_identifiable_clash_flows: 0,
            last_maintenance_day: 0,
        })
    }

    pub fn run(mut self, run_seconds: u64, stop: Arc<AtomicBool>) -> Result<()> {
        self.write_start_meta()?;
        let run_started = Instant::now();
        let mut next_poll = Instant::now();
        let mut last_flush = Instant::now();

        while !stop.load(Ordering::Relaxed)
            && (run_seconds == 0 || run_started.elapsed() < Duration::from_secs(run_seconds))
        {
            let now = self.sample_processes();
            self.sample_interfaces(now);
            self.sample_clash(now);

            if last_flush.elapsed() >= Duration::from_secs(self.settings.flush_seconds) {
                self.flush(now)?;
                let today = day_bucket(now);
                if self.last_maintenance_day != today {
                    self.database.maintenance(
                        now,
                        self.settings.detail_days,
                        self.settings.daily_days,
                    )?;
                    self.last_maintenance_day = today;
                }
                last_flush = Instant::now();
            }

            next_poll += Duration::from_secs(self.settings.poll_seconds);
            while !stop.load(Ordering::Relaxed) && Instant::now() < next_poll {
                let remaining = next_poll.saturating_duration_since(Instant::now());
                std::thread::sleep(remaining.min(Duration::from_millis(200)));
            }
            if Instant::now() > next_poll + Duration::from_secs(self.settings.poll_seconds) {
                next_poll = Instant::now();
            }
        }
        let stopped_at = Local::now().timestamp();
        self.flush(stopped_at)?;
        self.database.set_meta(&BTreeMap::from([
            ("collector_pid".into(), String::new()),
            ("collector_stopped_at".into(), stopped_at.to_string()),
        ]))
    }

    fn sample_interfaces(&mut self, now: i64) {
        match self.backend.interface_counters() {
            Ok(counters) => {
                for (interface, delta) in self.interface_tracker.apply(counters) {
                    self.pending
                        .interfaces
                        .entry((minute_bucket(now), interface))
                        .or_default()
                        .add(delta.upload, delta.download);
                }
                self.last_interface_sample = now;
                self.interface_error.clear();
            }
            Err(error) => self.interface_error = truncate_error(&error),
        }
    }

    fn sample_processes(&mut self) -> i64 {
        let sample = self.backend.process_traffic();
        let now = Local::now().timestamp();
        match sample {
            Ok(sample) => {
                self.active_process_flows = sample.active_flows;
                self.tracked_process_flows = sample.tracked_flows;
                self.nettop_restarts = sample.collector_restarts;
                if sample.baseline_discarded {
                    self.nettop_baselines_discarded =
                        self.nettop_baselines_discarded.saturating_add(1);
                }
                self.socket_owners
                    .retain(|_, (_, seen_at)| now.saturating_sub(*seen_at) <= 15);
                for owner in sample.socket_owners {
                    self.socket_owners.insert(owner.endpoint, (owner.app, now));
                }
                self.socket_owner_entries = self.socket_owners.len();
                for delta in sample.apps {
                    if is_proxy_carrier(&delta.app) {
                        continue;
                    }
                    let mut usage = UsageDelta::default();
                    usage.add(delta.upload, delta.download, delta.connections, now);
                    merge_pending_app(
                        &mut self.pending,
                        app_bucket(now, self.settings.app_bucket_seconds),
                        delta.app,
                        UsageSource::Direct,
                        &usage,
                    );
                }
                self.last_process_sample = now;
                self.process_error.clear();
            }
            Err(error) => {
                self.socket_owners
                    .retain(|_, (_, seen_at)| now.saturating_sub(*seen_at) <= 15);
                self.socket_owner_entries = self.socket_owners.len();
                self.process_error = truncate_error(&error);
            }
        }
        now
    }

    fn sample_clash(&mut self, now: i64) {
        let Some(clash) = self.clash.as_mut() else {
            return;
        };
        let socket_owners = &self.socket_owners;
        let backend = &mut self.backend;
        match clash.sample(now, |process, path, endpoint| {
            if !process.trim().is_empty() || !path.trim().is_empty() {
                backend.resolve_external_identity(process, path)
            } else {
                endpoint
                    .and_then(|value| socket_owners.get(value))
                    .map(|(app, _)| app.clone())
                    .unwrap_or_else(|| AppIdentity::process(UNKNOWN, ""))
            }
        }) {
            Ok(sample) => {
                self.active_clash_flows = sample.active_connections;
                self.actor_clash_flows = sample.actor_connections;
                self.identifiable_clash_flows = sample.identifiable_connections;
                self.metadata_identifiable_clash_flows = sample.metadata_identifiable_connections;
                self.fallback_identifiable_clash_flows = sample.fallback_identifiable_connections;
                for (app, delta) in sample.apps {
                    merge_pending_app(
                        &mut self.pending,
                        app_bucket(now, self.settings.app_bucket_seconds),
                        app,
                        UsageSource::Clash,
                        &delta,
                    );
                }
                self.pending
                    .proxy_totals
                    .entry(minute_bucket(now))
                    .or_default()
                    .add_with_actor(
                        sample.totals.upload,
                        sample.totals.download,
                        sample.totals.attributed_upload,
                        sample.totals.attributed_download,
                        sample.totals.actor_upload,
                        sample.totals.actor_download,
                    );
                self.last_clash_sample = now;
                self.clash_error.clear();
            }
            Err(error) => self.clash_error = truncate_error(&error),
        }
    }

    fn write_start_meta(&mut self) -> Result<()> {
        self.database.set_meta(&BTreeMap::from([
            ("collector_pid".into(), std::process::id().to_string()),
            ("collector_started_at".into(), self.started_at.to_string()),
            ("collector_mode".into(), "standard".into()),
            ("collector_engine".into(), "nettop_snapshot_v3".into()),
            (
                "history_started_at".into(),
                self.history_started_at.to_string(),
            ),
            (
                "attribution_started_at".into(),
                self.attribution_started_at.to_string(),
            ),
            (
                "clash_actor_started_at".into(),
                self.clash_actor_started_at.to_string(),
            ),
            (
                "app_bucket_seconds".into(),
                self.settings.app_bucket_seconds.to_string(),
            ),
        ]))
    }

    fn flush(&mut self, now: i64) -> Result<()> {
        self.pending.meta = BTreeMap::from([
            ("collector_pid".into(), std::process::id().to_string()),
            ("collector_started_at".into(), self.started_at.to_string()),
            ("collector_mode".into(), "standard".into()),
            ("collector_engine".into(), "nettop_snapshot_v3".into()),
            (
                "history_started_at".into(),
                self.history_started_at.to_string(),
            ),
            (
                "attribution_started_at".into(),
                self.attribution_started_at.to_string(),
            ),
            (
                "clash_actor_started_at".into(),
                self.clash_actor_started_at.to_string(),
            ),
            (
                "app_bucket_seconds".into(),
                self.settings.app_bucket_seconds.to_string(),
            ),
            ("last_flush_at".into(), now.to_string()),
            (
                "last_interface_sample_at".into(),
                self.last_interface_sample.to_string(),
            ),
            (
                "last_process_sample_at".into(),
                self.last_process_sample.to_string(),
            ),
            (
                "last_clash_sample_at".into(),
                self.last_clash_sample.to_string(),
            ),
            ("interface_error".into(), self.interface_error.clone()),
            ("process_error".into(), self.process_error.clone()),
            ("clash_error".into(), self.clash_error.clone()),
            (
                "active_process_flows".into(),
                self.active_process_flows.to_string(),
            ),
            (
                "tracked_process_flows".into(),
                self.tracked_process_flows.to_string(),
            ),
            ("nettop_restarts".into(), self.nettop_restarts.to_string()),
            (
                "nettop_baselines_discarded".into(),
                self.nettop_baselines_discarded.to_string(),
            ),
            (
                "socket_owner_entries".into(),
                self.socket_owner_entries.to_string(),
            ),
            (
                "active_clash_flows".into(),
                self.active_clash_flows.to_string(),
            ),
            (
                "actor_clash_flows".into(),
                self.actor_clash_flows.to_string(),
            ),
            (
                "tracked_clash_flows".into(),
                self.clash
                    .as_ref()
                    .map(ClashSampler::tracked_flows)
                    .unwrap_or_default()
                    .to_string(),
            ),
            (
                "identifiable_clash_flows".into(),
                self.identifiable_clash_flows.to_string(),
            ),
            (
                "metadata_identifiable_clash_flows".into(),
                self.metadata_identifiable_clash_flows.to_string(),
            ),
            (
                "fallback_identifiable_clash_flows".into(),
                self.fallback_identifiable_clash_flows.to_string(),
            ),
            (
                "interface_counter_baseline".into(),
                serde_json::to_string(self.interface_tracker.baseline())?,
            ),
        ]);
        if let Some(baseline) = self.clash.as_ref().and_then(ClashSampler::total_baseline) {
            self.pending.meta.insert(
                "clash_counter_baseline".into(),
                serde_json::to_string(&baseline)?,
            );
        }
        if let Some(started_at) = self.app_one_minute_started_at {
            self.pending
                .meta
                .insert("app_one_minute_started_at".into(), started_at.to_string());
        }
        self.database.flush(&self.pending)?;
        self.pending = FlushBatch {
            app_bucket_seconds: self.settings.app_bucket_seconds,
            ..FlushBatch::default()
        };
        Ok(())
    }
}

pub fn acquire_lock(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("open collector lock {}", path.display()))?;
    // SAFETY: flock operates on the valid descriptor owned by file.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        bail!("collector is already running");
    }
    Ok(file)
}

fn parse_setting<T>(database: &Database, key: &str, default: T) -> Result<T>
where
    T: std::str::FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    database
        .setting(key)?
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("parse setting {key}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_app_bucket_seconds(database: &Database) -> Result<i64> {
    match database.setting("app_granularity")?.as_deref() {
        None | Some("5m") => Ok(300),
        Some("1m") => Ok(60),
        Some(value) => bail!("invalid app_granularity setting {value:?}; use 1m or 5m"),
    }
}

fn app_bucket(timestamp: i64, bucket_seconds: i64) -> i64 {
    timestamp - timestamp.rem_euclid(bucket_seconds)
}

fn merge_pending_app(
    pending: &mut FlushBatch,
    bucket: i64,
    app: AppIdentity,
    source: UsageSource,
    delta: &UsageDelta,
) {
    let target = pending.apps.entry((bucket, app, source)).or_default();
    target.upload = target.upload.saturating_add(delta.upload);
    target.download = target.download.saturating_add(delta.download);
    target.connections = target.connections.saturating_add(delta.connections);
    if target.first_seen == 0 || (delta.first_seen > 0 && delta.first_seen < target.first_seen) {
        target.first_seen = delta.first_seen;
    }
    target.last_seen = target.last_seen.max(delta.last_seen);
}

fn truncate_error(error: &impl std::fmt::Display) -> String {
    let value = error.to_string().replace('\n', " ");
    value.chars().take(500).collect()
}
