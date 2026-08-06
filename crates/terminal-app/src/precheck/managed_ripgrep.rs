//! Managed ripgrep 预检 outcome、授权读写与下载线程。

use std::path::Path;

use app_config::appconfig;
use tool_runtime::builtin::{MANAGED_RIPGREP_NAME, ManagedRipgrepConfig};

/// managed ripgrep 的预检决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedRipgrepOutcome {
    /// 下载成功 → `allow_managed_rg = true`。
    Authorized,
    /// 选 fallback → `allow_managed_rg = false`，避免下次重问。
    Rejected,
}

impl ManagedRipgrepOutcome {
    fn allows_managed_ripgrep(&self) -> bool {
        matches!(self, Self::Authorized)
    }
}

/// precheck 在完整 config 加载前轻量读授权字段。
pub(crate) fn read_managed_ripgrep_config(config_path: &Path) -> ManagedRipgrepConfig {
    let auth = appconfig::read_managed_ripgrep_authorization(config_path);
    ManagedRipgrepConfig {
        allow_managed_rg: auth.allow_managed_rg,
    }
}

/// step 完成时 write-through：下载/拒绝已是 side effect，不能等整个 precheck 结束。
/// 写盘失败只 warning（二进制可能已可用）。
pub(crate) fn persist_managed_ripgrep_outcome(outcome: &ManagedRipgrepOutcome, config_path: &Path) {
    let result = if outcome.allows_managed_ripgrep() {
        appconfig::persist_managed_ripgrep_authorization_to_path(config_path)
    } else {
        appconfig::persist_managed_ripgrep_rejection_to_path(config_path)
    };
    if let Err(error) = result {
        eprintln!(
            "warning: failed to persist managed {MANAGED_RIPGREP_NAME} authorization: {error}"
        );
    }
}

/// 把 outcome 填进内存 Config。磁盘只由 step 的 write-through 负责，此处不再写盘。
pub(crate) fn sync_managed_ripgrep_outcome_to_config(
    outcome: Option<&ManagedRipgrepOutcome>,
    config: &mut appconfig::Config,
) {
    if let Some(outcome) = outcome {
        config.runtime.allow_managed_rg = Some(outcome.allows_managed_ripgrep());
    }
}

/// 独立线程 + current_thread runtime 跑下载（workspace 无 multi-thread feature）。
pub(crate) fn spawn_managed_ripgrep_install(
    managed_root: std::path::PathBuf,
    cancellation: tokio_util::sync::CancellationToken,
) -> (
    std::thread::JoinHandle<()>,
    tokio::sync::mpsc::UnboundedReceiver<tool_runtime::builtin::ManagedRipgrepProgress>,
) {
    use tokio::sync::mpsc::unbounded_channel;
    use tool_runtime::builtin::install_managed_ripgrep_with_progress;

    let (tx, rx) = unbounded_channel();
    let cancel_token = cancellation.clone();
    let join = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(error) => {
                let _ = tx.send(tool_runtime::builtin::ManagedRipgrepProgress::Failed {
                    error: format!("failed to create tokio runtime: {error}"),
                });
                return;
            }
        };
        runtime.block_on(async move {
            let _ = install_managed_ripgrep_with_progress(&managed_root, cancel_token, &tx).await;
        });
    });
    (join, rx)
}

/// 非 TTY 静默安装；失败返回错误描述，调用方走 fallback。
pub(crate) fn install_managed_ripgrep_silently(
    managed_root: &Path,
) -> std::result::Result<std::path::PathBuf, String> {
    use tokio_util::sync::CancellationToken;
    use tool_runtime::builtin::ManagedRipgrepProgress;

    let cancellation = CancellationToken::new();
    let (join, mut rx) = spawn_managed_ripgrep_install(managed_root.to_path_buf(), cancellation);

    let outcome = loop {
        match rx.blocking_recv() {
            Some(ManagedRipgrepProgress::Ready { path }) => break Ok(path),
            Some(ManagedRipgrepProgress::Failed { error }) => break Err(error),
            Some(_) => continue,
            None => break Err("download thread exited without reporting".to_string()),
        }
    };
    let _ = join.join();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> appconfig::Config {
        appconfig::load_from_paths(None, None).expect("load default config")
    }

    #[test]
    fn sync_managed_ripgrep_outcome_maps_authorization_decision() {
        let mut config = default_config();

        sync_managed_ripgrep_outcome_to_config(
            Some(&ManagedRipgrepOutcome::Authorized),
            &mut config,
        );
        assert_eq!(config.runtime.allow_managed_rg, Some(true));

        sync_managed_ripgrep_outcome_to_config(Some(&ManagedRipgrepOutcome::Rejected), &mut config);
        assert_eq!(config.runtime.allow_managed_rg, Some(false));
    }

    #[test]
    fn sync_managed_ripgrep_outcome_preserves_loaded_config_without_decision() {
        let mut config = default_config();
        config.runtime.allow_managed_rg = Some(true);

        sync_managed_ripgrep_outcome_to_config(None, &mut config);

        assert_eq!(config.runtime.allow_managed_rg, Some(true));
    }
}
