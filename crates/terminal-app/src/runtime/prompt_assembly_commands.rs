use runtime_domain::prompt_assembly::PromptAssemblyMutation;
use runtime_domain::session::{PromptAssemblyUpdateNotice, RuntimeEvent};

use super::AppRuntimeCoordinator;
use crate::prompt_assembly::{
    PromptAssemblyEditSession, dynamic_environment_session_config_from_manager,
};

/// `PromptSessionConfigRefreshTarget` 标识 commit 后的新 prelude 应作用于当前空会话还是下一次新会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptSessionConfigRefreshTarget {
    CurrentEmptySession,
    NextNewSession,
}

impl AppRuntimeCoordinator {
    fn prompt_session_config_refresh_target(&self) -> PromptSessionConfigRefreshTarget {
        if !self.conversation_worker.is_running()
            && self.pending_conversation_turn.is_none()
            && self.provider_conversation.is_history_empty()
            && self.provider_conversation.session_id().is_none()
        {
            PromptSessionConfigRefreshTarget::CurrentEmptySession
        } else {
            PromptSessionConfigRefreshTarget::NextNewSession
        }
    }

    /// `prompt_assembly_update_notice` 在 commit 后判断是否需要通知用户。
    ///
    /// 仅当 prelude / dynamic env 实际变化时返回 `Some`，并同步更新当前空会话的 provider 配置。
    fn prompt_assembly_update_notice(
        &mut self,
        session_prompt_config_changed: bool,
        manager: &runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot,
        dynamic_environment_session_config: &runtime_domain::dynamic_environment::DynamicEnvironmentSessionConfig,
    ) -> Option<PromptAssemblyUpdateNotice> {
        if !session_prompt_config_changed {
            return None;
        }
        match self.prompt_session_config_refresh_target() {
            PromptSessionConfigRefreshTarget::CurrentEmptySession => {
                self.provider_conversation
                    .set_prompt_prelude(Some(manager.resolution.prelude.clone()));
                self.provider_conversation
                    .set_dynamic_environment_session_config(Some(
                        dynamic_environment_session_config.clone(),
                    ));
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
        let store = self.session_store()?;
        let header = self.session_header()?;
        let session = PromptAssemblyEditSession::load(
            store,
            header.work_dir,
            self.prompt_assembly_tool_definitions().to_vec(),
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
        let outcome = {
            let store = self.session_store()?;
            let Some(session) = self.prompt_assembly_edit_session.as_mut() else {
                return Ok(());
            };
            session.commit(store)
        }
        .map_err(|error| error.to_string())?;
        // commit 成功（无论是否 dirty）都释放 edit session，避免 not-dirty 早退时
        // working copy 长期挂在 coordinator 上。
        self.prompt_assembly_edit_session = None;
        let manager = match outcome {
            Some(outcome) => outcome.manager,
            None => return Ok(()),
        };

        let dynamic_environment_session_config =
            dynamic_environment_session_config_from_manager(&manager);
        let prelude_changed =
            self.options.initial_prompt_prelude.as_ref() != Some(&manager.resolution.prelude);
        let dynamic_environment_config_changed = self
            .options
            .initial_dynamic_environment_session_config
            .as_ref()
            != Some(&dynamic_environment_session_config);
        let notice = self.prompt_assembly_update_notice(
            prelude_changed || dynamic_environment_config_changed,
            &manager,
            &dynamic_environment_session_config,
        );
        self.options.prompt_assembly_manager = Some(manager.clone());
        self.options.initial_prompt_prelude = Some(manager.resolution.prelude.clone());
        self.options.initial_dynamic_environment_session_config =
            Some(dynamic_environment_session_config);
        self.pending_runtime_events
            .push(RuntimeEvent::PromptAssemblyUpdated { manager, notice });
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
