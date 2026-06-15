use std::{
    collections::{HashMap, HashSet},
    sync::mpsc,
};

use provider_protocol::{ConversationItem, Role};
use runtime_domain::session::{
    ConversationEvent, RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityStatus,
    RuntimeToolActivityUpdate, RuntimeToolKind, TranscriptReplayItem, TranscriptReplayRole,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::ConversationWorkerEvent;
use crate::conversation::PreparedConversationPersistence;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConversationDelta {
    ProviderTurnStarted {
        user_entry_id: Option<String>,
    },
    ProviderContextItem {
        entry_id: Option<String>,
        item: ConversationItem,
    },
}

pub(super) enum SessionPersistenceCommand {
    ProviderTurnStarted,
    ProviderContextItem(ConversationItem),
    ToolActivityStarted(RuntimeToolActivity),
    ToolActivityUpdated(RuntimeToolActivityUpdate),
    TerminalSnapshot(RuntimeTerminalSnapshot),
    Flush {
        ack: oneshot::Sender<Result<(), String>>,
    },
}

#[derive(Default)]
pub(super) struct SessionPersistenceState {
    has_persisted_turn_start: bool,
    tool_activities: HashMap<String, RuntimeToolActivity>,
    final_tool_activity_ids: HashSet<String>,
}

pub(super) async fn run_session_persistence_actor(
    persistence: Option<PreparedConversationPersistence>,
    mut receiver: tokio_mpsc::UnboundedReceiver<SessionPersistenceCommand>,
    sender: mpsc::Sender<ConversationWorkerEvent>,
    cancellation: CancellationToken,
) {
    let mut state = SessionPersistenceState::default();
    while let Some(command) = receiver.recv().await {
        let result = match command {
            SessionPersistenceCommand::ProviderTurnStarted => {
                persist_turn_start(persistence.as_ref(), &sender, &cancellation, &mut state).await
            }
            SessionPersistenceCommand::ProviderContextItem(item) => {
                persist_context_item(
                    persistence.as_ref(),
                    &sender,
                    &cancellation,
                    item,
                    &mut state,
                )
                .await
            }
            SessionPersistenceCommand::ToolActivityStarted(activity) => {
                persist_tool_activity_started(persistence.as_ref(), activity, &mut state).await
            }
            SessionPersistenceCommand::ToolActivityUpdated(update) => {
                persist_tool_activity_update(persistence.as_ref(), update, &mut state).await
            }
            SessionPersistenceCommand::TerminalSnapshot(snapshot) => {
                persist_terminal_snapshot(persistence.as_ref(), snapshot).await
            }
            SessionPersistenceCommand::Flush { ack } => {
                let result = flush_persistence(persistence.as_ref()).await;
                let _ = ack.send(result.clone());
                result
            }
        };

        if let Err(message) = result {
            cancellation.cancel();
            let _ = sender.send(ConversationWorkerEvent::progress(
                ConversationEvent::Failed { message },
            ));
            return;
        }
    }
}

pub(super) async fn persist_turn_start(
    persistence: Option<&PreparedConversationPersistence>,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
    _cancellation: &CancellationToken,
    state: &mut SessionPersistenceState,
) -> Result<(), String> {
    if state.has_persisted_turn_start {
        let _ = sender.send(ConversationWorkerEvent::Session(
            ConversationDelta::ProviderTurnStarted {
                user_entry_id: None,
            },
        ));
        return Ok(());
    }

    let user_entry_id = if let Some(persistence) = persistence {
        persistence
            .store
            .append_config_change(&persistence.session_id, persistence.config_snapshot.clone())
            .await
            .map_err(|error| format!("persist conversation config change: {error}"))?;
        Some(
            persistence
                .store
                .append(
                    &persistence.session_id,
                    persistence.current_user_message.clone(),
                )
                .await
                .map_err(|error| format!("persist conversation user message: {error}"))?,
        )
    } else {
        None
    };

    if let Some(persistence) = persistence {
        append_transcript_replay_items(
            persistence,
            transcript_replay_items_from_context_item(&persistence.current_user_message, state),
        )
        .await?;
    }

    state.has_persisted_turn_start = true;
    let _ = sender.send(ConversationWorkerEvent::Session(
        ConversationDelta::ProviderTurnStarted { user_entry_id },
    ));
    Ok(())
}

pub(super) async fn persist_context_item(
    persistence: Option<&PreparedConversationPersistence>,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
    _cancellation: &CancellationToken,
    item: ConversationItem,
    state: &mut SessionPersistenceState,
) -> Result<(), String> {
    let entry_id = if let Some(persistence) = persistence {
        Some(
            persistence
                .store
                .append(&persistence.session_id, item.clone())
                .await
                .map_err(|error| format!("persist conversation item: {error}"))?,
        )
    } else {
        None
    };
    if let Some(persistence) = persistence {
        append_transcript_replay_items(
            persistence,
            transcript_replay_items_from_context_item(&item, state),
        )
        .await?;
    }
    let _ = sender.send(ConversationWorkerEvent::Session(
        ConversationDelta::ProviderContextItem { entry_id, item },
    ));
    Ok(())
}

pub(super) async fn persist_tool_activity_started(
    persistence: Option<&PreparedConversationPersistence>,
    activity: RuntimeToolActivity,
    state: &mut SessionPersistenceState,
) -> Result<(), String> {
    state
        .tool_activities
        .insert(activity.activity_id.clone(), activity.clone());
    if let Some(persistence) = persistence {
        append_transcript_replay_item(persistence, TranscriptReplayItem::ToolActivity { activity })
            .await?;
    }
    Ok(())
}

pub(super) async fn persist_tool_activity_update(
    persistence: Option<&PreparedConversationPersistence>,
    update: RuntimeToolActivityUpdate,
    state: &mut SessionPersistenceState,
) -> Result<(), String> {
    let activity_id = update.activity_id.clone();
    let is_final = matches!(
        update.status,
        Some(RuntimeToolActivityStatus::Completed | RuntimeToolActivityStatus::Failed)
    );
    let activity = match state.tool_activities.remove(&activity_id) {
        Some(mut activity) => {
            apply_tool_activity_update(&mut activity, update);
            activity
        }
        None => runtime_tool_activity_from_update(update),
    };
    if is_final {
        state.final_tool_activity_ids.insert(activity_id.clone());
    }
    state.tool_activities.insert(activity_id, activity.clone());
    if let Some(persistence) = persistence {
        append_transcript_replay_item(persistence, TranscriptReplayItem::ToolActivity { activity })
            .await?;
    }
    Ok(())
}

pub(super) async fn persist_terminal_snapshot(
    persistence: Option<&PreparedConversationPersistence>,
    snapshot: RuntimeTerminalSnapshot,
) -> Result<(), String> {
    if let Some(persistence) = persistence {
        append_transcript_replay_item(
            persistence,
            TranscriptReplayItem::TerminalSnapshot { snapshot },
        )
        .await?;
    }
    Ok(())
}

async fn append_transcript_replay_items(
    persistence: &PreparedConversationPersistence,
    items: Vec<TranscriptReplayItem>,
) -> Result<(), String> {
    for item in items {
        append_transcript_replay_item(persistence, item).await?;
    }
    Ok(())
}

async fn append_transcript_replay_item(
    persistence: &PreparedConversationPersistence,
    item: TranscriptReplayItem,
) -> Result<(), String> {
    persistence
        .store
        .append_transcript_replay(&persistence.session_id, item)
        .await
        .map(|_| ())
        .map_err(|error| format!("persist transcript replay item: {error}"))
}

fn transcript_replay_items_from_context_item(
    item: &ConversationItem,
    state: &SessionPersistenceState,
) -> Vec<TranscriptReplayItem> {
    match item {
        ConversationItem::Message { role, .. } => {
            let content = item.text_content();
            if content.trim().is_empty() {
                return Vec::new();
            }
            let item = match transcript_role_from_provider_role(*role) {
                Some(role) => TranscriptReplayItem::Message { role, content },
                None => TranscriptReplayItem::System { content },
            };
            vec![item]
        }
        ConversationItem::Reasoning { content, .. } => {
            if content.trim().is_empty() {
                Vec::new()
            } else {
                vec![TranscriptReplayItem::Reasoning {
                    content: content.clone(),
                }]
            }
        }
        ConversationItem::ToolResult { call_id, .. } => {
            if state.final_tool_activity_ids.contains(call_id) {
                return Vec::new();
            }
            let content = item.text_content();
            if content.trim().is_empty() {
                Vec::new()
            } else {
                vec![TranscriptReplayItem::ToolResult { content }]
            }
        }
    }
}

fn transcript_role_from_provider_role(role: Role) -> Option<TranscriptReplayRole> {
    match role {
        Role::System => None,
        Role::User => Some(TranscriptReplayRole::User),
        Role::Assistant => Some(TranscriptReplayRole::Assistant),
    }
}

fn apply_tool_activity_update(
    activity: &mut RuntimeToolActivity,
    update: RuntimeToolActivityUpdate,
) {
    if let Some(title) = update.title {
        activity.title = title;
    }
    if let Some(kind) = update.kind {
        activity.kind = kind;
    }
    if let Some(status) = update.status {
        activity.status = status;
    }
    if let Some(content) = update.content {
        activity.content = content;
    }
    if let Some(locations) = update.locations {
        activity.locations = locations;
    }
    if let Some(raw_input) = update.raw_input {
        activity.raw_input = Some(raw_input);
    }
    if let Some(raw_output) = update.raw_output {
        activity.raw_output = Some(raw_output);
    }
}

fn runtime_tool_activity_from_update(update: RuntimeToolActivityUpdate) -> RuntimeToolActivity {
    let activity_id = update.activity_id;
    let title = update
        .title
        .unwrap_or_else(|| format!("Tool activity {activity_id}"));
    RuntimeToolActivity {
        activity_id,
        title,
        kind: update.kind.unwrap_or(RuntimeToolKind::Other),
        status: update.status.unwrap_or(RuntimeToolActivityStatus::Pending),
        content: update.content.unwrap_or_default(),
        locations: update.locations.unwrap_or_default(),
        raw_input: update.raw_input,
        raw_output: update.raw_output,
    }
}

async fn flush_persistence(
    persistence: Option<&PreparedConversationPersistence>,
) -> Result<(), String> {
    if let Some(persistence) = persistence {
        persistence
            .store
            .flush(&persistence.session_id)
            .await
            .map_err(|error| format!("persist conversation flush: {error}"))?;
    }
    Ok(())
}

pub(super) async fn flush_session_persistence(
    sender: &tokio_mpsc::UnboundedSender<SessionPersistenceCommand>,
) -> Result<(), String> {
    let (ack_tx, ack_rx) = oneshot::channel();
    sender
        .send(SessionPersistenceCommand::Flush { ack: ack_tx })
        .map_err(|_| "conversation session persistence worker stopped unexpectedly".to_string())?;
    ack_rx
        .await
        .map_err(|_| "conversation session persistence worker dropped flush ack".to_string())?
}
