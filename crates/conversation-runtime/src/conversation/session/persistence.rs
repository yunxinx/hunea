use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, mpsc},
};

use provider_protocol::{ConversationItem, Role};
use runtime_domain::session::{
    ConversationEvent, RuntimeTerminalSnapshot, RuntimeToolActivity, RuntimeToolActivityStatus,
    RuntimeToolActivityUpdate, RuntimeToolKind, TranscriptReplayItem, TranscriptReplayRole,
    TranscriptUserMessage,
};
use tokio::sync::{mpsc as tokio_mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::ConversationWorkerEvent;
use crate::conversation::PreparedConversationPersistence;
use session_store::{SessionId, SessionStoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ConversationDelta {
    ProviderTurnStarted {
        session_id: Option<SessionId>,
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
        ack: oneshot::Sender<Result<(), Arc<SessionPersistenceError>>>,
    },
}

#[derive(Default)]
pub(super) struct SessionPersistenceState {
    has_persisted_turn_start: bool,
    session_id: Option<SessionId>,
    user_entry_id: Option<String>,
    tool_activities: HashMap<String, RuntimeToolActivity>,
    final_tool_activity_ids: HashSet<String>,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SessionPersistenceError {
    #[error("persist conversation config change failed: {source}")]
    PersistConfigChange {
        #[source]
        source: SessionStoreError,
    },
    #[error("persist conversation user message failed: {source}")]
    PersistUserMessage {
        #[source]
        source: SessionStoreError,
    },
    #[error("persist conversation item failed: {source}")]
    PersistConversationItem {
        #[source]
        source: SessionStoreError,
    },
    #[error("persist transcript replay item failed: {source}")]
    PersistTranscriptReplay {
        #[source]
        source: SessionStoreError,
    },
    #[error("create conversation session failed: {source}")]
    CreateSession {
        #[source]
        source: SessionStoreError,
    },
    #[error("persist conversation flush failed: {source}")]
    Flush {
        #[source]
        source: SessionStoreError,
    },
    #[error("conversation session has not started persistence")]
    MissingSession,
    #[error("conversation session persistence worker stopped unexpectedly")]
    WorkerStopped,
    #[error("conversation session persistence worker dropped flush ack")]
    FlushAckDropped,
}

pub(super) async fn run_session_persistence_actor(
    persistence: Option<PreparedConversationPersistence>,
    mut receiver: tokio_mpsc::Receiver<SessionPersistenceCommand>,
    sender: mpsc::Sender<ConversationWorkerEvent>,
    conversation_cancellation: CancellationToken,
) {
    let mut state = SessionPersistenceState::default();
    while let Some(command) = receiver.recv().await {
        let result = match command {
            SessionPersistenceCommand::ProviderTurnStarted => {
                persist_turn_start(persistence.as_ref(), &sender, &mut state)
                    .await
                    .map_err(Arc::new)
            }
            SessionPersistenceCommand::ProviderContextItem(item) => {
                persist_context_item(persistence.as_ref(), &sender, item, &mut state)
                    .await
                    .map_err(Arc::new)
            }
            SessionPersistenceCommand::ToolActivityStarted(activity) => {
                persist_tool_activity_started(persistence.as_ref(), activity, &mut state)
                    .await
                    .map_err(Arc::new)
            }
            SessionPersistenceCommand::ToolActivityUpdated(update) => {
                persist_tool_activity_update(persistence.as_ref(), update, &mut state)
                    .await
                    .map_err(Arc::new)
            }
            SessionPersistenceCommand::TerminalSnapshot(snapshot) => {
                persist_terminal_snapshot(persistence.as_ref(), snapshot, &state)
                    .await
                    .map_err(Arc::new)
            }
            SessionPersistenceCommand::Flush { ack } => {
                let result = flush_persistence(persistence.as_ref(), &state).await;
                match result {
                    Ok(()) => {
                        let _ = ack.send(Ok(()));
                        Ok(())
                    }
                    Err(error) => {
                        let error = Arc::new(error);
                        let _ = ack.send(Err(error.clone()));
                        Err(error)
                    }
                }
            }
        };

        if let Err(error) = result {
            let message = error.to_string();
            conversation_cancellation.cancel();
            let _ = sender.send(ConversationWorkerEvent::progress(
                ConversationEvent::Failed { message },
            ));
            cancel_pending_flushes(drain_pending_commands(&mut receiver), error);
            return;
        }
    }
}

fn drain_pending_commands(
    receiver: &mut tokio_mpsc::Receiver<SessionPersistenceCommand>,
) -> Vec<SessionPersistenceCommand> {
    receiver.close();
    let mut commands = Vec::new();
    while let Ok(command) = receiver.try_recv() {
        commands.push(command);
    }
    commands
}

fn cancel_pending_flushes(
    commands: impl IntoIterator<Item = SessionPersistenceCommand>,
    error: Arc<SessionPersistenceError>,
) {
    for command in commands {
        if let SessionPersistenceCommand::Flush { ack } = command {
            let _ = ack.send(Err(error.clone()));
        }
    }
}

pub(super) async fn persist_turn_start(
    persistence: Option<&PreparedConversationPersistence>,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
    state: &mut SessionPersistenceState,
) -> Result<(), SessionPersistenceError> {
    if state.has_persisted_turn_start {
        let _ = sender.send(ConversationWorkerEvent::Session(
            ConversationDelta::ProviderTurnStarted {
                session_id: state.session_id.clone(),
                user_entry_id: state.user_entry_id.clone(),
            },
        ));
        return Ok(());
    }

    let (session_id, user_entry_id) = if let Some(persistence) = persistence {
        let session_id = ensure_persistence_session(persistence).await?;
        persistence
            .store
            .append_config_change(&session_id, persistence.config_snapshot.clone())
            .await
            .map_err(|source| SessionPersistenceError::PersistConfigChange { source })?;
        let user_entry_ids = persistence
            .store
            .append_many(&session_id, persistence.current_user_items.clone())
            .await
            .map_err(|source| SessionPersistenceError::PersistUserMessage { source })?;
        let user_entry_id = user_entry_ids.last().cloned();
        (Some(session_id), user_entry_id)
    } else {
        (None, None)
    };

    if let (Some(persistence), Some(session_id)) = (persistence, session_id.as_ref()) {
        append_transcript_replay_items(
            persistence,
            session_id,
            transcript_replay_items_from_transcript_user_message(
                &persistence.transcript_user_message,
            ),
        )
        .await?;
        if !persistence.transcript_replay_after_user.is_empty() {
            append_transcript_replay_items(
                persistence,
                session_id,
                persistence.transcript_replay_after_user.clone(),
            )
            .await?;
        }
    }

    state.has_persisted_turn_start = true;
    state.session_id = session_id.clone();
    state.user_entry_id = user_entry_id.clone();
    let _ = sender.send(ConversationWorkerEvent::Session(
        ConversationDelta::ProviderTurnStarted {
            session_id,
            user_entry_id,
        },
    ));
    Ok(())
}

pub(super) async fn persist_context_item(
    persistence: Option<&PreparedConversationPersistence>,
    sender: &mpsc::Sender<ConversationWorkerEvent>,
    item: ConversationItem,
    state: &mut SessionPersistenceState,
) -> Result<(), SessionPersistenceError> {
    let active_session_id = if let Some(persistence) = persistence {
        Some(active_session_id(persistence, state)?)
    } else {
        None
    };
    let entry_id = if let (Some(persistence), Some(session_id)) =
        (persistence, active_session_id.as_ref())
    {
        Some(
            persistence
                .store
                .append(session_id, item.clone())
                .await
                .map_err(|source| SessionPersistenceError::PersistConversationItem { source })?,
        )
    } else {
        None
    };
    if let (Some(persistence), Some(session_id)) = (persistence, active_session_id.as_ref()) {
        append_transcript_replay_items(
            persistence,
            session_id,
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
) -> Result<(), SessionPersistenceError> {
    state
        .tool_activities
        .insert(activity.activity_id.clone(), activity.clone());
    if let Some(persistence) = persistence {
        let session_id = active_session_id(persistence, state)?;
        append_transcript_replay_item(
            persistence,
            &session_id,
            TranscriptReplayItem::ToolActivity { activity },
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn persist_tool_activity_update(
    persistence: Option<&PreparedConversationPersistence>,
    update: RuntimeToolActivityUpdate,
    state: &mut SessionPersistenceState,
) -> Result<(), SessionPersistenceError> {
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
        let session_id = active_session_id(persistence, state)?;
        append_transcript_replay_item(
            persistence,
            &session_id,
            TranscriptReplayItem::ToolActivity { activity },
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn persist_terminal_snapshot(
    persistence: Option<&PreparedConversationPersistence>,
    snapshot: RuntimeTerminalSnapshot,
    state: &SessionPersistenceState,
) -> Result<(), SessionPersistenceError> {
    if let Some(persistence) = persistence {
        let session_id = active_session_id(persistence, state)?;
        append_transcript_replay_item(
            persistence,
            &session_id,
            TranscriptReplayItem::TerminalSnapshot { snapshot },
        )
        .await?;
    }
    Ok(())
}

async fn append_transcript_replay_items(
    persistence: &PreparedConversationPersistence,
    session_id: &SessionId,
    items: Vec<TranscriptReplayItem>,
) -> Result<(), SessionPersistenceError> {
    for item in items {
        append_transcript_replay_item(persistence, session_id, item).await?;
    }
    Ok(())
}

async fn append_transcript_replay_item(
    persistence: &PreparedConversationPersistence,
    session_id: &SessionId,
    item: TranscriptReplayItem,
) -> Result<(), SessionPersistenceError> {
    persistence
        .store
        .append_transcript_replay(session_id, item)
        .await
        .map(|_| ())
        .map_err(|source| SessionPersistenceError::PersistTranscriptReplay { source })
}

async fn ensure_persistence_session(
    persistence: &PreparedConversationPersistence,
) -> Result<SessionId, SessionPersistenceError> {
    if let Some(session_id) = persistence.session_id.as_ref() {
        return Ok(session_id.clone());
    }

    let mut header = persistence.header_template.clone();
    header.initial_model = persistence.config_snapshot.model.clone();
    persistence
        .store
        .create_session(header)
        .await
        .map_err(|source| SessionPersistenceError::CreateSession { source })
}

fn active_session_id(
    persistence: &PreparedConversationPersistence,
    state: &SessionPersistenceState,
) -> Result<SessionId, SessionPersistenceError> {
    state
        .session_id
        .clone()
        .or_else(|| persistence.session_id.clone())
        .ok_or(SessionPersistenceError::MissingSession)
}

fn transcript_replay_items_from_context_item(
    item: &ConversationItem,
    state: &SessionPersistenceState,
) -> Vec<TranscriptReplayItem> {
    match item {
        ConversationItem::Message { role, .. } => {
            let content = item.summary_text_content();
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
            let content = item.summary_text_content();
            if content.trim().is_empty() {
                Vec::new()
            } else {
                vec![TranscriptReplayItem::ToolResult { content }]
            }
        }
    }
}

fn transcript_replay_items_from_transcript_user_message(
    message: &TranscriptUserMessage,
) -> Vec<TranscriptReplayItem> {
    if message.display_content().trim().is_empty() {
        return Vec::new();
    }

    if !message.requires_bound_replay() {
        return vec![TranscriptReplayItem::Message {
            role: TranscriptReplayRole::User,
            content: message.content.clone(),
        }];
    }

    vec![TranscriptReplayItem::BoundUserMessage {
        message: message.clone(),
    }]
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
    state: &SessionPersistenceState,
) -> Result<(), SessionPersistenceError> {
    if let Some(persistence) = persistence {
        let Some(session_id) = state
            .session_id
            .as_ref()
            .or(persistence.session_id.as_ref())
        else {
            return Ok(());
        };
        persistence
            .store
            .flush(session_id)
            .await
            .map_err(|source| SessionPersistenceError::Flush { source })?;
    }
    Ok(())
}

pub(super) async fn flush_session_persistence(
    sender: &tokio_mpsc::Sender<SessionPersistenceCommand>,
) -> Result<(), Arc<SessionPersistenceError>> {
    let (ack_tx, ack_rx) = oneshot::channel();
    sender
        .send(SessionPersistenceCommand::Flush { ack: ack_tx })
        .await
        .map_err(|_| Arc::new(SessionPersistenceError::WorkerStopped))?;
    ack_rx
        .await
        .map_err(|_| Arc::new(SessionPersistenceError::FlushAckDropped))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_pending_flushes_replies_to_waiting_acknowledgements() {
        let (flush_ack, flush_result) = oneshot::channel();
        let commands = vec![SessionPersistenceCommand::Flush { ack: flush_ack }];
        let error = Arc::new(SessionPersistenceError::WorkerStopped);

        cancel_pending_flushes(commands, error.clone());

        let received = flush_result
            .blocking_recv()
            .expect("flush acknowledgement should be sent")
            .expect_err("cancelled flush should receive the cancellation error");
        assert!(Arc::ptr_eq(&received, &error));
    }
}
