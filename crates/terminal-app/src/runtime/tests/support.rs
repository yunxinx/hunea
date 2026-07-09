pub(super) use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) use super::super::{
    AppRuntimeCoordinator, AppRuntimeOptions, ensure_conversation_target,
    should_defer_runtime_event_for_render_barrier,
};
pub(super) use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
pub(super) use runtime_domain::{
    model_catalog::ModelSelection,
    provider::ProviderKind,
    session::{
        ConversationTurnRequest, RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent,
        RuntimePermissionRequest, RuntimeTarget, RuntimeToolActivity, RuntimeToolActivityContent,
        RuntimeToolActivityRawValue, RuntimeToolActivityStatus, RuntimeToolKind,
        SessionBranchTreePayload, SessionLoadRequestId, SessionPickerRow, SessionPreviewPayload,
        SessionResumePayload, SessionTreePayload, SessionTreeRowKind, TranscriptReplayItem,
        TranscriptReplayRole,
    },
};
pub(super) use session_store::{
    ConfigSnapshot, InMemorySessionStore, LocalSessionStore, ProjectDir, SessionEntry,
    SessionEntryKind, SessionFlushStore, SessionHeader, SessionId, SessionLifecycleStore,
    SessionStore, SessionStoreError, session_filename,
};
pub(super) use terminal_ui::RuntimeCoordinator;

pub(super) fn runtime_coordinator(options: AppRuntimeOptions) -> AppRuntimeCoordinator {
    AppRuntimeCoordinator::new(options).expect("runtime coordinator should initialize")
}

pub(super) const fn request_id(value: u64) -> SessionLoadRequestId {
    SessionLoadRequestId::new(value)
}

pub(super) use super::store_fixtures::{
    CommittedLoadFailsAfterSetLeafStore, DelayedListSessionStore, FailingSessionStore,
    FailingSessionStoreLoad, LoadCountingSessionStore,
};

pub(super) fn temp_test_dir(prefix: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&root).expect("create temp root");
    root
}

pub(super) fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

pub(super) fn write_external_session_jsonl(
    hunea_dir: &Path,
    work_dir: &Path,
    session_id: &SessionId,
    user_text: &str,
) {
    let project_dir = hunea_dir
        .join("sessions")
        .join(ProjectDir::from_work_dir(work_dir).encoded_session_dir());
    fs::create_dir_all(&project_dir).expect("external project session dir should be creatable");
    let path = project_dir.join(session_filename(session_id));
    let entries = [
        SessionEntry {
            id: "header".to_string(),
            parent_id: None,
            timestamp: 1_717_514_800_000,
            kind: SessionEntryKind::Header(SessionHeader {
                session_id: session_id.clone(),
                work_dir: work_dir.to_path_buf(),
                session_name: Some("external session".to_string()),
                initial_model: "qwen3".to_string(),
                git_head: None,
                cli_version: None,
            }),
        },
        SessionEntry {
            id: "user-1".to_string(),
            parent_id: Some("header".to_string()),
            timestamp: 1_717_514_800_100,
            kind: SessionEntryKind::Item(ConversationItem::text(Role::User, user_text)),
        },
    ];
    let mut contents = String::new();
    for entry in entries {
        contents.push_str(&serde_json::to_string(&entry).expect("entry should serialize"));
        contents.push('\n');
    }
    fs::write(path, contents).expect("external session jsonl should be writable");
}

pub(super) fn wait_for_session_list_rows(
    coordinator: &mut AppRuntimeCoordinator,
) -> Vec<SessionPickerRow> {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionListLoaded { rows } => Some(rows),
            _ => None,
        },
        "session list rows",
    )
}

pub(super) fn wait_for_session_preview(
    coordinator: &mut AppRuntimeCoordinator,
) -> SessionPreviewPayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionPreviewLoaded { payload } => Some(payload),
            _ => None,
        },
        "session preview payload",
    )
}

pub(super) fn wait_for_session_resumed(
    coordinator: &mut AppRuntimeCoordinator,
) -> SessionResumePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionResumed { payload } => Some(payload),
            _ => None,
        },
        "session resumed payload",
    )
}

pub(super) fn wait_for_session_tree(coordinator: &mut AppRuntimeCoordinator) -> SessionTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionTreeLoaded { payload, .. } => Some(payload),
            _ => None,
        },
        "session tree payload",
    )
}

pub(super) fn wait_for_copy_picker_tree(
    coordinator: &mut AppRuntimeCoordinator,
) -> SessionTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::CopyPickerTreeLoaded { payload, .. } => Some(payload),
            _ => None,
        },
        "copy picker tree payload",
    )
}

pub(super) fn wait_for_session_branch_tree(
    coordinator: &mut AppRuntimeCoordinator,
) -> SessionBranchTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionBranchTreeLoaded { payload, .. } => Some(payload),
            _ => None,
        },
        "session branch tree payload",
    )
}

pub(super) fn wait_for_session_tree_preview(
    coordinator: &mut AppRuntimeCoordinator,
) -> SessionTreePayload {
    wait_for_runtime_event(
        coordinator,
        |event| match event {
            RuntimeEvent::SessionBranchPreviewLoaded { payload, .. } => Some(payload),
            _ => None,
        },
        "session tree preview payload",
    )
}

pub(super) fn wait_for_runtime_events(
    coordinator: &mut AppRuntimeCoordinator,
    expected: &str,
) -> Vec<RuntimeEvent> {
    for _ in 0..100 {
        let events = RuntimeCoordinator::drain_runtime_events(coordinator);
        if !events.is_empty() {
            return events;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{expected} should be emitted");
}

pub(super) fn wait_for_runtime_idle(coordinator: &mut AppRuntimeCoordinator) {
    for _ in 0..100 {
        let events = RuntimeCoordinator::drain_runtime_events(coordinator);
        assert!(
            events.is_empty(),
            "runtime should not emit events while waiting for no-op command: {events:?}"
        );
        if !RuntimeCoordinator::has_background_runtime(coordinator) {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("runtime should become idle");
}

pub(super) fn assert_no_runtime_events(coordinator: &mut AppRuntimeCoordinator, message: &str) {
    assert_eq!(
        RuntimeCoordinator::drain_runtime_events(coordinator),
        Vec::<RuntimeEvent>::new(),
        "{message}"
    );
}

pub(super) fn wait_for_runtime_event<T>(
    coordinator: &mut AppRuntimeCoordinator,
    mut select: impl FnMut(RuntimeEvent) -> Option<T>,
    expected: &str,
) -> T {
    for _ in 0..100 {
        for event in RuntimeCoordinator::drain_runtime_events(coordinator) {
            if let Some(value) = select(event) {
                return value;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("{expected} should be emitted");
}
