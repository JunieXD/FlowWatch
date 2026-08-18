//! macOS traffic collection backends.

#![cfg(target_os = "macos")]

use anyhow::{Context, Result, bail};
use flowwatch_core::{
    AppIdentity, AppTrafficDelta, ByteDelta, CounterObservation, EndpointOwner, FlowDeltaTracker,
    LocalEndpoint, NewFlowPolicy, ProcessFlowKey, ProcessTrafficSample, TrafficBackend, UNKNOWN,
};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const NETWORKSETUP: &str = "/usr/sbin/networksetup";
const NETTOP: &str = "/usr/bin/nettop";
const INTERFACE_REFRESH: Duration = Duration::from_secs(300);
const NETTOP_TIMEOUT: Duration = Duration::from_secs(5);
const PROCESS_TOMBSTONE_SECONDS: i64 = 86_400;
const MAX_TRACKED_PROCESS_FLOWS: usize = 100_000;

pub struct MacOsBackend {
    hardware_interfaces: HashSet<String>,
    last_interface_refresh: Option<Instant>,
    identities: AppIdentityResolver,
    process_tracker: FlowDeltaTracker<ProcessFlowKey>,
    sampled_once: bool,
}

impl MacOsBackend {
    pub fn new() -> Self {
        Self::with_poll_seconds(3)
    }

    pub fn with_poll_seconds(_poll_seconds: u64) -> Self {
        Self {
            hardware_interfaces: HashSet::new(),
            last_interface_refresh: None,
            identities: AppIdentityResolver::default(),
            process_tracker: FlowDeltaTracker::with_retention_and_policy(
                PROCESS_TOMBSTONE_SECONDS,
                MAX_TRACKED_PROCESS_FLOWS,
                NewFlowPolicy::Baseline,
            ),
            sampled_once: false,
        }
    }

    fn refresh_interfaces(&mut self, force: bool) -> Result<()> {
        if !force
            && !self.hardware_interfaces.is_empty()
            && self
                .last_interface_refresh
                .is_some_and(|last| last.elapsed() < INTERFACE_REFRESH)
        {
            return Ok(());
        }
        self.last_interface_refresh = Some(Instant::now());
        let output = Command::new(NETWORKSETUP)
            .arg("-listallhardwareports")
            .output()
            .context("无法运行 networksetup")?;
        if !output.status.success() {
            bail!("networksetup 执行失败：{}", output.status);
        }
        let mut interfaces = HashSet::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some(device) = line.strip_prefix("Device:") {
                let device = device.trim();
                if is_hardware_interface(device) {
                    interfaces.insert(device.to_string());
                }
            }
        }
        if interfaces.is_empty() {
            bail!("没有找到 enN 物理网卡");
        }
        self.hardware_interfaces = interfaces;
        Ok(())
    }

    pub fn resolve_external_identity(&mut self, process: &str, path: &str) -> AppIdentity {
        if path.is_empty() {
            self.identities.resolve_name(process)
        } else {
            self.identities.resolve_path(path, process)
        }
    }

    fn read_nettop_frame(&mut self) -> Result<ParsedNettopFrame> {
        let output = run_nettop_snapshot()?;
        parse_nettop_frame(&output, &mut self.identities)
    }
}

impl Default for MacOsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl TrafficBackend for MacOsBackend {
    type Error = anyhow::Error;

    fn interface_counters(&mut self) -> Result<HashMap<String, ByteDelta>> {
        self.refresh_interfaces(false)?;
        let all = route_interface_counters()?;
        Ok(all
            .into_iter()
            .filter(|(name, _)| self.hardware_interfaces.contains(name))
            .collect())
    }

