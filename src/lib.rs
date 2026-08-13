use anyhow::{Context, Result, anyhow, bail};
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

pub const PASTEBOARD_MIN_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandidateKind {
    Recommended,
    Optional,
    Caution,
    Protected,
}

impl CandidateKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recommended => "建议清理",
            Self::Optional => "可选缓存",
            Self::Caution => "谨慎清理",
            Self::Protected => "受保护",
        }
    }

    pub fn selectable(self) -> bool {
        self != Self::Protected
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupPolicy {
    ClearContents,
    MatchingFiles {
        extensions: &'static [&'static str],
        min_age: Duration,
    },
    StalePasteboardArchives {
        min_age: Duration,
    },
    Never,
}

#[derive(Clone, Debug)]
pub struct CandidateSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub path: PathBuf,
    pub description: &'static str,
    pub kind: CandidateKind,
    pub policy: CleanupPolicy,
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub allocated_bytes: u64,
    pub reclaimable_bytes: u64,
    pub logical_bytes: u64,
    pub files: u64,
    pub directories: u64,
    pub errors: Vec<String>,
    pub open_handles: bool,
}

impl ScanResult {
    pub fn accessible(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FullDiskAccess {
    Granted,
    Denied,
    Unknown,
}

impl FullDiskAccess {
    pub fn label(self) -> &'static str {
        match self {
            Self::Granted => "已授权",
            Self::Denied => "未授权",
            Self::Unknown => "无法判断",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VolumeUsage {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("无法确定当前用户主目录（HOME 未设置）"))
}

pub fn default_candidates(home: &Path) -> Vec<CandidateSpec> {
    let caches = home.join("Library/Caches");
    let application_support = home.join("Library/Application Support");
    vec![
        CandidateSpec {
            id: "shared-pasteboard",
            name: "通用剪贴板遗留归档",
            path: home.join(
                "Library/Group Containers/group.com.apple.coreservices.useractivityd/shared-pasteboard/archives",
            ),
            description: "只清理超过 1 小时、且未被进程使用的共享剪贴板归档",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::StalePasteboardArchives {
                min_age: PASTEBOARD_MIN_AGE,
            },
        },
        CandidateSpec {
            id: "google-cache",
            name: "Chrome 用户缓存",
            path: caches.join("Google"),
            description: "Chrome 可重建缓存；清理前请退出 Chrome",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "chrome-on-device-model",
            name: "Chrome 本地 AI 模型",
            path: application_support.join("Google/Chrome/OptGuideOnDeviceModel"),
            description: "Chrome 按需下载的本地模型；需要时可能重新下载",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "google-updater-cache",
            name: "Google Updater 缓存",
            path: application_support.join("Google/GoogleUpdater/crx_cache"),
            description: "Google 更新下载缓存；清理前请退出 Chrome",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "chrome-service-worker-cache",
            name: "Chrome 站点缓存",
            path: application_support.join("Google/Chrome/Default/Service Worker/CacheStorage"),
            description: "站点 Service Worker 缓存；可能影响离线网页，不清除登录信息",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "davinci-cache-clip",
            name: "DaVinci 渲染缓存",
            path: home.join("Movies/DaVinci Resolve/CacheClip"),
            description: "可重新生成的 CacheClip；清理前请退出 DaVinci Resolve",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "codex-runtime-cache",
            name: "Codex runtime 缓存",
            path: home.join(".cache/codex-runtimes"),
            description: "文档等功能的运行时缓存；需要时会重新下载",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "codex-cache",
            name: "Codex/ChatGPT 应用缓存",
            path: caches.join("com.openai.codex"),
            description: "可重建的应用缓存；清理前请退出 ChatGPT/Codex",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "adobe-camera-raw",
            name: "Adobe Camera Raw 2",
            path: caches.join("Adobe Camera Raw 2"),
            description: "Adobe Camera Raw 可重建缓存；建议先退出 Adobe 应用",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "lightroom-cache",
            name: "Lightroom Classic 缓存",
            path: caches.join("com.adobe.LightroomClassicCC7"),
            description: "Lightroom 可重建缓存；清理前请退出 Lightroom",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "adobe-cache",
            name: "Adobe 通用缓存",
            path: caches.join("Adobe"),
            description: "Adobe 应用可重建缓存；建议先退出 Adobe 应用",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "adobe-logs",
            name: "Adobe 日志",
            path: home.join("Library/Logs/Adobe"),
            description: "Adobe 诊断日志；清理前请退出 Adobe 应用",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "cargo-registry",
            name: "Cargo registry",
            path: home.join(".cargo/registry"),
            description: "Rust crate 下载缓存和解压源码；构建时会重新下载",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "npm-cache",
            name: "npm 缓存",
            path: home.join(".npm/_cacache"),
            description: "npm 包缓存；安装依赖时会重新下载",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "pnpm-store",
            name: "pnpm store",
            path: home.join("Library/pnpm/store"),
            description: "pnpm 内容寻址包仓库；清理后安装依赖会重新下载",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "pnpm-cache",
            name: "pnpm 元数据缓存",
            path: caches.join("pnpm"),
            description: "pnpm 可重建元数据缓存",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "homebrew-cache",
            name: "Homebrew 下载缓存",
            path: caches.join("Homebrew"),
            description: "已下载的软件包缓存，需要时会重新下载",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        },
        CandidateSpec {
            id: "downloaded-installers",
            name: "Downloads 安装包",
            path: home.join("Downloads"),
            description: "只删除顶层且超过 1 小时的 .dmg/.pkg；保留其他下载内容",
            kind: CandidateKind::Caution,
            policy: CleanupPolicy::MatchingFiles {
                extensions: &["dmg", "pkg"],
                min_age: PASTEBOARD_MIN_AGE,
            },
        },
        CandidateSpec {
            id: "trash",
            name: "废纸篓",
            path: home.join(".Trash"),
            description: "永久删除废纸篓内容，无法从废纸篓恢复",
            kind: CandidateKind::Caution,
            policy: CleanupPolicy::ClearContents,
        },
    ]
}

pub fn detect_full_disk_access(home: &Path) -> FullDiskAccess {
    let probes = [
        home.join(
            "Library/Group Containers/group.com.apple.coreservices.useractivityd/shared-pasteboard",
        ),
        home.join("Library/Mail"),
        home.join("Library/Messages"),
    ];

    let mut tested = false;
    for path in probes.iter().filter(|path| path.exists()) {
        tested = true;
        match fs::read_dir(path) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return FullDiskAccess::Denied;
            }
            Err(_) => return FullDiskAccess::Unknown,
        }
    }

    if tested {
        FullDiskAccess::Granted
    } else {
        FullDiskAccess::Unknown
    }
}

pub fn open_full_disk_access_settings() -> Result<()> {
    let status = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .status()
        .context("无法启动 macOS 系统设置")?;
    if !status.success() {
        bail!("open 命令返回失败状态：{status}");
    }
    Ok(())
}

pub fn volume_usage() -> Result<VolumeUsage> {
    let output = Command::new("df")
        .args(["-k", "/System/Volumes/Data"])
        .output()
        .context("无法执行 df")?;
    if !output.status.success() {
        bail!("df 执行失败");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .last()
        .ok_or_else(|| anyhow!("df 没有返回数据"))?;
    let columns: Vec<_> = line.split_whitespace().collect();
    if columns.len() < 4 {
        bail!("无法解析 df 输出：{line}");
    }
    let parse_kib = |value: &str| -> Result<u64> {
        Ok(value
            .parse::<u64>()
            .with_context(|| format!("无法解析 df 数值：{value}"))?
            .saturating_mul(1024))
    };

    Ok(VolumeUsage {
        total_bytes: parse_kib(columns[1])?,
        used_bytes: parse_kib(columns[2])?,
        available_bytes: parse_kib(columns[3])?,
    })
}

pub fn scan_candidate(spec: &CandidateSpec) -> ScanResult {
    if let CleanupPolicy::MatchingFiles {
        extensions,
        min_age,
    } = spec.policy
    {
        return scan_matching_files(&spec.path, extensions, min_age);
    }

    let mut result = ScanResult::default();
    let mut seen_files = HashSet::new();

    if !spec.path.exists() {
        return result;
    }
    if let Err(error) = accumulate_path(&spec.path, &mut result, &mut seen_files) {
        result
            .errors
            .push(format!("{}: {error}", spec.path.display()));
    }

    result.reclaimable_bytes = match spec.policy {
        CleanupPolicy::ClearContents => result.allocated_bytes,
        CleanupPolicy::MatchingFiles { .. } => unreachable!("已在上方单独处理"),
        CleanupPolicy::Never => 0,
        CleanupPolicy::StalePasteboardArchives { min_age } => {
            result.open_handles = has_open_handles(&spec.path).unwrap_or(true);
            if result.open_handles {
                0
            } else {
                reclaimable_direct_entries(&spec.path, min_age, &mut result.errors)
            }
        }
    };
    result
}

fn scan_matching_files(path: &Path, extensions: &[&str], min_age: Duration) -> ScanResult {
    let mut result = ScanResult::default();
    if !path.exists() {
        return result;
    }
    let cutoff = SystemTime::now()
        .checked_sub(min_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) => {
            result.errors.push(format!("{}: {error}", path.display()));
            return result;
        }
    };

    for entry in entries.flatten() {
        let entry_path = entry.path();
        match fs::symlink_metadata(&entry_path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && has_extension(&entry_path, extensions)
                    && metadata
                        .modified()
                        .map(|time| time <= cutoff)
                        .unwrap_or(false) =>
            {
                result.files += 1;
                result.allocated_bytes = result
                    .allocated_bytes
                    .saturating_add(allocated_size(&metadata));
                result.logical_bytes = result.logical_bytes.saturating_add(metadata.len());
            }
            Ok(_) => {}
            Err(error) => result
                .errors
                .push(format!("{}: {error}", entry_path.display())),
        }
    }
    result.reclaimable_bytes = result.allocated_bytes;
    result
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

fn accumulate_path(
    path: &Path,
    result: &mut ScanResult,
    seen_files: &mut HashSet<(u64, u64)>,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        result.files += 1;
        result.allocated_bytes = result
            .allocated_bytes
            .saturating_add(allocated_size(&metadata));
        result.logical_bytes = result.logical_bytes.saturating_add(metadata.len());
        return Ok(());
    }
    if metadata.is_file() {
        if seen_files.insert((metadata.dev(), metadata.ino())) {
            result.files += 1;
            result.allocated_bytes = result
                .allocated_bytes
                .saturating_add(allocated_size(&metadata));
            result.logical_bytes = result.logical_bytes.saturating_add(metadata.len());
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Ok(());
    }

    result.directories += 1;
    result.allocated_bytes = result
        .allocated_bytes
        .saturating_add(allocated_size(&metadata));
    let entries = fs::read_dir(path)?;
    for entry in entries {
        match entry {
            Ok(entry) => {
                if let Err(error) = accumulate_path(&entry.path(), result, seen_files) {
                    result
                        .errors
                        .push(format!("{}: {error}", entry.path().display()));
                }
            }
            Err(error) => result.errors.push(format!("{}: {error}", path.display())),
        }
    }
    Ok(())
}

fn allocated_size(metadata: &Metadata) -> u64 {
    metadata.blocks().saturating_mul(512)
}

fn reclaimable_direct_entries(path: &Path, min_age: Duration, errors: &mut Vec<String>) -> u64 {
    let cutoff = SystemTime::now()
        .checked_sub(min_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut total = 0_u64;
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => {
                let old_enough = metadata
                    .modified()
                    .map(|time| time <= cutoff)
                    .unwrap_or(false);
                if old_enough {
                    let mut scan = ScanResult::default();
                    let mut seen = HashSet::new();
                    if let Err(error) = accumulate_path(&entry.path(), &mut scan, &mut seen) {
                        errors.push(format!("{}: {error}", entry.path().display()));
                    } else {
                        total = total.saturating_add(scan.allocated_bytes);
                    }
                }
            }
            Err(error) => errors.push(format!("{}: {error}", entry.path().display())),
        }
    }
    total
}

fn has_open_handles(path: &Path) -> Result<bool> {
    let output = Command::new("/usr/sbin/lsof")
        .arg("+D")
        .arg(path)
        .output()
        .context("无法执行 lsof；为安全起见不会清理剪贴板归档")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.lines().skip(1).any(|line| !line.trim().is_empty()))
}

