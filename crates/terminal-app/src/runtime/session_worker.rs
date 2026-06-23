use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use conversation_runtime::ProviderConversation;
use runtime_domain::session::{
    RuntimeEvent, SessionLoadRequestId, SessionPickerRow, SessionResumePayload, SessionTreePayload,
};
use session_store::{ProjectDir, SessionHeader, SessionId, SessionListOptions, SessionStore};

use super::{
    session_branch_tree_payload, session_picker_row_from_meta, session_preview_payload,
    session_resume_payload, session_tree_load::SessionTreeLoadConsumer, session_tree_payload,
};

const SESSION_EVENT_DRAIN_WAIT: Duration = Duration::from_millis(2);
const SESSION_SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

pub(super) struct SessionStoreWorker {
    command_sender: Sender<SessionStoreCommand>,
    event_receiver: Receiver<SessionStoreWorkerEvent>,
    pending_commands: usize,
    pending_mutations: usize,
}

pub(super) enum SessionStoreWorkerEvent {
    Runtime {
        event: RuntimeEvent,
        completes_command: bool,
        is_mutation: bool,
    },
    Restored {
        conversation: ProviderConversation,
        payload: SessionResumePayload,
    },
    RestoredWithTree {
        conversation: ProviderConversation,
        resume_payload: SessionResumePayload,
        tree_request_id: SessionLoadRequestId,
        tree_payload: SessionTreePayload,
    },
    Noop,
    Failed {
        message: String,
        is_mutation: bool,
    },
}

enum SessionStoreCommand {
    ListSessions {
        store: Arc<dyn SessionStore>,
        project_dir: ProjectDir,
        active_session_id: Option<SessionId>,
    },
    LoadSessionPreview {
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
    },
    ResumeSession {
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
    },
    LoadSessionTree {
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
        consumer: SessionTreeLoadConsumer,
    },
    LoadBranchTree {
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
    },
    LoadBranchPreview {
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
        branch_row_id: String,
    },
    SwitchBranch {
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
        leaf_id: String,
    },
    SelectEntryRewind {
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
        entry_id: String,
    },
    SetLeaf {
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        leaf_id: String,
    },
    FlushAll {
        store: Arc<dyn SessionStore>,
        ack: Sender<Result<(), String>>,
    },
}

impl Default for SessionStoreWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionStoreWorker {
    pub(super) fn new() -> Self {
        let (command_sender, command_receiver) = mpsc::channel();
        let (event_sender, event_receiver) = mpsc::channel();
        // session store 可能包含阻塞文件系统路径；这里固定为专用 OS 线程，
        // 线程内用 current-thread runtime 驱动 store 的 async trait，避免阻塞 TUI 主循环。
        thread::spawn(move || run_session_worker(command_receiver, event_sender));
        Self {
            command_sender,
            event_receiver,
            pending_commands: 0,
            pending_mutations: 0,
        }
    }

    pub(super) fn has_pending_work(&self) -> bool {
        self.pending_commands > 0
    }

    pub(super) fn has_pending_mutation(&self) -> bool {
        self.pending_mutations > 0
    }

    pub(super) fn drain_events(&mut self) -> Vec<SessionStoreWorkerEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.event_receiver.try_recv() {
            self.mark_event_drained(&event);
            events.push(event);
        }

        if events.is_empty()
            && self.pending_commands > 0
            && let Ok(event) = self.event_receiver.recv_timeout(SESSION_EVENT_DRAIN_WAIT)
        {
            self.mark_event_drained(&event);
            events.push(event);
            while let Ok(event) = self.event_receiver.try_recv() {
                self.mark_event_drained(&event);
                events.push(event);
            }
        }

