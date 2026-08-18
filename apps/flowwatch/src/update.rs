use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

const REPOSITORY: &str = "JunieXD/FlowWatch";
const CURL: &str = "/usr/bin/curl";
const TAR: &str = "/usr/bin/tar";
const MAX_METADATA_BYTES: u64 = 1_048_576;
const MAX_ARCHIVE_BYTES: u64 = 67_108_864;
const MAX_BINARY_BYTES: u64 = 67_108_864;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    pub fn parse(raw: &str) -> Result<Self> {
        let value = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
        let parts = value.split('.').collect::<Vec<_>>();
        if parts.len() != 3
            || parts
                .iter()
                .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            bail!("版本号必须是 MAJOR.MINOR.PATCH，例如 0.2.0");
        }
        Ok(Self {
            major: parts[0].parse().context("主版本号过大")?,
            minor: parts[1].parse().context("次版本号过大")?,
            patch: parts[2].parse().context("修订版本号过大")?,
        })
    }

    pub fn tag(self) -> String {
        format!("v{}.{}.{}", self.major, self.minor, self.patch)
    }

    pub fn plain(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateDecision {
    Current,
    Upgrade,
    Downgrade,
}

pub fn decide(current: Version, target: Version) -> UpdateDecision {
    match target.cmp(&current) {
        Ordering::Equal => UpdateDecision::Current,
        Ordering::Greater => UpdateDecision::Upgrade,
        Ordering::Less => UpdateDecision::Downgrade,
    }
}

#[derive(Debug)]
pub struct UpdateResult {
    pub current: Version,
    pub target: Version,
    pub installed: bool,
}

pub fn run(check_only: bool, requested: Option<&str>) -> Result<UpdateResult> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))?;
    let target = match requested {
        Some(version) => Version::parse(version)?,
        None => latest_version()?,
    };
    match decide(current, target) {
        UpdateDecision::Current => {
            return Ok(UpdateResult {
                current,
                target,
                installed: false,
            });
        }
        UpdateDecision::Downgrade => {
            bail!("不会从 {} 降级到 {}", current.plain(), target.plain());
        }
        UpdateDecision::Upgrade if check_only => {
            return Ok(UpdateResult {
                current,
                target,
                installed: false,
            });
        }
        UpdateDecision::Upgrade => {}
    }

    let target_triple = target_triple()?;
    let asset = format!("flowwatch-{target_triple}.tar.gz");
    let base = release_download_base(&target.tag());
    let temporary = TemporaryDirectory::new()?;
    let archive = temporary.path.join(&asset);
    let checksums = temporary.path.join("SHA256SUMS");
    download_file(&format!("{base}/{asset}"), &archive, MAX_ARCHIVE_BYTES)?;
    download_file(
        &format!("{base}/SHA256SUMS"),
        &checksums,
        MAX_METADATA_BYTES,
    )?;
    verify_checksum(&archive, &checksums, &asset)?;
    extract_binary(&archive, &temporary.path)?;
    let binary = temporary.path.join("flowwatch");
    verify_binary_version(&binary, target)?;
    install_binary(&binary)?;
    Ok(UpdateResult {
        current,
        target,
        installed: true,
    })
}

fn install_binary(binary: &Path) -> Result<()> {
    let status = Command::new(binary)
        .arg("install")
        .status()
        .context("无法运行新版 FlowWatch 安装程序")?;
    if !status.success() {
        bail!("新版 FlowWatch 安装失败：{status}");
    }
    Ok(())
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

fn latest_version() -> Result<Version> {
    let api = std::env::var("FLOWWATCH_UPDATE_API")
        .unwrap_or_else(|_| format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"));
    let output = Command::new(CURL)
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "30",
            "--max-filesize",
            &MAX_METADATA_BYTES.to_string(),
            "--header",
            "Accept: application/vnd.github+json",
            "--header",
            "X-GitHub-Api-Version: 2022-11-28",
            "--user-agent",
            "FlowWatch updater",
            &api,
        ])
        .output()
        .context("无法查询 FlowWatch 最新版本")?;
    if !output.status.success() {
        bail!(
            "查询最新版本失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if output.stdout.len() as u64 > MAX_METADATA_BYTES {
        bail!("最新版本信息超过大小限制");
    }
    let release: LatestRelease =
        serde_json::from_slice(&output.stdout).context("GitHub 返回的版本信息无效")?;
    Version::parse(&release.tag_name)
}

fn release_download_base(tag: &str) -> String {
    std::env::var("FLOWWATCH_UPDATE_BASE")
        .map(|base| format!("{}/{tag}", base.trim_end_matches('/')))
        .unwrap_or_else(|_| format!("https://github.com/{REPOSITORY}/releases/download/{tag}"))
}

fn target_triple() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", architecture) => bail!("暂不支持 macOS 架构 {architecture}"),
        (system, _) => bail!("自更新暂不支持 {system}"),
    }
}

