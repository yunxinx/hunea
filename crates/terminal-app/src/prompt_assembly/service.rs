use std::{path::Path, sync::Arc};

use color_eyre::eyre::Result;
use runtime_domain::{
    prompt_assembly::{PromptAssemblyManagerSnapshot, PromptAssemblyMutation},
    session::TranscriptUserMessage,
};
use session_store::SessionStore;
use tool_runtime::ToolDefinition;

use super::{AttachedPromptMessageAssembly, load_prompt_assembly_manager_snapshot};

/// `PromptAssemblyWorkspace` 固定 prompt assembly 的项目目录与工具定义输入。
pub(crate) struct PromptAssemblyWorkspace<'a> {
    work_dir: &'a Path,
    tool_definitions: &'a [ToolDefinition],
}

impl<'a> PromptAssemblyWorkspace<'a> {
    /// `new` 创建一次 prompt assembly 读写操作的稳定上下文。
    pub(crate) fn new(work_dir: &'a Path, tool_definitions: &'a [ToolDefinition]) -> Self {
        Self {
            work_dir,
            tool_definitions,
        }
    }

    /// `load_manager` 读取当前全局与项目 prompt assembly 后解析管理快照。
    pub(crate) fn load_manager(
        &self,
        store: Arc<dyn SessionStore>,
    ) -> Result<PromptAssemblyManagerSnapshot> {
        load_prompt_assembly_manager_snapshot(store, self.work_dir, self.tool_definitions)
    }

    /// `apply_mutation` 应用 prompt assembly mutation 并返回最新管理快照。
    pub(crate) fn apply_mutation(
        &self,
        store: Arc<dyn SessionStore>,
        mutation: PromptAssemblyMutation,
    ) -> Result<PromptAssemblyManagerSnapshot> {
        super::apply_prompt_assembly_mutation(store, self.work_dir, mutation, self.tool_definitions)
    }

    /// `assemble_attached_prompt_message` 解析当前用户消息中的 `$skill` / `#prompt` 绑定。
    pub(crate) fn assemble_attached_prompt_message(
        &self,
        store: Option<Arc<dyn SessionStore>>,
        user_message: &TranscriptUserMessage,
    ) -> Result<AttachedPromptMessageAssembly> {
        super::assemble_attached_prompt_message(
            store,
            self.work_dir,
            user_message,
            self.tool_definitions,
        )
    }
}