        events
    }

    pub(super) fn list_sessions(
        &mut self,
        store: Arc<dyn SessionStore>,
        project_dir: ProjectDir,
        active_session_id: Option<SessionId>,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::ListSessions {
                store,
                project_dir,
                active_session_id,
            },
            false,
        )
    }

    pub(super) fn load_session_preview(
        &mut self,
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::LoadSessionPreview { store, session_id },
            false,
        )
    }

    pub(super) fn resume_session(
        &mut self,
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::ResumeSession {
                store,
                header,
                session_id,
            },
            true,
        )
    }

    pub(super) fn load_session_tree(
        &mut self,
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        consumer: SessionTreeLoadConsumer,
        request_id: SessionLoadRequestId,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::LoadSessionTree {
                store,
                session_id,
                request_id,
                consumer,
            },
            false,
        )
    }

    pub(super) fn load_branch_tree(
        &mut self,
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::LoadBranchTree {
                store,
                session_id,
                request_id,
            },
            false,
        )
    }

    pub(super) fn load_branch_preview(
        &mut self,
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
        branch_row_id: String,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::LoadBranchPreview {
                store,
                session_id,
                request_id,
                branch_row_id,
            },
            false,
        )
    }

    pub(super) fn switch_branch(
        &mut self,
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
        request_id: SessionLoadRequestId,
        leaf_id: String,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::SwitchBranch {
                store,
                header,
                session_id,
                request_id,
                leaf_id,
            },
            true,
        )
    }

    pub(super) fn select_entry_rewind(
        &mut self,
        store: Arc<dyn SessionStore>,
        header: SessionHeader,
        session_id: SessionId,
        entry_id: String,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::SelectEntryRewind {
                store,
                header,
                session_id,
                entry_id,
            },
            true,
        )
    }

    pub(super) fn set_leaf(
        &mut self,
        store: Arc<dyn SessionStore>,
        session_id: SessionId,
        leaf_id: String,
    ) -> Result<(), String> {
        self.send_command(
            SessionStoreCommand::SetLeaf {
                store,
                session_id,
                leaf_id,
            },
            true,
        )
    }

    pub(super) fn flush_all(&self, store: Arc<dyn SessionStore>) -> Result<(), String> {
        let (ack, receiver) = mpsc::channel();
        self.command_sender
            .send(SessionStoreCommand::FlushAll { store, ack })
            .map_err(|_| "session store worker stopped".to_string())?;
        receiver
            .recv_timeout(SESSION_SHUTDOWN_WAIT)
            .map_err(|_| "session store flush timed out".to_string())?
    }

    fn send_command(
        &mut self,
        command: SessionStoreCommand,
        is_mutation: bool,
    ) -> Result<(), String> {
        self.command_sender
            .send(command)
            .map_err(|_| "session store worker stopped".to_string())?;
        self.pending_commands = self.pending_commands.saturating_add(1);
        if is_mutation {
            self.pending_mutations = self.pending_mutations.saturating_add(1);
        }
        Ok(())
    }

    fn mark_event_drained(&mut self, event: &SessionStoreWorkerEvent) {
        if event.completes_command() {
            self.pending_commands = self.pending_commands.saturating_sub(1);
            if event.is_mutation_result() {
                self.pending_mutations = self.pending_mutations.saturating_sub(1);
            }
        }
    }
}

impl SessionStoreWorkerEvent {
    fn runtime(event: RuntimeEvent) -> Self {
        Self::Runtime {
            event,
            completes_command: true,
            is_mutation: false,
        }
    }

    fn runtime_mutation(event: RuntimeEvent) -> Self {
        Self::Runtime {
            event,
            completes_command: true,
            is_mutation: true,
        }
    }

    fn runtime_progress(event: RuntimeEvent) -> Self {
        Self::Runtime {
            event,
            completes_command: false,
            is_mutation: false,
        }
    }

    fn completes_command(&self) -> bool {
        match self {
            Self::Runtime {
                completes_command, ..
            } => *completes_command,
            Self::Restored { .. }
            | Self::RestoredWithTree { .. }
            | Self::Noop
            | Self::Failed { .. } => true,
        }
    }

    fn is_mutation_result(&self) -> bool {
        matches!(
            self,
            Self::Runtime {
                completes_command: true,
                is_mutation: true,
                ..
            } | Self::Restored { .. }
                | Self::RestoredWithTree { .. }
                | Self::Noop
                | Self::Failed {
                    is_mutation: true,
                    ..
                }
        )
    }
}

fn run_session_worker(
    command_receiver: Receiver<SessionStoreCommand>,
    event_sender: Sender<SessionStoreWorkerEvent>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = event_sender.send(SessionStoreWorkerEvent::Failed {
                message: format!("start session store worker runtime: {error}"),
                is_mutation: false,
            });
            return;
        }
    };

    while let Ok(command) = command_receiver.recv() {
        if let SessionStoreCommand::FlushAll { store, ack } = command {
            let result = runtime
                .block_on(store.flush_all())
                .map_err(|error| error.to_string());
            let _ = ack.send(result);
            continue;
        }

        if let SessionStoreCommand::ListSessions {
            store,
            project_dir,
            active_session_id,
        } = command
        {
            runtime.block_on(handle_list_sessions_command(
                store,
                project_dir,
                active_session_id,
                &event_sender,
            ));
            continue;
        }

        let event = runtime.block_on(handle_session_command(command));
        let _ = event_sender.send(event);
    }
}

