use std::{future::Future, pin::Pin, sync::Arc};

use provider_protocol::ConversationItem;
use runtime_domain::session::TranscriptReplayItem;

use crate::{
    ConfigSnapshot, ProjectDir, ResolvedSessionState, SessionBranchTreeSnapshot, SessionEntry,
    SessionEntryKind, SessionHeader, SessionId, SessionListOptions, SessionMeta, SessionStoreError,
    SessionTreeSnapshot, generate_entry_id, resolve as resolve_entries, resolve_state,
    session_branch_preview_snapshot, session_branch_tree_snapshot, session_tree_snapshot,
    session_tree_snapshot_for_leaf,
};

use super::super::{
    SessionStore, current_timestamp_ms, latest_non_leaf_id, requested_leaf_id, resolve_error,
};
use super::{
    LocalSessionHandle, LocalSessionStore, evict_idle_recorders, flush_handle, session_jsonl_path,
};

impl SessionStore for LocalSessionStore {
    fn create_session<'a>(
        &'a self,
        header: SessionHeader,
    ) -> Pin<Box<dyn Future<Output = Result<SessionId, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = SessionId::new();
            let mut header = header;
            header.session_id = session_id.clone();

            let jsonl_path = session_jsonl_path(&self.hunea_dir, &header.work_dir, &session_id);
            let header_entry = SessionEntry {
                id: "header".to_string(),
                parent_id: None,
                timestamp: current_timestamp_ms()?,
                kind: SessionEntryKind::Header(header),
            };
            let handle = Arc::new(LocalSessionHandle::new(
                jsonl_path.clone(),
                vec![header_entry.clone()],
            )?);

            {
                let _guard = handle.operation_lock.lock().await;
                handle.recorder.buffer(header_entry)?;
                handle.recorder.persist().await?;
            }

            let meta = handle.lock_state().session_meta.clone();
            let evicted_handles = {
                let mut recorders = self.recorders.write().await;
                recorders.insert(session_id.clone(), handle);
                evict_idle_recorders(&mut recorders, &session_id)
            };
            self.shutdown_evicted_recorders(evicted_handles).await?;
            self.index.upsert_session(&meta).await?;

            Ok(session_id)
        })
    }

    fn append<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: ConversationItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entry(session_id, SessionEntryKind::Item(item))
                .await
        })
    }

    fn append_many<'a>(
        &'a self,
        session_id: &'a SessionId,
        items: Vec<ConversationItem>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entries(
                session_id,
                items.into_iter().map(SessionEntryKind::Item).collect(),
            )
            .await
        })
    }

    fn append_config_change<'a>(
        &'a self,
        session_id: &'a SessionId,
        snapshot: ConfigSnapshot,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entry(session_id, SessionEntryKind::ConfigChange(snapshot))
                .await
                .map(|_| ())
        })
    }

    fn append_transcript_replay<'a>(
        &'a self,
        session_id: &'a SessionId,
        item: TranscriptReplayItem,
    ) -> Pin<Box<dyn Future<Output = Result<String, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            self.append_entry(session_id, SessionEntryKind::TranscriptReplay(item))
                .await
        })
    }

    fn set_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf_id = leaf_id.map(str::to_string);
            let handle = self.handle_for_session(&session_id).await?;
            let _guard = handle.operation_lock.lock().await;
            handle.recover_pending_state_entries().await?;

            let entry = {
                let state = handle.lock_state();
                if state.entries.is_empty() {
                    return Err(SessionStoreError::SessionNotFound { session_id });
                }

                if let Some(leaf_id) = requested_leaf_id.as_deref() {
                    state.require_existing_entry(leaf_id)?;
                }

                SessionEntry {
                    id: generate_entry_id(&state.entry_ids),
                    parent_id: latest_non_leaf_id(&state.entries),
                    timestamp: current_timestamp_ms()?,
                    kind: SessionEntryKind::Leaf {
                        target_id: requested_leaf_id.clone(),
                    },
                }
            };

            handle.recorder.buffer(entry.clone())?;
            if let Err(error) = handle.recorder.persist().await {
                let mut state = handle.lock_state();
                state.push_pending_state_entry(entry)?;
                return Err(error);
            }

            let meta = {
                let mut state = handle.lock_state();
                state.push_entry(entry, &handle.jsonl_path)?;
                state.session_meta.clone()
            };

            self.index.upsert_session(&meta).await
        })
    }

    fn resolve<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<ConversationItem>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf = leaf_id.map(str::to_string);
            let handle = self.handle_for_session(&session_id).await?;
            let state = handle.lock_state();
            let requested_leaf_id =
                requested_leaf_id(state.entries.as_slice(), requested_leaf.as_deref())?;
            resolve_entries(&state.entries, requested_leaf_id).map_err(resolve_error)
        })
    }

    fn load_session<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<ResolvedSessionState, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf = leaf_id.map(str::to_string);
            let handle = self.handle_for_session(&session_id).await?;
            let state = handle.lock_state();
            let requested_leaf_id =
                requested_leaf_id(state.entries.as_slice(), requested_leaf.as_deref())?;
            resolve_state(&state.entries, requested_leaf_id).map_err(resolve_error)
        })
    }

    fn load_session_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let handle = self.handle_for_session(&session_id).await?;
            let state = handle.lock_state();
            session_tree_snapshot(&state.entries).map_err(resolve_error)
        })
    }

    fn load_session_tree_for_leaf<'a>(
        &'a self,
        session_id: &'a SessionId,
        leaf_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_leaf = leaf_id.to_string();
            let handle = self.handle_for_session(&session_id).await?;
            let state = handle.lock_state();
            session_tree_snapshot_for_leaf(&state.entries, &requested_leaf).map_err(resolve_error)
        })
    }

    fn load_session_branch_preview<'a>(
        &'a self,
        session_id: &'a SessionId,
        branch_row_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<SessionTreeSnapshot, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = session_id.clone();
            let requested_branch = branch_row_id.to_string();
            let handle = self.handle_for_session(&session_id).await?;
            let state = handle.lock_state();
            session_branch_preview_snapshot(&state.entries, &requested_branch)
                .map_err(resolve_error)
        })
    }

    fn load_session_branch_tree<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<
        Box<dyn Future<Output = Result<SessionBranchTreeSnapshot, SessionStoreError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let session_id = session_id.clone();
            let handle = self.handle_for_session(&session_id).await?;
            let state = handle.lock_state();
            session_branch_tree_snapshot(&state.entries).map_err(resolve_error)
        })
    }

    fn list_sessions<'a>(
        &'a self,
        project_dir: &'a ProjectDir,
        options: SessionListOptions,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<SessionMeta>, SessionStoreError>> + Send + 'a>>
    {
        Box::pin(async move { self.index.list_sessions(project_dir, options).await })
    }

    fn get_session_meta<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<SessionMeta, SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = session_id.clone();
            if let Some(handle) = self.recorders.read().await.get(&session_id).cloned() {
                return Ok(handle.lock_state().session_meta.clone());
            }

            self.index.get_session_meta(&session_id.to_string()).await
        })
    }

    fn flush<'a>(
        &'a self,
        session_id: &'a SessionId,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let session_id = session_id.clone();
            let handle = self.handle_for_session(&session_id).await?;
            let _guard = handle.operation_lock.lock().await;
            handle.recorder.flush().await?;
            let meta = {
                let mut state = handle.lock_state();
                state.commit_pending_state_entries(&handle.jsonl_path)?;
                state.session_meta.clone()
            };
            self.index.upsert_session(&meta).await
        })
    }

    fn flush_all<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), SessionStoreError>> + Send + 'a>> {
        Box::pin(async move {
            let handles = self
                .recorders
                .read()
                .await
                .values()
                .cloned()
                .collect::<Vec<_>>();
            for handle in handles {
                let meta = flush_handle(&handle).await?;
                self.index.upsert_session(&meta).await?;
            }
            Ok(())
        })
    }
}
