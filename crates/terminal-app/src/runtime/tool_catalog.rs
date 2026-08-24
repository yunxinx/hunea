//! Tool capability 的 owner-scoped registration 与只读执行快照。

use std::{
    collections::BTreeMap,
    fmt,
    sync::{Arc, Mutex, Weak},
};

use tokio_util::sync::CancellationToken;
use tool_runtime::{
    Tool, ToolCall, ToolDefinition, ToolExecutionContext, ToolExecutionFuture,
    ToolExecutorRegistry, ToolPermissionPreview,
};

/// `ToolCatalog` 集中拥有 host 当前可见的完整 tool 集合。
///
/// caller 只能注册 tool、读取独立 executor snapshot 或创建过滤后的 session view；
/// registration identity 与逆操作留在 module 内部。
#[derive(Clone, Default)]
pub(super) struct ToolCatalog {
    state: Arc<Mutex<ToolCatalogState>>,
}

#[derive(Default)]
struct ToolCatalogState {
    next_registration_id: u64,
    registry: ToolExecutorRegistry,
    registrations: BTreeMap<String, ToolRegistrationRecord>,
}

struct ToolRegistrationRecord {
    id: u64,
    owner: String,
}

#[derive(Clone, PartialEq, Eq, thiserror::Error)]
pub(super) enum ToolCatalogError {
    #[error("tool {tool_name} is already registered")]
    DuplicateTool {
        tool_name: String,
        existing_owner: String,
    },
}

impl fmt::Debug for ToolCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTool { tool_name, .. } => formatter
                .debug_struct("DuplicateTool")
                .field("tool_name", tool_name)
                .finish_non_exhaustive(),
        }
    }
}

/// `ToolRegistration` 是一次或一组 tool registration 的幂等逆操作。
///
/// handle Drop 与显式 `dispose` 等价；只有 identity 仍匹配当前 slot 时才会移除 tool，
/// 因此旧 handle 不会误删后续 registration。
pub(super) struct ToolRegistration {
    catalog: Weak<Mutex<ToolCatalogState>>,
    entries: Vec<ToolRegistrationEntry>,
    is_disposed: bool,
}

struct ToolRegistrationEntry {
    tool_name: String,
    registration_id: u64,
}

impl ToolCatalog {
    /// 接管一个已构造的 registry，并把其中所有 tool 归入同一个 owner registration。
    pub(super) fn adopt_registry(
        owner: impl Into<String>,
        registry: ToolExecutorRegistry,
    ) -> Result<(Self, ToolRegistration), ToolCatalogError> {
        let owner = owner.into();
        let catalog = Self::default();
        let mut registration = ToolRegistration::new(&catalog.state, Vec::new());
        for tool in registry.tools() {
            registration.merge(catalog.register(owner.clone(), SharedTool(tool))?);
        }
        Ok((catalog, registration))
    }

    /// 注册一个 tool；重名 registration 在改变 catalog 前被拒绝。
    pub(super) fn register<T>(
        &self,
        owner: impl Into<String>,
        tool: T,
    ) -> Result<ToolRegistration, ToolCatalogError>
    where
        T: Tool + 'static,
    {
        let owner = owner.into();
        let tool_name = tool.definition().name;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = state.registrations.get(&tool_name) {
            return Err(ToolCatalogError::DuplicateTool {
                tool_name,
                existing_owner: existing.owner.clone(),
            });
        }
        let registration_id = next_registration_id(&mut state);
        state.registry.insert(tool);
        state.registrations.insert(
            tool_name.clone(),
            ToolRegistrationRecord {
                id: registration_id,
                owner,
            },
        );
        drop(state);
        Ok(ToolRegistration::new(
            &self.state,
            vec![ToolRegistrationEntry {
                tool_name,
                registration_id,
            }],
        ))
    }

    /// 返回与 catalog map 独立、tool body 以 `Arc` 共享的 executor snapshot。
    pub(super) fn snapshot(&self) -> ToolExecutorRegistry {
        self.filtered(|_| true)
    }

    /// 创建独立的 session executor view；过滤不会改变完整 catalog。
    pub(super) fn filtered(&self, keep_tool: impl Fn(&str) -> bool) -> ToolExecutorRegistry {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .registry
            .filtered(keep_tool)
    }

    /// 按稳定名称顺序返回当前完整 tool definitions。
    pub(super) fn definitions(&self) -> Vec<ToolDefinition> {
        self.snapshot()
            .definitions()
            .definitions()
            .cloned()
            .collect()
    }
}

impl ToolRegistration {
    fn new(catalog: &Arc<Mutex<ToolCatalogState>>, entries: Vec<ToolRegistrationEntry>) -> Self {
        Self {
            catalog: Arc::downgrade(catalog),
            entries,
            is_disposed: false,
        }
    }