async fn handle_list_sessions_command(
    store: Arc<dyn SessionStore>,
    project_dir: ProjectDir,
    active_session_id: Option<SessionId>,
    event_sender: &Sender<SessionStoreWorkerEvent>,
) {
    // session picker 先显示 SQLite 缓存结果，再用 repair 后的结果刷新，避免扫盘阻塞入口。
    let initial_rows = match list_session_rows(
        store.as_ref(),
        &project_dir,
        active_session_id.as_ref(),
        SessionListOptions::default(),
    )
    .await
    {
        Ok(rows) => rows,
        Err(error) => {
            let _ = event_sender.send(failed(error.to_string(), false));
            return;
        }
    };
    let _ = event_sender.send(SessionStoreWorkerEvent::runtime_progress(
        RuntimeEvent::SessionListLoaded {
            rows: initial_rows.clone(),
        },
    ));

    match list_session_rows(
        store.as_ref(),
        &project_dir,
        active_session_id.as_ref(),
        SessionListOptions { repair: true },
    )
    .await
    {
        Ok(repaired_rows) if repaired_rows != initial_rows => {
            let _ = event_sender.send(SessionStoreWorkerEvent::runtime(
                RuntimeEvent::SessionListLoaded {
                    rows: repaired_rows,
                },
            ));
        }
        Ok(_) => {
            let _ = event_sender.send(SessionStoreWorkerEvent::Noop);
        }
        Err(error) => {
            let _ = event_sender.send(failed(error.to_string(), false));
        }
    }
}

async fn handle_session_command(command: SessionStoreCommand) -> SessionStoreWorkerEvent {
    match command {
        SessionStoreCommand::ListSessions { .. } => {
            unreachable!("list sessions is handled by the worker loop")
        }
        SessionStoreCommand::LoadSessionPreview { store, session_id } => {
            match store.load_session(&session_id, None).await {
                Ok(restored_state) => {
                    SessionStoreWorkerEvent::runtime(RuntimeEvent::SessionPreviewLoaded {
                        payload: session_preview_payload(session_id, restored_state),
                    })
                }
                Err(error) => failed(error.to_string(), false),
            }
        }
        SessionStoreCommand::ResumeSession {
            store,
            header,
            session_id,
        } => match restore_conversation(store, header, session_id, None).await {
            Ok((conversation, payload)) => SessionStoreWorkerEvent::Restored {
                conversation,
                payload,
            },
            Err(message) => failed(message, true),
        },
        SessionStoreCommand::LoadSessionTree {
            store,
            session_id,
            request_id,
            consumer,
        } => match store.load_session_tree(&session_id).await {
            Ok(snapshot) => SessionStoreWorkerEvent::runtime(
                consumer.loaded_event(request_id, session_tree_payload(snapshot)),
            ),
            Err(error) => SessionStoreWorkerEvent::runtime(
                consumer.failed_event(request_id, error.to_string()),
            ),
        },
        SessionStoreCommand::LoadBranchTree {
            store,
            session_id,
            request_id,
        } => match store.load_session_branch_tree(&session_id).await {
            Ok(snapshot) => {
                SessionStoreWorkerEvent::runtime(RuntimeEvent::SessionBranchTreeLoaded {
                    request_id,
                    payload: session_branch_tree_payload(snapshot),
                })
            }
            Err(error) => {
                SessionStoreWorkerEvent::runtime(RuntimeEvent::SessionBranchTreeLoadFailed {
                    request_id,
                    message: error.to_string(),
                })
            }
        },
        SessionStoreCommand::LoadBranchPreview {
            store,
            session_id,
            request_id,
            branch_row_id,
        } => match store
            .load_session_branch_preview(&session_id, &branch_row_id)
            .await
        {
            Ok(snapshot) => {
                SessionStoreWorkerEvent::runtime(RuntimeEvent::SessionBranchPreviewLoaded {
                    request_id,
                    payload: session_tree_payload(snapshot),
                })
            }
            Err(error) => {
                SessionStoreWorkerEvent::runtime(RuntimeEvent::SessionBranchPreviewLoadFailed {
                    request_id,
                    message: error.to_string(),
                })
            }
        },
        SessionStoreCommand::SwitchBranch {
            store,
            header,
            session_id,
            request_id,
            leaf_id,
        } => match switch_branch(store, header, session_id, leaf_id).await {
            Ok((conversation, resume_payload, tree_payload)) => {
                SessionStoreWorkerEvent::RestoredWithTree {
                    conversation,
                    resume_payload,
                    tree_request_id: request_id,
                    tree_payload,
                }
            }
            Err(message) => {
                SessionStoreWorkerEvent::runtime_mutation(RuntimeEvent::SessionBranchSwitchFailed {
                    request_id,
                    message,
                })
            }
        },
        SessionStoreCommand::SelectEntryRewind {
            store,
            header,
            session_id,
            entry_id,
        } => match select_entry_rewind(store, header, session_id, entry_id).await {
            Ok(Some((conversation, payload))) => SessionStoreWorkerEvent::Restored {
                conversation,
                payload,
            },
            Ok(None) => SessionStoreWorkerEvent::Noop,
            Err(message) => failed(message, true),
        },
        SessionStoreCommand::SetLeaf {
            store,
            session_id,
            leaf_id,
        } => match store.set_leaf(&session_id, Some(&leaf_id)).await {
            Ok(()) => SessionStoreWorkerEvent::Noop,
            Err(error) => failed(error.to_string(), true),
        },
        SessionStoreCommand::FlushAll { .. } => unreachable!("flush is handled by worker loop"),
    }
}