fn download_file(url: &str, destination: &Path, maximum_bytes: u64) -> Result<()> {
    let status = Command::new(CURL)
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--retry",
            "3",
            "--retry-delay",
            "1",
            "--max-time",
            "300",
            "--max-filesize",
            &maximum_bytes.to_string(),
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .status()
        .with_context(|| format!("无法下载 {url}"))?;
    if !status.success() {
        bail!("下载失败：{url}");
    }
    let size = destination.metadata()?.len();
    if size == 0 || size > maximum_bytes {
        bail!("下载文件大小无效：{size} 字节");
    }
    Ok(())
}

fn verify_checksum(archive: &Path, checksums: &Path, asset: &str) -> Result<()> {
    let content = fs::read_to_string(checksums).context("无法读取 SHA256SUMS")?;
    if content.len() as u64 > MAX_METADATA_BYTES {
        bail!("SHA256SUMS 超过大小限制");
    }
    let matches = content
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let hash = fields.next()?;
            let name = fields.next()?.trim_start_matches('*');
            (name == asset && fields.next().is_none()).then_some(hash)
        })
        .collect::<Vec<_>>();
    let [expected] = matches.as_slice() else {
        bail!("SHA256SUMS 必须且只能包含一个 {asset} 条目");
    };
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{asset} 的 SHA-256 条目无效");
    }
    let actual = sha256_file(archive)?;
    if !expected.eq_ignore_ascii_case(&actual) {
        bail!("{asset} 的 SHA-256 校验失败，已停止更新");
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_binary(archive: &Path, destination: &Path) -> Result<()> {
    let binary = destination.join("flowwatch");
    let mut child = Command::new(TAR)
        .args(["-xOzf"])
        .arg(archive)
        .arg("flowwatch")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("无法解压新版 FlowWatch")?;
    let mut stdout = child.stdout.take().context("无法读取新版 FlowWatch")?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o700);
    }
    let mut file = options.open(&binary)?;
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = stdout.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_BINARY_BYTES {
            let _ = child.kill();
            let _ = child.wait();
            drop(file);
            let _ = fs::remove_file(&binary);
            bail!("发布归档中的 flowwatch 程序超过大小限制");
        }
        file.write_all(&buffer[..read])?;
    }
    file.sync_all()?;
    drop(file);
    let status = child.wait().context("无法等待解压程序结束")?;
    if !status.success() {
        let _ = fs::remove_file(&binary);
        bail!("解压新版 FlowWatch 失败：{status}");
    }
    if total == 0 || !binary.is_file() {
        let _ = fs::remove_file(&binary);
        bail!("发布归档中不包含 flowwatch 程序");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn verify_binary_version(binary: &Path, target: Version) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .context("无法运行下载的 FlowWatch")?;
    if !output.status.success() {
        bail!("下载的 FlowWatch 无法运行：{}", output.status);
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let expected = format!("flowwatch {}", target.plain());
    if actual != expected {
        bail!("下载的程序版本为“{actual}”，预期为“{expected}”");
    }
    Ok(())
}

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new() -> Result<Self> {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "flowwatch-update-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path)
            .with_context(|| format!("无法创建更新临时目录 {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self { path })
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn versions_compare_without_lexicographic_errors() {
        let current = Version::parse("v0.9.9").unwrap();
        let newer = Version::parse("0.10.0").unwrap();
        assert_eq!(decide(current, newer), UpdateDecision::Upgrade);
        assert_eq!(decide(newer, current), UpdateDecision::Downgrade);
        assert_eq!(decide(current, current), UpdateDecision::Current);
        assert_eq!(newer.tag(), "v0.10.0");
        assert!(Version::parse("1.2").is_err());
        assert!(Version::parse("1.2.3-beta").is_err());
    }

    #[test]
    fn current_and_downgrade_requests_never_download() {
        let current = env!("CARGO_PKG_VERSION");
        let result = run(true, Some(current)).unwrap();
        assert!(!result.installed);
        assert_eq!(result.current, result.target);
        assert!(run(false, Some("0.0.0")).is_err());
    }

    #[test]
    fn checksum_requires_one_exact_matching_asset() {
        let temporary = TemporaryDirectory::new().unwrap();
        let archive = temporary.path.join("flowwatch-test.tar.gz");
        fs::write(&archive, b"release bytes").unwrap();
        let hash = sha256_file(&archive).unwrap();
        let checksums = temporary.path.join("SHA256SUMS");
        fs::write(&checksums, format!("{hash}  flowwatch-test.tar.gz\n")).unwrap();
        verify_checksum(&archive, &checksums, "flowwatch-test.tar.gz").unwrap();

        fs::write(&checksums, format!("{hash}  another.tar.gz\n")).unwrap();
        assert!(verify_checksum(&archive, &checksums, "flowwatch-test.tar.gz").is_err());
        fs::write(
            &checksums,
            format!("{hash}  flowwatch-test.tar.gz\n{hash}  flowwatch-test.tar.gz\n"),
        )
        .unwrap();
        assert!(verify_checksum(&archive, &checksums, "flowwatch-test.tar.gz").is_err());
        fs::write(
            &checksums,
            format!("{}  flowwatch-test.tar.gz\n", "0".repeat(64)),
        )
        .unwrap();
        assert!(verify_checksum(&archive, &checksums, "flowwatch-test.tar.gz").is_err());
    }

    #[test]
    fn temporary_directory_is_removed_after_drop() {
        let path = {
            let temporary = TemporaryDirectory::new().unwrap();
            fs::write(temporary.path.join("partial-download"), b"partial").unwrap();
            temporary.path.clone()
        };
        assert!(!path.exists());
    }

    #[test]
    fn archive_extraction_selects_only_the_flowwatch_file() {
        let temporary = TemporaryDirectory::new().unwrap();
        let source = temporary.path.join("source");
        let extracted = temporary.path.join("extracted");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&extracted).unwrap();
        fs::write(source.join("flowwatch"), b"binary").unwrap();
        fs::write(source.join("unrelated"), b"must not extract").unwrap();
        let archive = temporary.path.join("release.tar.gz");
        let status = Command::new(TAR)
            .args(["-czf"])
            .arg(&archive)
            .args(["-C"])
            .arg(&source)
            .args(["flowwatch", "unrelated"])
            .status()
            .unwrap();
        assert!(status.success());
        extract_binary(&archive, &extracted).unwrap();
        assert_eq!(fs::read(extracted.join("flowwatch")).unwrap(), b"binary");
        assert!(!extracted.join("unrelated").exists());

        let missing = temporary.path.join("missing.tar.gz");
        let status = Command::new(TAR)
            .args(["-czf"])
            .arg(&missing)
            .args(["-C"])
            .arg(&source)
            .arg("unrelated")
            .status()
            .unwrap();
        assert!(status.success());
        let missing_output = temporary.path.join("missing-output");
        fs::create_dir(&missing_output).unwrap();
        assert!(extract_binary(&missing, &missing_output).is_err());
        assert!(!missing_output.join("flowwatch").exists());
    }

    #[test]
    fn downloaded_binary_must_report_the_exact_target_version() {
        let temporary = TemporaryDirectory::new().unwrap();
        let binary = temporary.path.join("flowwatch");
        make_executable(&binary, "#!/bin/sh\nprintf 'flowwatch 0.2.0\\n'\n");
        verify_binary_version(&binary, Version::parse("0.2.0").unwrap()).unwrap();
        assert!(verify_binary_version(&binary, Version::parse("0.2.1").unwrap()).is_err());

        make_executable(&binary, "#!/bin/sh\nexit 7\n");
        assert!(verify_binary_version(&binary, Version::parse("0.2.0").unwrap()).is_err());
    }

    #[test]
    fn installer_failure_is_returned_to_the_user() {
        let temporary = TemporaryDirectory::new().unwrap();
        let binary = temporary.path.join("flowwatch");
        make_executable(&binary, "#!/bin/sh\nexit 7\n");
        assert!(install_binary(&binary).is_err());
    }

    #[test]
    fn failed_download_is_reported() {
        let temporary = TemporaryDirectory::new().unwrap();
        let destination = temporary.path.join("partial");
        assert!(
            download_file(
                "file:///definitely/missing/flowwatch-release.tar.gz",
                &destination,
                1024,
            )
            .is_err()
        );
    }
}
