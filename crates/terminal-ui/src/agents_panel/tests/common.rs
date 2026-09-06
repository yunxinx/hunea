use crossterm::event::{KeyCode, KeyEvent};
use runtime_domain::agent::{
    AgentActivitySummary, AgentId, AgentObjective, AgentObservationId, AgentObservationRejection,
    AgentObservationRequestId, AgentOverviewDelta, AgentOverviewDeltaKind, AgentOverviewRow,
    AgentOverviewSnapshot, AgentPermissionRequest, AgentPermissionState, AgentPermissionTarget,
    AgentPermissionUpdate, AgentProjectionEvent, AgentProjectionRevision, AgentProjectionStatus,
    AgentRuntimeGeneration, AgentTitle, AgentTurnId, AgentViewSnapshot,
};
use runtime_domain::session::{
    RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent, RuntimePermissionOption,
    RuntimePermissionOptionKind, RuntimePermissionRequest, RuntimeTarget,
};

use crate::{
    AppEffect, AppEvent, Model, StartupBannerOptions,
    runner::runtime_port::{ModelRuntimePort, PromptRuntimePort, RuntimeCommandPort},
    runtime::RuntimeEventApply,
};

/// overview snapshot fixture 的固定 observation identity。
pub(super) const FIXTURE_OBSERVATION_ID: u64 = 11;
pub(super) const FIXTURE_GENERATION: u64 = 1;

pub(super) fn press_key(model: &mut Model, code: KeyCode) -> Option<AppEffect> {
    model.update(AppEvent::Key(KeyEvent::from(code)))
}

pub(super) fn agent_title(text: &str) -> AgentTitle {
    AgentTitle::resolve(
        &AgentObjective::new("fallback objective").expect("fixture objective should be valid"),
        Some(text),
    )
    .expect("fixture title should resolve")
}

pub(super) fn overview_row(
    agent_id: u64,
    title: &str,
    status: AgentProjectionStatus,
) -> AgentOverviewRow {
    AgentOverviewRow {
        agent_id: AgentId::new(agent_id),
        title: agent_title(title),
        status,
        latest_activity: AgentActivitySummary::Thinking,
        elapsed_ms: Some(83_000),
        tool_uses: Some(3),
        token_usage: Some(2_048),
    }
}

pub(super) fn sample_rows() -> Vec<AgentOverviewRow> {
    vec![
        overview_row(2, "research task", AgentProjectionStatus::Working),
        overview_row(3, "write docs", AgentProjectionStatus::Completed),
    ]
}

pub(super) fn overview_snapshot(rows: Vec<AgentOverviewRow>) -> AgentOverviewSnapshot {
    AgentOverviewSnapshot {
        observation_id: AgentObservationId::new(FIXTURE_OBSERVATION_ID),
        generation: AgentRuntimeGeneration::new(FIXTURE_GENERATION),
        revision: AgentProjectionRevision::new(1),
        rows,
    }
}

pub(super) fn apply_overview_snapshot(
    model: &mut Model,
    request_id: AgentObservationRequestId,
    rows: Vec<AgentOverviewRow>,
) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentsOverviewSnapshotLoaded {
            request_id,
            snapshot: overview_snapshot(rows),
        },
    )));
}

pub(super) fn apply_overview_delta(model: &mut Model, kind: AgentOverviewDeltaKind) {
    apply_overview_delta_with_identity(model, kind, FIXTURE_OBSERVATION_ID, FIXTURE_GENERATION);
}

pub(super) fn apply_overview_delta_with_identity(
    model: &mut Model,
    kind: AgentOverviewDeltaKind,
    observation_id: u64,
    generation: u64,
) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentsOverviewUpdated {
            delta: AgentOverviewDelta {
                observation_id: AgentObservationId::new(observation_id),
                generation: AgentRuntimeGeneration::new(generation),
                revision: AgentProjectionRevision::new(2),
                kind,
            },
        },
    )));
}

pub(super) fn apply_overview_rejection(
    model: &mut Model,
    request_id: AgentObservationRequestId,
    reason: AgentObservationRejection,
) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentObservationRejected { request_id, reason },
    )));
}

pub(super) fn view_snapshot(
    agent_id: u64,
    observation_id: u64,
    answer: Option<&str>,
) -> AgentViewSnapshot {
    view_snapshot_with_permission(agent_id, observation_id, answer, None)
}

