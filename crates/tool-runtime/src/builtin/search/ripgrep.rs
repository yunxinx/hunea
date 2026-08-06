use std::{
    env,
    fs::{self},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, task};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::error::SearchToolError;

const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// spawn `--version` 超时，防止挂起候选阻塞启动。
const SPAWN_VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SPAWN_VERIFY_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);
/// 下载进度节流：避免每个 chunk 都塞 channel。
const PROGRESS_EMIT_BYTES: u64 = 64 * 1024;
const PROGRESS_EMIT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// `ManagedRipgrepConfig` 保存 managed ripgrep 的授权配置面。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedRipgrepConfig {
    pub allow_managed_rg: Option<bool>,
}

impl ManagedRipgrepConfig {
    pub fn allows(&self) -> bool {
        self.allow_managed_rg == Some(true)
    }

    /// 区分「未配置」与「明确拒绝」：仅 `Some(false)` 返回 true。
    pub fn rejects(&self) -> bool {
        self.allow_managed_rg == Some(false)
    }
}

const RIPGREP_BINARY_NAME: &str = "rg";
/// Managed ripgrep 在 precheck 与安装器中共享的展示名。
pub const MANAGED_RIPGREP_NAME: &str = "ripgrep";
/// Managed ripgrep 固定安装版本。
pub const MANAGED_RIPGREP_VERSION: &str = "15.1.0";
const RIPGREP_REPOSITORY: &str = "BurntSushi/ripgrep";

fn ripgrep_executable_file_name() -> &'static str {
    if cfg!(windows) {
        "rg.exe"
    } else {
        RIPGREP_BINARY_NAME
    }
}

/// Managed ripgrep 可用性（纯检测，无网络）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagedRipgrepStatus {
    SystemPath(PathBuf),
    Bundled(PathBuf),
    ManagedReady(PathBuf),
    /// 已授权但 managed 二进制缺失/损坏。
    NeedsRebuild,
    /// 未授权且不可用，需用户决策。
    NeedsDownload,
    /// `allow_managed_rg = Some(false)`。
    NotAuthorized,
    /// Termux/Android：managed 二进制不兼容。
    AndroidIncompatible,
}

/// 安装进度事件（precheck 通过 channel 接收）。
#[derive(Debug, Clone)]
pub enum ManagedRipgrepProgress {
    /// `bytes_total` 为 None 表示无 content-length。
    Downloading {
        bytes_received: u64,
        bytes_total: Option<u64>,
    },
    Verifying,
    Extracting,
    Installing,
    Ready {
        path: PathBuf,
    },
    Failed {
        error: String,
    },
}

/// 安装对外错误：不暴露内部 `SearchToolError`；详情走 `ManagedRipgrepProgress::Failed`。
#[derive(Debug, thiserror::Error)]
pub enum ManagedRipgrepInstallError {
    #[error("managed ripgrep install interrupted")]
    Interrupted,
    #[error("{message}")]
    Other { message: String },
}

