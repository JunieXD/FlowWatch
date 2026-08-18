//! Cross-platform SQLite storage for FlowWatch.

use anyhow::{Context, Result};
use chrono::{Local, LocalResult, NaiveDate, TimeZone};
use flowwatch_core::{AppIdentity, ByteDelta, ClashConfig, CoreDelta, UsageDelta, UsageSource};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Default)]
pub struct FlushBatch {
    pub apps: HashMap<(i64, AppIdentity, UsageSource), UsageDelta>,
    pub app_bucket_seconds: i64,
    pub interfaces: HashMap<(i64, String), ByteDelta>,
    pub proxy_totals: HashMap<i64, CoreDelta>,
    pub meta: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AppUsage {
    pub app: AppIdentity,
    pub direct_upload: u64,
    pub direct_download: u64,
    pub clash_upload: u64,
    pub clash_download: u64,
    pub enhanced_upload: u64,
    pub enhanced_download: u64,
    pub connections: u64,
    pub last_seen: i64,
    pub identity_count: u32,
    pub identity_ids: Vec<String>,
    pub executable_paths: Vec<String>,
}

impl AppUsage {
    pub fn upload(&self) -> u64 {
        self.direct_upload
            .saturating_add(self.clash_upload)
            .saturating_add(self.enhanced_upload)
    }

