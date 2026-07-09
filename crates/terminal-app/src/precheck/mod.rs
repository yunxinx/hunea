//! 启动前预检阶段：在主 TUI 启动前检测配置可访问性，必要时引导用户进入便携模式。
//!
//! 参见 `.trellis/tasks/07-08-startup-precheck-config-fallback/design.md`。

mod accessibility;
mod config_probe;
mod managed_search;
mod screen;
mod step;
mod steps;

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use color_eyre::eyre::{Result, WrapErr, eyre};

use accessibility::{Accessibility, probe_global_config_dir_accessibility};
use config_probe::{PortableMarkerProbe, probe_portable_marker};
pub(crate) use managed_search::sync_managed_search_outcomes_to_config;
use managed_search::{
    ManagedSearchOutcome, install_managed_tool_silently, read_managed_search_config,
};
use runtime_domain::paths::{
    CONFIG_FILE_NAME, DataDirResolution, hunea_config_dir, resolve_data_dir,
};
use screen::PrecheckScreen;
use terminal_ui::MinimalTerminalSession;
use tool_runtime::builtin::{
    ManagedSearchToolConfig, ManagedToolKind, ManagedToolStatus, detect_managed_tool_status,
};

/// `PrecheckResult` 是预检阶段的输出，供主启动流程使用。
///
/// 预检只做**目录级**决策（全局能否读写、是否进便携模式），
/// 不加载/不解析 config.toml——那是后续 `load_with_resolution` 的事。
#[derive(Debug)]
pub struct PrecheckResult {
    /// 预检决定的数据目录（全局 or 工作区便携）
    pub data_dir_resolution: DataDirResolution,
    /// 当前工作目录。
    ///
    /// `None` 表示 cwd 解析失败：启动侧不叠加工作区 `.hunea/` 配置，
    /// 且 `load_with_resolution` 会把默认 style 落到 `Ms`。
    pub working_dir: Option<PathBuf>,
    /// 用户是否选择退出（如便携模式确认时选 Quit）
    pub should_exit: bool,
    /// step 决策结果：磁盘已 write-through；此字段只供进主 TUI 填内存 Config。
    pub(crate) managed_search_outcomes: Vec<ManagedSearchOutcome>,
}

/// `PrecheckContext` 汇总预检探测结果，供 step 编排决策。
pub(crate) struct PrecheckContext {
    pub working_dir: PathBuf,
    pub portable_marker: PortableMarkerProbe,
    pub global_accessibility: Accessibility,
    /// 轻量读的 `allow_managed_*`（完整 config 尚未加载）。
    pub managed_search_config: ManagedSearchToolConfig,
    /// 受管工具安装根（= `DataDirResolution::config_dir()`）。
    ///
    /// 故意不兼容旧版硬编码的 `~/.hunea`：统一落到 config/data 目录
    /// （全局 `~/.config/hunea/` 或便携 `<working_dir>/.hunea/`），无迁移。
    /// 便携模式切换后由 screen 同步到 widget。
    pub managed_root: PathBuf,
}

/// `run` 是预检阶段入口，在主 TUI 启动前执行。
///
/// TTY 环境下进入预检 TUI；非 TTY 环境走 `run_non_interactive` fallback。
/// 无交互 step 时不进入 alternate screen，避免正常启动闪屏。
///
/// cwd 不可用时：若全局配置目录可用则退回纯全局模式（无工作区叠加）；
/// 全局也不可用则 fatal（无法进入便携模式）。
pub fn run() -> Result<PrecheckResult> {
    if !io::stdout().is_terminal() {
        return run_non_interactive();
    }

    match std::env::current_dir() {
        Ok(working_dir) => run_with_working_dir(working_dir),
        Err(_) => run_without_working_dir(),
    }
}

fn run_with_working_dir(working_dir: PathBuf) -> Result<PrecheckResult> {
    let portable_marker = probe_portable_marker(&working_dir);

    if matches!(portable_marker, PortableMarkerProbe::WorkspaceInaccessible) {
        return Err(eyre!(
            "workspace directory is not accessible; cannot run precheck"
        ));
    }

    let global_accessibility = probe_global_config_dir_accessibility();
    let initial_resolution = resolve_data_dir(&working_dir, portable_marker.is_present())
        .ok_or_else(|| eyre!("cannot resolve hunea data directory (is HOME set?)"))?;

    let config_path = initial_resolution.config_dir().join(CONFIG_FILE_NAME);
    let managed_search_config = read_managed_search_config(&config_path);

    let ctx = PrecheckContext {
        working_dir: working_dir.clone(),
        portable_marker,
        global_accessibility,
        managed_search_config,
        managed_root: initial_resolution.config_dir().to_path_buf(),
    };

    let screen = PrecheckScreen::new(&ctx, initial_resolution);
    if !screen.needs_interaction() {
        return Ok(screen.into_result());
    }

    let mut session = MinimalTerminalSession::enter().wrap_err("enter precheck terminal")?;
    screen.run(&mut session)
}

