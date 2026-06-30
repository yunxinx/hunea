use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use provider_protocol::{ContentBlock, ConversationItem, Role, ToolCall};
use runtime_domain::prompt_assembly::{
    PromptPreludeSection, PromptPreludeSnapshot, PromptSourceKind, PromptSourceOrigin,
};
use session_store::{
    InMemorySessionStore, SessionHeader, SessionId, SessionStore, SessionStoreError,
};

use super::{PersistedConversationItem, ProviderConversation, ProviderConversationError};
use crate::ProviderKind;
use runtime_domain::session::{
    ConversationTurnRequest, TranscriptReplayItem, TranscriptReplayRole, TranscriptUserMessage,
};

#[test]
fn prepare_turn_uses_session_history_and_current_user_message() {
    let mut session = ProviderConversation::new();
    session.commit_turn_items([cached_item(ConversationItem::text(
        Role::Assistant,
        "first answer",
    ))]);

    let request = session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "follow up"),
        ))
        .expect("turn should prepare");

    let visible_text = request
        .items()
        .iter()
        .map(ConversationItem::text_content)
        .collect::<Vec<_>>();
    assert_eq!(visible_text, vec!["first answer", "follow up"]);
    assert_eq!(session.history().len(), 1);
}

#[test]
fn history_exposes_borrowed_items_without_collecting_owned_history() {
    let mut session = ProviderConversation::new();
    session.commit_turn_items([cached_item(ConversationItem::text(Role::User, "hello"))]);

    let mut history = session.history();

    assert_eq!(history.len(), 1);
    assert_eq!(
        history.next().map(ConversationItem::text_content),
        Some("hello".to_string())
    );
    assert_eq!(history.len(), 0);
}

#[test]
fn prepare_turn_prepends_system_prompt_without_persisting_it_in_history() {
    let mut session = ProviderConversation::new();
    session.set_system_prompt(Some("You are helpful".to_string()));

    let request = session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        ))
        .expect("turn should prepare");

    assert_eq!(request.items()[0].role(), Some(Role::System));
    assert_eq!(request.items()[0].text_content(), "You are helpful");
    assert!(session.is_history_empty());
}

#[test]
fn prepare_turn_uses_prompt_prelude_effective_system_prompt() {
    let mut session = ProviderConversation::new();
    session.set_prompt_prelude(Some(PromptPreludeSnapshot {
        sections: vec![
            PromptPreludeSection {
                reference_id: "core-system".to_string(),
                kind: PromptSourceKind::CoreSystemPrompt,
                title: "Core system prompt".to_string(),
                origin: Some(PromptSourceOrigin::Builtin),
                body: "core guidance".to_string(),
            },
            PromptPreludeSection {
                reference_id: "repo-rules".to_string(),
                kind: PromptSourceKind::ExtraPrompt,
                title: "repo-rules".to_string(),
                origin: Some(PromptSourceOrigin::Project),
                body: "project rules".to_string(),
            },
        ],
    }));

    let request = session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "hello"),
        ))
        .expect("turn should prepare");

    assert_eq!(request.items()[0].role(), Some(Role::System));
    assert_eq!(
        request.items()[0].text_content(),
        "core guidance\n\nproject rules"
    );
}

#[test]
fn prepare_turn_rejects_non_user_message() {
    let mut session = ProviderConversation::new();

    let error = session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::Assistant, "not a user turn"),
        ))
        .expect_err("assistant turn should be rejected");

    assert!(matches!(
        error,
        ProviderConversationError::NonUserTurnMessage
    ));
}

#[test]
fn truncate_after_user_turns_keeps_provider_context_before_selected_turn() {
    let mut session = ProviderConversation::new();
    session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "first question"),
        ))
        .expect("first turn should prepare");
    assert!(session.commit_pending_user(None, None));
    session.commit_turn_items([cached_item(ConversationItem::text(
        Role::Assistant,
        "first answer",
    ))]);
    session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "second question"),
        ))
        .expect("second turn should prepare");
    assert!(session.commit_pending_user(None, None));
    session.commit_turn_items([cached_item(ConversationItem::text(
        Role::Assistant,
        "second answer",
    ))]);

    session
        .truncate_after_user_turns(1)
        .expect("truncate should succeed");

    let visible_text = session
        .history()
        .map(ConversationItem::text_content)
        .collect::<Vec<_>>();
    assert_eq!(visible_text, vec!["first question", "first answer"]);
}

#[test]
fn rollback_pending_user_discards_unstarted_turn() {
    let mut session = ProviderConversation::new();
    session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "never sent"),
        ))
        .expect("turn should prepare");

    assert!(session.rollback_pending_user());
    assert!(session.is_history_empty());
}

#[test]
fn commit_pending_user_persists_started_turn_once() {
    let mut session = ProviderConversation::new();
    session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "sent"),
        ))
        .expect("turn should prepare");

    assert!(session.commit_pending_user(None, None));
    assert!(!session.commit_pending_user(None, None));
    assert_eq!(
        session.history().next().map(ConversationItem::text_content),
        Some("sent".to_string())
    );
    assert_eq!(session.history().len(), 1);
}