    fn process_traffic(&mut self) -> Result<ProcessTrafficSample> {
        self.refresh_interfaces(false)?;
        let frame = self.read_nettop_frame()?;
        let baseline_discarded = !self.sampled_once;
        self.sampled_once = true;

        let mut active_flows = 0usize;
        let mut socket_owners = HashMap::new();
        let mut observations = Vec::new();

        for socket in frame.sockets {
            if socket.interface == "lo0" {
                if let Some(endpoint) = socket.local_endpoint {
                    socket_owners.insert(endpoint, socket.app);
                }
                continue;
            }
            if !self.hardware_interfaces.contains(&socket.interface) {
                continue;
            }
            active_flows = active_flows.saturating_add(1);
            observations.push(CounterObservation {
                id: socket.id,
                upload: socket.upload,
                download: socket.download,
                key: ProcessFlowKey {
                    app: socket.app,
                    target: String::new(),
                    interface: socket.interface,
                    protocol: socket.network,
                },
            });
        }

        let deltas = self.process_tracker.apply(observations, unix_timestamp());
        let mut by_app: HashMap<AppIdentity, AppTrafficDelta> = HashMap::new();
        for (key, delta) in deltas {
            let app = key.app;
            let entry = by_app
                .entry(app.clone())
                .or_insert_with(|| AppTrafficDelta {
                    app,
                    upload: 0,
                    download: 0,
                    connections: 0,
                });
            entry.upload = entry.upload.saturating_add(delta.upload);
            entry.download = entry.download.saturating_add(delta.download);
            entry.connections = entry.connections.saturating_add(delta.connections);
        }

        Ok(ProcessTrafficSample {
            apps: by_app.into_values().collect(),
            socket_owners: socket_owners
                .into_iter()
                .map(|(endpoint, app)| EndpointOwner { endpoint, app })
                .collect(),
            active_flows,
            tracked_flows: self.process_tracker.tracked_flows(),
            baseline_discarded,
            collector_restarts: 0,
        })
    }
}

