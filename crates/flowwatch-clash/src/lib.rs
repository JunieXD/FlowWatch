use anyhow::{Context, Result, bail};
use flowwatch_core::{
    AppIdentity, ByteDelta, ClashConfig, CoreDelta, CounterObservation, FlowDeltaTracker,
    LocalEndpoint, ProxyFlowKey, UNKNOWN, UsageDelta, monotonic_delta,
};
use serde::{Deserialize, Deserializer};
use std::collections::{HashMap, HashSet};

const LOOPBACK: [&str; 3] = ["127.0.0.1", "::1", "localhost"];
const UNRESOLVED_GRACE_SECONDS: i64 = 6;
const RESOLUTION_RETENTION_SECONDS: i64 = 900;
const MAX_RESOLVED_FLOWS: usize = 50_000;

#[derive(Debug, Default)]
pub struct ClashSample {
    pub apps: HashMap<AppIdentity, UsageDelta>,
    pub totals: CoreDelta,
    pub active_connections: usize,
    pub actor_connections: usize,
    pub identifiable_connections: usize,
    pub metadata_identifiable_connections: usize,
    pub fallback_identifiable_connections: usize,
}

pub struct ClashSampler {
    config: ClashConfig,
    tracker: FlowDeltaTracker<ProxyFlowKey>,
    previous_total: Option<ByteDelta>,
    resolved_apps: HashMap<String, CachedResolution>,
    pending_unknown: HashMap<String, PendingUnknown>,
    sampled_once: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolutionMethod {
    Metadata,
    SocketFallback,
    Lan,
}

#[derive(Debug, Clone)]
struct CachedResolution {
    app: AppIdentity,
    method: ResolutionMethod,
    last_seen_at: i64,
}

#[derive(Debug, Clone)]
struct PendingUnknown {
    observation: CounterObservation<ProxyFlowKey>,
    first_seen_at: i64,
}

impl ClashSampler {
    pub fn new(config: ClashConfig, previous_total: Option<ByteDelta>) -> Self {
        Self {
            config,
            tracker: FlowDeltaTracker::with_retention(900, 50_000),
            previous_total,
            resolved_apps: HashMap::new(),
            pending_unknown: HashMap::new(),
            sampled_once: false,
        }
    }

    pub fn config(&self) -> &ClashConfig {
        &self.config
    }

    pub fn total_baseline(&self) -> Option<ByteDelta> {
        self.previous_total
    }

    pub fn tracked_flows(&self) -> usize {
        self.tracker.tracked_flows()
    }

    pub fn sample<F>(&mut self, now: i64, resolver: F) -> Result<ClashSample>
    where
        F: FnMut(&str, &str, Option<&LocalEndpoint>) -> AppIdentity,
    {
        let url = connections_url(&self.config.controller)?;
        let mut request = minreq::get(url).with_timeout(3);
        if !self.config.secret.is_empty() {
            request =
                request.with_header("Authorization", format!("Bearer {}", self.config.secret));
        }
        let response = request.send().context("request Clash connections")?;
        if response.status_code < 200 || response.status_code >= 300 {
            bail!("Clash controller returned HTTP {}", response.status_code);
        }
        let payload: ConnectionsPayload = response.json().context("decode Clash response")?;
        Ok(self.sample_payload(payload, now, resolver))
    }

