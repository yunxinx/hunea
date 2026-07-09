//! 搜索工具预检 outcome、类型桥接、授权读写与下载线程。

use std::path::Path;

use app_config::appconfig;
use runtime_domain::session::ManagedSearchTool;
use tool_runtime::builtin::{ManagedSearchToolConfig, ManagedToolKind};

/// 单个工具的预检决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedSearchOutcome {
    /// 下载成功 → `allow_managed_* = true`。
    Authorized(ManagedToolKind),
    /// 选 fallback → `allow_managed_* = false`，避免下次重问。
    Rejected(ManagedToolKind),
}

pub(crate) fn to_runtime_domain_tool(tool: ManagedToolKind) -> ManagedSearchTool {
    match tool {
        ManagedToolKind::Ripgrep => ManagedSearchTool::Ripgrep,
        ManagedToolKind::Fd => ManagedSearchTool::Fd,
    }
}

/// precheck 在完整 config 加载前轻量读授权字段。
pub(crate) fn read_managed_search_config(config_path: &Path) -> ManagedSearchToolConfig {
    let auth = appconfig::read_managed_search_authorization(config_path);
    ManagedSearchToolConfig {
        allow_managed_rg: auth.allow_managed_rg,
        allow_managed_fd: auth.allow_managed_fd,
    }
}

/// step 完成时 write-through：下载/拒绝已是 side effect，不能等整个 precheck 结束。
/// 写盘失败只 warning（二进制可能已可用）。
pub(crate) fn persist_managed_search_outcome(outcome: &ManagedSearchOutcome, config_path: &Path) {
    let (tool, authorized) = match outcome {
        ManagedSearchOutcome::Authorized(tool) => (*tool, true),
        ManagedSearchOutcome::Rejected(tool) => (*tool, false),
    };
    let domain_tool = to_runtime_domain_tool(tool);
    let result = if authorized {
        appconfig::persist_managed_search_tool_authorization_to_path(config_path, domain_tool)
    } else {
        appconfig::persist_managed_search_tool_rejection_to_path(config_path, domain_tool)
    };
    if let Err(error) = result {
        eprintln!(
            "warning: failed to persist {} managed search authorization: {error}",
            tool.display_name()
        );
    }
}

/// 把 outcomes 填进内存 Config。磁盘只由 step 的 write-through 负责，此处不再写盘。
pub(crate) fn sync_managed_search_outcomes_to_config(
    outcomes: &[ManagedSearchOutcome],
    config: &mut appconfig::Config,
) {
    for outcome in outcomes {
        let (tool, authorized) = match outcome {
            ManagedSearchOutcome::Authorized(tool) => (*tool, true),
            ManagedSearchOutcome::Rejected(tool) => (*tool, false),
        };
        match tool {
            ManagedToolKind::Ripgrep => config.runtime.allow_managed_rg = Some(authorized),
            ManagedToolKind::Fd => config.runtime.allow_managed_fd = Some(authorized),
        }
    }
}

/// 独立线程 + current_thread runtime 跑下载（workspace 无 multi-thread feature）。
pub(crate) fn spawn_managed_install(
    tool: ManagedToolKind,
    managed_root: std::path::PathBuf,
    cancellation: tokio_util::sync::CancellationToken,
) -> (
    std::thread::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<tool_runtime::builtin::ManagedToolProgress>,
) {
    use tokio::sync::mpsc::unbounded_channel;
    use tool_runtime::builtin::install_managed_tool_with_progress;

    let (tx, rx) = unbounded_channel();
    let cancel_token = cancellation.clone();
    let join = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(error) => {
                let _ = tx.send(tool_runtime::builtin::ManagedToolProgress::Failed {
                    error: format!("failed to create tokio runtime: {error}"),
                });
                return;
            }
        };
        runtime.block_on(async move {
            let _ =
                install_managed_tool_with_progress(tool, &managed_root, cancel_token, &tx).await;
        });
    });
    (join, rx)
}

/// 非 TTY 静默安装；失败返回错误描述，调用方走 fallback。
pub(crate) fn install_managed_tool_silently(
    tool: ManagedToolKind,
    managed_root: &Path,
) -> std::result::Result<std::path::PathBuf, String> {
    use tokio_util::sync::CancellationToken;
    use tool_runtime::builtin::ManagedToolProgress;

    let cancellation = CancellationToken::new();
    let (join, mut rx) = spawn_managed_install(tool, managed_root.to_path_buf(), cancellation);

    let outcome = loop {
        match rx.blocking_recv() {
            Some(ManagedToolProgress::Ready { path }) => break Ok(path),
            Some(ManagedToolProgress::Failed { error }) => break Err(error),
            Some(_) => continue,
            None => break Err("download thread exited without reporting".to_string()),
        }
    };
    let _ = join.join();
    outcome
}