fn run_nettop_snapshot() -> Result<Vec<u8>> {
    let mut child = Command::new(NETTOP)
        .args([
            "-L",
            "1",
            "-x",
            "-n",
            "-c",
            "-t",
            "external",
            "-t",
            "loopback",
            "-J",
            "interface,bytes_in,bytes_out",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动 nettop 采样")?;
    let mut stdout = child.stdout.take().context("无法读取 nettop 输出")?;
    let reader = match std::thread::Builder::new()
        .name("flowwatch-nettop-reader".to_string())
        .spawn(move || {
            let mut output = Vec::new();
            stdout.read_to_end(&mut output).map(|_| output)
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("无法启动 nettop 读取线程");
        }
    };
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().context("无法读取 nettop 退出状态")? {
            break status;
        }
        if started.elapsed() >= NETTOP_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            bail!("nettop 采样超过 {} 秒仍未完成", NETTOP_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = reader
        .join()
        .map_err(|_| anyhow::anyhow!("nettop 读取线程异常退出"))?
        .context("无法读取 nettop 采样结果")?;
    if !status.success() {
        bail!("nettop 采样失败：{status}");
    }
    Ok(output)
}

fn unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or_default()
}

pub fn route_interface_counters() -> Result<HashMap<String, ByteDelta>> {
    let mut raw = [RawInterfaceCounter::default(); 64];
    let mut written = 0usize;
    // SAFETY: raw and written are valid writable buffers for the C shim.
    let error = unsafe { flowwatch_interface_counters(raw.as_mut_ptr(), raw.len(), &mut written) };
    if error != 0 {
        return Err(std::io::Error::from_raw_os_error(error)).context("无法读取网卡计数");
    }
    let mut counters = HashMap::new();
    for item in &raw[..written] {
        // SAFETY: the C shim fills every emitted name using if_indextoname.
        let interface = unsafe { CStr::from_ptr(item.name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        counters.insert(
            interface,
            ByteDelta {
                upload: item.upload,
                download: item.download,
            },
        );
    }
    Ok(counters)
}

#[derive(Debug)]
struct ParsedSocket {
    id: String,
    app: AppIdentity,
    interface: String,
    network: String,
    local_endpoint: Option<LocalEndpoint>,
    upload: u64,
    download: u64,
}

#[derive(Debug, Default)]
struct ParsedNettopFrame {
    sockets: Vec<ParsedSocket>,
}

fn parse_nettop_frame(
    output: &[u8],
    identities: &mut AppIdentityResolver,
) -> Result<ParsedNettopFrame> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(output);
    let mut columns: HashMap<String, usize> = HashMap::new();
    let mut current_app = AppIdentity::process(UNKNOWN, "");
    let mut current_pid = 0i32;
    let mut sockets = Vec::new();
    let mut active_pids = HashSet::new();

    for record in reader.records() {
        let record = record.context("无法解析 nettop 数据")?;
        let label = record.get(0).unwrap_or("").trim();
        if label.is_empty() {
            columns = record
                .iter()
                .enumerate()
                .filter_map(|(index, value)| {
                    let value = value.trim();
                    (!value.is_empty()).then(|| (value.to_string(), index))
                })
                .collect();
            continue;
        }
        if !is_socket_label(label) {
            let (fallback, pid) = parse_process_label(label);
            current_pid = pid;
            if pid > 0 {
                active_pids.insert(pid);
            }
            current_app = identities.resolve(pid, &fallback);
            continue;
        }

        let Some(interface) = field(&record, &columns, "interface") else {
            continue;
        };
        let Some(download) = field(&record, &columns, "bytes_in").and_then(parse_u64) else {
            continue;
        };
        let Some(upload) = field(&record, &columns, "bytes_out").and_then(parse_u64) else {
            continue;
        };
        let network = label
            .split_once(' ')
            .map(|(value, _)| value)
            .unwrap_or("")
            .to_ascii_lowercase();
        sockets.push(ParsedSocket {
            id: format!("{current_pid}|{interface}|{label}"),
            app: current_app.clone(),
            interface: interface.to_string(),
            network,
            local_endpoint: local_endpoint_from_label(label),
            upload,
            download,
        });
    }
    identities.retain_pids(&active_pids);
    Ok(ParsedNettopFrame { sockets })
}

fn field<'a>(
    record: &'a csv::StringRecord,
    columns: &HashMap<String, usize>,
    name: &str,
) -> Option<&'a str> {
    record.get(*columns.get(name)?).map(str::trim)
}

fn parse_u64(value: &str) -> Option<u64> {
    (!value.is_empty()).then(|| value.parse().ok()).flatten()
}

fn is_hardware_interface(name: &str) -> bool {
    name.strip_prefix("en").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn is_socket_label(label: &str) -> bool {
    ["tcp4 ", "tcp6 ", "udp4 ", "udp6 ", "quic4 ", "quic6 "]
        .iter()
        .any(|prefix| label.starts_with(prefix))
}

fn parse_process_label(label: &str) -> (String, i32) {
    let Some((name, raw_pid)) = label.rsplit_once('.') else {
        return (label.trim().to_string(), 0);
    };
    let Ok(pid) = raw_pid.parse() else {
        return (label.trim().to_string(), 0);
    };
    (name.trim().to_string(), pid)
}

fn local_endpoint_from_label(label: &str) -> Option<LocalEndpoint> {
    let (network, endpoints) = label.split_once(' ')?;
    let local = endpoints.split_once("<->")?.0.trim();
    let (address, raw_port) = if network.ends_with('4') {
        local.rsplit_once(':')?
    } else {
        local.rsplit_once('.')?
    };
    LocalEndpoint::new(network, address, raw_port.parse().ok()?)
}

#[derive(Default)]
struct AppIdentityResolver {
    by_path: HashMap<String, AppIdentity>,
    by_pid: HashMap<i32, (String, AppIdentity)>,
    by_name: HashMap<String, Option<AppIdentity>>,
}

impl AppIdentityResolver {
    fn resolve(&mut self, pid: i32, fallback: &str) -> AppIdentity {
        if let Some((cached_name, identity)) = self.by_pid.get(&pid)
            && cached_name == fallback
            && !identity.executable_path.is_empty()
        {
            return identity.clone();
        }
        let path = process_path(pid).unwrap_or_default();
        let identity = if path.is_empty() {
            self.resolve_name(fallback)
        } else {
            self.resolve_path(&path, fallback)
        };
        self.by_pid
            .insert(pid, (fallback.to_string(), identity.clone()));
        identity
    }

    fn resolve_path(&mut self, path: &str, fallback: &str) -> AppIdentity {
        let normalized_path = canonical_executable_path(path);
        if let Some(identity) = self.by_path.get(&normalized_path) {
            let identity = identity.clone();
            self.remember_name(fallback, &identity);
            self.remember_name(&identity.name, &identity);
            return identity;
        }
        let identity = identity_from_path(&normalized_path, fallback);
        self.by_path.insert(normalized_path, identity.clone());
        self.remember_name(fallback, &identity);
        self.remember_name(&identity.name, &identity);
        identity
    }

    fn resolve_name(&self, fallback: &str) -> AppIdentity {
        let key = normalized_process_name(fallback);
        self.by_name
            .get(&key)
            .and_then(Option::as_ref)
            .cloned()
            .unwrap_or_else(|| AppIdentity::process(fallback, ""))
    }

    fn remember_name(&mut self, name: &str, identity: &AppIdentity) {
        let key = normalized_process_name(name);
        if key.is_empty() || key == normalized_process_name(UNKNOWN) {
            return;
        }
        match self.by_name.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(Some(identity.clone()));
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if entry
                    .get()
                    .as_ref()
                    .is_some_and(|existing| existing.id != identity.id)
                {
                    entry.insert(None);
                }
            }
        }
    }

    fn retain_pids(&mut self, active: &HashSet<i32>) {
        self.by_pid.retain(|pid, _| active.contains(pid));
    }
}