/// cwd 不可用时的降级路径。
///
/// 没有 working dir 就无法探测便携标记、也无法写工作区 `.hunea/`，
/// 因此只在全局配置目录可用时以纯全局模式继续；两边都不可用则 fatal。
/// 这比“cwd 失败直接退出”更接近历史行为（旧实现会落到 Ms 默认继续跑）。
fn run_without_working_dir() -> Result<PrecheckResult> {
    match probe_global_config_dir_accessibility() {
        Accessibility::Available => {
            let global_dir = hunea_config_dir()
                .ok_or_else(|| eyre!("cannot resolve hunea data directory (is HOME set?)"))?;
            Ok(PrecheckResult {
                data_dir_resolution: DataDirResolution::Global(global_dir),
                working_dir: None,
                should_exit: false,
                managed_search_outcomes: Vec::new(),
            })
        }
        Accessibility::Unavailable { .. } => Err(eyre!(
            "working directory is unavailable and global config directory is inaccessible; \
             cannot start hunea"
        )),
    }
}

/// 非 TTY：便携模式按探测结果直接解析；已授权且缺失的搜索工具静默安装，失败走 fallback。
fn run_non_interactive() -> Result<PrecheckResult> {
    let mut result = match std::env::current_dir() {
        Ok(working_dir) => {
            let portable_marker = probe_portable_marker(&working_dir);
            let global_accessibility = probe_global_config_dir_accessibility();
            resolve_non_interactive(Some(&working_dir), &portable_marker, &global_accessibility)?
        }
        Err(_) => run_without_working_dir()?,
    };

    apply_silent_managed_install(&mut result);
    Ok(result)
}

/// 非 TTY：仅 `allows=true` 且缺失/损坏时静默安装；失败 warning，不写 outcome。
fn apply_silent_managed_install(result: &mut PrecheckResult) {
    let config_path = result
        .data_dir_resolution
        .config_dir()
        .join(CONFIG_FILE_NAME);
    let managed_search_config = read_managed_search_config(&config_path);
    let managed_root = result.data_dir_resolution.config_dir();
    for tool in [ManagedToolKind::Ripgrep, ManagedToolKind::Fd] {
        if !managed_search_config.allows(tool) {
            continue;
        }
        let status = detect_managed_tool_status(tool, &managed_search_config, managed_root);
        match status {
            ManagedToolStatus::NeedsDownload | ManagedToolStatus::NeedsRebuild => {
                if let Err(error) = install_managed_tool_silently(tool, managed_root) {
                    eprintln!(
                        "warning: failed to install {} silently: {error}; using Rust fallback",
                        tool.display_name()
                    );
                }
            }
            _ => {}
        }
    }
}

/// 纯逻辑：根据探测结果决定非交互模式下的 `PrecheckResult`（便于单测，无 I/O）。
///
/// 非 TTY 无法弹出便携模式确认 UI，因此：
/// - 已有 portable marker 或全局可用 → 直接解析继续
/// - 全局不可用且无 marker → fatal，提示用户到 TTY 环境设置便携模式
fn resolve_non_interactive(
    working_dir: Option<&Path>,
    portable_marker: &PortableMarkerProbe,
    global_accessibility: &Accessibility,
) -> Result<PrecheckResult> {
    match (working_dir, portable_marker, global_accessibility) {
        // cwd 缺失时与 `run_without_working_dir` 同语义。
        (None, _, Accessibility::Available) => {
            let global_dir = hunea_config_dir()
                .ok_or_else(|| eyre!("cannot resolve hunea data directory (is HOME set?)"))?;
            Ok(PrecheckResult {
                data_dir_resolution: DataDirResolution::Global(global_dir),
                working_dir: None,
                should_exit: false,
                managed_search_outcomes: Vec::new(),
            })
        }
        (None, _, Accessibility::Unavailable { .. }) => Err(eyre!(
            "working directory is unavailable and global config directory is inaccessible; \
             cannot start hunea"
        )),
        (Some(_), PortableMarkerProbe::WorkspaceInaccessible, _) => Err(eyre!(
            "workspace directory is not accessible; cannot run precheck"
        )),
        (Some(working_dir), PortableMarkerProbe::Present, _) => {
            let resolution = resolve_data_dir(working_dir, true)
                .ok_or_else(|| eyre!("cannot resolve hunea data directory"))?;
            Ok(PrecheckResult {
                data_dir_resolution: resolution,
                working_dir: Some(working_dir.to_path_buf()),
                should_exit: false,
                managed_search_outcomes: Vec::new(),
            })
        }
        (Some(working_dir), PortableMarkerProbe::Absent, Accessibility::Available) => {
            let resolution = resolve_data_dir(working_dir, false)
                .ok_or_else(|| eyre!("cannot resolve hunea data directory"))?;
            Ok(PrecheckResult {
                data_dir_resolution: resolution,
                working_dir: Some(working_dir.to_path_buf()),
                should_exit: false,
                managed_search_outcomes: Vec::new(),
            })
        }
        (Some(_), PortableMarkerProbe::Absent, Accessibility::Unavailable { .. }) => Err(eyre!(
            "global config directory is inaccessible and no portable marker found; \
             cannot enter portable mode in non-interactive environment. \
             Run hunea in a TTY environment to set up portable mode."
        )),
    }
}

#[cfg(test)]
mod tests;