    fn sample_payload<F>(
        &mut self,
        payload: ConnectionsPayload,
        now: i64,
        mut resolver: F,
    ) -> ClashSample
    where
        F: FnMut(&str, &str, Option<&LocalEndpoint>) -> AppIdentity,
    {
        let active_connections = payload.connections.len();
        let mut actor_connections = 0;
        let mut identifiable_connections = 0;
        let mut metadata_identifiable_connections = 0;
        let mut fallback_identifiable_connections = 0;
        let initializing = !self.sampled_once;
        let mut active_ids = HashSet::new();
        let mut observations = Vec::new();
        for connection in payload.connections {
            if connection.id.trim().is_empty() {
                continue;
            }
            active_ids.insert(connection.id.clone());
            let metadata = connection.metadata.unwrap_or_default();
            let source_ip = nonempty(metadata.source_ip, UNKNOWN);
            let inbound = nonempty(
                if metadata.inbound_name.trim().is_empty() {
                    metadata.kind
                } else {
                    metadata.inbound_name
                },
                UNKNOWN,
            );
            let has_process_hint =
                !metadata.process.trim().is_empty() || !metadata.process_path.trim().is_empty();
            let endpoint = LocalEndpoint::new(&metadata.network, &source_ip, metadata.source_port);
            let mut app = resolver(&metadata.process, &metadata.process_path, endpoint.as_ref());
            let mut resolution = if app.is_known() {
                Some(if has_process_hint {
                    ResolutionMethod::Metadata
                } else {
                    ResolutionMethod::SocketFallback
                })
            } else {
                self.resolved_apps.get(&connection.id).map(|cached| {
                    app = cached.app.clone();
                    cached.method
                })
            };
            if app.name == UNKNOWN
                && !LOOPBACK.contains(&source_ip.as_str())
                && source_ip != UNKNOWN
            {
                app = AppIdentity {
                    id: format!("lan:{source_ip}"),
                    name: format!("[LAN] {source_ip}"),
                    executable_path: String::new(),
                };
                resolution = Some(ResolutionMethod::Lan);
            }
            if proxy_flow_is_actor(&source_ip, &inbound) {
                actor_connections += 1;
                if app.is_known() {
                    identifiable_connections += 1;
                    match resolution {
                        Some(ResolutionMethod::Metadata) => metadata_identifiable_connections += 1,
                        Some(ResolutionMethod::SocketFallback) => {
                            fallback_identifiable_connections += 1
                        }
                        _ => {}
                    }
                }
            }
            if let Some(method) = resolution {
                self.resolved_apps.insert(
                    connection.id.clone(),
                    CachedResolution {
                        app: app.clone(),
                        method,
                        last_seen_at: now,
                    },
                );
            }
            let observation = CounterObservation {
                id: connection.id.clone(),
                upload: connection.upload,
                download: connection.download,
                key: ProxyFlowKey {
                    app,
                    source_ip,
                    inbound,
                    network: metadata.network.to_ascii_lowercase(),
                },
            };
            if proxy_flow_is_actor(&observation.key.source_ip, &observation.key.inbound)
                && !observation.key.app.is_known()
            {
                let new_pending = !self.pending_unknown.contains_key(&connection.id);
                let release = {
                    let pending = self
                        .pending_unknown
                        .entry(connection.id.clone())
                        .or_insert_with(|| PendingUnknown {
                            observation: observation.clone(),
                            first_seen_at: now,
                        });
                    pending.observation = observation;
                    now.saturating_sub(pending.first_seen_at) >= UNRESOLVED_GRACE_SECONDS
                };
                if initializing && new_pending {
                    let mut baseline = self
                        .pending_unknown
                        .get(&connection.id)
                        .expect("pending unknown flow must exist")
                        .observation
                        .clone();
                    baseline.key.inbound = "INNER".to_string();
                    observations.push(baseline);
                }
                if release {
                    observations.push(
                        self.pending_unknown
                            .remove(&connection.id)
                            .expect("pending unknown flow must exist")
                            .observation,
                    );
                }
            } else {
                self.pending_unknown.remove(&connection.id);
                observations.push(observation);
            }
        }
        let closed_pending: Vec<_> = self
            .pending_unknown
            .keys()
            .filter(|id| !active_ids.contains(*id))
            .cloned()
            .collect();
        for id in closed_pending {
            if let Some(pending) = self.pending_unknown.remove(&id) {
                observations.push(pending.observation);
            }
        }
        self.prune_resolutions(now);
        let deltas = self.tracker.apply(observations, now);
        self.sampled_once = true;
        let mut apps: HashMap<AppIdentity, UsageDelta> = HashMap::new();
        let mut actor_upload = 0u64;
        let mut actor_download = 0u64;
        for (key, delta) in deltas {
            if proxy_flow_is_actor(&key.source_ip, &key.inbound) {
                actor_upload = actor_upload.saturating_add(delta.upload);
                actor_download = actor_download.saturating_add(delta.download);
                merge_usage(apps.entry(key.app).or_default(), &delta);
            }
        }
        let attributed_upload = apps
            .iter()
            .filter(|(app, _)| app.is_known())
            .map(|(_, delta)| delta.upload)
            .sum();
        let attributed_download = apps
            .iter()
            .filter(|(app, _)| app.is_known())
            .map(|(_, delta)| delta.download)
            .sum();

        let current_total = ByteDelta {
            upload: payload.upload_total,
            download: payload.download_total,
        };
        let (upload, download) = self.previous_total.map_or((0, 0), |previous| {
            (
                monotonic_delta(current_total.upload, previous.upload),
                monotonic_delta(current_total.download, previous.download),
            )
        });
        self.previous_total = Some(current_total);
        ClashSample {
            apps,
            totals: CoreDelta {
                upload,
                download,
                attributed_upload,
                attributed_download,
                actor_upload,
                actor_download,
                actor_bytes_known: true,
            },
            active_connections,
            actor_connections,
            identifiable_connections,
            metadata_identifiable_connections,
            fallback_identifiable_connections,
        }
    }

