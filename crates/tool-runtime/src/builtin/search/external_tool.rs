use std::{
    env,
    fs::{self},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use flate2::read::GzDecoder;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, task};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    ToolCall, ToolDefinition, ToolExecutionContext, ToolKind, ToolPermissionDecision,
    ToolPermissionPolicy, ToolPermissionRequest, ToolProgress,
};

use super::error::SearchToolError;

const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const DOWNLOAD_REQUEST_SUFFIX: &str = "-managed-download";
const REBUILD_REQUEST_SUFFIX: &str = "-managed-rebuild";

/// `ManagedSearchToolConfig` 保存 `rg` / `fd` 受管安装的授权配置面。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedSearchToolConfig {
    pub allow_managed_rg: Option<bool>,
    pub allow_managed_fd: Option<bool>,
}

impl ManagedSearchToolConfig {
    pub(crate) fn allows(&self, tool: ManagedToolKind) -> bool {
        match tool {
            ManagedToolKind::Ripgrep => self.allow_managed_rg == Some(true),
            ManagedToolKind::Fd => self.allow_managed_fd == Some(true),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedToolKind {
    Ripgrep,
    Fd,
}

impl ManagedToolKind {
    pub(crate) const fn binary_name(self) -> &'static str {
        match self {
            Self::Ripgrep => "rg",
            Self::Fd => "fd",
        }
    }

    pub(crate) const fn system_binary_names(self) -> &'static [&'static str] {
        match self {
            Self::Ripgrep => &["rg"],
            Self::Fd => &["fd", "fdfind"],
        }
    }

    const fn version(self) -> &'static str {
        match self {
            Self::Ripgrep => "15.1.0",
            Self::Fd => "10.3.0",
        }
    }