pub fn clean_candidate(spec: &CandidateSpec, home: &Path) -> Result<u64> {
    validate_cleanup_target(spec, home)?;
    let before = scan_candidate(spec).reclaimable_bytes;

    match spec.policy {
        CleanupPolicy::Never => bail!("{} 被标记为受保护数据", spec.name),
        CleanupPolicy::ClearContents => clear_directory_contents(&spec.path)?,
        CleanupPolicy::MatchingFiles {
            extensions,
            min_age,
        } => remove_matching_files(&spec.path, extensions, min_age)?,
        CleanupPolicy::StalePasteboardArchives { min_age } => {
            if has_open_handles(&spec.path)? {
                bail!("剪贴板归档正在被进程使用，已拒绝清理");
            }
            remove_stale_direct_entries(&spec.path, min_age)?;
        }
    }

    let after = scan_candidate(spec).reclaimable_bytes;
    Ok(before.saturating_sub(after))
}

fn remove_matching_files(path: &Path, extensions: &[&str], min_age: Duration) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let cutoff = SystemTime::now()
        .checked_sub(min_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in fs::read_dir(path).with_context(|| format!("无法读取 {}", path.display()))? {
        let entry = entry?;
        let entry_path = entry.path();
        let metadata = fs::symlink_metadata(&entry_path)?;
        if metadata.is_file()
            && !metadata.file_type().is_symlink()
            && has_extension(&entry_path, extensions)
            && metadata
                .modified()
                .map(|time| time <= cutoff)
                .unwrap_or(false)
        {
            fs::remove_file(&entry_path)
                .with_context(|| format!("无法删除文件 {}", entry_path.display()))?;
        }
    }
    Ok(())
}