impl From<SearchToolError> for ManagedRipgrepInstallError {
    fn from(error: SearchToolError) -> Self {
        match error {
            SearchToolError::Interrupted => Self::Interrupted,
            other => Self::Other {
                message: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RipgrepBackend {
    SystemPath,
    Bundled,
    Managed,
}

impl RipgrepBackend {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SystemPath => "system_path",
            Self::Bundled => "bundled",
            Self::Managed => "managed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RipgrepCommand {
    pub(crate) path: PathBuf,
    pub(crate) backend: RipgrepBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RipgrepCommandPlan {
    Ready(RipgrepCommand),
    Unavailable,
}

#[derive(Debug, Clone, Copy)]
struct ManagedRipgrepManifest {
    asset_name: &'static str,
    sha256: &'static str,
    archive_kind: ArchiveKind,
}

impl ManagedRipgrepManifest {
    fn url(self) -> String {
        format!(
            "https://github.com/{}/releases/download/{}/{}",
            RIPGREP_REPOSITORY, MANAGED_RIPGREP_VERSION, self.asset_name
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Debug, Clone)]
struct RipgrepInstallPaths {
    archive_path: PathBuf,
    extract_dir: PathBuf,
    version_temp_dir: PathBuf,
    final_version_dir: PathBuf,
    version_binary: PathBuf,
    stable_entry: PathBuf,
}

pub(crate) fn resolve_ripgrep_command_plan(
    config: &ManagedRipgrepConfig,
    managed_root: &Path,
) -> RipgrepCommandPlan {
    // 运行时只做文件检查，不 spawn。严格验证留给 precheck 的 detect_managed_ripgrep_status；
    // 合并两套会让热路径 fork 或让 precheck 变松。坏 system/bundled 由 managed_ripgrep_fallback_for 兜底。
    //
    // 授权契约：allow_managed_rg 只管「下载/重建/静默装」；已装好的 managed 二进制能否用
    // 只看 rejects（Some(false) 硬拒绝）。None/true 且文件在 → 可用。
    if let Some(path) = find_system_ripgrep_path() {
        return RipgrepCommandPlan::Ready(RipgrepCommand {
            path,
            backend: RipgrepBackend::SystemPath,
        });
    }
    if let Some(path) = find_bundled_ripgrep_path() {
        return RipgrepCommandPlan::Ready(RipgrepCommand {
            path,
            backend: RipgrepBackend::Bundled,
        });
    }
    if !config.rejects()
        && let Some(path) = managed_ripgrep_entry_file_exists(managed_root)
    {
        return RipgrepCommandPlan::Ready(RipgrepCommand {
            path,
            backend: RipgrepBackend::Managed,
        });
    }
    RipgrepCommandPlan::Unavailable
}

/// primary 非 managed 时，返回 managed 候选供执行失败后重试。
/// 与 `resolve_ripgrep_command_plan` 同一授权契约：仅 `rejects` 挡住已装二进制。
pub(crate) fn managed_ripgrep_fallback_for(
    plan: &RipgrepCommandPlan,
    config: &ManagedRipgrepConfig,
    managed_root: &Path,
) -> Option<RipgrepCommand> {
    match plan {
        RipgrepCommandPlan::Ready(cmd) if cmd.backend == RipgrepBackend::Managed => None,
        RipgrepCommandPlan::Unavailable => None,
        _ => {
            if config.rejects() {
                return None;
            }
            managed_ripgrep_entry_file_exists(managed_root).map(|path| RipgrepCommand {
                path,
                backend: RipgrepBackend::Managed,
            })
        }
    }
}

/// 纯检测（spawn `--version`），无网络。顺序：system → bundled → managed → Android/授权。
///
/// system/bundled ripgrep 不受 managed 授权影响；`Some(false)` 明确固定使用 Rust fallback，
/// 因此不会探测、使用或提示重建 managed binary。
pub fn detect_managed_ripgrep_status(
    config: &ManagedRipgrepConfig,
    managed_root: &Path,
) -> ManagedRipgrepStatus {
    if let Some(path) = find_system_ripgrep_verified() {
        return ManagedRipgrepStatus::SystemPath(path);
    }
    if let Some(path) = find_bundled_ripgrep_verified() {
        return ManagedRipgrepStatus::Bundled(path);
    }
    if config.rejects() {
        return ManagedRipgrepStatus::NotAuthorized;
    }
    if let Some(path) = usable_managed_ripgrep_entry_verified(managed_root) {
        return ManagedRipgrepStatus::ManagedReady(path);
    }
    // manifest 无 android 目标，下载也不可用。
    if env::consts::OS == "android" {
        return ManagedRipgrepStatus::AndroidIncompatible;
    }
    // 已授权：文件在但 spawn 失败 → Rebuild；文件不在 → Download。
    if config.allows() {
        if managed_ripgrep_entry_file_exists(managed_root).is_some() {
            return ManagedRipgrepStatus::NeedsRebuild;
        }
        return ManagedRipgrepStatus::NeedsDownload;
    }
    ManagedRipgrepStatus::NeedsDownload
}

// --- 轻量检查（运行时热路径用，只检查文件存在性与可执行权限）---

fn find_system_ripgrep_path() -> Option<PathBuf> {
    [RIPGREP_BINARY_NAME]
        .iter()
        .find_map(|name| find_executable_on_path(name))
}

fn find_executable_on_path(binary_name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|directory| directory.join(binary_name))
        .find(|candidate| is_executable_file(candidate))
}

fn find_bundled_ripgrep_path() -> Option<PathBuf> {
    let executable_dir = env::current_exe().ok()?.parent()?.to_path_buf();
    [
        executable_dir
            .join("tools")
            .join(RIPGREP_BINARY_NAME)
            .join(MANAGED_RIPGREP_VERSION)
            .join(ripgrep_executable_file_name()),
        executable_dir
            .join("tools")
            .join(ripgrep_executable_file_name()),
        executable_dir.join(ripgrep_executable_file_name()),
    ]
    .into_iter()
    .find(|candidate| is_executable_file(candidate))
}

fn managed_ripgrep_entry_file_exists(managed_root: &Path) -> Option<PathBuf> {
    let entry = managed_ripgrep_entry_path(managed_root);
    is_executable_file(&entry).then_some(entry)
}

// --- spawn 验证（precheck 用，一次性，带超时）---

fn find_system_ripgrep_verified() -> Option<PathBuf> {
    // 对 PATH 上全部候选 spawn 验证，避免坏的第一个短路后续可用项。
    let paths = env::var_os("PATH")?;
    [RIPGREP_BINARY_NAME]
        .iter()
        .flat_map(|name| env::split_paths(&paths).map(move |directory| directory.join(name)))
        .filter(|candidate| is_executable_file(candidate))
        .find(|path| verify_binary_via_spawn(path))
}

fn find_bundled_ripgrep_verified() -> Option<PathBuf> {
    find_bundled_ripgrep_path().filter(|path| verify_binary_via_spawn(path))
}

fn usable_managed_ripgrep_entry_verified(managed_root: &Path) -> Option<PathBuf> {
    managed_ripgrep_entry_file_exists(managed_root).filter(|path| verify_binary_via_spawn(path))
}

/// 带进度的 managed ripgrep 安装。cancel 后清临时文件，不续传。`progress_tx` send 失败表示调用方已退出。
pub async fn install_managed_ripgrep_with_progress(
    managed_root: &Path,
    cancellation: CancellationToken,
    progress_tx: &tokio::sync::mpsc::UnboundedSender<ManagedRipgrepProgress>,
) -> Result<PathBuf, ManagedRipgrepInstallError> {
    let outcome =
        install_managed_ripgrep_with_progress_inner(managed_root, &cancellation, progress_tx).await;
    match &outcome {
        Ok(path) => {
            let _ = progress_tx.send(ManagedRipgrepProgress::Ready { path: path.clone() });
        }
        Err(error) => {
            let _ = progress_tx.send(ManagedRipgrepProgress::Failed {
                error: error.to_string(),
            });
        }
    }
    outcome.map_err(ManagedRipgrepInstallError::from)
}

async fn install_managed_ripgrep_with_progress_inner(
    managed_root: &Path,
    cancellation: &CancellationToken,
    progress_tx: &tokio::sync::mpsc::UnboundedSender<ManagedRipgrepProgress>,
) -> Result<PathBuf, SearchToolError> {
    ensure_not_cancelled(cancellation)?;
    let manifest =
        ripgrep_manifest_for_current_platform().ok_or(SearchToolError::NoManagedRipgrepAsset)?;
    let stable_entry = managed_ripgrep_entry_path(managed_root);
    let version_binary = managed_ripgrep_version_binary_path(managed_root);
    if is_executable_file(&stable_entry)
        && verify_binary_on_blocking_pool(&stable_entry, cancellation).await?
    {
        return Ok(stable_entry);
    }
    if is_executable_file(&version_binary)
        && verify_binary_on_blocking_pool(&version_binary, cancellation).await?
    {
        let cancellation = cancellation.clone();
        let version_binary = version_binary.clone();
        let stable_entry_for_update = stable_entry.clone();
        task::spawn_blocking(move || {
            update_stable_entry(&version_binary, &stable_entry_for_update, &cancellation)
        })
        .await
        .map_err(|source| SearchToolError::JoinTask {
            operation: "managed entry update",
            source,
        })??;
        return Ok(stable_entry);
    }

    let temp_root = managed_root.join("tmp");
    tokio::fs::create_dir_all(&temp_root)
        .await
        .map_err(|source| SearchToolError::PathIo {
            operation: "create managed ripgrep temp directory",
            path: temp_root.clone(),
            source,
        })?;
    let unique = format!(
        "{}-{}-{}",
        RIPGREP_BINARY_NAME,
        std::process::id(),
        unix_millis()
    );
    let paths = RipgrepInstallPaths {
        archive_path: temp_root.join(format!("{unique}.archive")),
        extract_dir: temp_root.join(format!("{unique}.extract")),
        version_temp_dir: temp_root.join(format!("{unique}.version")),
        final_version_dir: managed_ripgrep_version_dir(managed_root),
        version_binary,
        stable_entry,
    };

    let install_result = async {
        let url = manifest.url();
        download_archive(&url, &paths.archive_path, cancellation, Some(progress_tx)).await?;
        let blocking_paths = paths.clone();
        let blocking_cancellation = cancellation.clone();
        let progress_tx_for_blocking = progress_tx.clone();
        let stable_entry = task::spawn_blocking(move || {
            install_archive_blocking(
                blocking_paths,
                manifest.sha256,
                manifest.archive_kind,
                ripgrep_executable_file_name(),
                blocking_cancellation,
                Some(&progress_tx_for_blocking),
            )
        })
        .await
        .map_err(|source| SearchToolError::JoinTask {
            operation: "managed install",
            source,
        })??;
        Ok(stable_entry)
    }
    .await;

    cleanup_install_paths(&paths).await;
    install_result
}

async fn download_archive(
    url: &str,
    destination: &Path,
    cancellation: &CancellationToken,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<ManagedRipgrepProgress>>,
) -> Result<(), SearchToolError> {
    ensure_not_cancelled(cancellation)?;
    let parsed =
        Url::parse(url).map_err(|source| SearchToolError::InvalidManagedRipgrepUrl { source })?;
    if parsed.host_str() != Some("github.com") {
        return Err(SearchToolError::UnofficialManagedRipgrepUrl {
            url: url.to_string(),
        });
    }

    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(SearchToolError::Interrupted),
        response = reqwest::Client::new()
            .get(url)
            .timeout(DOWNLOAD_TIMEOUT)
            .send() => response.map_err(|source| SearchToolError::Download { source })?,
    };
    let response = response
        .error_for_status()
        .map_err(|source| SearchToolError::Download { source })?;
    let bytes_total = response.content_length();
    let mut response = response;
    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|source| SearchToolError::PathIo {
            operation: "create download file",
            path: destination.to_path_buf(),
            source,
        })?;

    let mut bytes_received: u64 = 0;
    let mut last_emitted_bytes: u64 = 0;
    let mut last_emitted_at = std::time::Instant::now()
        .checked_sub(PROGRESS_EMIT_INTERVAL)
        .unwrap_or_else(std::time::Instant::now);
    loop {
        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(SearchToolError::Interrupted),
            chunk = response.chunk() => chunk
                .map_err(|source| SearchToolError::ReadDownloadBody { source })?,
        };
        let Some(chunk) = chunk else {
            break;
        };
        tokio::select! {
            _ = cancellation.cancelled() => return Err(SearchToolError::Interrupted),
            write = file.write_all(&chunk) => {
                write.map_err(|source| SearchToolError::PathIo {
                    operation: "write download",
                    path: destination.to_path_buf(),
                    source,
                })?;
            }
        }
        bytes_received = bytes_received.saturating_add(chunk.len() as u64);
        if let Some(tx) = progress_tx {
            let now = std::time::Instant::now();
            if should_emit_download_progress(
                bytes_received,
                last_emitted_bytes,
                now.duration_since(last_emitted_at),
            ) {
                let _ = tx.send(ManagedRipgrepProgress::Downloading {
                    bytes_received,
                    bytes_total,
                });
                last_emitted_bytes = bytes_received;
                last_emitted_at = now;
            }
        }
    }
    // 循环结束强制发一次最终字节，避免节流吞掉 100%。
    if let Some(tx) = progress_tx
        && bytes_received > 0
        && bytes_received != last_emitted_bytes
    {
        let _ = tx.send(ManagedRipgrepProgress::Downloading {
            bytes_received,
            bytes_total,
        });
    }
    tokio::select! {
        _ = cancellation.cancelled() => Err(SearchToolError::Interrupted),
        sync = file.sync_all() => sync.map_err(|source| SearchToolError::PathIo {
            operation: "sync download",
            path: destination.to_path_buf(),
            source,
        }),
    }
}

fn install_archive_blocking(
    paths: RipgrepInstallPaths,
    expected_sha256: &str,
    archive_kind: ArchiveKind,
    executable_file_name: &str,
    cancellation: CancellationToken,
    progress_tx: Option<&tokio::sync::mpsc::UnboundedSender<ManagedRipgrepProgress>>,
) -> Result<PathBuf, SearchToolError> {
    ensure_not_cancelled(&cancellation)?;
    if let Some(tx) = progress_tx {
        let _ = tx.send(ManagedRipgrepProgress::Verifying);
    }
    verify_sha256(&paths.archive_path, expected_sha256, &cancellation)?;
    ensure_not_cancelled(&cancellation)?;
    if let Some(tx) = progress_tx {
        let _ = tx.send(ManagedRipgrepProgress::Extracting);
    }
    fs::create_dir_all(&paths.extract_dir).map_err(|source| SearchToolError::PathIo {
        operation: "create extraction directory",
        path: paths.extract_dir.clone(),
        source,
    })?;
    extract_archive(
        &paths.archive_path,
        &paths.extract_dir,
        archive_kind,
        &cancellation,
    )?;
    let extracted_binary =
        find_extracted_binary(&paths.extract_dir, executable_file_name, &cancellation)?;
    ensure_not_cancelled(&cancellation)?;
    fs::create_dir_all(&paths.version_temp_dir).map_err(|source| SearchToolError::PathIo {
        operation: "create version temp directory",
        path: paths.version_temp_dir.clone(),
        source,
    })?;
    let temp_binary = paths.version_temp_dir.join(executable_file_name);
    copy_file_with_cancellation(&extracted_binary, &temp_binary, &cancellation)?;
    make_executable(&temp_binary)?;
    ensure_not_cancelled(&cancellation)?;
    if let Some(tx) = progress_tx {
        let _ = tx.send(ManagedRipgrepProgress::Installing);
    }
    if paths.final_version_dir.exists() {
        let _ = fs::remove_dir_all(&paths.final_version_dir);
    }
    if let Some(parent) = paths.final_version_dir.parent() {
        fs::create_dir_all(parent).map_err(|source| SearchToolError::PathIo {
            operation: "create managed version directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::rename(&paths.version_temp_dir, &paths.final_version_dir).map_err(|source| {
        SearchToolError::PathIo {
            operation: "publish managed version directory",
            path: paths.final_version_dir.clone(),
            source,
        }
    })?;
    update_stable_entry(&paths.version_binary, &paths.stable_entry, &cancellation)?;
    Ok(paths.stable_entry)
}

fn verify_sha256(
    path: &Path,
    expected: &str,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    let mut file = fs::File::open(path).map_err(|source| SearchToolError::PathIo {
        operation: "read archive for checksum",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        ensure_not_cancelled(cancellation)?;
        let read = file
            .read(&mut buffer)
            .map_err(|source| SearchToolError::PathIo {
                operation: "read archive for checksum",
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let actual = base16ct::lower::encode_string(&digest);
    if actual == expected {
        return Ok(());
    }
    let _ = fs::remove_file(path);
    Err(SearchToolError::ChecksumMismatch {
        expected: expected.to_string(),
        actual,
    })
}

fn extract_archive(
    archive_path: &Path,
    destination: &Path,
    archive_kind: ArchiveKind,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    match archive_kind {
        ArchiveKind::TarGz => extract_tar_gz_archive(archive_path, destination, cancellation),
        ArchiveKind::Zip => extract_zip_archive(archive_path, destination, cancellation),
    }
}

fn extract_tar_gz_archive(
    archive_path: &Path,
    destination: &Path,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    let archive = fs::File::open(archive_path).map_err(|source| SearchToolError::PathIo {
        operation: "open tar archive",
        path: archive_path.to_path_buf(),
        source,
    })?;
    let decoder = GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive
        .entries()
        .map_err(|source| SearchToolError::PathIo {
            operation: "read tar archive",
            path: archive_path.to_path_buf(),
            source,
        })?;
    for entry in entries {
        ensure_not_cancelled(cancellation)?;
        let mut entry = entry.map_err(|source| SearchToolError::PathIo {
            operation: "read tar entry",
            path: archive_path.to_path_buf(),
            source,
        })?;
        let unpacked = entry
            .unpack_in(destination)
            .map_err(|source| SearchToolError::PathIo {
                operation: "extract tar archive",
                path: destination.to_path_buf(),
                source,
            })?;
        if !unpacked {
            return Err(SearchToolError::TarPathOutsideExtraction);
        }
    }
    Ok(())
}

fn extract_zip_archive(
    archive_path: &Path,
    destination: &Path,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    let archive = fs::File::open(archive_path).map_err(|source| SearchToolError::PathIo {
        operation: "open zip archive",
        path: archive_path.to_path_buf(),
        source,
    })?;
    let mut archive =
        zip::ZipArchive::new(archive).map_err(|source| SearchToolError::ReadZip { source })?;
    for index in 0..archive.len() {
        ensure_not_cancelled(cancellation)?;
        let mut entry = archive
            .by_index(index)
            .map_err(|source| SearchToolError::ReadZipEntry { source })?;
        let Some(relative_path) = entry.enclosed_name() else {
            return Err(SearchToolError::ZipPathOutsideExtraction);
        };
        let output_path = destination.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|source| SearchToolError::PathIo {
                operation: "create zip directory",
                path: output_path,
                source,
            })?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|source| SearchToolError::PathIo {
                operation: "create zip parent directory",
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let mut output =
            fs::File::create(&output_path).map_err(|source| SearchToolError::PathIo {
                operation: "create zip output file",
                path: output_path.clone(),
                source,
            })?;
        copy_reader_with_cancellation(&mut entry, &mut output, cancellation)?;
    }
    Ok(())
}

fn find_extracted_binary(
    root: &Path,
    file_name: &str,
    cancellation: &CancellationToken,
) -> Result<PathBuf, SearchToolError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        ensure_not_cancelled(cancellation)?;
        for entry in fs::read_dir(&directory).map_err(|source| SearchToolError::PathIo {
            operation: "read extracted archive directory",
            path: directory.clone(),
            source,
        })? {
            ensure_not_cancelled(cancellation)?;
            let entry = entry.map_err(|source| SearchToolError::PathIo {
                operation: "read extracted archive entry",
                path: directory.clone(),
                source,
            })?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|source| SearchToolError::PathIo {
                    operation: "read extracted archive file type",
                    path: path.clone(),
                    source,
                })?;
            if file_type.is_dir() {
                stack.push(path);
            } else if entry.file_name().to_string_lossy() == file_name {
                return Ok(path);
            }
        }
    }
    Err(SearchToolError::MissingExtractedBinary {
        file_name: file_name.to_string(),
    })
}

fn update_stable_entry(
    source: &Path,
    stable_entry: &Path,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    ensure_not_cancelled(cancellation)?;
    if let Some(parent) = stable_entry.parent() {
        fs::create_dir_all(parent).map_err(|source| SearchToolError::PathIo {
            operation: "create managed bin directory",
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let temp_entry = stable_entry.with_extension(format!("tmp-{}", unix_millis()));
    let result = (|| {
        copy_file_with_cancellation(source, &temp_entry, cancellation)?;
        make_executable(&temp_entry)?;
        ensure_not_cancelled(cancellation)?;
        fs::rename(&temp_entry, stable_entry).map_err(|source| SearchToolError::PathIo {
            operation: "publish managed entry",
            path: stable_entry.to_path_buf(),
            source,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_entry);
    }
    result
}

fn copy_file_with_cancellation(
    source: &Path,
    destination: &Path,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    let mut source_file =
        fs::File::open(source).map_err(|source_error| SearchToolError::PathIo {
            operation: "open source file",
            path: source.to_path_buf(),
            source: source_error,
        })?;
    let mut destination =
        fs::File::create(destination).map_err(|source| SearchToolError::PathIo {
            operation: "create destination file",
            path: destination.to_path_buf(),
            source,
        })?;
    copy_reader_with_cancellation(&mut source_file, &mut destination, cancellation)
}

fn copy_reader_with_cancellation(
    reader: &mut impl Read,
    writer: &mut impl Write,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    let mut buffer = [0; 64 * 1024];
    loop {
        ensure_not_cancelled(cancellation)?;
        let read = reader
            .read(&mut buffer)
            .map_err(|source| SearchToolError::Io {
                operation: "read file",
                source,
            })?;
        if read == 0 {
            return Ok(());
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|source| SearchToolError::Io {
                operation: "write file",
                source,
            })?;
    }
}

async fn cleanup_install_paths(paths: &RipgrepInstallPaths) {
    let _ = tokio::fs::remove_file(&paths.archive_path).await;
    let _ = tokio::fs::remove_dir_all(&paths.extract_dir).await;
    let _ = tokio::fs::remove_dir_all(&paths.version_temp_dir).await;
}

fn ensure_not_cancelled(cancellation: &CancellationToken) -> Result<(), SearchToolError> {
    if cancellation.is_cancelled() {
        Err(SearchToolError::Interrupted)
    } else {
        Ok(())
    }
}

/// 路径布局：`{managed_root}/bin/<exe>`、`{managed_root}/tools/rg/<version>/`。
///
/// `managed_root` 由调用方注入（= data/config dir），**不**再硬编码 `~/.hunea`。
/// 旧路径无兼容、无迁移——干净实现优先。
fn managed_ripgrep_entry_path(managed_root: &Path) -> PathBuf {
    managed_root
        .join("bin")
        .join(ripgrep_executable_file_name())
}

fn managed_ripgrep_version_binary_path(managed_root: &Path) -> PathBuf {
    managed_ripgrep_version_dir(managed_root).join(ripgrep_executable_file_name())
}

fn managed_ripgrep_version_dir(managed_root: &Path) -> PathBuf {
    managed_ripgrep_root(managed_root).join(MANAGED_RIPGREP_VERSION)
}

fn managed_ripgrep_root(managed_root: &Path) -> PathBuf {
    managed_root.join("tools").join(RIPGREP_BINARY_NAME)
}

fn is_executable_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// 下载进度是否该发：首次、≥64KiB 增量、或距上次 ≥100ms。最终字节由调用方强制补发。
fn should_emit_download_progress(
    bytes_received: u64,
    last_emitted_bytes: u64,
    elapsed_since_last: std::time::Duration,
) -> bool {
    last_emitted_bytes == 0
        || bytes_received.saturating_sub(last_emitted_bytes) >= PROGRESS_EMIT_BYTES
        || elapsed_since_last >= PROGRESS_EMIT_INTERVAL
}

async fn verify_binary_on_blocking_pool(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<bool, SearchToolError> {
    let path = path.to_path_buf();
    let cancellation = cancellation.clone();
    task::spawn_blocking(move || verify_binary_via_spawn_cancellable(&path, &cancellation))
        .await
        .map_err(|source| SearchToolError::JoinTask {
            operation: "managed binary verification",
            source,
        })?
}

/// spawn `--version`：比文件权限检查更严，能发现坏架构/缺库。
///
/// std 无 `Child::wait_timeout`，用 try_wait + 短 sleep 做 deadline；超时 kill。
/// 不引入 wait_timeout/nix 依赖——启动期各工具约 5–20ms，足够。
fn verify_binary_via_spawn(path: &Path) -> bool {
    let cancellation = CancellationToken::new();
    verify_binary_via_spawn_cancellable(path, &cancellation).unwrap_or(false)
}

fn verify_binary_via_spawn_cancellable(
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<bool, SearchToolError> {
    ensure_not_cancelled(cancellation)?;
    let Ok(mut child) = std::process::Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return Ok(false);
    };
    let started = std::time::Instant::now();
    while started.elapsed() < SPAWN_VERIFY_TIMEOUT {
        if cancellation.is_cancelled() {
            terminate_child(&mut child);
            return Err(SearchToolError::Interrupted);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                ensure_not_cancelled(cancellation)?;
                return Ok(status.success());
            }
            Ok(None) => std::thread::sleep(SPAWN_VERIFY_POLL_INTERVAL),
            Err(_) => break,
        }
    }
    terminate_child(&mut child);
    ensure_not_cancelled(cancellation)?;
    Ok(false)
}

fn terminate_child(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn make_executable(path: &Path) -> Result<(), SearchToolError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|source| SearchToolError::PathIo {
                operation: "stat managed binary",
                path: path.to_path_buf(),
                source,
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).map_err(|source| SearchToolError::PathIo {
            operation: "set managed binary permissions",
            path: path.to_path_buf(),
            source,
        })?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn ripgrep_manifest_for_current_platform() -> Option<ManagedRipgrepManifest> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-aarch64-apple-darwin.tar.gz",
            sha256: "378e973289176ca0c6054054ee7f631a065874a352bf43f0fa60ef079b6ba715",
            archive_kind: ArchiveKind::TarGz,
        },
        ("macos", "x86_64") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-x86_64-apple-darwin.tar.gz",
            sha256: "64811cb24e77cac3057d6c40b63ac9becf9082eedd54ca411b475b755d334882",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "x86_64") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-x86_64-unknown-linux-musl.tar.gz",
            sha256: "1c9297be4a084eea7ecaedf93eb03d058d6faae29bbc57ecdaf5063921491599",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "aarch64") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-aarch64-unknown-linux-gnu.tar.gz",
            sha256: "2b661c6ef508e902f388e9098d9c4c5aca72c87b55922d94abdba830b4dc885e",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "x86") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-i686-unknown-linux-gnu.tar.gz",
            sha256: "0300c58864b1de49da08f714d56ce10328dcbf6de37a404486fe2696e95692f1",
            archive_kind: ArchiveKind::TarGz,
        },
        ("windows", "aarch64") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-aarch64-pc-windows-msvc.zip",
            sha256: "00d931fb5237c9696ca49308818edb76d8eb6fc132761cb2a1bd616b2df02f8e",
            archive_kind: ArchiveKind::Zip,
        },
        ("windows", "x86_64") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-x86_64-pc-windows-msvc.zip",
            sha256: "124510b94b6baa3380d051fdf4650eaa80a302c876d611e9dba0b2e18d87493a",
            archive_kind: ArchiveKind::Zip,
        },
        ("windows", "x86") => ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-i686-pc-windows-msvc.zip",
            sha256: "725be85a1e8f92878a548f40ee4f6df64bc93b809586462b3c6d884e1de1e83a",
            archive_kind: ArchiveKind::Zip,
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use tokio_util::sync::CancellationToken;

    use super::*;

    #[tokio::test]
    async fn download_archive_observes_pre_cancelled_token_without_writing_file() {
        let root = temp_root("managed-download-cancelled");
        let archive_path = root.join("archive.tar.gz");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let result = download_archive(
            "https://github.com/example/project/releases/download/v1/archive.tar.gz",
            &archive_path,
            &cancellation,
            None,
        )
        .await;

        assert!(matches!(result, Err(SearchToolError::Interrupted)));
        assert!(!archive_path.exists());
        cleanup(&root);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn managed_install_verification_does_not_block_runtime_and_observes_cancellation() {
        let root = temp_root("managed-verification-cancelled");
        let stable_entry = managed_ripgrep_entry_path(&root);
        fs::create_dir_all(
            stable_entry
                .parent()
                .expect("stable entry should have parent"),
        )
        .expect("create managed bin directory");
        write_executable(
            &root.join("bin"),
            ripgrep_executable_file_name(),
            "#!/bin/sh\nexec sleep 30\n",
        );
        let cancellation = CancellationToken::new();
        let cancellation_after_delay = cancellation.clone();
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let started = std::time::Instant::now();

        let (install_result, ()) = tokio::join!(
            install_managed_ripgrep_with_progress(&root, cancellation, &progress_tx),
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                cancellation_after_delay.cancel();
            }
        );

        assert!(matches!(
            install_result,
            Err(ManagedRipgrepInstallError::Interrupted)
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "verification cancellation took {:?}",
            started.elapsed()
        );
        cleanup(&root);
    }

    fn temp_root(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    // --- ManagedRipgrepConfig ---

    #[test]
    fn config_allows_only_explicit_true() {
        let config = ManagedRipgrepConfig {
            allow_managed_rg: Some(true),
        };
        assert!(config.allows());
    }

    #[test]
    fn config_rejects_only_explicit_false() {
        let config = ManagedRipgrepConfig {
            allow_managed_rg: Some(false),
        };
        assert!(config.rejects());
    }

    #[test]
    fn config_default_neither_allows_nor_rejects() {
        let config = ManagedRipgrepConfig::default();
        assert!(!config.allows());
        assert!(!config.rejects());
    }

    #[test]
    fn manifest_url_uses_the_ripgrep_release_tag() {
        let manifest = ManagedRipgrepManifest {
            asset_name: "ripgrep-15.1.0-test.tar.gz",
            sha256: "unused",
            archive_kind: ArchiveKind::TarGz,
        };

        assert_eq!(
            manifest.url(),
            "https://github.com/BurntSushi/ripgrep/releases/download/15.1.0/\
ripgrep-15.1.0-test.tar.gz"
        );
    }

    // --- managed_ripgrep_fallback_for ---

    #[test]
    fn managed_ripgrep_fallback_for_returns_none_when_primary_is_managed() {
        let config = ManagedRipgrepConfig::default();
        let plan = RipgrepCommandPlan::Ready(RipgrepCommand {
            path: PathBuf::from("/fake/rg"),
            backend: RipgrepBackend::Managed,
        });
        assert!(
            managed_ripgrep_fallback_for(&plan, &config, Path::new("/tmp/fake-managed-root"))
                .is_none()
        );
    }

    #[test]
    fn managed_ripgrep_fallback_for_returns_none_when_unavailable() {
        let config = ManagedRipgrepConfig::default();
        let result = managed_ripgrep_fallback_for(
            &RipgrepCommandPlan::Unavailable,
            &config,
            Path::new("/tmp/fake-managed-root"),
        );
        assert!(result.is_none());
    }

    #[test]
    fn managed_ripgrep_fallback_for_returns_none_when_rejected() {
        let config = ManagedRipgrepConfig {
            allow_managed_rg: Some(false),
        };
        let plan = RipgrepCommandPlan::Ready(RipgrepCommand {
            path: PathBuf::from("/fake/rg"),
            backend: RipgrepBackend::SystemPath,
        });
        assert!(
            managed_ripgrep_fallback_for(&plan, &config, Path::new("/tmp/fake-managed-root"))
                .is_none()
        );
    }

    // --- download progress throttle ---

    #[test]
    fn should_emit_download_progress_on_first_update() {
        assert!(should_emit_download_progress(
            1024,
            0,
            std::time::Duration::from_millis(0),
        ));
    }

    #[test]
    fn should_emit_download_progress_on_byte_threshold() {
        // 刚发过、增量不足、时间也不足 → 不发
        assert!(!should_emit_download_progress(
            PROGRESS_EMIT_BYTES + 1024,
            PROGRESS_EMIT_BYTES,
            std::time::Duration::from_millis(10),
        ));
        // 增量够 → 发
        assert!(should_emit_download_progress(
            PROGRESS_EMIT_BYTES * 2,
            PROGRESS_EMIT_BYTES,
            std::time::Duration::from_millis(10),
        ));
    }

    #[test]
    fn should_emit_download_progress_on_time_threshold() {
        assert!(should_emit_download_progress(
            PROGRESS_EMIT_BYTES + 1,
            PROGRESS_EMIT_BYTES,
            PROGRESS_EMIT_INTERVAL,
        ));
    }

    // --- verify_binary_via_spawn ---

    #[cfg(unix)]
    #[test]
    fn verify_binary_via_spawn_returns_false_for_missing_path() {
        assert!(!verify_binary_via_spawn(Path::new(
            "/nonexistent/hunea-binary"
        )));
    }

    #[cfg(unix)]
    #[test]
    fn verify_binary_via_spawn_returns_true_for_healthy_binary() {
        let root = temp_root("verify-spawn-healthy");
        let script = write_executable(&root, "fake-tool", "#!/bin/sh\nexit 0\n");
        assert!(verify_binary_via_spawn(&script));
        cleanup(&root);
    }

    #[cfg(unix)]
    #[test]
    fn verify_binary_via_spawn_returns_false_for_failing_binary() {
        let root = temp_root("verify-spawn-failing");
        let script = write_executable(&root, "fake-tool", "#!/bin/sh\nexit 1\n");
        assert!(!verify_binary_via_spawn(&script));
        cleanup(&root);
    }

    #[cfg(unix)]
    fn write_executable(root: &Path, name: &str, content: &str) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let path = root.join(name);
        let mut file = fs::File::create(&path).expect("create fake executable");
        file.write_all(content.as_bytes())
            .expect("write fake executable");
        file.sync_all().expect("sync fake executable");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .expect("chmod fake executable");
        path
    }
}