    const fn display_name(self) -> &'static str {
        match self {
            Self::Ripgrep => "rg",
            Self::Fd => "fd",
        }
    }

    const fn repository(self) -> &'static str {
        match self {
            Self::Ripgrep => "BurntSushi/ripgrep",
            Self::Fd => "sharkdp/fd",
        }
    }

    const fn authorization_field(self) -> &'static str {
        match self {
            Self::Ripgrep => "allow_managed_rg",
            Self::Fd => "allow_managed_fd",
        }
    }

    const fn executable_file_name(self) -> &'static str {
        #[cfg(windows)]
        {
            match self {
                Self::Ripgrep => "rg.exe",
                Self::Fd => "fd.exe",
            }
        }
        #[cfg(not(windows))]
        {
            self.binary_name()
        }
    }

    fn manifest(self) -> Option<ManagedToolManifest> {
        manifest_for_current_platform(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExternalToolBackend {
    SystemPath,
    Bundled,
    Managed,
}

impl ExternalToolBackend {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SystemPath => "system_path",
            Self::Bundled => "bundled",
            Self::Managed => "managed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalCommand {
    pub(crate) path: PathBuf,
    pub(crate) backend: ExternalToolBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExternalCommandPlan {
    Ready(ExternalCommand),
    AskManagedRebuild,
    InstallManaged,
    AskManagedDownload,
}

#[derive(Debug, Clone, Copy)]
struct ManagedToolManifest {
    asset_name: &'static str,
    sha256: &'static str,
    archive_kind: ArchiveKind,
}

impl ManagedToolManifest {
    fn url(self, tool: ManagedToolKind) -> String {
        let tag = match tool {
            ManagedToolKind::Ripgrep => tool.version().to_string(),
            ManagedToolKind::Fd => format!("v{}", tool.version()),
        };
        format!(
            "https://github.com/{}/releases/download/{tag}/{}",
            tool.repository(),
            self.asset_name
        )
    }
}

#[derive(Debug, Clone, Copy)]
enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Debug, Clone)]
struct ManagedInstallPaths {
    archive_path: PathBuf,
    extract_dir: PathBuf,
    version_temp_dir: PathBuf,
    final_version_dir: PathBuf,
    version_binary: PathBuf,
    stable_entry: PathBuf,
}

pub(crate) fn resolve_external_command_plan(
    tool: ManagedToolKind,
    config: &ManagedSearchToolConfig,
) -> ExternalCommandPlan {
    if let Some(path) = find_system_binary(tool) {
        return ExternalCommandPlan::Ready(ExternalCommand {
            path,
            backend: ExternalToolBackend::SystemPath,
        });
    }
    if let Some(path) = find_bundled_binary(tool) {
        return ExternalCommandPlan::Ready(ExternalCommand {
            path,
            backend: ExternalToolBackend::Bundled,
        });
    }
    if config.allows(tool) && managed_cache_needs_rebuild(tool) {
        return ExternalCommandPlan::AskManagedRebuild;
    }
    if let Some(path) = usable_managed_entry(tool) {
        return ExternalCommandPlan::Ready(ExternalCommand {
            path,
            backend: ExternalToolBackend::Managed,
        });
    }
    if config.allows(tool) {
        ExternalCommandPlan::InstallManaged
    } else {
        ExternalCommandPlan::AskManagedDownload
    }
}

pub(crate) async fn resolve_external_command_from_plan(
    tool: ManagedToolKind,
    plan: ExternalCommandPlan,
    context: &ToolExecutionContext<'_>,
) -> Option<ExternalCommand> {
    match plan {
        ExternalCommandPlan::Ready(command) => Some(command),
        ExternalCommandPlan::AskManagedRebuild => {
            if !confirm_managed_rebuild(tool, context).await {
                return None;
            }
            install_managed_command(tool, context).await
        }
        ExternalCommandPlan::InstallManaged => install_managed_command(tool, context).await,
        ExternalCommandPlan::AskManagedDownload => {
            if !confirm_managed_download(tool, context).await {
                return None;
            }
            install_managed_command(tool, context).await
        }
    }
}

async fn install_managed_command(
    tool: ManagedToolKind,
    context: &ToolExecutionContext<'_>,
) -> Option<ExternalCommand> {
    match install_managed_tool(tool, context).await {
        Ok(path) => {
            context.emit(ToolProgress::ManagedSearchToolAuthorization {
                tool_name: tool.binary_name().to_string(),
            });
            Some(ExternalCommand {
                path,
                backend: ExternalToolBackend::Managed,
            })
        }
        Err(error) => {
            context.emit(ToolProgress::SystemMessage {
                message: format!(
                    "{} managed install failed; using Rust fallback. {error}",
                    tool.display_name()
                ),
            });
            None
        }
    }
}

fn find_system_binary(tool: ManagedToolKind) -> Option<PathBuf> {
    tool.system_binary_names()
        .iter()
        .find_map(|name| find_on_path(name))
}

fn find_on_path(binary_name: &str) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|directory| directory.join(binary_name))
        .find(|candidate| is_executable_file(candidate))
}

fn find_bundled_binary(tool: ManagedToolKind) -> Option<PathBuf> {
    let executable_dir = env::current_exe().ok()?.parent()?.to_path_buf();
    [
        executable_dir
            .join("tools")
            .join(tool.binary_name())
            .join(tool.version())
            .join(tool.executable_file_name()),
        executable_dir
            .join("tools")
            .join(tool.executable_file_name()),
        executable_dir.join(tool.executable_file_name()),
    ]
    .into_iter()
    .find(|candidate| is_executable_file(candidate))
}

fn usable_managed_entry(tool: ManagedToolKind) -> Option<PathBuf> {
    let entry = managed_entry_path(tool)?;
    is_executable_file(&entry).then_some(entry)
}

fn managed_cache_needs_rebuild(tool: ManagedToolKind) -> bool {
    let Some(entry) = managed_entry_path(tool) else {
        return false;
    };
    let Some(version_binary) = managed_version_binary_path(tool) else {
        return false;
    };
    !is_executable_file(&entry) || !is_executable_file(&version_binary)
}