fn process_path(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: proc_pidpath receives a valid process id and writable byte buffer.
    let size = unsafe { proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if size <= 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(size as usize);
    Some(String::from_utf8_lossy(&buffer[..end]).into_owned())
}

fn canonical_executable_path(path: &str) -> String {
    let path = Path::new(path);
    std::fs::canonicalize(path)
        .ok()
        .and_then(|value| value.to_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn normalized_process_name(name: &str) -> String {
    name.trim().to_lowercase()
}

fn identity_from_path(executable: &str, fallback: &str) -> AppIdentity {
    let Some(bundle_path) = outermost_app_bundle(Path::new(executable)) else {
        return AppIdentity::process(preferred_process_name(executable, fallback), executable);
    };
    let plist_path = bundle_path.join("Contents/Info.plist");
    let Ok(value) = plist::Value::from_file(&plist_path) else {
        return AppIdentity::process(preferred_process_name(executable, fallback), executable);
    };
    let Some(dictionary) = value.as_dictionary() else {
        return AppIdentity::process(preferred_process_name(executable, fallback), executable);
    };
    let bundle_id = dictionary
        .get("CFBundleIdentifier")
        .and_then(plist::Value::as_string)
        .unwrap_or("");
    let display_name = ["CFBundleDisplayName", "CFBundleName"]
        .iter()
        .find_map(|key| dictionary.get(key).and_then(plist::Value::as_string))
        .or_else(|| bundle_path.file_stem().and_then(|value| value.to_str()))
        .unwrap_or(fallback);
    AppIdentity {
        id: if bundle_id.is_empty() {
            format!("app:{}", bundle_path.display())
        } else {
            format!("bundle:{bundle_id}")
        },
        name: display_name.to_string(),
        executable_path: executable.to_string(),
    }
}

fn outermost_app_bundle(path: &Path) -> Option<PathBuf> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        if component.as_os_str().to_str().is_some_and(|value| {
            let value = value.to_ascii_lowercase();
            value.ends_with(".app") || value.ends_with(".app.bundle")
        }) {
            return Some(current);
        }
    }
    None
}

fn preferred_process_name(executable: &str, fallback: &str) -> String {
    let fallback = fallback.trim();
    let Some(file_name) = Path::new(executable)
        .file_name()
        .and_then(|value| value.to_str())
    else {
        return fallback.to_string();
    };
    if fallback.chars().count() == 15
        && file_name
            .to_lowercase()
            .starts_with(&fallback.to_lowercase())
        && file_name.chars().count() > fallback.chars().count()
    {
        file_name.to_string()
    } else {
        fallback.to_string()
    }
}

#[link(name = "proc")]
unsafe extern "C" {
    fn proc_pidpath(pid: libc::c_int, buffer: *mut libc::c_void, buffersize: u32) -> libc::c_int;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RawInterfaceCounter {
    name: [libc::c_char; libc::IF_NAMESIZE],
    upload: u64,
    download: u64,
}

impl Default for RawInterfaceCounter {
    fn default() -> Self {
        Self {
            name: [0; libc::IF_NAMESIZE],
            upload: 0,
            download: 0,
        }
    }
}

unsafe extern "C" {
    fn flowwatch_interface_counters(
        output: *mut RawInterfaceCounter,
        capacity: usize,
        written: *mut usize,
    ) -> libc::c_int;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_external_and_loopback_nettop_rows() {
        let input = b",interface,bytes_in,bytes_out,\n\
Example App.999999,,300,400,\n\
tcp4 192.0.2.1:50000<->198.51.100.5:443,en9,300,400,\n\
tcp4 10.0.0.1:50001<->10.0.0.2:443,bridge100,50,60,\n\
tcp4 127.0.0.1:50002<->127.0.0.1:7897,lo0,70,80,\n";
        let mut identities = AppIdentityResolver::default();
        let frame = parse_nettop_frame(input, &mut identities).unwrap();
        assert_eq!(frame.sockets.len(), 3);
        let physical = frame
            .sockets
            .iter()
            .find(|row| row.interface == "en9")
            .unwrap();
        assert_eq!(physical.upload, 400);
        assert_eq!(physical.download, 300);
        assert_eq!(physical.app.name, "Example App");
        let loopback = frame
            .sockets
            .iter()
            .find(|row| row.interface == "lo0")
            .unwrap();
        assert_eq!(
            loopback.local_endpoint,
            LocalEndpoint::new("tcp", "127.0.0.1", 50002)
        );
    }

    #[test]
    fn parses_ipv6_and_quic_local_endpoints() {
        assert_eq!(
            local_endpoint_from_label("tcp6 ::1.54321<->::1.7897"),
            LocalEndpoint::new("tcp", "::1", 54321)
        );
        assert_eq!(
            local_endpoint_from_label("quic4 127.0.0.1:54322<->127.0.0.1:443"),
            LocalEndpoint::new("udp", "127.0.0.1", 54322)
        );
    }

    #[test]
    fn scopes_duplicate_multicast_sockets_to_their_interface() {
        let input = b",interface,bytes_in,bytes_out,\n\
Browser Helper.999999,,300,400,\n\
udp4 *:5353<->*:*,en9,300,400,\n\
udp4 *:5353<->*:*,en1,250,350,\n";
        let mut identities = AppIdentityResolver::default();
        let frame = parse_nettop_frame(input, &mut identities).unwrap();
        assert_eq!(frame.sockets.len(), 2);
        assert_ne!(frame.sockets[0].id, frame.sockets[1].id);
    }

    #[test]
    fn finds_outermost_application_bundle() {
        let path = Path::new(
            "/Applications/Browser.app/Contents/Frameworks/Helper.app/Contents/MacOS/Helper",
        );
        assert_eq!(
            outermost_app_bundle(path).unwrap(),
            PathBuf::from("/Applications/Browser.app")
        );

        let clone = Path::new(
            "/private/tmp/com.example.Browser.code_sign_clone/Browser.app.bundle/Contents/MacOS/Browser",
        );
        assert_eq!(
            outermost_app_bundle(clone).unwrap(),
            PathBuf::from("/private/tmp/com.example.Browser.code_sign_clone/Browser.app.bundle")
        );
    }

    #[test]
    fn expands_nettop_truncated_names_from_the_executable() {
        assert_eq!(
            preferred_process_name(
                "/System/Library/Example/CloudTelemetryService",
                "CloudTelemetryS"
            ),
            "CloudTelemetryService"
        );
        assert_eq!(
            preferred_process_name("/usr/bin/example", "custom label"),
            "custom label"
        );
    }

    #[test]
    fn reuses_only_unambiguous_name_aliases() {
        let mut resolver = AppIdentityResolver::default();
        let known = resolver.resolve_path("/opt/example/bin/gh", "gh");
        assert_eq!(resolver.resolve_name("gh"), known);

        resolver.resolve_path("/another/example/bin/gh", "gh");
        let ambiguous = resolver.resolve_name("gh");
        assert_eq!(ambiguous.id, "process:gh");
        assert!(ambiguous.executable_path.is_empty());
    }

    #[test]
    fn reads_live_64_bit_interface_counters() {
        let counters = route_interface_counters().unwrap();
        assert!(counters.contains_key("lo0"));
        assert!(!counters.is_empty());
    }
}