fn validate_cleanup_target(spec: &CandidateSpec, home: &Path) -> Result<()> {
    let protected_data = home.join("bej48_local");
    if spec.kind == CandidateKind::Protected || spec.policy == CleanupPolicy::Never {
        bail!("受保护路径永远不能清理：{}", spec.path.display());
    }
    if spec.path.starts_with(&protected_data) {
        bail!("拒绝清理受保护用户数据：{}", spec.path.display());
    }
    if !spec.path.starts_with(home) || spec.path == home {
        bail!("拒绝清理主目录之外或主目录本身：{}", spec.path.display());
    }
    if spec
        .path
        .components()
        .any(|component| component.as_os_str() == "..")
    {
        bail!("拒绝包含上级目录组件的路径");
    }
    if let Ok(metadata) = fs::symlink_metadata(&spec.path)
        && metadata.file_type().is_symlink()
    {
        bail!("拒绝清理符号链接目标：{}", spec.path.display());
    }
    Ok(())
}

fn clear_directory_contents(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path).with_context(|| format!("无法读取 {}", path.display()))? {
        let entry = entry?;
        remove_entry(&entry.path())?;
    }
    Ok(())
}

fn remove_stale_direct_entries(path: &Path, min_age: Duration) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let cutoff = SystemTime::now()
        .checked_sub(min_age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for entry in fs::read_dir(path).with_context(|| format!("无法读取 {}", path.display()))? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata
            .modified()
            .map(|time| time <= cutoff)
            .unwrap_or(false)
        {
            remove_entry(&entry.path())?;
        }
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).with_context(|| format!("无法删除目录 {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("无法删除文件 {}", path.display()))?;
    }
    Ok(())
}

