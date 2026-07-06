use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::{Result, WrapErr};
use runtime_domain::prompt_assembly::persistence::{
    PromptAssemblyScopeState, load_project_prompt_assembly_state,
    save_project_prompt_assembly_state,
};
use runtime_domain::prompt_assembly::{PromptAssemblyManagerSnapshot, PromptAssemblyMutation};
use session_store::SessionStore;
use tool_runtime::ToolDefinition;

use crate::session_store_bridge::run_session_store_future;

/// `PromptAssemblyCommitOutcome` 表示一次成功 commit 的结果。
#[derive(Debug, Clone)]
pub(crate) struct PromptAssemblyCommitOutcome {
    pub manager: PromptAssemblyManagerSnapshot,
}

/// `PromptAssemblyEditSession` 持有 `/prompt` overlay 编辑期间的 working state。
///
/// 进入 overlay 时 load 一份 global + project state 作为 working copy，所有 mutate
/// 只改内存；退出时 `commit` 比较 working 与 baseline，等价则不落盘，不等则 save。
/// 这让 overlay 内的操作即时反馈，且抵消操作（如禁用再启用）不会触发误通知。
pub(crate) struct PromptAssemblyEditSession {
    work_dir: PathBuf,
    tool_definitions: Vec<ToolDefinition>,
    global_state: PromptAssemblyScopeState,
    project_state: PromptAssemblyScopeState,
    baseline_global_state: PromptAssemblyScopeState,
    baseline_project_state: PromptAssemblyScopeState,
}

impl PromptAssemblyEditSession {
    /// `load` 同步读取 global + project state 构造 edit session。
    pub(crate) fn load(
        store: Arc<dyn SessionStore>,
        work_dir: PathBuf,
        tool_definitions: Vec<ToolDefinition>,
    ) -> Result<Self> {
        let global_state = run_session_store_future(
            move || async move { store.load_global_prompt_assembly_state().await },
            "begin prompt assembly edit",
        )
        .wrap_err("load global prompt assembly state")??;
        let project_state = load_project_prompt_assembly_state(&work_dir)
            .wrap_err("load project prompt assembly state")?;
        Ok(Self::new(
            work_dir,
            tool_definitions,
            global_state,
            project_state,
        ))
    }

    fn new(
        work_dir: PathBuf,
        tool_definitions: Vec<ToolDefinition>,
        global_state: PromptAssemblyScopeState,
        project_state: PromptAssemblyScopeState,
    ) -> Self {
        let baseline_global_state = global_state.clone();
        let baseline_project_state = project_state.clone();
        Self {
            work_dir,
            tool_definitions,
            global_state,
            project_state,
            baseline_global_state,
            baseline_project_state,
        }
    }

    /// `apply_mutation` 在内存中对 working state 应用一次 mutation，返回刷新后的 snapshot。
    pub(crate) fn apply_mutation(
        &mut self,
        mutation: PromptAssemblyMutation,
    ) -> Result<PromptAssemblyManagerSnapshot> {
        super::apply_mutation_to_scope_states(
            &self.work_dir,
            &mut self.global_state,
            &mut self.project_state,
            mutation,
            &self.tool_definitions,
        )?;
        Ok(self.snapshot())
    }

    /// `snapshot` 基于 working state 重新解析出当前 overlay 视图。
    pub(crate) fn snapshot(&self) -> PromptAssemblyManagerSnapshot {
        super::resolve_prompt_assembly_manager_snapshot(
            &self.work_dir,
            &self.global_state,
            &self.project_state,
            &self.tool_definitions,
        )
    }

    /// `commit` 在 working copy 上落盘：若 not dirty 则不落盘、不通知；若 dirty 则 save。
    ///
    /// 失败时保留 working copy（`&mut self` 不消费），调用方可重试或继续编辑。
    /// 每次 save 成功后同步 baseline，避免重试时重复 save 已成功的部分。
    pub(crate) fn commit(
        &mut self,
        store: Arc<dyn SessionStore>,
    ) -> Result<Option<PromptAssemblyCommitOutcome>> {
        let global_changed = self.global_state != self.baseline_global_state;
        let project_changed = self.project_state != self.baseline_project_state;
        if !global_changed && !project_changed {
            return Ok(None);
        }

        if global_changed {
            let save_store = Arc::clone(&store);
            let save_state = self.global_state.clone();
            run_session_store_future(
                move || async move {
                    save_store
                        .save_global_prompt_assembly_state(&save_state)
                        .await
                },
                "commit prompt assembly edit",
            )
            .wrap_err("save global prompt assembly state")??;
            self.baseline_global_state = self.global_state.clone();
        }
        if project_changed {
            save_project_prompt_assembly_state(&self.work_dir, &self.project_state)
                .wrap_err("save project prompt assembly state")?;
            self.baseline_project_state = self.project_state.clone();
        }

        let manager = super::resolve_prompt_assembly_manager_snapshot(
            &self.work_dir,
            &self.global_state,
            &self.project_state,
            &self.tool_definitions,
        );
        Ok(Some(PromptAssemblyCommitOutcome { manager }))
    }
}