async fn confirm_managed_rebuild(
    tool: ManagedToolKind,
    context: &ToolExecutionContext<'_>,
) -> bool {
    let Some(install_root) = managed_tool_root(tool) else {
        return false;
    };
    let request = ToolPermissionRequest::new(
        ToolCall::new(
            format!("{}{}", tool.binary_name(), REBUILD_REQUEST_SUFFIX),
            format!("managed_{}_rebuild", tool.binary_name()),
            json!({
                "tool": tool.display_name(),
                "source": format!("https://github.com/{}", tool.repository()),
                "install_dir": install_root.display().to_string(),
                "description": format!(
                    "The previously authorized managed {} binary is missing or damaged. Rebuild it now, or reject to use Rust fallback.",
                    tool.display_name()
                )
            }),
        ),
        permission_definition(
            format!("managed_{}_rebuild", tool.binary_name()),
            format!("Rebuild {}", tool.display_name()),
        ),
    );
    matches!(
        context.request_permission(request).await,
        ToolPermissionDecision::Allow
    )
}

async fn confirm_managed_download(
    tool: ManagedToolKind,
    context: &ToolExecutionContext<'_>,
) -> bool {
    let Some(install_root) = managed_tool_root(tool) else {
        return false;
    };
    let request = ToolPermissionRequest::new(
        ToolCall::new(
            format!("{}{}", tool.binary_name(), DOWNLOAD_REQUEST_SUFFIX),
            format!("managed_{}_download", tool.binary_name()),
            json!({
                "tool": tool.display_name(),
                "source": format!("https://github.com/{}", tool.repository()),
                "install_dir": install_root.display().to_string(),
                "config_field": tool.authorization_field(),
                "description": format!(
                    "Download {} from official GitHub Releases, verify SHA256, install under hunea managed tools, and remember this authorization.",
                    tool.display_name()
                )
            }),
        ),
        permission_definition(
            format!("managed_{}_download", tool.binary_name()),
            format!("Download {}", tool.display_name()),
        ),
    );
    matches!(
        context.request_permission(request).await,
        ToolPermissionDecision::Allow
    )
}

fn permission_definition(name: String, label: String) -> ToolDefinition {
    ToolDefinition::new(name)
        .with_label(label)
        .with_kind(ToolKind::Fetch)
        .with_description("Install a pinned external search tool for hunea managed use.")
        .with_input_schema(json!({ "type": "object" }))
        .with_permission_policy(ToolPermissionPolicy::Ask)
}

