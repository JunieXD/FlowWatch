use anyhow::{Context, Result, bail};
use chrono::{Datelike, Local, TimeZone};
use flowwatch_store::{AlertRule, Database, day_bucket};
use std::collections::HashSet;
use std::process::Command;

pub trait Notifier {
    fn notify(&self, title: &str, message: &str) -> Result<()>;
}

pub struct MacNotifier;

impl Notifier for MacNotifier {
    fn notify(&self, title: &str, message: &str) -> Result<()> {
        let output = Command::new("/usr/bin/osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
                title,
                message,
            ])
            .output()
            .context("无法调用 macOS 通知服务")?;
        if !output.status.success() {
            bail!(
                "macOS 通知失败：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggeredAlert {
    pub rule_id: i64,
    pub stage: u8,
    pub used_bytes: u64,
    pub period_start: i64,
}

pub fn check_and_notify<N: Notifier>(
    database: &mut Database,
    now: i64,
    notifier: &N,
) -> Result<Vec<TriggeredAlert>> {
    let rules = database.alert_rules()?;
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    let mut triggered = Vec::new();
    for rule in rules.into_iter().filter(|rule| rule.enabled) {
        let period_start = alert_period_start(&rule.period, now)?;
        let used = rule_usage(database, &rule, period_start, now.saturating_add(1))?;
        let percent = (used as u128).saturating_mul(100) / rule.threshold_bytes.max(1) as u128;
        let stage = if percent >= 100 {
            Some(100u8)
        } else if percent >= 80 {
            Some(80u8)
        } else {
            None
        };
        let Some(stage) = stage else {
            continue;
        };
        if database.alert_event_exists(rule.id, period_start, stage)? {
            continue;
        }
        let message = alert_message(&rule, used, stage);
        notifier.notify("FlowWatch 流量提醒", &message)?;
        database.record_alert_event(rule.id, period_start, stage, now)?;
        triggered.push(TriggeredAlert {
            rule_id: rule.id,
            stage,
            used_bytes: used,
            period_start,
        });
    }
    Ok(triggered)
}

pub fn send_test_notification<N: Notifier>(notifier: &N) -> Result<()> {
    notifier.notify(
        "FlowWatch 测试通知",
        "通知功能正常。达到流量限额时，FlowWatch 会在这里提醒你。",
    )
}

fn alert_period_start(period: &str, now: i64) -> Result<i64> {
    match period {
        "daily" => Ok(day_bucket(now)),
        "monthly" => {
            let local = Local
                .timestamp_opt(now, 0)
                .single()
                .context("无法把提醒时间转换为本地时间")?;
            Local
                .with_ymd_and_hms(local.year(), local.month(), 1, 0, 0, 0)
                .single()
                .map(|value| value.timestamp())
                .context("无法确定本月开始时间")
        }
        value => bail!("提醒规则的周期无效：{value}"),
    }
}

fn rule_usage(database: &Database, rule: &AlertRule, start: i64, end: i64) -> Result<u64> {
    if rule.app_ids.is_empty() {
        return Ok(database
            .query_interfaces(start, end)?
            .into_iter()
            .fold(0u64, |total, row| {
                total.saturating_add(row.upload.saturating_add(row.download))
            }));
    }
    let ids: HashSet<_> = rule.app_ids.iter().map(String::as_str).collect();
    Ok(database
        .query_apps(start, end)?
        .into_iter()
        .filter(|row| {
            ids.contains(row.app.id.as_str())
                || row.identity_ids.iter().any(|id| ids.contains(id.as_str()))
        })
        .fold(0u64, |total, row| {
            total.saturating_add(row.upload().saturating_add(row.download()))
        }))
}

fn alert_message(rule: &AlertRule, used: u64, stage: u8) -> String {
    let subject = if rule.app_ids.is_empty() {
        "这台 Mac".to_string()
    } else {
        format!("应用“{}”", rule.app_name)
    };
    let period = if rule.period == "monthly" {
        "本月"
    } else {
        "今天"
    };
    let caveat = if rule.app_ids.is_empty() {
        ""
    } else {
        "；应用数据可能不完整"
    };
    format!(
        "{subject}{period}已使用 {}，达到 {} 限额的 {stage}%{caveat}",
        human_bytes(used),
        human_bytes(rule.threshold_bytes),
    )
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
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

#[cfg(test)]
mod tests {
    use super::*;
    use flowwatch_core::{AppIdentity, ByteDelta, UsageDelta, UsageSource};
    use flowwatch_store::{FlushBatch, minute_bucket};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct FakeNotifier {
        messages: Mutex<Vec<String>>,
    }

    struct FailingNotifier;

    impl Notifier for FailingNotifier {
        fn notify(&self, _title: &str, _message: &str) -> Result<()> {
            bail!("测试通知失败")
        }
    }

    impl Notifier for FakeNotifier {
        fn notify(&self, _title: &str, message: &str) -> Result<()> {
            self.messages.lock().unwrap().push(message.to_string());
            Ok(())
        }
    }

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn alerts_trigger_at_eighty_and_one_hundred_percent_only_once() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "flowwatch-alerts-{}-{timestamp}-{sequence}",
            std::process::id()
        ));
        let mut database = Database::open(root.join("traffic.sqlite3")).unwrap();
        let now = Local::now().timestamp();
        let bucket = minute_bucket(now);
        let app = AppIdentity {
            id: "bundle:example".into(),
            name: "Example".into(),
            executable_path: "/Applications/Example.app/Example".into(),
        };
        let mut batch = FlushBatch::default();
        batch.interfaces.insert(
            (bucket, "en0".into()),
            ByteDelta {
                upload: 60,
                download: 60,
            },
        );
        batch.apps.insert(
            (bucket, app, UsageSource::Direct),
            UsageDelta {
                upload: 45,
                download: 45,
                connections: 1,
                first_seen: now,
                last_seen: now,
            },
        );
        database.flush(&batch).unwrap();
        let system_rule = database.add_alert_rule("daily", &[], "", 100, now).unwrap();
        let app_rule = database
            .add_alert_rule("daily", &["bundle:example".into()], "Example", 100, now)
            .unwrap();
        let notifier = FakeNotifier::default();

        let first = check_and_notify(&mut database, now, &notifier).unwrap();
        assert_eq!(first.len(), 2);
        assert!(
            first
                .iter()
                .any(|alert| alert.rule_id == system_rule && alert.stage == 100)
        );
        assert!(
            first
                .iter()
                .any(|alert| alert.rule_id == app_rule && alert.stage == 80)
        );
        assert!(
            check_and_notify(&mut database, now, &notifier)
                .unwrap()
                .is_empty()
        );

        let mut more = FlushBatch::default();
        more.apps.insert(
            (
                bucket,
                AppIdentity {
                    id: "bundle:example".into(),
                    name: "Example".into(),
                    executable_path: "/Applications/Example.app/Example".into(),
                },
                UsageSource::Direct,
            ),
            UsageDelta {
                upload: 10,
                download: 10,
                connections: 1,
                first_seen: now,
                last_seen: now,
            },
        );
        database.flush(&more).unwrap();
        let second = check_and_notify(&mut database, now, &notifier).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].rule_id, app_rule);
        assert_eq!(second[0].stage, 100);
        assert!(
            notifier
                .messages
                .lock()
                .unwrap()
                .iter()
                .any(|message| message.contains("应用数据可能不完整"))
        );
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn period_boundaries_use_local_day_and_month() {
        let now = Local
            .with_ymd_and_hms(2026, 8, 19, 14, 30, 0)
            .single()
            .unwrap()
            .timestamp();
        assert_eq!(alert_period_start("daily", now).unwrap(), day_bucket(now));
        assert_eq!(
            alert_period_start("monthly", now).unwrap(),
            Local
                .with_ymd_and_hms(2026, 8, 1, 0, 0, 0)
                .single()
                .unwrap()
                .timestamp()
        );
        assert!(alert_period_start("weekly", now).is_err());
    }

    #[test]
    fn disabled_rules_are_skipped_and_notification_failures_are_retried() {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "flowwatch-alert-failure-{}-{timestamp}",
            std::process::id()
        ));
        let mut database = Database::open(root.join("traffic.sqlite3")).unwrap();
        let now = Local::now().timestamp();
        let mut batch = FlushBatch::default();
        batch.interfaces.insert(
            (minute_bucket(now), "en0".into()),
            ByteDelta {
                upload: 100,
                download: 0,
            },
        );
        database.flush(&batch).unwrap();
        let id = database.add_alert_rule("daily", &[], "", 100, now).unwrap();
        database.set_alert_rule_enabled(id, false).unwrap();
        assert!(
            check_and_notify(&mut database, now, &FailingNotifier)
                .unwrap()
                .is_empty()
        );

        database.set_alert_rule_enabled(id, true).unwrap();
        assert!(check_and_notify(&mut database, now, &FailingNotifier).is_err());
        assert!(
            !database
                .alert_event_exists(id, day_bucket(now), 100)
                .unwrap()
        );
        assert!(check_and_notify(&mut database, now, &FailingNotifier).is_err());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
