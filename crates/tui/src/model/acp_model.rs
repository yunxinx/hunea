use mo_core::{
    model_catalog::{ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource},
    session::RuntimeModelConfig,
};

use super::Model;

/// `AcpModelSelectionRollback` 保存 ACP 模型切换请求确认前的本地选择状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcpModelSelectionRollback {
    agent_id: String,
    selected_model: Option<ModelSelection>,
    acp_current_model: Option<String>,
}

pub(crate) fn acp_model_provider_id(agent_id: &str) -> String {
    format!("acp:{agent_id}")
}

fn acp_model_label(name: &str, value: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        value.to_string()
    } else {
        name.to_string()
    }
}

fn acp_model_entry_description(name: &str, value: &str) -> Option<String> {
    let label = acp_model_label(name, value);
    (label != value).then_some(label)
}

impl Model {
    pub(crate) fn activate_acp_model_scope(&mut self, agent_id: &str) {
        let provider_id = acp_model_provider_id(agent_id);
        self.pending_acp_model_rollback = None;
        self.model_catalog = ModelCatalog::new(vec![ModelProvider::acp(
            provider_id,
            format!("ACP: {agent_id}"),
            Vec::new(),
        )]);
        self.selected_model = None;
        self.acp_current_model = None;
        self.acp_model_config_id = None;
        self.bump_status_line_revision();
        self.sync_model_panel_to_selection();
    }

    pub(crate) fn apply_acp_model_config(&mut self, agent_id: &str, config: RuntimeModelConfig) {
        let provider_id = acp_model_provider_id(agent_id);
        let display_name = format!("ACP: {agent_id}");
        let mut entries = config
            .options
            .into_iter()
            .filter_map(|option| {
                let value = option.value.trim().to_string();
                if value.is_empty() {
                    return None;
                }
                let description = acp_model_entry_description(&option.name, &value);
                Some(ModelEntry::new(value, description, ModelSource::Acp))
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            entries.push(ModelEntry::new(
                config.current_value.clone(),
                acp_model_entry_description(&config.current_name, &config.current_value),
                ModelSource::Acp,
            ));
        }
        let current_label = acp_model_label(&config.current_name, &config.current_value);

        self.model_catalog = ModelCatalog::new(vec![ModelProvider::acp(
            provider_id.clone(),
            display_name,
            entries,
        )]);
        self.selected_model = Some(ModelSelection::new(provider_id, config.current_value));
        self.set_acp_current_model(Some(current_label));
        self.acp_model_config_id = config.config_id;
        self.commit_pending_acp_model_change(agent_id);
        self.sync_model_panel_to_selection();
    }

    /// `begin_pending_acp_model_change` 记录 ACP 模型切换乐观更新前的本地状态。
    pub(crate) fn begin_pending_acp_model_change(&mut self, agent_id: &str) {
        if self
            .pending_acp_model_rollback
            .as_ref()
            .is_some_and(|snapshot| snapshot.agent_id == agent_id)
        {
            return;
        }

        self.pending_acp_model_rollback = Some(AcpModelSelectionRollback {
            agent_id: agent_id.to_string(),
            selected_model: self.selected_model.clone(),
            acp_current_model: self.acp_current_model.clone(),
        });
    }

    /// `commit_pending_acp_model_change` 在 agent 确认切换成功后丢弃回滚快照。
    pub(crate) fn commit_pending_acp_model_change(&mut self, agent_id: &str) {
        if self
            .pending_acp_model_rollback
            .as_ref()
            .is_some_and(|snapshot| snapshot.agent_id == agent_id)
        {
            self.pending_acp_model_rollback = None;
        }
    }

    /// `rollback_pending_acp_model_change` 在 ACP 模型切换失败后恢复旧选择。
    pub(crate) fn rollback_pending_acp_model_change(&mut self, agent_id: &str) {
        let Some(snapshot) = self.pending_acp_model_rollback.take() else {
            return;
        };
        if snapshot.agent_id != agent_id {
            self.pending_acp_model_rollback = Some(snapshot);
            return;
        }

        let changed = self.selected_model != snapshot.selected_model
            || self.acp_current_model != snapshot.acp_current_model;
        self.selected_model = snapshot.selected_model;
        self.acp_current_model = snapshot.acp_current_model;
        if changed {
            self.bump_status_line_revision();
            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
            }
        }
        self.sync_model_panel_to_selection();
    }
}