async fn install_managed_tool(
    tool: ManagedToolKind,
    context: &ToolExecutionContext<'_>,
) -> Result<PathBuf, SearchToolError> {
    ensure_not_cancelled(context.cancellation())?;
    let manifest = tool.manifest().ok_or(SearchToolError::NoManagedAsset {
        tool: tool.display_name(),
    })?;
    let stable_entry =
        managed_entry_path(tool).ok_or(SearchToolError::ManagedDirectoryUnavailable {
            tool: tool.display_name(),
        })?;
    let version_binary =
        managed_version_binary_path(tool).ok_or(SearchToolError::ManagedDirectoryUnavailable {
            tool: tool.display_name(),
        })?;
    if is_executable_file(&stable_entry) {
        return Ok(stable_entry);
    }
    if is_executable_file(&version_binary) {
        let cancellation = context.cancellation().clone();
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

    let app_root = app_managed_root().ok_or(SearchToolError::ManagedDirectoryUnavailable {
        tool: tool.display_name(),
    })?;
    let temp_root = app_root.join("tmp");
    tokio::fs::create_dir_all(&temp_root)
        .await
        .map_err(|source| SearchToolError::PathIo {
            operation: "create managed tools temp directory",
            path: temp_root.clone(),
            source,
        })?;
    let unique = format!(
        "{}-{}-{}",
        tool.binary_name(),
        std::process::id(),
        unix_millis()
    );
    let paths = ManagedInstallPaths {
        archive_path: temp_root.join(format!("{unique}.archive")),
        extract_dir: temp_root.join(format!("{unique}.extract")),
        version_temp_dir: temp_root.join(format!("{unique}.version")),
        final_version_dir: managed_version_dir(tool).ok_or(
            SearchToolError::ManagedDirectoryUnavailable {
                tool: tool.display_name(),
            },
        )?,
        version_binary,
        stable_entry,
    };

    let install_result = async {
        let url = manifest.url(tool);
        emit_install_message(
            context,
            format!("Downloading {} from {url}", tool.display_name()),
        );
        download_archive(&url, &paths.archive_path, context.cancellation()).await?;
        emit_install_message(context, format!("Verifying {}", tool.display_name()));
        emit_install_message(context, format!("Installing {}", tool.display_name()));
        let blocking_paths = paths.clone();
        let cancellation = context.cancellation().clone();
        let stable_entry = task::spawn_blocking(move || {
            install_archive_blocking(
                blocking_paths,
                manifest.sha256,
                manifest.archive_kind,
                tool.executable_file_name(),
                cancellation,
            )
        })
        .await
        .map_err(|source| SearchToolError::JoinTask {
            operation: "managed install",
            source,
        })??;
        emit_install_message(context, format!("{} is ready", tool.display_name()));
        Ok(stable_entry)
    }
    .await;

    cleanup_install_paths(&paths).await;
    install_result
}

fn emit_install_message(context: &ToolExecutionContext<'_>, message: String) {
    context.emit(ToolProgress::SystemMessage { message });
}

async fn download_archive(
    url: &str,
    destination: &Path,
    cancellation: &CancellationToken,
) -> Result<(), SearchToolError> {
    ensure_not_cancelled(cancellation)?;
    let parsed =
        Url::parse(url).map_err(|source| SearchToolError::InvalidManagedToolUrl { source })?;
    if parsed.host_str() != Some("github.com") {
        return Err(SearchToolError::UnofficialManagedToolUrl {
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
    let mut response = response;
    let mut file = tokio::fs::File::create(destination)
        .await
        .map_err(|source| SearchToolError::PathIo {
            operation: "create download file",
            path: destination.to_path_buf(),
            source,
        })?;

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
    paths: ManagedInstallPaths,
    expected_sha256: &str,
    archive_kind: ArchiveKind,
    executable_file_name: &str,
    cancellation: CancellationToken,
) -> Result<PathBuf, SearchToolError> {
    ensure_not_cancelled(&cancellation)?;
    verify_sha256(&paths.archive_path, expected_sha256, &cancellation)?;
    ensure_not_cancelled(&cancellation)?;
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
    let actual = format!("{:x}", hasher.finalize());
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

async fn cleanup_install_paths(paths: &ManagedInstallPaths) {
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

fn managed_entry_path(tool: ManagedToolKind) -> Option<PathBuf> {
    Some(
        app_managed_root()?
            .join("bin")
            .join(tool.executable_file_name()),
    )
}

fn managed_version_binary_path(tool: ManagedToolKind) -> Option<PathBuf> {
    Some(managed_version_dir(tool)?.join(tool.executable_file_name()))
}

fn managed_version_dir(tool: ManagedToolKind) -> Option<PathBuf> {
    Some(managed_tool_root(tool)?.join(tool.version()))
}

fn managed_tool_root(tool: ManagedToolKind) -> Option<PathBuf> {
    Some(app_managed_root()?.join("tools").join(tool.binary_name()))
}

fn app_managed_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| BaseDirs::new().map(|dirs| dirs.data_dir().to_path_buf()))
            .map(|path| path.join("hunea"))
    }
    #[cfg(not(windows))]
    {
        BaseDirs::new().map(|dirs| dirs.home_dir().join(".hunea"))
    }
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

fn manifest_for_current_platform(tool: ManagedToolKind) -> Option<ManagedToolManifest> {
    match tool {
        ManagedToolKind::Ripgrep => ripgrep_manifest_for_current_platform(),
        ManagedToolKind::Fd => fd_manifest_for_current_platform(),
    }
}

fn ripgrep_manifest_for_current_platform() -> Option<ManagedToolManifest> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-aarch64-apple-darwin.tar.gz",
            sha256: "378e973289176ca0c6054054ee7f631a065874a352bf43f0fa60ef079b6ba715",
            archive_kind: ArchiveKind::TarGz,
        },
        ("macos", "x86_64") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-x86_64-apple-darwin.tar.gz",
            sha256: "64811cb24e77cac3057d6c40b63ac9becf9082eedd54ca411b475b755d334882",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "x86_64") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-x86_64-unknown-linux-musl.tar.gz",
            sha256: "1c9297be4a084eea7ecaedf93eb03d058d6faae29bbc57ecdaf5063921491599",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "aarch64") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-aarch64-unknown-linux-gnu.tar.gz",
            sha256: "2b661c6ef508e902f388e9098d9c4c5aca72c87b55922d94abdba830b4dc885e",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "x86") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-i686-unknown-linux-gnu.tar.gz",
            sha256: "0300c58864b1de49da08f714d56ce10328dcbf6de37a404486fe2696e95692f1",
            archive_kind: ArchiveKind::TarGz,
        },
        ("windows", "aarch64") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-aarch64-pc-windows-msvc.zip",
            sha256: "00d931fb5237c9696ca49308818edb76d8eb6fc132761cb2a1bd616b2df02f8e",
            archive_kind: ArchiveKind::Zip,
        },
        ("windows", "x86_64") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-x86_64-pc-windows-msvc.zip",
            sha256: "124510b94b6baa3380d051fdf4650eaa80a302c876d611e9dba0b2e18d87493a",
            archive_kind: ArchiveKind::Zip,
        },
        ("windows", "x86") => ManagedToolManifest {
            asset_name: "ripgrep-15.1.0-i686-pc-windows-msvc.zip",
            sha256: "725be85a1e8f92878a548f40ee4f6df64bc93b809586462b3c6d884e1de1e83a",
            archive_kind: ArchiveKind::Zip,
        },
        _ => return None,
    })
}

