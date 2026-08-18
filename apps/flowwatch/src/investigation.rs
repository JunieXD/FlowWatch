use anyhow::{Context, Result, bail};
use flowwatch_store::Database;
use serde::{Deserialize, Serialize};

pub const SETTING_KEY: &str = "investigation_state";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InvestigationState {
    pub started_at: i64,
    pub ends_at: i64,
    pub original_poll_seconds: u64,
    pub original_app_granularity: String,
}

impl InvestigationState {
    pub fn active_at(&self, now: i64) -> bool {
        self.started_at <= now && now < self.ends_at
    }

    pub fn remaining_seconds(&self, now: i64) -> u64 {
        u64::try_from(self.ends_at.saturating_sub(now).max(0)).unwrap_or_default()
    }
}

pub fn load(database: &Database) -> Result<Option<InvestigationState>> {
    database
        .setting(SETTING_KEY)?
        .map(|raw| serde_json::from_str(&raw).context("无法解析调查模式状态"))
        .transpose()
}

pub fn start(
    database: &mut Database,
    now: i64,
    duration_seconds: u64,
    original_poll_seconds: u64,
    original_app_granularity: &str,
) -> Result<InvestigationState> {
    if let Some(existing) = load(database)?
        && existing.active_at(now)
    {
        bail!("调查模式已经在运行，请先查看状态或停止当前调查");
    }
    let duration = i64::try_from(duration_seconds).context("调查时长过大")?;
    let state = InvestigationState {
        started_at: now,
        ends_at: now.saturating_add(duration),
        original_poll_seconds,
        original_app_granularity: original_app_granularity.to_string(),
    };
    database.set_setting(SETTING_KEY, &serde_json::to_string(&state)?)?;
    Ok(state)
}

pub fn stop(database: &mut Database) -> Result<Option<InvestigationState>> {
    let existing = load(database)?;
    database.delete_setting(SETTING_KEY)?;
    Ok(existing)
}

pub fn clear_if_expired(database: &mut Database, now: i64) -> Result<bool> {
    let expired = load(database)?.is_some_and(|state| !state.active_at(now));
    if expired {
        database.delete_setting(SETTING_KEY)?;
    }
    Ok(expired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_database() -> (std::path::PathBuf, Database) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "flowwatch-investigation-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        let database = Database::open(root.join("traffic.sqlite3")).unwrap();
        (root, database)
    }

    #[test]
    fn investigation_round_trips_and_preserves_original_settings() {
        let (root, mut database) = temporary_database();
        let state = start(&mut database, 1_000, 1_800, 3, "5m").unwrap();
        assert_eq!(load(&database).unwrap(), Some(state.clone()));
        assert!(state.active_at(2_000));
        assert!(!state.active_at(2_800));
        assert_eq!(state.remaining_seconds(2_000), 800);
        assert!(start(&mut database, 2_000, 600, 3, "5m").is_err());
        assert_eq!(stop(&mut database).unwrap(), Some(state));
        assert_eq!(load(&database).unwrap(), None);
        start(&mut database, 3_000, 300, 3, "5m").unwrap();
        assert!(!clear_if_expired(&mut database, 3_299).unwrap());
        assert!(clear_if_expired(&mut database, 3_300).unwrap());
        assert_eq!(load(&database).unwrap(), None);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