pub fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = (1024_u64 * 1024 * 1024) as f64;
    let value = bytes as f64 / GIB;
    if value >= 1000.0 {
        format!("{value:.0} GiB")
    } else if value >= 100.0 {
        format!("{value:.1} GiB")
    } else if value >= 10.0 {
        format!("{value:.2} GiB")
    } else {
        format!("{value:.3} GiB")
    }
}

pub fn display_path(path: &Path, home: &Path) -> OsString {
    path.strip_prefix(home)
        .map(|relative| Path::new("~").join(relative).into_os_string())
        .unwrap_or_else(|_| path.as_os_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::symlink;

    fn test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "clean-my-mac-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn format_bytes_is_human_readable() {
        assert_eq!(format_bytes(0), "0.000 GiB");
        assert_eq!(format_bytes(512 * 1024 * 1024), "0.500 GiB");
        assert_eq!(format_bytes(50 * 1024 * 1024 * 1024), "50.00 GiB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024 * 1024), "2048 GiB");
    }

    #[test]
    fn clear_contents_preserves_root_directory() -> Result<()> {
        let home = test_dir("clear");
        let target = home.join("Library/Caches/Test");
        fs::create_dir_all(target.join("nested"))?;
        fs::write(target.join("nested/data"), vec![1_u8; 4096])?;
        fs::write(target.join("file"), b"cache")?;
        let spec = CandidateSpec {
            id: "test",
            name: "test",
            path: target.clone(),
            description: "test",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        };

        clean_candidate(&spec, &home)?;
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(&target)?.count(), 0);
        fs::remove_dir_all(home)?;
        Ok(())
    }

    #[test]
    fn protected_candidate_cannot_be_cleaned() -> Result<()> {
        let home = test_dir("protected");
        let target = home.join("bej48_local");
        fs::create_dir_all(&target)?;
        let mut file = fs::File::create(target.join("important"))?;
        file.write_all(b"do not delete")?;
        let spec = CandidateSpec {
            id: "protected",
            name: "protected",
            path: target.clone(),
            description: "test",
            kind: CandidateKind::Protected,
            policy: CleanupPolicy::Never,
        };

        assert!(clean_candidate(&spec, &home).is_err());
        assert!(target.join("important").exists());
        fs::remove_dir_all(home)?;
        Ok(())
    }

    #[test]
    fn bej48_local_subpaths_are_always_rejected() -> Result<()> {
        let home = test_dir("bej48-guard");
        let target = home.join("bej48_local/project/cache");
        fs::create_dir_all(&target)?;
        fs::write(target.join("important"), b"do not delete")?;
        let spec = CandidateSpec {
            id: "disguised-cache",
            name: "disguised-cache",
            path: target.clone(),
            description: "test",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        };

        assert!(clean_candidate(&spec, &home).is_err());
        assert!(target.join("important").exists());
        fs::remove_dir_all(home)?;
        Ok(())
    }

    #[test]
    fn paths_outside_home_are_rejected() {
        let home = test_dir("guard");
        let spec = CandidateSpec {
            id: "bad",
            name: "bad",
            path: PathBuf::from("/tmp/not-allowed"),
            description: "test",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        };
        assert!(clean_candidate(&spec, &home).is_err());
    }

    #[test]
    fn pasteboard_cleanup_keeps_archive_directory() -> Result<()> {
        let home = test_dir("pasteboard");
        let target = home.join("Library/Group Containers/test/shared-pasteboard/archives");
        fs::create_dir_all(&target)?;
        fs::write(target.join("stale-archive"), vec![7_u8; 8192])?;
        let spec = CandidateSpec {
            id: "pasteboard-test",
            name: "pasteboard-test",
            path: target.clone(),
            description: "test",
            kind: CandidateKind::Recommended,
            policy: CleanupPolicy::StalePasteboardArchives {
                min_age: Duration::ZERO,
            },
        };

        clean_candidate(&spec, &home)?;
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(&target)?.count(), 0);
        fs::remove_dir_all(home)?;
        Ok(())
    }

    #[test]
    fn matching_files_cleanup_only_removes_top_level_installers() -> Result<()> {
        let home = test_dir("installers");
        let downloads = home.join("Downloads");
        fs::create_dir_all(downloads.join("nested"))?;
        fs::write(downloads.join("app.dmg"), b"installer")?;
        fs::write(downloads.join("update.PKG"), b"installer")?;
        fs::write(downloads.join("notes.txt"), b"keep")?;
        fs::write(downloads.join("nested/keep.dmg"), b"keep")?;
        let spec = CandidateSpec {
            id: "installer-test",
            name: "installer-test",
            path: downloads.clone(),
            description: "test",
            kind: CandidateKind::Caution,
            policy: CleanupPolicy::MatchingFiles {
                extensions: &["dmg", "pkg"],
                min_age: Duration::ZERO,
            },
        };

        let scan = scan_candidate(&spec);
        assert_eq!(scan.files, 2);
        assert!(scan.reclaimable_bytes > 0);
        clean_candidate(&spec, &home)?;
        assert!(!downloads.join("app.dmg").exists());
        assert!(!downloads.join("update.PKG").exists());
        assert!(downloads.join("notes.txt").exists());
        assert!(downloads.join("nested/keep.dmg").exists());
        fs::remove_dir_all(home)?;
        Ok(())
    }

    #[test]
    fn symlink_cleanup_target_is_rejected() -> Result<()> {
        let home = test_dir("symlink-home");
        let outside = test_dir("symlink-outside");
        fs::create_dir_all(&home)?;
        fs::create_dir_all(&outside)?;
        fs::write(outside.join("important"), b"keep")?;
        let target = home.join("fake-cache");
        symlink(&outside, &target)?;
        let spec = CandidateSpec {
            id: "symlink-test",
            name: "symlink-test",
            path: target,
            description: "test",
            kind: CandidateKind::Optional,
            policy: CleanupPolicy::ClearContents,
        };

        assert!(clean_candidate(&spec, &home).is_err());
        assert!(outside.join("important").exists());
        fs::remove_file(home.join("fake-cache"))?;
        fs::remove_dir_all(home)?;
        fs::remove_dir_all(outside)?;
        Ok(())
    }
}
