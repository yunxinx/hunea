use runtime_domain::prompt_assembly::PromptAssemblyMutation;
use runtime_domain::session::{PromptAssemblyUpdateNotice, RuntimeEvent};

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::PromptAssemblyEditSession;

/// `PromptSessionConfigRefreshTarget` 标识 commit 后的新 prelude 应作用于当前空会话还是下一次新会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSessionConfigRefreshTarget {
    CurrentEmptySession,
    NextNewSession,
}

impl AppRuntimeCoordinator {
    fn prompt_session_config_refresh_target(&self) -> PromptSessionConfigRefreshTarget {
        if self.components.agent_runtime.is_idle_empty_session() {
            PromptSessionConfigRefreshTarget::CurrentEmptySession
        } else {
            PromptSessionConfigRefreshTarget::NextNewSession
        }
    }

    /// `prompt_assembly_update_notice` 在 commit 后判断是否需要通知用户。
    ///
    /// 仅当 prelude / dynamic env / 工具启停实际变化时返回 `Some`；
    /// 命中当前空会话时同步更新 provider 配置与 session 工具集。
    fn prompt_assembly_update_notice(
        &mut self,
        session_prompt_config_changed: bool,
        manager: &runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot,
    ) -> Option<PromptAssemblyUpdateNotice> {
        if !session_prompt_config_changed {
            return None;
        }
        match self.prompt_session_config_refresh_target() {
            PromptSessionConfigRefreshTarget::CurrentEmptySession => {
                let session_workspace_tools =
                    super::session_tools_for_manager(&self.components.tool_catalog, Some(manager));
                let prompt_assembly = self.components.prompt_assembly.session_snapshot();
                self.components
                    .agent_runtime
                    .update_empty_session_configuration(
                        prompt_assembly,
                        session_workspace_tools.clone(),
                    );
                self.components.session_workspace_tools = session_workspace_tools;
                Some(PromptAssemblyUpdateNotice::CurrentEmptySessionUpdated)
            }
            PromptSessionConfigRefreshTarget::NextNewSession => {
                Some(PromptAssemblyUpdateNotice::NextNewSessionUpdated)
            }
        }
    }

    /// `begin_prompt_assembly_edit_impl` 进入 `/prompt` overlay：load 一份 working copy，返回初始 snapshot。
    ///
    /// 若 coordinator 已持有未提交的 edit session（上次 commit 失败保留），复用该 session
    /// 而非从磁盘重新 load——避免覆盖未落盘的编辑。
    pub(super) fn begin_prompt_assembly_edit_impl(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        if let Some(session) = self.prompt_assembly_edit_session.as_ref() {
            return Ok(session.snapshot());
        }
        let views = self.session_views()?;
        let header = self.session_header()?;
        let session = PromptAssemblyEditSession::load(
            views.prompt_assembly,
            header.work_dir,
            self.options.hunea_config_dir.clone(),
            self.prompt_assembly_tool_definitions(),
        )
        .map_err(|error| error.to_string())?;
        let snapshot = session.snapshot();
        self.prompt_assembly_edit_session = Some(session);
        Ok(snapshot)
    }

    /// `apply_prompt_assembly_edit_mutation_impl` 在 working copy 上同步应用 mutation。
    pub(super) fn apply_prompt_assembly_edit_mutation_impl(
        &mut self,
        mutation: PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        let session = self
            .prompt_assembly_edit_session
            .as_mut()
            .ok_or_else(|| "prompt assembly edit session is not active".to_string())?;
        session
            .apply_mutation(mutation)
            .map_err(|error| error.to_string())
    }

    /// `commit_prompt_assembly_edit_impl` 退出 `/prompt` overlay：commit working copy。
    ///
    /// 若 not dirty 则不落盘、不通知；若 dirty 则 save + push `RuntimeEvent::PromptAssemblyUpdated`。
    /// 成功路径（无论是否 dirty）都释放 edit session；失败时保留 session 供重试或继续编辑。
    pub(super) fn commit_prompt_assembly_edit_impl(&mut self) -> Result<(), String> {
        if let Some(manager) = self
            .prompt_assembly_edit_session
            .as_ref()
            .map(PromptAssemblyEditSession::snapshot)
        {
            self.components
                .prompt_assembly
                .validate_manager_replacement(Some(&manager))
                .map_err(|error| error.to_string())?;
        }
        let outcome = {
            let views = self.session_views()?;
            let Some(session) = self.prompt_assembly_edit_session.as_mut() else {
                return Ok(());
            };
            session.commit(views.prompt_assembly)
        }
        .map_err(|error| error.to_string())?;
        let manager = match outcome {
            Some(outcome) => outcome.manager,
            None => {
                // not-dirty commit 已经完整成功，不需要更新 capability 或发送事件。
                self.prompt_assembly_edit_session = None;
                return Ok(());
            }
        };

        let previous_manager = self.components.prompt_assembly.manager_snapshot();
        let dynamic_environment_session_config =
            crate::prompt_assembly::dynamic_environment_session_config_from_manager(&manager);
        let prelude_changed = previous_manager
            .as_ref()
            .map(|manager| &manager.resolution.prelude)
            != Some(&manager.resolution.prelude);
        let dynamic_environment_config_changed = previous_manager
            .as_ref()
            .map(crate::prompt_assembly::dynamic_environment_session_config_from_manager)
            .as_ref()
            != Some(&dynamic_environment_session_config);
        // 工具启停可能不影响 prelude（如禁用无 guidelines 的工具），因此需要相对
        // capability 当前持有的 live manager 单独参与变化检测。
        let tool_enablement_changed = super::manager_disabled_tool_names(previous_manager.as_ref())
            != super::manager_disabled_tool_names(Some(&manager));
        self.components
            .prompt_assembly
            .replace_manager(Some(manager.clone()))
            .map_err(|error| error.to_string())?;
        let notice = self.prompt_assembly_update_notice(
            prelude_changed || dynamic_environment_config_changed || tool_enablement_changed,
            &manager,
        );
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated { manager, notice });
        // capability replacement、空 session refresh 与 event publication 均成功后，working
        // copy 才完成一次完整的产品操作生命周期。
        self.prompt_assembly_edit_session = None;
        Ok(())
    }

    /// `peek_prompt_assembly_edit_snapshot` 返回当前 working copy 的 snapshot，不修改状态。
    ///
    /// 用于测试：进入 edit session 后立即观察 load 结果，无需触发 mutation。
    #[cfg(test)]
    pub(super) fn peek_prompt_assembly_edit_snapshot(
        &self,
    ) -> Option<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot> {
        self.prompt_assembly_edit_session
            .as_ref()
            .map(|session| session.snapshot())
    }
}