fn fd_manifest_for_current_platform() -> Option<ManagedToolManifest> {
    Some(match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-aarch64-apple-darwin.tar.gz",
            sha256: "0570263812089120bc2a5d84f9e65cd0c25e4a4d724c80075c357239c74ae904",
            archive_kind: ArchiveKind::TarGz,
        },
        ("macos", "x86_64") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-x86_64-apple-darwin.tar.gz",
            sha256: "50d30f13fe3d5914b14c4fff5abcbd4d0cdab4b855970a6956f4f006c17117a3",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "aarch64") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-aarch64-unknown-linux-musl.tar.gz",
            sha256: "996b9b1366433b211cb3bbedba91c9dbce2431842144d925428ead0adf32020b",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "x86_64") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-x86_64-unknown-linux-musl.tar.gz",
            sha256: "2b6bfaae8c48f12050813c2ffe1884c61ea26e750d803df9c9114550a314cd14",
            archive_kind: ArchiveKind::TarGz,
        },
        ("linux", "x86") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-i686-unknown-linux-musl.tar.gz",
            sha256: "e761dfc5baff0fb91cd1428f1475fae0e9d70dfbf55c10e9db803566abf70fad",
            archive_kind: ArchiveKind::TarGz,
        },
        ("windows", "aarch64") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-aarch64-pc-windows-msvc.zip",
            sha256: "bf9b1e31bcac71c1e95d49c56f0d872f525b95d03854e94b1d4dd6786f825cc5",
            archive_kind: ArchiveKind::Zip,
        },
        ("windows", "x86_64") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-x86_64-pc-windows-msvc.zip",
            sha256: "318aa2a6fa664325933e81fda60d523fff29444129e91ebf0726b5b3bcd8b059",
            archive_kind: ArchiveKind::Zip,
        },
        ("windows", "x86") => ManagedToolManifest {
            asset_name: "fd-v10.3.0-i686-pc-windows-msvc.zip",
            sha256: "1e1c1c677d01c1df9e54095d727f61649401ac54a5946cecb3fbe3d002615fd8",
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
        )
        .await;

        assert!(matches!(result, Err(SearchToolError::Interrupted)));
        assert!(!archive_path.exists());
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
}