async fn list_session_rows(
    store: &dyn SessionStore,
    project_dir: &ProjectDir,
    active_session_id: Option<&SessionId>,
    options: SessionListOptions,
) -> Result<Vec<SessionPickerRow>, session_store::SessionStoreError> {
    let metas = store.list_sessions(project_dir, options).await?;
    Ok(metas
        .into_iter()
        .filter(|meta| active_session_id.is_none_or(|session_id| meta.session_id != *session_id))
        .map(session_picker_row_from_meta)
        .collect())
}

async fn restore_conversation(
    store: Arc<dyn SessionStore>,
    header: SessionHeader,
    session_id: SessionId,
    leaf_id: Option<&str>,
) -> Result<(ProviderConversation, SessionResumePayload), String> {
    let restored_state = store
        .load_session(&session_id, leaf_id)
        .await
        .map_err(|error| error.to_string())?;
    let conversation = ProviderConversation::with_resolved_session_store(
        store,
        header,
        Some(session_id.clone()),
        &restored_state,
    )
    .map_err(|error| error.to_string())?;
    let payload = session_resume_payload(session_id, restored_state);
    Ok((conversation, payload))
}

async fn switch_branch(
    store: Arc<dyn SessionStore>,
    header: SessionHeader,
    session_id: SessionId,
    leaf_id: String,
) -> Result<
    (
        ProviderConversation,
        SessionResumePayload,
        SessionTreePayload,
    ),
    String,
> {
    let (conversation, resume_payload) =
        restore_conversation(store.clone(), header, session_id.clone(), Some(&leaf_id)).await?;
    let tree_snapshot = store
        .load_session_tree_for_leaf(&session_id, &leaf_id)
        .await
        .map_err(|error| error.to_string())?;
    store
        .set_leaf(&session_id, Some(&leaf_id))
        .await
        .map_err(|error| error.to_string())?;
    Ok((
        conversation,
        resume_payload,
        session_tree_payload(tree_snapshot),
    ))
}

async fn select_entry_rewind(
    store: Arc<dyn SessionStore>,
    header: SessionHeader,
    session_id: SessionId,
    entry_id: String,
) -> Result<Option<(ProviderConversation, SessionResumePayload)>, String> {
    let snapshot = store
        .load_session_tree(&session_id)
        .await
        .map_err(|error| error.to_string())?;
    let selected_row = snapshot
        .rows
        .iter()
        .find(|row| row.id == entry_id)
        .ok_or_else(|| format!("Tree row `{entry_id}` was not found"))?;
    let Some(rewind_target_id) = selected_row.rewind_target_id.as_deref() else {
        return Ok(None);
    };
    let rewind_target_id = rewind_target_id.to_string();
    let (conversation, payload) = restore_conversation(
        store.clone(),
        header,
        session_id.clone(),
        Some(&rewind_target_id),
    )
    .await?;
    store
        .set_leaf(&session_id, Some(&rewind_target_id))
        .await
        .map_err(|error| error.to_string())?;
    Ok(Some((conversation, payload)))
}

fn failed(message: String, is_mutation: bool) -> SessionStoreWorkerEvent {
    SessionStoreWorkerEvent::Failed {
        message,
        is_mutation,
    }
}