#[test]
fn commit_turn_items_keeps_tool_items_for_future_turns() {
    let mut session = ProviderConversation::new();
    session.commit_turn_items([
        cached_item(ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new(
                "call-1",
                "read",
                r#"{"path":"Cargo.toml"}"#.to_string(),
            )],
        )),
        cached_item(ConversationItem::tool_result(
            "call-1",
            vec![ContentBlock::Text("1\t[package]".to_string())],
            false,
        )),
        cached_item(ConversationItem::text(Role::Assistant, "done")),
    ]);

    let request = session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "next"),
        ))
        .expect("turn should prepare");

    assert_eq!(request.items().len(), 4);
    assert_eq!(request.items()[0].role(), Some(Role::Assistant));
    assert!(matches!(
        &request.items()[1],
        ConversationItem::ToolResult {
            is_error: false,
            ..
        }
    ));
}

#[test]
fn prepare_turn_with_transcript_keeps_provider_and_transcript_user_messages_separate() {
    let work_dir = PathBuf::from("/tmp/hunea-provider-conversation");
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut conversation =
        ProviderConversation::with_session_store(store, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let turn = ConversationTurnRequest::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "qwen3",
        Some("http://127.0.0.1:1234/v1".to_string()),
        None,
        None,
        ConversationItem::text(
            Role::User,
            "<skill>\n<name>code-review</name>\nbody\n</skill>\n\nraw user message",
        ),
    );
    let transcript_user_message = TranscriptUserMessage {
        content: "raw user message".to_string(),
        skill_bindings: Vec::new(),
    };
    let transcript_replay_after_user = vec![TranscriptReplayItem::Message {
        role: TranscriptReplayRole::Assistant,
        content: "synthetic replay".to_string(),
    }];

    let request = conversation
        .prepare_turn_with_transcript(
            &turn,
            Some(transcript_user_message.clone()),
            transcript_replay_after_user.clone(),
        )
        .expect("turn should prepare");
    let persistence = request
        .persistence_cloned()
        .expect("persistence should be attached");

    assert_eq!(
        request.items().last().map(ConversationItem::text_content),
        Some("<skill>\n<name>code-review</name>\nbody\n</skill>\n\nraw user message".to_string())
    );
    assert_eq!(
        persistence.current_user_message.text_content(),
        "<skill>\n<name>code-review</name>\nbody\n</skill>\n\nraw user message"
    );
    assert_eq!(persistence.transcript_user_message, transcript_user_message);
    assert_eq!(
        persistence.transcript_replay_after_user,
        transcript_replay_after_user
    );
}

#[test]
fn persisted_conversation_loads_resolved_history_for_prepare_turn() {
    let work_dir = tempdir_path("resolved-history");
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
        .expect("session should be created");
    let existing_items = vec![
        ConversationItem::text(Role::User, "hello"),
        ConversationItem::text(Role::Assistant, "hi"),
    ];
    for item in &existing_items {
        block_on_session(store.append(&session_id, item.clone()))
            .expect("history item should persist");
    }
    let restored_state =
        block_on_session(store.load_session(&session_id, None)).expect("session state should load");
    let store_trait: Arc<dyn SessionStore> = store;
    let mut session = ProviderConversation::with_resolved_session_store(
        store_trait,
        sample_header(&work_dir, "qwen3"),
        Some(session_id),
        &restored_state,
    )
    .expect("persisted conversation should load history");

    let request = session
        .prepare_turn(&ConversationTurnRequest::new(
            "local",
            ProviderKind::OpenAiCompatible,
            "qwen3",
            Some("http://127.0.0.1:1234/v1".to_string()),
            None,
            None,
            ConversationItem::text(Role::User, "follow up"),
        ))
        .expect("turn should prepare from resolved history");

    let visible_text = request
        .items()
        .iter()
        .map(ConversationItem::text_content)
        .collect::<Vec<_>>();
    assert_eq!(visible_text, vec!["hello", "hi", "follow up"]);
    assert!(session.history().eq(existing_items.iter()));
}

#[test]
fn append_items_keeps_provider_history_in_sync() {
    let work_dir = tempdir_path("append-items");
    let store_trait: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::new());
    let mut session =
        ProviderConversation::with_session_store(store_trait, sample_header(&work_dir, "qwen3"))
            .expect("persisted conversation should initialize");
    let items = vec![
        ConversationItem::text(Role::User, "question"),
        ConversationItem::assistant_with_tool_calls(
            String::new(),
            vec![ToolCall::new("call-1", "read", "{}")],
        ),
        ConversationItem::tool_result(
            "call-1",
            vec![ContentBlock::Text("done".to_string())],
            false,
        ),
    ];

    session
        .append_items(items.clone())
        .expect("items should append to provider history");

    assert!(session.history().eq(items.iter()));
}