    fn prune_resolutions(&mut self, now: i64) {
        self.resolved_apps.retain(|_, cached| {
            now.saturating_sub(cached.last_seen_at) <= RESOLUTION_RETENTION_SECONDS
        });
        if self.resolved_apps.len() > MAX_RESOLVED_FLOWS {
            let mut entries: Vec<_> = self.resolved_apps.drain().collect();
            entries.sort_unstable_by_key(|entry| std::cmp::Reverse(entry.1.last_seen_at));
            entries.truncate(MAX_RESOLVED_FLOWS);
            self.resolved_apps.extend(entries);
        }
    }
}

pub fn proxy_flow_is_actor(source_ip: &str, inbound: &str) -> bool {
    let normalized = inbound.to_ascii_uppercase();
    if source_ip == UNKNOWN || normalized == "INNER" || normalized.contains("TUN") {
        return false;
    }
    if !LOOPBACK.contains(&source_ip) {
        return true;
    }
    !normalized.is_empty() && normalized != "(LEGACY)"
}

fn connections_url(controller: &str) -> Result<String> {
    let raw = controller.trim().trim_end_matches('/');
    let raw = if raw.starts_with("http://") {
        raw.to_string()
    } else if raw.contains("://") {
        bail!("only local http Clash controllers are supported in v0.1")
    } else {
        format!("http://{raw}")
    };
    let authority = raw
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("");
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split_once(']').map(|value| value.0).unwrap_or("")
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if port.bytes().all(|byte| byte.is_ascii_digit()) {
            host
        } else {
            ""
        }
    } else {
        authority
    };
    if !LOOPBACK.contains(&host) && host != "0.0.0.0" {
        bail!("Clash controller must be bound to localhost in v0.1");
    }
    Ok(format!("{raw}/connections"))
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

fn nonempty(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnectionsPayload {
    #[serde(default)]
    connections: Vec<ClashConnection>,
    #[serde(default)]
    upload_total: u64,
    #[serde(default)]
    download_total: u64,
}

#[derive(Debug, Deserialize)]
struct ClashConnection {
    #[serde(default)]
    id: String,
    #[serde(default)]
    upload: u64,
    #[serde(default)]
    download: u64,
    metadata: Option<ClashMetadata>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClashMetadata {
    #[serde(default)]
    process: String,
    #[serde(default)]
    process_path: String,
    #[serde(default)]
    #[serde(rename = "sourceIP", alias = "sourceIp")]
    source_ip: String,
    #[serde(default, deserialize_with = "deserialize_port")]
    source_port: u16,
    #[serde(default)]
    inbound_name: String,
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    network: String,
}

fn deserialize_port<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Port {
        Number(u64),
        Text(String),
    }

    Ok(match Option::<Port>::deserialize(deserializer)? {
        Some(Port::Number(value)) => u16::try_from(value).unwrap_or_default(),
        Some(Port::Text(value)) => value.parse().unwrap_or_default(),
        None => 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(upload: u64, download: u64, inbound: &str, source: &str) -> ConnectionsPayload {
        ConnectionsPayload {
            upload_total: upload,
            download_total: download,
            connections: vec![ClashConnection {
                id: "one".into(),
                upload,
                download,
                metadata: Some(ClashMetadata {
                    process: "Example".into(),
                    process_path: "/Applications/Example.app/bin".into(),
                    source_ip: source.into(),
                    source_port: 50_000,
                    inbound_name: inbound.into(),
                    kind: "HTTPS".into(),
                    network: "TCP".into(),
                }),
            }],
        }
    }

    fn config() -> ClashConfig {
        ClashConfig {
            enabled: true,
            controller: "127.0.0.1:9090".into(),
            secret: String::new(),
        }
    }

    #[test]
    fn baselines_then_attributes_mixed_proxy_flow() {
        let mut sampler = ClashSampler::new(config(), None);
        let resolve =
            |name: &str, path: &str, _: Option<&LocalEndpoint>| AppIdentity::process(name, path);
        assert!(
            sampler
                .sample_payload(payload(100, 200, "DEFAULT-MIXED", "127.0.0.1"), 10, resolve,)
                .apps
                .is_empty()
        );
        let sample =
            sampler.sample_payload(payload(150, 260, "DEFAULT-MIXED", "127.0.0.1"), 15, resolve);
        let usage = sample.apps.values().next().unwrap();
        assert_eq!((usage.upload, usage.download), (50, 60));
        assert_eq!((sample.totals.upload, sample.totals.download), (50, 60));
        assert_eq!(
            (sample.totals.actor_upload, sample.totals.actor_download),
            (50, 60)
        );
        assert!(sample.totals.actor_bytes_known);
    }

    #[test]
    fn tun_and_inner_are_not_added_to_app_totals() {
        assert!(!proxy_flow_is_actor("127.0.0.1", "DEFAULT-TUN"));
        assert!(!proxy_flow_is_actor("192.168.1.2", "DEFAULT-TUN"));
        assert!(!proxy_flow_is_actor(UNKNOWN, "Inner"));
        assert!(proxy_flow_is_actor("192.168.1.20", "HTTP"));
    }

    #[test]
    fn excluded_tun_bytes_are_not_reported_as_attributed() {
        let mut sampler = ClashSampler::new(config(), None);
        let resolve =
            |name: &str, path: &str, _: Option<&LocalEndpoint>| AppIdentity::process(name, path);
        sampler.sample_payload(payload(100, 200, "DEFAULT-TUN", "127.0.0.1"), 10, resolve);
        let sample =
            sampler.sample_payload(payload(150, 260, "DEFAULT-TUN", "127.0.0.1"), 15, resolve);
        assert!(sample.apps.is_empty());
        assert_eq!(sample.totals.attributed_upload, 0);
        assert_eq!(sample.totals.attributed_download, 0);
        assert_eq!(
            (sample.totals.actor_upload, sample.totals.actor_download),
            (0, 0)
        );
    }

    #[test]
    fn unknown_process_bytes_remain_visible_but_unattributed() {
        let mut sampler = ClashSampler::new(config(), None);
        let resolve =
            |_: &str, _: &str, _: Option<&LocalEndpoint>| AppIdentity::process(UNKNOWN, "");
        sampler.sample_payload(payload(100, 200, "DEFAULT-MIXED", "127.0.0.1"), 10, resolve);
        let sample =
            sampler.sample_payload(payload(150, 260, "DEFAULT-MIXED", "127.0.0.1"), 16, resolve);
        assert_eq!(sample.apps.values().next().unwrap().upload, 50);
        assert_eq!(sample.totals.attributed_upload, 0);
        assert_eq!(
            (sample.totals.actor_upload, sample.totals.actor_download),
            (50, 60)
        );
        assert_eq!(sample.identifiable_connections, 0);
    }

    #[test]
    fn only_local_controller_is_accepted() {
        assert_eq!(
            connections_url("127.0.0.1:9090").unwrap(),
            "http://127.0.0.1:9090/connections"
        );
        assert!(connections_url("https://example.com").is_err());
        assert!(connections_url("http://example.com:9090").is_err());
        assert_eq!(
            connections_url("http://[::1]:9090").unwrap(),
            "http://[::1]:9090/connections"
        );
    }

    #[test]
    fn decodes_mihomo_acronym_fields() {
        let payload: ConnectionsPayload = serde_json::from_str(
            r#"{
                "uploadTotal": 10,
                "downloadTotal": 20,
                "connections": [{
                    "id": "one",
                    "upload": 3,
                    "download": 4,
                    "metadata": {
                        "sourceIP": "127.0.0.1",
                        "sourcePort": "54321",
                        "inboundName": "mixed-in",
                        "process": "Example",
                        "processPath": "/Applications/Example.app/Contents/MacOS/Example",
                        "network": "tcp"
                    }
                }]
            }"#,
        )
        .unwrap();
        let metadata = payload.connections[0].metadata.as_ref().unwrap();
        assert_eq!(metadata.source_ip, "127.0.0.1");
        assert_eq!(metadata.source_port, 54321);
        assert_eq!(metadata.inbound_name, "mixed-in");
    }

    #[test]
    fn socket_fallback_is_counted_separately() {
        let mut first = payload(100, 200, "DEFAULT-MIXED", "127.0.0.1");
        let metadata = first.connections[0].metadata.as_mut().unwrap();
        metadata.process.clear();
        metadata.process_path.clear();
        let mut second = payload(150, 260, "DEFAULT-MIXED", "127.0.0.1");
        let metadata = second.connections[0].metadata.as_mut().unwrap();
        metadata.process.clear();
        metadata.process_path.clear();

        let mut sampler = ClashSampler::new(config(), None);
        let resolve = |_: &str, _: &str, endpoint: Option<&LocalEndpoint>| {
            assert_eq!(endpoint.unwrap().port, 50_000);
            AppIdentity::process("Fallback App", "/Applications/Fallback.app/bin")
        };
        sampler.sample_payload(first, 10, resolve);
        let sample = sampler.sample_payload(second, 15, resolve);
        assert_eq!(sample.actor_connections, 1);
        assert_eq!(sample.identifiable_connections, 1);
        assert_eq!(sample.metadata_identifiable_connections, 0);
        assert_eq!(sample.fallback_identifiable_connections, 1);
    }

    #[test]
    fn transient_unknown_is_attributed_when_socket_owner_arrives() {
        let mut first = payload(100, 200, "DEFAULT-MIXED", "127.0.0.1");
        let metadata = first.connections[0].metadata.as_mut().unwrap();
        metadata.process.clear();
        metadata.process_path.clear();
        let mut second = payload(150, 260, "DEFAULT-MIXED", "127.0.0.1");
        let metadata = second.connections[0].metadata.as_mut().unwrap();
        metadata.process.clear();
        metadata.process_path.clear();

        let mut sampler = ClashSampler::new(config(), None);
        sampler.sample_payload(first, 10, |_, _, _| AppIdentity::process(UNKNOWN, ""));
        let sample = sampler.sample_payload(second, 13, |_, _, _| {
            AppIdentity::process("Resolved App", "/Applications/Resolved.app/bin")
        });
        let usage = sample.apps.values().next().unwrap();
        assert_eq!((usage.upload, usage.download), (50, 60));
        assert_eq!(sample.fallback_identifiable_connections, 1);
    }

    #[test]
    fn known_connection_keeps_its_resolution_when_lookup_flickers() {
        let mut sampler = ClashSampler::new(config(), None);
        sampler.sample_payload(
            payload(100, 200, "DEFAULT-MIXED", "127.0.0.1"),
            10,
            |name, path, _| AppIdentity::process(name, path),
        );
        let sample = sampler.sample_payload(
            payload(150, 260, "DEFAULT-MIXED", "127.0.0.1"),
            13,
            |_, _, _| AppIdentity::process(UNKNOWN, ""),
        );
        let usage = sample.apps.values().next().unwrap();
        assert_eq!((usage.upload, usage.download), (50, 60));
        assert_eq!(sample.metadata_identifiable_connections, 1);
    }
}