pub(super) fn view_snapshot_with_permission(
    agent_id: u64,
    observation_id: u64,
    answer: Option<&str>,
    permission: Option<AgentPermissionRequest>,
) -> AgentViewSnapshot {
    let revision = AgentProjectionRevision::new(3);
    AgentViewSnapshot {
        observation_id: AgentObservationId::new(observation_id),
        generation: AgentRuntimeGeneration::new(FIXTURE_GENERATION),
        revision,
        transcript: runtime_domain::agent::AgentTranscriptSnapshot {
            observation_id: AgentObservationId::new(observation_id),
            generation: AgentRuntimeGeneration::new(FIXTURE_GENERATION),
            revision,
            agent_id: AgentId::new(agent_id),
            title: agent_title("research task"),
            status: AgentProjectionStatus::Working,
            items: vec![
                runtime_domain::agent::AgentTranscriptItem::User {
                    content: "summarize the repo".to_string(),
                },
                runtime_domain::agent::AgentTranscriptItem::Tool {
                    title: "QueryDatabase: users".to_string(),
                    content: "3 rows returned".to_string(),
                },
                runtime_domain::agent::AgentTranscriptItem::Assistant {
                    content: answer.unwrap_or("partial draft").to_string(),
                },
            ],
        },
        preview: runtime_domain::agent::AgentPreviewSnapshot {
            generation: AgentRuntimeGeneration::new(FIXTURE_GENERATION),
            revision,
            agent_id: AgentId::new(agent_id),
            title: agent_title("research task"),
            status: AgentProjectionStatus::Working,
            latest_activity: AgentActivitySummary::Thinking,
            elapsed_ms: Some(83_000),
            latest_committed_answer: answer.map(str::to_string),
            permission,
        },
    }
}

/// 全局 pending 投影测试的 permission request fixture。
///
/// options 是 runtime-issued 全集（option_id 稳定，label 短），
/// `target.generation` 与 FIXTURE_GENERATION 对齐以便断言 typed identity。
pub(super) fn permission_request(
    agent_id: u64,
    request_id: &str,
    state: AgentPermissionState,
    occurred_at_ms: i64,
) -> AgentPermissionRequest {
    AgentPermissionRequest {
        target: AgentPermissionTarget {
            agent_id: AgentId::new(agent_id),
            turn_id: AgentTurnId::new(7),
            generation: AgentRuntimeGeneration::new(FIXTURE_GENERATION),
            runtime_target: RuntimeTarget::provider("local", "qwen3"),
            request_id: request_id.to_string(),
        },
        request: RuntimePermissionRequest::new(
            request_id,
            Some("Run database query".to_string()),
            vec![
                RuntimePermissionOption::new(
                    format!("{request_id}-allow"),
                    "Allow",
                    RuntimePermissionOptionKind::AllowOnce,
                ),
                RuntimePermissionOption::new(
                    format!("{request_id}-deny"),
                    "Deny",
                    RuntimePermissionOptionKind::RejectOnce,
                ),
            ],
        ),
        state,
        occurred_at_ms,
    }
}

pub(super) fn apply_permission_update(
    model: &mut Model,
    agent_id: u64,
    generation: u64,
    request: Option<AgentPermissionRequest>,
) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentPermissionUpdated {
            update: AgentPermissionUpdate {
                agent_id: AgentId::new(agent_id),
                generation: AgentRuntimeGeneration::new(generation),
                request,
            },
        },
    )));
}

pub(super) fn apply_view_snapshot_loaded(
    model: &mut Model,
    request_id: AgentObservationRequestId,
    snapshot: AgentViewSnapshot,
) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentViewSnapshotLoaded {
            request_id,
            snapshot,
        },
    )));
}

pub(super) fn apply_view_updated(model: &mut Model, snapshot: AgentViewSnapshot) {
    model.apply_runtime_event(RuntimeEvent::AgentProjection(Box::new(
        AgentProjectionEvent::AgentViewUpdated { snapshot },
    )));
}

pub(super) fn ready_panel_model() -> Model {
    ready_panel_model_with_rows(sample_rows())
}

pub(super) fn ready_panel_model_with_rows(rows: Vec<AgentOverviewRow>) -> Model {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_window(100, 24);
    model.set_palette(crate::theme::default_palette(), true);
    let request_id = model.open_agents_panel_loading();
    apply_overview_snapshot(&mut model, request_id, rows);
    model
}

/// 记录派发命令的 test port，用于断言 runner 侧 StopObserving/StopAgent 派发。
#[derive(Default)]
pub(super) struct RecordingRuntimePort {
    pub(super) commands: Vec<RuntimeCommand>,
}

impl RuntimeCommandPort for RecordingRuntimePort {
    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        self.commands.push(command);
        Ok(RuntimeCommandReceipt::Accepted)
    }
}

impl ModelRuntimePort for RecordingRuntimePort {
    fn drain_model_provider_refresh_events(
        &mut self,
    ) -> Vec<runtime_domain::model_catalog::ModelProviderRefreshEvent> {
        Vec::new()
    }

    fn persist_selected_model(
        &mut self,
        _selection: &runtime_domain::model_catalog::ModelSelection,
    ) -> Result<(), String> {
        Ok(())
    }

    fn refresh_model_provider(
        &mut self,
        _request: runtime_domain::model_catalog::ProviderSyncRequest,
    ) -> Result<(), String> {
        Ok(())
    }
}

impl PromptRuntimePort for RecordingRuntimePort {
    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        _mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    fn commit_prompt_assembly_edit(&mut self) -> Result<(), String> {
        Ok(())
    }
}