    /// 幂等撤销本 handle 仍拥有的 registration。
    pub(super) fn dispose(&mut self) {
        if self.is_disposed {
            return;
        }
        self.is_disposed = true;
        let Some(catalog) = self.catalog.upgrade() else {
            return;
        };
        let mut state = catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in self.entries.drain(..) {
            let owns_current_registration = state
                .registrations
                .get(&entry.tool_name)
                .is_some_and(|current| current.id == entry.registration_id);
            if owns_current_registration {
                state.registrations.remove(&entry.tool_name);
                state.registry.remove(&entry.tool_name);
            }
        }
    }

    fn merge(&mut self, mut registration: Self) {
        self.entries.append(&mut registration.entries);
        registration.is_disposed = true;
    }
}

impl Drop for ToolRegistration {
    fn drop(&mut self) {
        self.dispose();
    }
}

fn next_registration_id(state: &mut ToolCatalogState) -> u64 {
    let id = state.next_registration_id;
    state.next_registration_id = state
        .next_registration_id
        .checked_add(1)
        .expect("tool registration id space should be unreachable");
    id
}

struct SharedTool(Arc<dyn Tool>);

impl Tool for SharedTool {
    fn definition(&self) -> ToolDefinition {
        self.0.definition()
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        cancellation: &'a CancellationToken,
    ) -> ToolExecutionFuture<'a> {
        self.0.execute(call, cancellation)
    }

    fn execute_with_context<'a>(
        &'a self,
        call: ToolCall,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionFuture<'a> {
        self.0.execute_with_context(call, context)
    }

    fn permission_preview(
        &self,
        call: &ToolCall,
        cancellation: &CancellationToken,
    ) -> Option<ToolPermissionPreview> {
        self.0.permission_preview(call, cancellation)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio_util::sync::CancellationToken;
    use tool_runtime::{
        ToolCall, ToolExecutionFuture, ToolExecutor, ToolPermissionPolicy, ToolPermissionPreview,
        ToolProgress, ToolProgressSink, ToolResult, ToolResultOutcome,
    };

    use super::*;
    use crate::runtime::effect_scope::EffectScope;

    struct StubTool {
        name: &'static str,
    }

    impl Tool for StubTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new(self.name)
        }

        fn execute<'a>(
            &'a self,
            call: ToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, String::new()) })
        }
    }

    struct ContextAwareTool;

    impl Tool for ContextAwareTool {
        fn definition(&self) -> ToolDefinition {
            ToolDefinition::new("write")
                .with_input_schema(json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" }
                    },
                    "required": ["text"],
                    "additionalProperties": false
                }))
                .with_permission_policy(ToolPermissionPolicy::Ask)
        }

        fn execute<'a>(
            &'a self,
            call: ToolCall,
            _cancellation: &'a CancellationToken,
        ) -> ToolExecutionFuture<'a> {
            Box::pin(async move { ToolResult::success(call.call_id, "without-context") })
        }

        fn execute_with_context<'a>(
            &'a self,
            call: ToolCall,
            context: ToolExecutionContext<'a>,
        ) -> ToolExecutionFuture<'a> {
            context.emit(ToolProgress::SystemMessage {
                message: "context-forwarded".to_string(),
            });
            Box::pin(async move { ToolResult::success(call.call_id, "with-context") })
        }

        fn permission_preview(
            &self,
            _call: &ToolCall,
            _cancellation: &CancellationToken,
        ) -> Option<ToolPermissionPreview> {
            Some(ToolPermissionPreview {
                path: "fixture.txt".to_string(),
                old_text: Some("before".to_string()),
                new_text: "after".to_string(),
                is_truncated: false,
                snapshot: None,
            })
        }
    }

    fn names(catalog: &ToolCatalog) -> Vec<String> {
        catalog
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect()
    }

    #[test]
    fn registration_dispose_is_idempotent_and_removes_only_owned_tool() {
        let catalog = ToolCatalog::default();
        let mut read = catalog
            .register("builtin", StubTool { name: "read" })
            .expect("read should register");
        let _bash = catalog
            .register("builtin", StubTool { name: "bash" })
            .expect("bash should register");
        assert_eq!(names(&catalog), vec!["bash", "read"]);

        read.dispose();
        read.dispose();

        assert_eq!(names(&catalog), vec!["bash"]);
    }

    #[test]
    fn dropping_registration_runs_the_inverse() {
        let catalog = ToolCatalog::default();
        {
            let _registration = catalog
                .register("fixture", StubTool { name: "read" })
                .expect("fixture should register");
            assert_eq!(names(&catalog), vec!["read"]);
        }
        assert!(names(&catalog).is_empty());
    }

    #[test]
    fn duplicate_registration_is_rejected_without_replacing_the_owner() {
        let catalog = ToolCatalog::default();
        let _registration = catalog
            .register("builtin", StubTool { name: "read" })
            .expect("first owner should register");

        let error = match catalog.register("extension", StubTool { name: "read" }) {
            Ok(_) => panic!("duplicate tool name must be rejected"),
            Err(error) => error,
        };

        assert_eq!(
            error,
            ToolCatalogError::DuplicateTool {
                tool_name: "read".to_string(),
                existing_owner: "builtin".to_string(),
            }
        );
        assert_eq!(error.to_string(), "tool read is already registered");
        assert!(!error.to_string().contains("builtin"));
        assert!(!format!("{error:?}").contains("builtin"));
        assert_eq!(names(&catalog), vec!["read"]);
    }

    #[test]
    fn old_disposer_cannot_remove_fresh_catalog_registration() {
        let catalog = ToolCatalog::default();
        let mut old_registration = catalog
            .register("old", StubTool { name: "read" })
            .expect("old tool should register");
        old_registration.dispose();
        let _fresh_registration = catalog
            .register("fresh", StubTool { name: "read" })
            .expect("fresh tool should register");

        old_registration.dispose();

        assert_eq!(names(&catalog), vec!["read"]);
    }

    #[test]
    fn filtered_view_keeps_the_full_catalog_unchanged() {
        let catalog = ToolCatalog::default();
        let _read = catalog
            .register("fixture", StubTool { name: "read" })
            .expect("read should register");
        let _bash = catalog
            .register("fixture", StubTool { name: "bash" })
            .expect("bash should register");

        let filtered = catalog.filtered(|name| name != "bash");

        assert_eq!(
            filtered
                .definitions()
                .definitions()
                .map(|definition| definition.name.clone())
                .collect::<Vec<_>>(),
            vec!["read".to_string()]
        );
        assert_eq!(names(&catalog), vec!["bash", "read"]);
    }

    #[test]
    fn effect_scope_disposal_removes_the_adopted_registry() {
        let mut registry = ToolExecutorRegistry::new();
        registry.insert(StubTool { name: "read" });
        registry.insert(StubTool { name: "bash" });
        let (catalog, mut registration) =
            ToolCatalog::adopt_registry("workspace", registry).expect("registry should be adopted");
        let scope = EffectScope::default();
        scope
            .register("workspace-tools", move || {
                registration.dispose();
                Ok(())
            })
            .expect("catalog disposer should register");

        let first = scope.dispose();
        let second = scope.dispose();

        assert!(first.failures.is_empty());
        assert!(second.failures.is_empty());
        assert!(names(&catalog).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn adopted_registry_preserves_schema_permission_preview_and_execution_context() {
        let mut registry = ToolExecutorRegistry::new();
        registry.insert(ContextAwareTool);
        let (catalog, _registration) =
            ToolCatalog::adopt_registry("workspace", registry).expect("registry should be adopted");

        let definition = catalog
            .definitions()
            .into_iter()
            .next()
            .expect("adopted tool definition should remain visible");
        assert_eq!(definition.name, "write");
        assert_eq!(definition.permission_policy, ToolPermissionPolicy::Ask);
        assert!(definition.input_schema.is_some());

        let snapshot = catalog.snapshot();
        let cancellation = CancellationToken::new();
        let invalid = snapshot
            .execute_tool_with_context(
                ToolCall::new("invalid", "write", json!({})),
                ToolExecutionContext::new(&cancellation),
            )
            .await;
        assert_eq!(invalid.outcome(), ToolResultOutcome::Error);
        assert!(invalid.content().contains("arguments do not match schema"));

        let call = ToolCall::new("valid", "write", json!({ "text": "after" }));
        assert_eq!(
            snapshot.permission_preview(&call, &cancellation),
            Some(ToolPermissionPreview {
                path: "fixture.txt".to_string(),
                old_text: Some("before".to_string()),
                new_text: "after".to_string(),
                is_truncated: false,
                snapshot: None,
            })
        );
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let executed = snapshot
            .execute_tool_with_context(
                call,
                ToolExecutionContext::new(&cancellation)
                    .with_progress_sink(ToolProgressSink::from_sender(progress_tx)),
            )
            .await;

        assert_eq!(executed.outcome(), ToolResultOutcome::Success);
        assert_eq!(executed.content().text(), "with-context");
        assert_eq!(
            progress_rx.try_recv(),
            Ok(ToolProgress::SystemMessage {
                message: "context-forwarded".to_string(),
            })
        );
    }
}