#[test]
fn persisted_conversation_restores_latest_system_prompt_snapshot() {
    let work_dir = tempdir_path("restored-config");
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
        .expect("session should be created");
    block_on_session(store.append(&session_id, ConversationItem::text(Role::User, "hello")))
        .expect("user item should persist");
    block_on_session(store.append_config_change(
        &session_id,
        session_store::ConfigSnapshot {
            provider_id: "local".to_string(),
            model: "qwen3".to_string(),
            system_prompt: Some("keep it sharp".to_string()),
            prompt_prelude: None,
        },
    ))
    .expect("config snapshot should persist");

    let restored_state =
        block_on_session(store.load_session(&session_id, None)).expect("session state should load");
    let store_trait: Arc<dyn SessionStore> = store;
    let session = ProviderConversation::with_resolved_session_store(
        store_trait,
        sample_header(&work_dir, "qwen3"),
        Some(session_id),
        &restored_state,
    )
    .expect("persisted conversation should load config");

    assert_eq!(session.system_prompt(), Some("keep it sharp"));
}

#[test]
fn persisted_conversation_restores_prompt_prelude_snapshot() {
    let work_dir = tempdir_path("restored-prelude");
    let store = Arc::new(InMemorySessionStore::new());
    let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
        .expect("session should be created");
    let prompt_prelude = PromptPreludeSnapshot {
        sections: vec![
            PromptPreludeSection {
                reference_id: "core-system".to_string(),
                kind: PromptSourceKind::CoreSystemPrompt,
                title: "Core system prompt".to_string(),
                origin: Some(PromptSourceOrigin::Builtin),
                body: "builtin core".to_string(),
            },
            PromptPreludeSection {
                reference_id: "skill-discovery".to_string(),
                kind: PromptSourceKind::SkillDiscovery,
                title: "Skill discovery source".to_string(),
                origin: Some(PromptSourceOrigin::Project),
                body: "<available_skills></available_skills>".to_string(),
            },
        ],
    };
    block_on_session(store.append_config_change(
        &session_id,
        session_store::ConfigSnapshot {
            provider_id: "local".to_string(),
            model: "qwen3".to_string(),
            system_prompt: Some("stale fallback".to_string()),
            prompt_prelude: Some(prompt_prelude.clone()),
        },
    ))
    .expect("config snapshot should persist");

    let restored_state =
        block_on_session(store.load_session(&session_id, None)).expect("session state should load");
    let store_trait: Arc<dyn SessionStore> = store;
    let session = ProviderConversation::with_resolved_session_store(
        store_trait,
        sample_header(&work_dir, "qwen3"),
        Some(session_id),
        &restored_state,
    )
    .expect("persisted conversation should load config");

    assert_eq!(session.prompt_prelude(), Some(&prompt_prelude));
    assert_eq!(
        session.system_prompt(),
        Some("builtin core\n\n<available_skills></available_skills>")
    );
}

#[test]
fn truncate_after_user_turns_branches_within_existing_session() {
    let work_dir = tempdir_path("truncate-branch");
    let store = Arc::new(InMemorySessionStore::new());
    let initial_history = vec![
        ConversationItem::text(Role::User, "first"),
        ConversationItem::text(Role::Assistant, "alpha"),
        ConversationItem::text(Role::User, "second"),
        ConversationItem::text(Role::Assistant, "beta"),
    ];
    let session_id = block_on_session(store.create_session(sample_header(&work_dir, "qwen3")))
        .expect("session should be created");
    for item in &initial_history {
        block_on_session(store.append(&session_id, item.clone()))
            .expect("history item should persist");
    }
    let restored_state =
        block_on_session(store.load_session(&session_id, None)).expect("session state should load");
    let first_assistant_entry_id = restored_state.items[1].entry_id.clone();
    let store_trait: Arc<dyn SessionStore> = store;
    let mut session = ProviderConversation::with_resolved_session_store(
        store_trait,
        sample_header(&work_dir, "qwen3"),
        Some(session_id.clone()),
        &restored_state,
    )
    .expect("persisted conversation should initialize");

    let leaf_update = session
        .truncate_after_user_turns(1)
        .expect("truncate should keep original session");

    assert_eq!(
        leaf_update,
        Some((session_id, first_assistant_entry_id)),
        "truncate should report the leaf update for async persistence"
    );
    let expected_history = [
        ConversationItem::text(Role::User, "first"),
        ConversationItem::text(Role::Assistant, "alpha"),
    ];
    assert!(session.history().eq(expected_history.iter()));
}

fn sample_header(work_dir: &Path, model: &str) -> SessionHeader {
    SessionHeader {
        session_id: SessionId::new(),
        work_dir: work_dir.to_path_buf(),
        session_name: Some("test-session".to_string()),
        initial_model: model.to_string(),
        git_head: Some("abc123".to_string()),
        cli_version: Some("0.5.7".to_string()),
    }
}

fn tempdir_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "lumos-provider-conversation-{label}-{}",
        std::process::id()
    ))
}

fn cached_item(item: ConversationItem) -> PersistedConversationItem {
    PersistedConversationItem {
        entry_id: None,
        item,
    }
}

fn block_on_session<T>(
    future: impl std::future::Future<Output = Result<T, SessionStoreError>>,
) -> Result<T, SessionStoreError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    runtime.block_on(future)
}
