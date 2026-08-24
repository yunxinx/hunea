use runtime_domain::session::{
    RuntimeCommandReceipt, RuntimeEvent, SessionBranchTreePayload, SessionLoadRequestId,
};
use session_store::{ProjectDir, SessionId};

use super::{AppRuntimeCoordinator, session_tree_load::SessionTreeLoadConsumer};

impl AppRuntimeCoordinator {
    pub(super) fn list_sessions(&mut self) -> Result<RuntimeCommandReceipt, String> {
        let views = self.session_views()?;
        let header = self.session_header()?;
        let project_dir = ProjectDir::from_work_dir(&header.work_dir);
        let active_session_id = self
            .components
            .agent_session()?
            .snapshot()
            .session_id
            .or(Some(header.session_id));
        self.components.session_store_worker.list_sessions(
            views,
            project_dir,
            active_session_id,
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn resume_session(
        &mut self,
        session_id: &str,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.components.agent_session()?;
        if self.components.agent_port().activity().is_busy() {
            return Err("Cannot resume session while a request is running".to_string());
        }
        self.ensure_session_mutation_available("resume session")?;

        let session_id = session_id
            .parse::<SessionId>()
            .map_err(|error| format!("Invalid session id: {error}"))?;
        let views = self.session_views()?;
        let header = self.session_header()?;
        self.components
            .session_store_worker
            .resume_session(views, header, session_id)?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn load_session_preview(
        &mut self,
        session_id: &str,
    ) -> Result<RuntimeCommandReceipt, String> {
        let session_id = session_id
            .parse::<SessionId>()
            .map_err(|error| format!("Invalid session id: {error}"))?;
        let views = self.session_views()?;
        self.components
            .session_store_worker
            .load_session_preview(views, session_id)?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn load_entry_tree(
        &mut self,
        request_id: SessionLoadRequestId,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.load_session_tree_for_target(
            SessionTreeLoadTarget::SessionTree(SessionTreeLoadConsumer::EntryTree),
            request_id,
        )
    }

    pub(super) fn load_copy_picker_tree(
        &mut self,
        request_id: SessionLoadRequestId,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.load_session_tree_for_target(
            SessionTreeLoadTarget::SessionTree(SessionTreeLoadConsumer::CopyPicker),
            request_id,
        )
    }

    pub(super) fn load_message_history_startup_cache(
        &mut self,
    ) -> Result<RuntimeCommandReceipt, String> {
        let views = self.session_views()?;
        self.components
            .session_store_worker
            .load_message_history_startup_cache(views)?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn check_prompt_assembly_missing_sources(
        &mut self,
    ) -> Result<RuntimeCommandReceipt, String> {
        let views = self.session_views()?;
        let header = self.session_header()?;
        self.components
            .session_store_worker
            .check_prompt_assembly_missing_sources(
                views,
                header.work_dir,
                self.options.hunea_config_dir.clone(),
            )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn load_message_history_picker_rows(
        &mut self,
        request_id: SessionLoadRequestId,
    ) -> Result<RuntimeCommandReceipt, String> {
        let views = self.session_views()?;
        self.components
            .session_store_worker
            .load_message_history_picker_rows(views, request_id)?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn record_message_history(
        &mut self,
        entry_id: runtime_domain::session::MessageHistoryEntryId,
        text: String,
        limit: usize,
    ) -> Result<RuntimeCommandReceipt, String> {
        let views = self.session_views()?;
        self.components
            .session_store_worker
            .record_message_history(views, entry_id, text, limit)?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn load_session_tree_for_target(
        &mut self,
        target: SessionTreeLoadTarget,
        request_id: SessionLoadRequestId,
    ) -> Result<RuntimeCommandReceipt, String> {
        let Some(session_id) = self.active_session_id_or_empty_tree_event(target, request_id)?
        else {
            return Ok(RuntimeCommandReceipt::Accepted);
        };
        let views = self.session_views()?;
        match target {
            SessionTreeLoadTarget::SessionTree(consumer) => {
                self.components.session_store_worker.load_session_tree(
                    views.clone(),
                    session_id,
                    consumer,
                    request_id,
                )?;
            }
            SessionTreeLoadTarget::BranchTree => {
                self.components
                    .session_store_worker
                    .load_branch_tree(views, session_id, request_id)?;
            }
        }
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn load_branch_preview(
        &mut self,
        request_id: SessionLoadRequestId,
        branch_row_id: &str,
    ) -> Result<RuntimeCommandReceipt, String> {
        let session_id = self
            .components
            .agent_session()?
            .snapshot()
            .session_id
            .ok_or_else(|| "No active persisted session to preview".to_string())?;
        let views = self.session_views()?;
        self.components.session_store_worker.load_branch_preview(
            views,
            session_id,
            request_id,
            branch_row_id.to_string(),
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn load_branch_tree(
        &mut self,
        request_id: SessionLoadRequestId,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.load_session_tree_for_target(SessionTreeLoadTarget::BranchTree, request_id)
    }

    pub(super) fn switch_branch(
        &mut self,
        request_id: SessionLoadRequestId,
        leaf_id: &str,
    ) -> Result<RuntimeCommandReceipt, String> {
        let session_snapshot = self.components.agent_session()?.snapshot();
        if self.components.agent_port().activity().is_busy() {
            return Err("Cannot switch branch while a request is running".to_string());
        }
        self.ensure_session_mutation_available("switch branch")?;
        let session_id = session_snapshot
            .session_id
            .ok_or_else(|| "No active persisted session to switch branch".to_string())?;
        let views = self.session_views()?;
        let header = self.session_header()?;
        self.components.session_store_worker.switch_branch(
            views,
            header,
            session_id,
            request_id,
            leaf_id.to_string(),
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    pub(super) fn select_entry_rewind(
        &mut self,
        entry_id: &str,
    ) -> Result<RuntimeCommandReceipt, String> {
        let session_snapshot = self.components.agent_session()?.snapshot();
        if self.components.agent_port().activity().is_busy() {
            return Err("Cannot rewind session while a request is running".to_string());
        }
        self.ensure_session_mutation_available("rewind session")?;
        let session_id = session_snapshot
            .session_id
            .ok_or_else(|| "No active persisted session to rewind".to_string())?;
        let views = self.session_views()?;
        let header = self.session_header()?;
        self.components.session_store_worker.select_entry_rewind(
            views,
            header,
            session_id,
            entry_id.to_string(),
        )?;
        Ok(RuntimeCommandReceipt::Accepted)
    }

    fn active_session_id_or_empty_tree_event(
        &mut self,
        target: SessionTreeLoadTarget,
        request_id: SessionLoadRequestId,
    ) -> Result<Option<SessionId>, String> {
        let snapshot = self.components.agent_session()?.snapshot();
        let Some(session_id) = snapshot.session_id else {
            if snapshot.is_history_empty {
                self.pending_runtime_events
                    .push(target.empty_tree_event(request_id));
                return Ok(None);
            }
            return Err(target.no_session_error().to_string());
        };
        Ok(Some(session_id))
    }
}

#[derive(Debug, Clone, Copy)]
enum SessionTreeLoadTarget {
    SessionTree(SessionTreeLoadConsumer),
    BranchTree,
}

impl SessionTreeLoadTarget {
    fn empty_tree_event(self, request_id: SessionLoadRequestId) -> RuntimeEvent {
        match self {
            Self::SessionTree(consumer) => consumer.empty_tree_event(request_id),
            Self::BranchTree => RuntimeEvent::SessionBranchTreeLoaded {
                request_id,
                payload: SessionBranchTreePayload {
                    nodes: Vec::new(),
                    current_branch_row_id: None,
                    total_message_count: 0,
                },
            },
        }
    }

    const fn no_session_error(self) -> &'static str {
        match self {
            Self::SessionTree(consumer) => consumer.no_session_error(),
            Self::BranchTree => "No active persisted session to show branch tree",
        }
    }
}
