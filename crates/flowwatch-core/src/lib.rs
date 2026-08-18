use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::Hash;
use std::marker::PhantomData;
use std::net::IpAddr;

pub const UNKNOWN: &str = "(unknown)";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClashConfig {
    pub enabled: bool,
    pub controller: String,
    pub secret: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Authoritative,
    Enhanced,
    BestEffort,
    Supplemental,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    Direct,
    Clash,
    Enhanced,
}

impl UsageSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Clash => "clash",
            Self::Enhanced => "enhanced",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppIdentity {
    pub id: String,
    pub name: String,
    pub executable_path: String,
}

impl AppIdentity {
    pub fn process(name: impl Into<String>, path: impl Into<String>) -> Self {
        let name = nonempty(name.into(), UNKNOWN);
        let executable_path = path.into();
        let id = if executable_path.is_empty() {
            format!("process:{}", name.to_lowercase())
        } else {
            format!("path:{executable_path}")
        };
        Self {
            id,
            name,
            executable_path,
        }
    }

    pub fn is_known(&self) -> bool {
        self.name != UNKNOWN
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LocalEndpoint {
    pub network: String,
    pub address: String,
    pub port: u16,
}

impl LocalEndpoint {
    pub fn new(network: &str, address: &str, port: u16) -> Option<Self> {
        if port == 0 {
            return None;
        }
        let network = network.trim().to_ascii_lowercase();
        let network = if network.starts_with("tcp") {
            "tcp"
        } else if network.starts_with("udp") || network.starts_with("quic") {
            "udp"
        } else {
            return None;
        };
        let raw_address = address.trim().trim_matches(['[', ']']);
        let address = raw_address
            .parse::<IpAddr>()
            .map(|value| value.to_string())
            .unwrap_or_else(|_| raw_address.to_ascii_lowercase());
        if address.is_empty() || address == "*" {
            return None;
        }
        Some(Self {
            network: network.to_string(),
            address,
            port,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppTrafficDelta {
    pub app: AppIdentity,
    pub upload: u64,
    pub download: u64,
    pub connections: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointOwner {
    pub endpoint: LocalEndpoint,
    pub app: AppIdentity,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessTrafficSample {
    pub apps: Vec<AppTrafficDelta>,
    pub socket_owners: Vec<EndpointOwner>,
    pub active_flows: usize,
    pub tracked_flows: usize,
    pub baseline_discarded: bool,
    pub collector_restarts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessFlowKey {
    pub app: AppIdentity,
    pub target: String,
    pub interface: String,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProxyFlowKey {
    pub app: AppIdentity,
    pub source_ip: String,
    pub inbound: String,
    pub network: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CounterObservation<K> {
    pub id: String,
    pub upload: u64,
    pub download: u64,
    pub key: K,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteDelta {
    pub upload: u64,
    pub download: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreDelta {
    pub upload: u64,
    pub download: u64,
    pub attributed_upload: u64,
    pub attributed_download: u64,
    pub actor_upload: u64,
    pub actor_download: u64,
    /// Whether actor bytes were available for every row in this aggregate.
    pub actor_bytes_known: bool,
}

impl CoreDelta {
    pub fn add(
        &mut self,
        upload: u64,
        download: u64,
        attributed_upload: u64,
        attributed_download: u64,
    ) {
        self.upload = self.upload.saturating_add(upload);
        self.download = self.download.saturating_add(download);
        self.attributed_upload = self.attributed_upload.saturating_add(attributed_upload);
        self.attributed_download = self.attributed_download.saturating_add(attributed_download);
    }

    pub fn add_with_actor(
        &mut self,
        upload: u64,
        download: u64,
        attributed_upload: u64,
        attributed_download: u64,
        actor_upload: u64,
        actor_download: u64,
    ) {
        self.add(upload, download, attributed_upload, attributed_download);
        self.actor_upload = self.actor_upload.saturating_add(actor_upload);
        self.actor_download = self.actor_download.saturating_add(actor_download);
        self.actor_bytes_known = true;
    }
}

impl ByteDelta {
    pub fn add(&mut self, upload: u64, download: u64) {
        self.upload = self.upload.saturating_add(upload);
        self.download = self.download.saturating_add(download);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageDelta {
    pub upload: u64,
    pub download: u64,
    pub connections: u64,
    pub first_seen: i64,
    pub last_seen: i64,
}

impl UsageDelta {
    pub fn add(&mut self, upload: u64, download: u64, connections: u64, now: i64) {
        self.upload = self.upload.saturating_add(upload);
        self.download = self.download.saturating_add(download);
        self.connections = self.connections.saturating_add(connections);
        if self.first_seen == 0 || now < self.first_seen {
            self.first_seen = now;
        }
        self.last_seen = self.last_seen.max(now);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviousFlow {
    upload: u64,
    download: u64,
    counted: bool,
    last_seen_at: i64,
}

#[derive(Debug, Clone)]
pub struct FlowDeltaTracker<K> {
    previous: HashMap<String, PreviousFlow>,
    baselined: bool,
    retention_seconds: i64,
    max_tracked_flows: usize,
    new_flow_policy: NewFlowPolicy,
    key: PhantomData<fn() -> K>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NewFlowPolicy {
    #[default]
    CountCurrent,
    Baseline,
}

impl<K> Default for FlowDeltaTracker<K> {
    fn default() -> Self {
        Self::with_retention(86_400, 100_000)
    }
}

impl<K> FlowDeltaTracker<K> {
    pub fn with_retention(retention_seconds: i64, max_tracked_flows: usize) -> Self {
        Self::with_retention_and_policy(
            retention_seconds,
            max_tracked_flows,
            NewFlowPolicy::CountCurrent,
        )
    }

    pub fn with_retention_and_policy(
        retention_seconds: i64,
        max_tracked_flows: usize,
        new_flow_policy: NewFlowPolicy,
    ) -> Self {
        Self {
            previous: HashMap::new(),
            baselined: false,
            retention_seconds: retention_seconds.max(0),
            max_tracked_flows: max_tracked_flows.max(1),
            new_flow_policy,
            key: PhantomData,
        }
    }

    pub fn tracked_flows(&self) -> usize {
        self.previous.len()
    }
}

impl<K> FlowDeltaTracker<K>
where
    K: Clone + Eq + Hash,
{
    pub fn apply(
        &mut self,
        observations: impl IntoIterator<Item = CounterObservation<K>>,
        now: i64,
    ) -> HashMap<K, UsageDelta> {
        let initial = !self.baselined;
        let mut next = HashMap::new();
        let mut deltas: HashMap<K, UsageDelta> = HashMap::new();

        for observation in observations {
            let old = self.previous.remove(&observation.id);
            let (upload, download, mut counted) = if initial {
                (0, 0, false)
            } else if let Some(old) = &old {
                let reset = observation.upload < old.upload || observation.download < old.download;
                if reset && self.new_flow_policy == NewFlowPolicy::Baseline {
                    (0, 0, false)
                } else {
                    let counted = if reset { false } else { old.counted };
                    (
                        monotonic_delta(observation.upload, old.upload),
                        monotonic_delta(observation.download, old.download),
                        counted,
                    )
                }
            } else if self.new_flow_policy == NewFlowPolicy::Baseline {
                (0, 0, false)
            } else {
                (observation.upload, observation.download, false)
            };

            let connections = if (upload > 0 || download > 0) && !counted {
                counted = true;
                1
            } else {
                0
            };
            if upload > 0 || download > 0 || connections > 0 {
                deltas.entry(observation.key.clone()).or_default().add(
                    upload,
                    download,
                    connections,
                    now,
                );
            }
            next.insert(
                observation.id,
                PreviousFlow {
                    upload: observation.upload,
                    download: observation.download,
                    counted,
                    last_seen_at: now,
                },
            );
        }

        for (id, old) in self.previous.drain() {
            if now.saturating_sub(old.last_seen_at) <= self.retention_seconds {
                next.insert(id, old);
            }
        }
        if next.len() > self.max_tracked_flows {
            let mut entries: Vec<_> = next.drain().collect();
            entries.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1.last_seen_at));
            entries.truncate(self.max_tracked_flows);
            next.extend(entries);
        }
        self.previous = next;
        self.baselined = true;
        deltas
    }
}

#[derive(Debug, Clone)]
pub struct AbsoluteCounterTracker<K> {
    previous: HashMap<K, ByteDelta>,
    baselined: bool,
}

impl<K> Default for AbsoluteCounterTracker<K> {
    fn default() -> Self {
        Self {
            previous: HashMap::new(),
            baselined: false,
        }
    }
}

impl<K> AbsoluteCounterTracker<K>
where
    K: Clone + Eq + Hash,
{
    pub fn from_baseline(previous: HashMap<K, ByteDelta>) -> Self {
        let baselined = !previous.is_empty();
        Self {
            previous,
            baselined,
        }
    }

    pub fn baseline(&self) -> &HashMap<K, ByteDelta> {
        &self.previous
    }

    pub fn apply(&mut self, current: HashMap<K, ByteDelta>) -> HashMap<K, ByteDelta> {
        let initial = !self.baselined;
        let mut result = HashMap::new();
        for (key, counters) in &current {
            let Some(old) = self.previous.get(key) else {
                continue;
            };
            if initial {
                continue;
            }
            let delta = ByteDelta {
                upload: monotonic_delta(counters.upload, old.upload),
                download: monotonic_delta(counters.download, old.download),
            };
            if delta.upload > 0 || delta.download > 0 {
                result.insert(key.clone(), delta);
            }
        }
        self.previous = current;
        self.baselined = true;
        result
    }
}

pub trait TrafficBackend {
    type Error: std::fmt::Debug + std::fmt::Display + Send + Sync + 'static;

    fn interface_counters(&mut self) -> Result<HashMap<String, ByteDelta>, Self::Error>;
    fn process_traffic(&mut self) -> Result<ProcessTrafficSample, Self::Error>;
}

pub fn monotonic_delta(current: u64, previous: u64) -> u64 {
    if current >= previous {
        current - previous
    } else {
        current
    }
}

pub fn is_proxy_carrier(app: &AppIdentity) -> bool {
    let executable = PathLikeName::from_path(&app.executable_path)
        .unwrap_or_else(|| app.name.to_ascii_lowercase());
    executable == "clash"
        || executable == "clash-meta"
        || executable == "verge-mihomo"
        || executable.starts_with("mihomo")
        || executable.starts_with("clash-")
}

struct PathLikeName;

impl PathLikeName {
    fn from_path(path: &str) -> Option<String> {
        (!path.is_empty()).then(|| {
            path.rsplit(['/', '\\'])
                .next()
                .unwrap_or(path)
                .to_ascii_lowercase()
        })
    }
}

fn nonempty(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(id: &str, upload: u64, download: u64, key: &str) -> CounterObservation<String> {
        CounterObservation {
            id: id.to_string(),
            upload,
            download,
            key: key.to_string(),
        }
    }

    #[test]
    fn flow_tracker_baselines_then_counts_new_and_reset_connections() {
        let mut tracker = FlowDeltaTracker::default();
        assert!(
            tracker
                .apply([observation("one", 100, 200, "app")], 10)
                .is_empty()
        );

        let deltas = tracker.apply(
            [
                observation("one", 150, 260, "app"),
                observation("two", 10, 20, "app"),
            ],
            15,
        );
        assert_eq!(deltas["app"].upload, 60);
        assert_eq!(deltas["app"].download, 80);
        assert_eq!(deltas["app"].connections, 2);

        let deltas = tracker.apply([observation("one", 25, 10, "app")], 20);
        assert_eq!(deltas["app"].upload, 25);
        assert_eq!(deltas["app"].download, 10);
        assert_eq!(deltas["app"].connections, 1);
    }

    #[test]
    fn long_missing_period_does_not_double_count_reappearing_flow() {
        let mut tracker = FlowDeltaTracker::with_retention(7_200, 100);
        tracker.apply([observation("one", 100, 100, "app")], 10);
        tracker.apply([], 15);
        tracker.apply([], 3_600);
        let deltas = tracker.apply([observation("one", 130, 140, "app")], 4_000);
        assert_eq!(deltas["app"].upload, 30);
        assert_eq!(deltas["app"].download, 40);
        assert_eq!(deltas["app"].connections, 1);
    }

    #[test]
    fn expired_tombstone_treats_reappearing_id_as_new() {
        let mut tracker = FlowDeltaTracker::with_retention(60, 100);
        tracker.apply([observation("one", 100, 100, "app")], 10);
        tracker.apply([], 71);
        let deltas = tracker.apply([observation("one", 130, 140, "app")], 80);
        assert_eq!(deltas["app"].upload, 130);
        assert_eq!(deltas["app"].download, 140);
        assert_eq!(deltas["app"].connections, 1);
    }

    #[test]
    fn conservative_tracker_baselines_new_and_reset_flows() {
        let mut tracker =
            FlowDeltaTracker::with_retention_and_policy(60, 100, NewFlowPolicy::Baseline);
        tracker.apply([observation("one", 100, 100, "app")], 10);

        assert!(
            tracker
                .apply([observation("two", 5_000, 7_000, "app")], 15)
                .is_empty()
        );
        let deltas = tracker.apply([observation("two", 5_100, 7_200, "app")], 20);
        assert_eq!(deltas["app"].upload, 100);
        assert_eq!(deltas["app"].download, 200);
        assert_eq!(deltas["app"].connections, 1);

        assert!(
            tracker
                .apply([observation("two", 10, 20, "app")], 25)
                .is_empty()
        );
    }

    #[test]
    fn tracker_enforces_tombstone_capacity() {
        let mut tracker = FlowDeltaTracker::with_retention(3_600, 2);
        tracker.apply(
            [
                observation("one", 1, 1, "app"),
                observation("two", 1, 1, "app"),
                observation("three", 1, 1, "app"),
            ],
            10,
        );
        assert_eq!(tracker.tracked_flows(), 2);
    }

    #[test]
    fn absolute_counter_uses_persisted_baseline_and_handles_reset() {
        let mut tracker = AbsoluteCounterTracker::from_baseline(HashMap::from([(
            "en0".to_string(),
            ByteDelta {
                upload: 1_000,
                download: 2_000,
            },
        )]));
        let delta = tracker.apply(HashMap::from([(
            "en0".to_string(),
            ByteDelta {
                upload: 1_300,
                download: 2_500,
            },
        )]));
        assert_eq!(delta["en0"].upload, 300);
        assert_eq!(delta["en0"].download, 500);

        let delta = tracker.apply(HashMap::from([(
            "en0".to_string(),
            ByteDelta {
                upload: 20,
                download: 30,
            },
        )]));
        assert_eq!(delta["en0"].upload, 20);
        assert_eq!(delta["en0"].download, 30);
    }
}
