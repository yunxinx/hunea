use std::{path::Path, sync::Arc};

use color_eyre::eyre::Result;
use runtime_domain::{
    prompt_assembly::PromptAssemblyManagerSnapshot, session::TranscriptUserMessage,
};
use session_store::SessionStore;
use tool_runtime::ToolDefinition;

use super::{AttachedPromptMessageAssembly, load_prompt_assembly_manager_snapshot};

/// `PromptAssemblyWorkspace` 固定 prompt assembly 的项目目录、数据目录与工具定义输入。
///
/// `work_dir` 与 `config_dir` 语义不同，即使便携模式下路径碰巧相同也不能合并为一个字段：
/// - `work_dir`：项目根，用于发现项目级 AGENTS.md / skills / prompts
/// - `config_dir`：数据目录（全局 `~/.config/hunea/` 或便携 `.hunea/`），用于全局 AGENTS.md
pub(crate) struct PromptAssemblyWorkspace<'a> {
    work_dir: &'a Path,
    /// 全局 AGENTS.md 等用户级文件所在数据目录（由 `DataDirResolution::config_dir` 注入）
    config_dir: &'a Path,
    tool_definitions: &'a [ToolDefinition],
}

impl<'a> PromptAssemblyWorkspace<'a> {
    /// `new` 创建一次 prompt assembly 读写操作的稳定上下文。
    ///
    /// 调用方必须显式传入 `config_dir`，不要用 `work_dir` 冒充——全局模式下二者不是同一路径。
    pub(crate) fn new(
        work_dir: &'a Path,
        config_dir: &'a Path,
        tool_definitions: &'a [ToolDefinition],
    ) -> Self {
        Self {
            work_dir,
            config_dir,
            tool_definitions,
        }
    }

    /// `load_manager` 读取当前全局与项目 prompt assembly 后解析管理快照。
    pub(crate) fn load_manager(
        &self,
        store: Arc<dyn SessionStore>,
    ) -> Result<PromptAssemblyManagerSnapshot> {
        load_prompt_assembly_manager_snapshot(
            store,
            self.work_dir,
            self.config_dir,
            self.tool_definitions,
        )
    }

    /// `assemble_attached_prompt_message` 解析当前用户消息中的 `$skill` / `#prompt` 绑定。
    pub(crate) fn assemble_attached_prompt_message(
        &self,
        manager: Option<&PromptAssemblyManagerSnapshot>,
        user_message: &TranscriptUserMessage,
    ) -> AttachedPromptMessageAssembly {
        super::assemble_attached_prompt_message(manager, self.work_dir, user_message)
    }
}