    pub fn download(&self) -> u64 {
        self.direct_download
            .saturating_add(self.clash_download)
            .saturating_add(self.enhanced_download)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct InterfaceUsage {
    pub interface: String,
    pub upload: u64,
    pub download: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpikeUsage {
    pub bucket: i64,
    pub upload: u64,
    pub download: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributionAnomaly {
    pub bucket: i64,
    pub direct_upload: u64,
    pub direct_download: u64,
    pub physical_upload: u64,
    pub physical_download: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributionGap {
    pub bucket: i64,
    pub physical_upload: u64,
    pub physical_download: u64,
    pub attributed_upload: u64,
    pub attributed_download: u64,
    pub gap_upload: u64,
    pub gap_download: u64,
    pub clash_upload: u64,
    pub clash_download: u64,
    pub clash_attributed_upload: u64,
    pub clash_attributed_download: u64,
    pub clash_actor_upload: u64,
    pub clash_actor_download: u64,
    pub clash_actor_bytes_known: bool,
}

pub struct Database {
    path: PathBuf,
    connection: Connection,
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent()
            && !parent.exists()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建 {}", parent.display()))?;
            restrict_directory(parent)?;
        }
        let connection = Connection::open(&path)
            .with_context(|| format!("无法打开数据库 {}", path.display()))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        connection.pragma_update(None, "temp_store", "MEMORY")?;
        connection.pragma_update(None, "cache_size", -2048)?;
        connection.pragma_update(None, "wal_autocheckpoint", 100)?;
        connection.pragma_update(None, "journal_size_limit", 1_048_576)?;
        connection.busy_timeout(std::time::Duration::from_secs(10))?;
        let mut database = Self { path, connection };
        database.create_schema()?;
        database.restrict_files()?;
        Ok(database)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn create_schema(&mut self) -> Result<()> {
        self.connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS app_usage_5m (
                bucket INTEGER NOT NULL,
                app_id TEXT NOT NULL,
                app_name TEXT NOT NULL,
                executable_path TEXT NOT NULL,
                source TEXT NOT NULL,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                connections INTEGER NOT NULL,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                PRIMARY KEY (bucket, app_id, source)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS app_usage_1m (
                bucket INTEGER NOT NULL,
                app_id TEXT NOT NULL,
                app_name TEXT NOT NULL,
                executable_path TEXT NOT NULL,
                source TEXT NOT NULL,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                connections INTEGER NOT NULL,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                PRIMARY KEY (bucket, app_id, source)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS app_usage_daily (
                bucket INTEGER NOT NULL,
                app_id TEXT NOT NULL,
                app_name TEXT NOT NULL,
                executable_path TEXT NOT NULL,
                source TEXT NOT NULL,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                connections INTEGER NOT NULL,
                first_seen INTEGER NOT NULL,
                last_seen INTEGER NOT NULL,
                PRIMARY KEY (bucket, app_id, source)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS interface_minute (
                bucket INTEGER NOT NULL,
                interface TEXT NOT NULL,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                PRIMARY KEY (bucket, interface)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS interface_daily (
                bucket INTEGER NOT NULL,
                interface TEXT NOT NULL,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                PRIMARY KEY (bucket, interface)
            ) WITHOUT ROWID;

            CREATE TABLE IF NOT EXISTS proxy_minute (
                bucket INTEGER PRIMARY KEY,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                attributed_upload INTEGER NOT NULL,
                attributed_download INTEGER NOT NULL,
                actor_upload INTEGER,
                actor_download INTEGER
            );

            CREATE TABLE IF NOT EXISTS proxy_daily (
                bucket INTEGER PRIMARY KEY,
                upload INTEGER NOT NULL,
                download INTEGER NOT NULL,
                attributed_upload INTEGER NOT NULL,
                attributed_download INTEGER NOT NULL,
                actor_upload INTEGER,
                actor_download INTEGER
            );
            ",
        )?;
        self.ensure_proxy_actor_columns()?;
        self.set_meta_value("schema_version", &SCHEMA_VERSION.to_string())?;
        Ok(())
    }

    fn ensure_proxy_actor_columns(&self) -> Result<()> {
        for table in ["proxy_minute", "proxy_daily"] {
            for column in ["actor_upload", "actor_download"] {
                let present: bool = self.connection.query_row(
                    &format!("SELECT COUNT(*) > 0 FROM pragma_table_info('{table}') WHERE name=?1"),
                    [column],
                    |row| row.get(0),
                )?;
                if !present {
                    self.connection.execute(
                        &format!("ALTER TABLE {table} ADD COLUMN {column} INTEGER"),
                        [],
                    )?;
                }
            }
        }
        Ok(())
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO settings(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()?)
    }

    pub fn set_clash_config(&mut self, config: &ClashConfig) -> Result<()> {
        self.set_setting("clash_config", &serde_json::to_string(config)?)
    }

    pub fn clash_config(&self) -> Result<Option<ClashConfig>> {
        self.setting("clash_config")?
            .map(|raw| serde_json::from_str(&raw).context("无法解析 Clash 设置"))
            .transpose()
    }

    pub fn meta(&self) -> Result<BTreeMap<String, String>> {
        let mut statement = self.connection.prepare("SELECT key, value FROM meta")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_meta(&mut self, values: &BTreeMap<String, String>) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            )?;
            for (key, value) in values {
                statement.execute(params![key, value])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    fn set_meta_value(&mut self, key: &str, value: &str) -> Result<()> {
        self.connection.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn flush(&mut self, batch: &FlushBatch) -> Result<()> {
        let transaction = self.connection.transaction()?;
        {
            let app_table = if batch.app_bucket_seconds == 60 {
                "app_usage_1m"
            } else {
                "app_usage_5m"
            };
            let mut app_statement = transaction.prepare(&format!(
                "INSERT INTO {app_table}(
                    bucket, app_id, app_name, executable_path, source,
                    upload, download, connections, first_seen, last_seen
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(bucket, app_id, source) DO UPDATE SET
                    app_name=excluded.app_name,
                    executable_path=excluded.executable_path,
                    upload=upload+excluded.upload,
                    download=download+excluded.download,
                    connections=connections+excluded.connections,
                    first_seen=MIN(first_seen, excluded.first_seen),
                    last_seen=MAX(last_seen, excluded.last_seen)"
            ))?;
            for ((bucket, app, source), delta) in &batch.apps {
                app_statement.execute(params![
                    bucket,
                    app.id,
                    app.name,
                    app.executable_path,
                    source.as_str(),
                    sql_u64(delta.upload),
                    sql_u64(delta.download),
                    sql_u64(delta.connections),
                    delta.first_seen,
                    delta.last_seen,
                ])?;
            }

            let mut interface_statement = transaction.prepare(
                "INSERT INTO interface_minute(bucket, interface, upload, download)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(bucket, interface) DO UPDATE SET
                    upload=upload+excluded.upload,
                    download=download+excluded.download",
            )?;
            for ((bucket, interface), delta) in &batch.interfaces {
                interface_statement.execute(params![
                    bucket,
                    interface,
                    sql_u64(delta.upload),
                    sql_u64(delta.download),
                ])?;
            }

            let mut proxy_statement = transaction.prepare(
                "INSERT INTO proxy_minute(
                    bucket, upload, download, attributed_upload, attributed_download,
                    actor_upload, actor_download
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(bucket) DO UPDATE SET
                    upload=upload+excluded.upload,
                    download=download+excluded.download,
                    attributed_upload=attributed_upload+excluded.attributed_upload,
                    attributed_download=attributed_download+excluded.attributed_download,
                    actor_upload=CASE
                        WHEN proxy_minute.actor_upload IS NULL OR excluded.actor_upload IS NULL THEN NULL
                        ELSE proxy_minute.actor_upload+excluded.actor_upload
                    END,
                    actor_download=CASE
                        WHEN proxy_minute.actor_download IS NULL OR excluded.actor_download IS NULL THEN NULL
                        ELSE proxy_minute.actor_download+excluded.actor_download
                    END",
            )?;
            for (bucket, delta) in &batch.proxy_totals {
                let actor_upload = delta.actor_bytes_known.then(|| sql_u64(delta.actor_upload));
                let actor_download = delta
                    .actor_bytes_known
                    .then(|| sql_u64(delta.actor_download));
                proxy_statement.execute(params![
                    bucket,
                    sql_u64(delta.upload),
                    sql_u64(delta.download),
                    sql_u64(delta.attributed_upload),
                    sql_u64(delta.attributed_download),
                    actor_upload,
                    actor_download,
                ])?;
            }

            let mut meta_statement = transaction.prepare(
                "INSERT INTO meta(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            )?;
            for (key, value) in &batch.meta {
                meta_statement.execute(params![key, value])?;
            }
        }
        transaction.commit()?;
        self.restrict_files()?;
        Ok(())
    }

    pub fn query_apps(&self, start: i64, end: i64) -> Result<Vec<AppUsage>> {
        let one_minute_start = if self.setting("app_granularity")?.as_deref() == Some("1m") {
            self.connection
                .query_row(
                    "SELECT value FROM meta WHERE key='app_one_minute_started_at'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|value| value.parse::<i64>().ok())
        } else {
            None
        };
        let (five_minute_start, five_minute_end) = match one_minute_start {
            Some(transition) if start >= transition => (end, end),
            Some(transition) => (five_minute_bucket(start), end.min(transition)),
            None => (five_minute_bucket(start), end),
        };
        let mut statement = self.connection.prepare(
            "SELECT app_id, app_name, executable_path, source,
                    SUM(upload), SUM(download), SUM(connections), MAX(last_seen)
             FROM (
                SELECT app_id, app_name, executable_path, source,
                       upload, download, connections, last_seen
                FROM app_usage_1m WHERE bucket >= ?1 AND bucket < ?2
                UNION ALL
                SELECT app_id, app_name, executable_path, source,
                       upload, download, connections, last_seen
                FROM app_usage_5m WHERE bucket >= ?3 AND bucket < ?4
                UNION ALL
                SELECT app_id, app_name, executable_path, source,
                       upload, download, connections, last_seen
                FROM app_usage_daily WHERE bucket >= ?5 AND bucket < ?2
             )
             GROUP BY app_id, app_name, executable_path, source",
        )?;
        let rows = statement.query_map(
            params![
                minute_bucket(start),
                end,
                five_minute_start,
                five_minute_end,
                day_bucket(start)
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    db_u64(row.get::<_, i64>(4)?),
                    db_u64(row.get::<_, i64>(5)?),
                    db_u64(row.get::<_, i64>(6)?),
                    row.get::<_, i64>(7)?,
                ))
            },
        )?;
        let mut by_app: HashMap<String, AppUsage> = HashMap::new();
        for row in rows {
            let (id, name, path, source, upload, download, connections, last_seen) = row?;
            let identity = AppIdentity {
                id: id.clone(),
                name,
                executable_path: path,
            };
            let usage = by_app.entry(id.clone()).or_insert_with(|| AppUsage {
                app: identity.clone(),
                ..AppUsage::default()
            });
            record_app_identity(usage, &identity);
            match source.as_str() {
                "clash" => {
                    usage.clash_upload = usage.clash_upload.saturating_add(upload);
                    usage.clash_download = usage.clash_download.saturating_add(download);
                }
                "enhanced" => {
                    usage.enhanced_upload = usage.enhanced_upload.saturating_add(upload);
                    usage.enhanced_download = usage.enhanced_download.saturating_add(download);
                }
                _ => {
                    usage.direct_upload = usage.direct_upload.saturating_add(upload);
                    usage.direct_download = usage.direct_download.saturating_add(download);
                }
            }
            usage.connections = usage.connections.saturating_add(connections);
            usage.last_seen = usage.last_seen.max(last_seen);
        }
        Ok(consolidate_app_usages(by_app.into_values()))
    }

    pub fn group_apps_for_display(rows: Vec<AppUsage>) -> Vec<AppUsage> {
        let mut groups: HashMap<String, AppUsage> = HashMap::new();
        for row in rows {
            let key = executable_file_name(&row.app.executable_path)
                .filter(|name| !name.is_empty())
                .map(|name| {
                    format!(
                        "{}\0{}",
                        normalized_name(&row.app.name),
                        normalized_name(name)
                    )
                })
                .unwrap_or_else(|| format!("id\0{}", row.app.id));
            match groups.entry(key) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(row);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let target = entry.get_mut();
                    let distinct_ids = target.app.id != row.app.id;
                    let group_name = target.app.name.clone();
                    let group_executable = executable_file_name(&target.app.executable_path)
                        .unwrap_or(&group_name)
                        .to_string();
                    merge_app_usage(target, row);
                    if distinct_ids {
                        target.app.id = format!(
                            "group:{}:{}",
                            normalized_name(&group_name),
                            normalized_name(&group_executable)
                        );
                        target.app.name = group_name;
                        target.app.executable_path.clear();
                    }
                }
            }
        }
        groups.into_values().collect()
    }

    pub fn query_interfaces(&self, start: i64, end: i64) -> Result<Vec<InterfaceUsage>> {
        let mut statement = self.connection.prepare(
            "SELECT interface, SUM(upload), SUM(download)
             FROM (
                SELECT interface, upload, download
                FROM interface_minute WHERE bucket >= ?1 AND bucket < ?2
                UNION ALL
                SELECT interface, upload, download
                FROM interface_daily WHERE bucket >= ?3 AND bucket < ?4
             ) GROUP BY interface",
        )?;
        let rows = statement.query_map(
            params![minute_bucket(start), end, day_bucket(start), end],
            |row| {
                Ok(InterfaceUsage {
                    interface: row.get(0)?,
                    upload: db_u64(row.get(1)?),
                    download: db_u64(row.get(2)?),
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn query_spikes(&self, start: i64, end: i64) -> Result<Vec<SpikeUsage>> {
        let mut statement = self.connection.prepare(
            "SELECT bucket, SUM(upload) AS total_upload, SUM(download) AS total_download
             FROM interface_minute
             WHERE bucket >= ?1 AND bucket < ?2
             GROUP BY bucket",
        )?;
        let rows = statement.query_map(params![minute_bucket(start), end], |row| {
            Ok(SpikeUsage {
                bucket: row.get(0)?,
                upload: db_u64(row.get(1)?),
                download: db_u64(row.get(2)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn query_attribution_gaps(
        &self,
        start: i64,
        end: i64,
        bucket_seconds: i64,
    ) -> Result<Vec<AttributionGap>> {
        let bucket_seconds = if bucket_seconds == 60 { 60 } else { 300 };
        let app_rows = if bucket_seconds == 60 {
            "SELECT bucket, upload, download
             FROM app_usage_1m
             WHERE bucket >= ?1 AND bucket < ?2 AND app_name <> ?3"
        } else {
            "SELECT bucket, upload, download
             FROM app_usage_5m
             WHERE bucket >= ?1 AND bucket < ?2 AND app_name <> ?3
             UNION ALL
             SELECT bucket - (bucket % 300), upload, download
             FROM app_usage_1m
             WHERE bucket >= ?1 AND bucket < ?2 AND app_name <> ?3"
        };
        let sql = format!(
            "WITH physical AS (
                 SELECT bucket - (bucket % {bucket_seconds}) AS bucket,
                        SUM(upload) AS upload, SUM(download) AS download
                 FROM interface_minute
                 WHERE bucket >= ?1 AND bucket < ?2
                 GROUP BY bucket - (bucket % {bucket_seconds})
             ), apps AS (
                 SELECT bucket, SUM(upload) AS upload, SUM(download) AS download
                 FROM ({app_rows})
                 GROUP BY bucket
             ), proxy AS (
                 SELECT bucket - (bucket % {bucket_seconds}) AS bucket,
                        SUM(upload) AS upload, SUM(download) AS download,
                        SUM(attributed_upload) AS attributed_upload,
                        SUM(attributed_download) AS attributed_download,
                        COALESCE(SUM(actor_upload), 0) AS actor_upload,
                        COALESCE(SUM(actor_download), 0) AS actor_download,
                        MIN(actor_upload IS NOT NULL AND actor_download IS NOT NULL) AS actor_known
                 FROM proxy_minute
                 WHERE bucket >= ?1 AND bucket < ?2
                 GROUP BY bucket - (bucket % {bucket_seconds})
             )
             SELECT physical.bucket, physical.upload, physical.download,
                    COALESCE(apps.upload, 0), COALESCE(apps.download, 0),
                    COALESCE(proxy.upload, 0), COALESCE(proxy.download, 0),
                    COALESCE(proxy.attributed_upload, 0),
                    COALESCE(proxy.attributed_download, 0),
                    COALESCE(proxy.actor_upload, 0), COALESCE(proxy.actor_download, 0),
                    COALESCE(proxy.actor_known, 0)
             FROM physical
             LEFT JOIN apps USING(bucket)
             LEFT JOIN proxy USING(bucket)"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(
            params![
                detail_bucket(start, bucket_seconds),
                end,
                flowwatch_core::UNKNOWN
            ],
            |row| {
                let physical_upload = db_u64(row.get(1)?);
                let physical_download = db_u64(row.get(2)?);
                let attributed_upload = db_u64(row.get(3)?);
                let attributed_download = db_u64(row.get(4)?);
                Ok(AttributionGap {
                    bucket: row.get(0)?,
                    physical_upload,
                    physical_download,
                    attributed_upload,
                    attributed_download,
                    gap_upload: physical_upload.saturating_sub(attributed_upload),
                    gap_download: physical_download.saturating_sub(attributed_download),
                    clash_upload: db_u64(row.get(5)?),
                    clash_download: db_u64(row.get(6)?),
                    clash_attributed_upload: db_u64(row.get(7)?),
                    clash_attributed_download: db_u64(row.get(8)?),
                    clash_actor_upload: db_u64(row.get(9)?),
                    clash_actor_download: db_u64(row.get(10)?),
                    clash_actor_bytes_known: row.get(11)?,
                })
            },
        )?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn direct_attribution_anomalies(
        &self,
        start: i64,
        end: i64,
    ) -> Result<Vec<AttributionAnomaly>> {
        let mut statement = self.connection.prepare(
            "WITH physical AS (
                 SELECT bucket - (bucket % 300) AS bucket,
                        SUM(upload) AS upload, SUM(download) AS download
                 FROM interface_minute
                 WHERE bucket >= ?1 AND bucket < ?2
                 GROUP BY bucket - (bucket % 300)
             ), direct AS (
                 SELECT bucket, SUM(upload) AS upload, SUM(download) AS download
                 FROM (
                    SELECT bucket, upload, download
                    FROM app_usage_5m
                    WHERE source='direct' AND bucket >= ?1 AND bucket < ?2
                    UNION ALL
                    SELECT bucket - (bucket % 300), upload, download
                    FROM app_usage_1m
                    WHERE source='direct' AND bucket >= ?1 AND bucket < ?2
                 )
                 GROUP BY bucket
             )
             SELECT direct.bucket, direct.upload, direct.download,
                    physical.upload, physical.download
             FROM direct JOIN physical USING(bucket)
             WHERE direct.bucket + 300 <= ?2
               AND (
                 direct.upload > physical.upload + MAX(1048576, physical.upload / 10)
                 OR direct.download > physical.download + MAX(1048576, physical.download / 10)
               )
             ORDER BY direct.bucket",
        )?;
        let rows = statement.query_map(params![five_minute_bucket(start), end], |row| {
            Ok(AttributionAnomaly {
                bucket: row.get(0)?,
                direct_upload: db_u64(row.get(1)?),
                direct_download: db_u64(row.get(2)?),
                physical_upload: db_u64(row.get(3)?),
                physical_download: db_u64(row.get(4)?),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn proxy_totals(&self, start: i64, end: i64) -> Result<CoreDelta> {
        self.connection
            .query_row(
                "SELECT COALESCE(SUM(upload), 0), COALESCE(SUM(download), 0),
                        COALESCE(SUM(attributed_upload), 0),
                        COALESCE(SUM(attributed_download), 0),
                        COALESCE(SUM(actor_upload), 0),
                        COALESCE(SUM(actor_download), 0),
                        COALESCE(MIN(actor_upload IS NOT NULL AND actor_download IS NOT NULL), 0)
                 FROM (
                    SELECT upload, download, attributed_upload, attributed_download,
                           actor_upload, actor_download
                    FROM proxy_minute WHERE bucket >= ?1 AND bucket < ?2
                    UNION ALL
                    SELECT upload, download, attributed_upload, attributed_download,
                           actor_upload, actor_download
                    FROM proxy_daily WHERE bucket >= ?3 AND bucket < ?4
                 )",
                params![minute_bucket(start), end, day_bucket(start), end],
                |row| {
                    Ok(CoreDelta {
                        upload: db_u64(row.get(0)?),
                        download: db_u64(row.get(1)?),
                        attributed_upload: db_u64(row.get(2)?),
                        attributed_download: db_u64(row.get(3)?),
                        actor_upload: db_u64(row.get(4)?),
                        actor_download: db_u64(row.get(5)?),
                        actor_bytes_known: row.get(6)?,
                    })
                },
            )
            .map_err(Into::into)
    }

    pub fn maintenance(&mut self, now: i64, detail_days: i64, daily_days: i64) -> Result<()> {
        let detail_cutoff = day_bucket(now - detail_days * 86_400);
        let daily_cutoff = day_bucket(now - daily_days * 86_400);
        let old_apps = self.read_old_apps(detail_cutoff)?;
        let old_interfaces = self.read_old_interfaces(detail_cutoff)?;
        let old_proxy = self.read_old_proxy(detail_cutoff)?;
        let transaction = self.connection.transaction()?;

        for ((bucket, app, source), delta) in old_apps {
            transaction.execute(
                "INSERT INTO app_usage_daily(
                    bucket, app_id, app_name, executable_path, source,
                    upload, download, connections, first_seen, last_seen
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(bucket, app_id, source) DO UPDATE SET
                    app_name=excluded.app_name,
                    executable_path=excluded.executable_path,
                    upload=upload+excluded.upload,
                    download=download+excluded.download,
                    connections=connections+excluded.connections,
                    first_seen=MIN(first_seen, excluded.first_seen),
                    last_seen=MAX(last_seen, excluded.last_seen)",
                params![
                    bucket,
                    app.id,
                    app.name,
                    app.executable_path,
                    source,
                    sql_u64(delta.upload),
                    sql_u64(delta.download),
                    sql_u64(delta.connections),
                    delta.first_seen,
                    delta.last_seen,
                ],
            )?;
        }
        for ((bucket, interface), delta) in old_interfaces {
            transaction.execute(
                "INSERT INTO interface_daily(bucket, interface, upload, download)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(bucket, interface) DO UPDATE SET
                    upload=upload+excluded.upload,
                    download=download+excluded.download",
                params![
                    bucket,
                    interface,
                    sql_u64(delta.upload),
                    sql_u64(delta.download)
                ],
            )?;
        }
        for (bucket, delta) in old_proxy {
            let actor_upload = delta.actor_bytes_known.then(|| sql_u64(delta.actor_upload));
            let actor_download = delta
                .actor_bytes_known
                .then(|| sql_u64(delta.actor_download));
            transaction.execute(
                "INSERT INTO proxy_daily(
                    bucket, upload, download, attributed_upload, attributed_download,
                    actor_upload, actor_download
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(bucket) DO UPDATE SET
                    upload=upload+excluded.upload,
                    download=download+excluded.download,
                    attributed_upload=attributed_upload+excluded.attributed_upload,
                    attributed_download=attributed_download+excluded.attributed_download,
                    actor_upload=CASE
                        WHEN proxy_daily.actor_upload IS NULL OR excluded.actor_upload IS NULL THEN NULL
                        ELSE proxy_daily.actor_upload+excluded.actor_upload
                    END,
                    actor_download=CASE
                        WHEN proxy_daily.actor_download IS NULL OR excluded.actor_download IS NULL THEN NULL
                        ELSE proxy_daily.actor_download+excluded.actor_download
                    END",
                params![
                    bucket,
                    sql_u64(delta.upload),
                    sql_u64(delta.download),
                    sql_u64(delta.attributed_upload),
                    sql_u64(delta.attributed_download),
                    actor_upload,
                    actor_download,
                ],
            )?;
        }
        transaction.execute(
            "DELETE FROM app_usage_5m WHERE bucket < ?1",
            [detail_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM app_usage_1m WHERE bucket < ?1",
            [detail_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM interface_minute WHERE bucket < ?1",
            [detail_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM proxy_minute WHERE bucket < ?1",
            [detail_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM app_usage_daily WHERE bucket < ?1",
            [daily_cutoff],
        )?;
        transaction.execute(
            "DELETE FROM interface_daily WHERE bucket < ?1",
            [daily_cutoff],
        )?;
        transaction.execute("DELETE FROM proxy_daily WHERE bucket < ?1", [daily_cutoff])?;
        transaction.commit()?;
        self.connection
            .execute_batch("PRAGMA incremental_vacuum(100); PRAGMA optimize;")?;
        Ok(())
    }

    pub fn integrity_check(&self) -> Result<String> {
        Ok(self
            .connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?)
    }

    pub fn size_bytes(&self) -> u64 {
        [
            self.path.clone(),
            PathBuf::from(format!("{}-wal", self.path.display())),
            PathBuf::from(format!("{}-shm", self.path.display())),
        ]
        .iter()
        .filter_map(|path| path.metadata().ok().map(|metadata| metadata.len()))
        .sum()
    }

    fn read_old_apps(
        &self,
        cutoff: i64,
    ) -> Result<HashMap<(i64, AppIdentity, String), UsageDelta>> {
        let mut statement = self.connection.prepare(
            "SELECT bucket, app_id, app_name, executable_path, source,
                    upload, download, connections, first_seen, last_seen
             FROM app_usage_5m WHERE bucket < ?1
             UNION ALL
             SELECT bucket, app_id, app_name, executable_path, source,
                    upload, download, connections, first_seen, last_seen
             FROM app_usage_1m WHERE bucket < ?1",
        )?;
        let rows = statement.query_map([cutoff], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                AppIdentity {
                    id: row.get(1)?,
                    name: row.get(2)?,
                    executable_path: row.get(3)?,
                },
                row.get::<_, String>(4)?,
                UsageDelta {
                    upload: db_u64(row.get(5)?),
                    download: db_u64(row.get(6)?),
                    connections: db_u64(row.get(7)?),
                    first_seen: row.get(8)?,
                    last_seen: row.get(9)?,
                },
            ))
        })?;
        let mut result: HashMap<(i64, AppIdentity, String), UsageDelta> = HashMap::new();
        for row in rows {
            let (bucket, app, source, delta) = row?;
            merge_usage(
                result.entry((day_bucket(bucket), app, source)).or_default(),
                &delta,
            );
        }
        Ok(result)
    }

    fn read_old_interfaces(&self, cutoff: i64) -> Result<HashMap<(i64, String), ByteDelta>> {
        let mut statement = self.connection.prepare(
            "SELECT bucket, interface, upload, download
             FROM interface_minute WHERE bucket < ?1",
        )?;
        let rows = statement.query_map([cutoff], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                db_u64(row.get(2)?),
                db_u64(row.get(3)?),
            ))
        })?;
        let mut result: HashMap<(i64, String), ByteDelta> = HashMap::new();
        for row in rows {
            let (bucket, interface, upload, download) = row?;
            result
                .entry((day_bucket(bucket), interface))
                .or_default()
                .add(upload, download);
        }
        Ok(result)
    }

    fn read_old_proxy(&self, cutoff: i64) -> Result<HashMap<i64, CoreDelta>> {
        let mut statement = self.connection.prepare(
            "SELECT bucket, upload, download, attributed_upload, attributed_download,
                    actor_upload, actor_download
             FROM proxy_minute WHERE bucket < ?1",
        )?;
        let rows = statement.query_map([cutoff], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                db_u64(row.get(1)?),
                db_u64(row.get(2)?),
                db_u64(row.get(3)?),
                db_u64(row.get(4)?),
                row.get::<_, Option<i64>>(5)?.map(db_u64),
                row.get::<_, Option<i64>>(6)?.map(db_u64),
            ))
        })?;
        let mut result: HashMap<i64, CoreDelta> = HashMap::new();
        for row in rows {
            let (
                bucket,
                upload,
                download,
                attributed_upload,
                attributed_download,
                actor_upload,
                actor_download,
            ) = row?;
            let delta = CoreDelta {
                upload,
                download,
                attributed_upload,
                attributed_download,
                actor_upload: actor_upload.unwrap_or_default(),
                actor_download: actor_download.unwrap_or_default(),
                actor_bytes_known: actor_upload.is_some() && actor_download.is_some(),
            };
            match result.entry(day_bucket(bucket)) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(delta);
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    merge_core_delta(entry.get_mut(), &delta);
                }
            }
        }
        Ok(result)
    }

    fn restrict_files(&self) -> Result<()> {
        restrict_file(&self.path)?;
        restrict_file(Path::new(&format!("{}-wal", self.path.display())))?;
        restrict_file(Path::new(&format!("{}-shm", self.path.display())))?;
        Ok(())
    }
}

pub fn minute_bucket(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(60)
}

pub fn five_minute_bucket(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(300)
}

fn detail_bucket(timestamp: i64, bucket_seconds: i64) -> i64 {
    timestamp - timestamp.rem_euclid(bucket_seconds)
}

pub fn day_bucket(timestamp: i64) -> i64 {
    let date = Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|value| value.date_naive())
        .unwrap_or_else(|| Local::now().date_naive());
    local_midnight(date)
}

fn local_midnight(date: NaiveDate) -> i64 {
    let naive = date.and_hms_opt(0, 0, 0).expect("midnight is valid");
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) | LocalResult::Ambiguous(value, _) => value.timestamp(),
        LocalResult::None => {
            Local
                .from_local_datetime(&date.and_hms_opt(1, 0, 0).expect("01:00 is valid"))
                .earliest()
                .expect("local date has a valid time")
                .timestamp()
                - 3600
        }
    }
}

fn merge_usage(target: &mut UsageDelta, source: &UsageDelta) {
    target.upload = target.upload.saturating_add(source.upload);
    target.download = target.download.saturating_add(source.download);
    target.connections = target.connections.saturating_add(source.connections);
    if target.first_seen == 0 || (source.first_seen > 0 && source.first_seen < target.first_seen) {
        target.first_seen = source.first_seen;
    }
    target.last_seen = target.last_seen.max(source.last_seen);
}

fn merge_core_delta(target: &mut CoreDelta, source: &CoreDelta) {
    target.upload = target.upload.saturating_add(source.upload);
    target.download = target.download.saturating_add(source.download);
    target.attributed_upload = target
        .attributed_upload
        .saturating_add(source.attributed_upload);
    target.attributed_download = target
        .attributed_download
        .saturating_add(source.attributed_download);
    target.actor_upload = target.actor_upload.saturating_add(source.actor_upload);
    target.actor_download = target.actor_download.saturating_add(source.actor_download);
    target.actor_bytes_known &= source.actor_bytes_known;
}

fn consolidate_app_usages(rows: impl IntoIterator<Item = AppUsage>) -> Vec<AppUsage> {
    let mut by_app: HashMap<String, AppUsage> = HashMap::new();
    for mut row in rows {
        improve_truncated_process_name(&mut row.app);
        let identity = row.app.clone();
        record_app_identity(&mut row, &identity);
        if let Some(bundle_id) = code_sign_clone_bundle_id(&row.app.executable_path) {
            row.app.id = format!("bundle:{bundle_id}");
        }
        match by_app.entry(row.app.id.clone()) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(row);
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                merge_app_usage(entry.get_mut(), row);
            }
        }
    }

    let process_ids: Vec<_> = by_app
        .iter()
        .filter(|(_, row)| row.app.id.starts_with("process:") && row.app.executable_path.is_empty())
        .map(|(id, _)| id.clone())
        .collect();
    for process_id in process_ids {
        let Some(process) = by_app.get(&process_id) else {
            continue;
        };
        let process_name = normalized_name(&process.app.name);
        let candidate_ids: Vec<_> = by_app
            .iter()
            .filter(|(id, candidate)| {
                *id != &process_id
                    && !candidate.app.executable_path.is_empty()
                    && normalized_name(&candidate.app.name) == process_name
                    && executable_file_name(&candidate.app.executable_path)
                        .is_some_and(|name| normalized_name(name) == process_name)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if candidate_ids.len() == 1 {
            let source = by_app
                .remove(&process_id)
                .expect("process alias exists while consolidating query results");
            let target = by_app
                .get_mut(&candidate_ids[0])
                .expect("unique alias target exists while consolidating query results");
            merge_app_usage(target, source);
        }
    }
    by_app.into_values().collect()
}

fn merge_app_usage(target: &mut AppUsage, source: AppUsage) {
    for identity_id in source.identity_ids {
        if !target.identity_ids.contains(&identity_id) {
            target.identity_ids.push(identity_id);
        }
    }
    for path in source.executable_paths {
        if !path.is_empty() && !target.executable_paths.contains(&path) {
            target.executable_paths.push(path);
        }
    }
    target.identity_count = target.identity_ids.len().max(1) as u32;
    if identity_quality(&source.app) > identity_quality(&target.app) {
        target.app = source.app;
    }
    target.direct_upload = target.direct_upload.saturating_add(source.direct_upload);
    target.direct_download = target
        .direct_download
        .saturating_add(source.direct_download);
    target.clash_upload = target.clash_upload.saturating_add(source.clash_upload);
    target.clash_download = target.clash_download.saturating_add(source.clash_download);
    target.enhanced_upload = target
        .enhanced_upload
        .saturating_add(source.enhanced_upload);
    target.enhanced_download = target
        .enhanced_download
        .saturating_add(source.enhanced_download);
    target.connections = target.connections.saturating_add(source.connections);
    target.last_seen = target.last_seen.max(source.last_seen);
}

fn record_app_identity(usage: &mut AppUsage, identity: &AppIdentity) {
    if !usage.identity_ids.contains(&identity.id) {
        usage.identity_ids.push(identity.id.clone());
    }
    if !identity.executable_path.is_empty()
        && !usage.executable_paths.contains(&identity.executable_path)
    {
        usage
            .executable_paths
            .push(identity.executable_path.clone());
    }
    usage.identity_count = usage.identity_ids.len().max(1) as u32;
}

fn identity_quality(identity: &AppIdentity) -> u8 {
    let mut quality = 0u8;
    if !identity.executable_path.is_empty() {
        quality = quality.saturating_add(1);
    }
    if identity.id.starts_with("bundle:") {
        quality = quality.saturating_add(2);
    }
    if !identity.executable_path.contains(".code_sign_clone/") {
        quality = quality.saturating_add(1);
    }
    quality
}

fn code_sign_clone_bundle_id(path: &str) -> Option<&str> {
    path.split('/').find_map(|component| {
        let id = component.strip_suffix(".code_sign_clone")?;
        (!id.is_empty() && id.contains('.')).then_some(id)
    })
}

fn improve_truncated_process_name(identity: &mut AppIdentity) {
    if identity.name.chars().count() != 15 {
        return;
    }
    let Some(file_name) = executable_file_name(&identity.executable_path) else {
        return;
    };
    if file_name.chars().count() > identity.name.chars().count()
        && normalized_name(file_name).starts_with(&normalized_name(&identity.name))
    {
        identity.name = file_name.to_string();
    }
}

fn executable_file_name(path: &str) -> Option<&str> {
    Path::new(path).file_name().and_then(|value| value.to_str())
}

fn normalized_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn sql_u64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn db_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_root(prefix: &str) -> PathBuf {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{timestamp}-{sequence}",
            std::process::id()
        ))
    }

    fn temporary_database() -> (PathBuf, Database) {
        let root = temporary_root("flowwatch-store");
        let path = root.join("traffic.sqlite3");
        let database = Database::open(&path).unwrap();
        (root, database)
    }

    #[test]
    fn flush_and_query_preserve_sources_and_totals() {
        let (root, mut database) = temporary_database();
        let now = Local::now().timestamp();
        let app = AppIdentity {
            id: "bundle:example".into(),
            name: "Example".into(),
            executable_path: "/Applications/Example.app/bin".into(),
        };
        let mut batch = FlushBatch::default();
        batch.apps.insert(
            (five_minute_bucket(now), app.clone(), UsageSource::Direct),
            UsageDelta {
                upload: 100,
                download: 200,
                connections: 1,
                first_seen: now,
                last_seen: now,
            },
        );
        batch.apps.insert(
            (five_minute_bucket(now), app, UsageSource::Clash),
            UsageDelta {
                upload: 30,
                download: 40,
                connections: 1,
                first_seen: now,
                last_seen: now,
            },
        );
        batch.interfaces.insert(
            (minute_bucket(now), "en0".into()),
            ByteDelta {
                upload: 180,
                download: 300,
            },
        );
        database.flush(&batch).unwrap();

        let apps = database.query_apps(now - 60, now + 60).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].upload(), 130);
        assert_eq!(apps[0].download(), 240);
        let interfaces = database.query_interfaces(now - 60, now + 60).unwrap();
        assert_eq!(interfaces[0].upload, 180);
        assert_eq!(database.integrity_check().unwrap(), "ok");
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn app_usage(id: &str, name: &str, path: &str, direct: u64, clash: u64) -> AppUsage {
        AppUsage {
            app: AppIdentity {
                id: id.into(),
                name: name.into(),
                executable_path: path.into(),
            },
            direct_upload: direct,
            clash_download: clash,
            connections: 1,
            last_seen: 100,
            ..AppUsage::default()
        }
    }

    #[test]
    fn consolidates_strong_historical_identity_aliases() {
        let rows = consolidate_app_usages([
            app_usage(
                "bundle:com.microsoft.edgemac",
                "Microsoft Edge",
                "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
                100,
                0,
            ),
            app_usage(
                "path:/private/var/folders/X/com.microsoft.edgemac.code_sign_clone/clone/Microsoft Edge.app.bundle/Contents/MacOS/Microsoft Edge",
                "Microsoft Edge",
                "/private/var/folders/X/com.microsoft.edgemac.code_sign_clone/clone/Microsoft Edge.app.bundle/Contents/MacOS/Microsoft Edge",
                0,
                20,
            ),
            app_usage("process:gh", "gh", "", 10, 0),
            app_usage(
                "path:/opt/homebrew/bin/gh",
                "gh",
                "/opt/homebrew/bin/gh",
                30,
                0,
            ),
        ]);

        assert_eq!(rows.len(), 2);
        let edge = rows
            .iter()
            .find(|row| row.app.id == "bundle:com.microsoft.edgemac")
            .unwrap();
        assert_eq!(edge.upload(), 100);
        assert_eq!(edge.download(), 20);
        assert!(edge.app.executable_path.starts_with("/Applications/"));
        let gh = rows.iter().find(|row| row.app.name == "gh").unwrap();
        assert_eq!(gh.upload(), 40);
        assert_eq!(gh.connections, 2);
    }

    #[test]
    fn keeps_ambiguous_process_aliases_separate() {
        let rows = consolidate_app_usages([
            app_usage("process:tool", "tool", "", 10, 0),
            app_usage("path:/one/tool", "tool", "/one/tool", 20, 0),
            app_usage("path:/two/tool", "tool", "/two/tool", 30, 0),
        ]);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn repairs_truncated_process_display_names() {
        let rows = consolidate_app_usages([app_usage(
            "path:/System/Library/CloudTelemetryService",
            "CloudTelemetryS",
            "/System/Library/CloudTelemetryService",
            1,
            0,
        )]);
        assert_eq!(rows[0].app.name, "CloudTelemetryService");
    }

    #[test]
    fn groups_same_named_executable_copies_for_display() {
        let rows = consolidate_app_usages([
            app_usage(
                "path:/project-a/chrome-headless-shell",
                "chrome-headless-shell",
                "/project-a/chrome-headless-shell",
                100,
                0,
            ),
            app_usage(
                "path:/project-b/chrome-headless-shell",
                "chrome-headless-shell",
                "/project-b/chrome-headless-shell",
                200,
                0,
            ),
        ]);
        let rows = Database::group_apps_for_display(rows);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].upload(), 300);
        assert_eq!(rows[0].identity_count, 2);
        assert_eq!(rows[0].executable_paths.len(), 2);
        assert!(rows[0].app.id.starts_with("group:"));
    }

    #[test]
    fn one_minute_apps_and_gaps_use_the_fine_detail_table() {
        let (root, mut database) = temporary_database();
        let bucket = minute_bucket(Local::now().timestamp()).saturating_sub(60);
        let app = AppIdentity::process("Example", "/Applications/Example.app/bin");
        let mut old_batch = FlushBatch::default();
        old_batch.apps.insert(
            (five_minute_bucket(bucket), app.clone(), UsageSource::Direct),
            UsageDelta {
                upload: 1_000,
                download: 2_000,
                connections: 1,
                first_seen: bucket - 1,
                last_seen: bucket - 1,
            },
        );
        database.flush(&old_batch).unwrap();
        database.set_setting("app_granularity", "1m").unwrap();
        database
            .set_meta(&BTreeMap::from([(
                "app_one_minute_started_at".to_string(),
                bucket.to_string(),
            )]))
            .unwrap();
        let mut batch = FlushBatch {
            app_bucket_seconds: 60,
            ..FlushBatch::default()
        };
        batch.apps.insert(
            (bucket, app, UsageSource::Direct),
            UsageDelta {
                upload: 100,
                download: 200,
                connections: 1,
                first_seen: bucket,
                last_seen: bucket,
            },
        );
        batch.interfaces.insert(
            (bucket, "en0".into()),
            ByteDelta {
                upload: 180,
                download: 300,
            },
        );
        let mut proxy = CoreDelta::default();
        proxy.add_with_actor(120, 240, 80, 160, 100, 200);
        batch.proxy_totals.insert(bucket, proxy);
        database.flush(&batch).unwrap();

        let apps = database.query_apps(bucket, bucket + 60).unwrap();
        assert_eq!(apps.len(), 1);
        assert_eq!((apps[0].upload(), apps[0].download()), (100, 200));
        let stored: i64 = database
            .connection
            .query_row("SELECT COUNT(*) FROM app_usage_1m", [], |row| row.get(0))
            .unwrap();
        assert_eq!(stored, 1);

        let gaps = database
            .query_attribution_gaps(bucket, bucket + 60, 60)
            .unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!((gaps[0].gap_upload, gaps[0].gap_download), (80, 100));
        assert_eq!(
            (gaps[0].clash_actor_upload, gaps[0].clash_actor_download),
            (100, 200)
        );
        assert!(gaps[0].clash_actor_bytes_known);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migrates_legacy_proxy_rows_without_inventing_actor_bytes() {
        let root = temporary_root("flowwatch-legacy-store");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("traffic.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
                 INSERT INTO meta VALUES ('schema_version', '1');
                 CREATE TABLE proxy_minute (
                    bucket INTEGER PRIMARY KEY,
                    upload INTEGER NOT NULL,
                    download INTEGER NOT NULL,
                    attributed_upload INTEGER NOT NULL,
                    attributed_download INTEGER NOT NULL
                 );
                 CREATE TABLE proxy_daily (
                    bucket INTEGER PRIMARY KEY,
                    upload INTEGER NOT NULL,
                    download INTEGER NOT NULL,
                    attributed_upload INTEGER NOT NULL,
                    attributed_download INTEGER NOT NULL
                 );",
            )
            .unwrap();
        let old_bucket = minute_bucket(Local::now().timestamp()).saturating_sub(120);
        connection
            .execute(
                "INSERT INTO proxy_minute VALUES (?1, 100, 200, 80, 160)",
                [old_bucket],
            )
            .unwrap();
        drop(connection);

        let mut database = Database::open(&path).unwrap();
        assert_eq!(
            database.meta().unwrap().get("schema_version").unwrap(),
            &SCHEMA_VERSION.to_string()
        );
        let legacy = database.proxy_totals(old_bucket, old_bucket + 60).unwrap();
        assert!(!legacy.actor_bytes_known);
        assert_eq!((legacy.upload, legacy.download), (100, 200));

        let new_bucket = old_bucket + 60;
        let mut batch = FlushBatch::default();
        let mut proxy = CoreDelta::default();
        proxy.add_with_actor(50, 60, 40, 50, 45, 55);
        batch.proxy_totals.insert(new_bucket, proxy);
        database.flush(&batch).unwrap();
        let current = database.proxy_totals(new_bucket, new_bucket + 60).unwrap();
        assert!(current.actor_bytes_known);
        assert_eq!((current.actor_upload, current.actor_download), (45, 55));
        let combined = database.proxy_totals(old_bucket, new_bucket + 60).unwrap();
        assert!(!combined.actor_bytes_known);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clash_secret_round_trips_inside_database() {
        let (root, mut database) = temporary_database();
        let config = ClashConfig {
            enabled: true,
            controller: "http://127.0.0.1:9090".into(),
            secret: "leading-zero-secret".into(),
        };
        database.set_clash_config(&config).unwrap();
        let loaded = database.clash_config().unwrap().unwrap();
        assert_eq!(loaded.secret, config.secret);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flags_direct_bucket_that_exceeds_physical_bounds() {
        let (root, mut database) = temporary_database();
        let bucket = five_minute_bucket(Local::now().timestamp());
        let mut batch = FlushBatch::default();
        batch.apps.insert(
            (
                bucket,
                AppIdentity::process("Example", "/Applications/Example.app/bin"),
                UsageSource::Direct,
            ),
            UsageDelta {
                upload: 2 * 1_048_576,
                download: 0,
                connections: 1,
                first_seen: bucket,
                last_seen: bucket,
            },
        );
        batch.interfaces.insert(
            (bucket, "en0".into()),
            ByteDelta {
                upload: 100,
                download: 100,
            },
        );
        database.flush(&batch).unwrap();

        let anomalies = database
            .direct_attribution_anomalies(bucket, bucket + 301)
            .unwrap();
        assert_eq!(anomalies.len(), 1);
        assert_eq!(anomalies[0].direct_upload, 2 * 1_048_576);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn open_does_not_change_existing_parent_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_root("flowwatch-parent");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let database = Database::open(root.join("traffic.sqlite3")).unwrap();
        let mode = root.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
