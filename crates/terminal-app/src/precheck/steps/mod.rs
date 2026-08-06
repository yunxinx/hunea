//! 具体 step widget 实现：渲染、键盘交互、状态查询。

pub(crate) mod config_accessibility;
mod layout;
pub(crate) mod portable_mode_confirm;
pub(crate) mod portable_mode_recovery;
pub(crate) mod ripgrep_precheck;
mod ripgrep_progress;

pub(crate) use config_accessibility::ConfigAccessibilityWidget;
pub(crate) use portable_mode_confirm::{ConfirmSelection, PortableModeConfirmWidget};
pub(crate) use portable_mode_recovery::{PortableModeRecoveryWidget, RecoverySelection};
pub(crate) use ripgrep_precheck::RipgrepPrecheckWidget;
