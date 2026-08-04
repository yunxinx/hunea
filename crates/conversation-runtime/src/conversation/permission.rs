use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

use runtime_domain::session::{
    ConversationEvent, RuntimePermissionOption, RuntimePermissionOptionKind,
    RuntimePermissionRequest,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tool_runtime::{
    ToolPermissionDecision, ToolPermissionFuture, ToolPermissionHandler, ToolPermissionRequest,
    ToolPermissionRule, ToolPermissionRuleBehavior, ToolPermissionRuleSet,
};

use tool_loop_runtime::runtime_tool_activity_update_from_permission_request;

const CONVERSATION_PERMISSION_REQUEST_PREFIX: &str = "conversation-permission";
const ALLOW_ONCE_OPTION_ID: &str = "allow_once";
const ALLOW_ALWAYS_OPTION_ID: &str = "allow_always";
const REJECT_ONCE_OPTION_ID: &str = "reject_once";
const REJECT_ALWAYS_OPTION_ID: &str = "reject_always";
const TOOL_PERMISSION_DENIED: &str = "Tool permission denied";
const USER_REJECTED_TOOL_CALL: &str = "user rejected the tool call";

type PermissionResponseSender = oneshot::Sender<Option<String>>;

/// `ConversationPermissionBroker` 保存 conversation tool Ask 请求与 TUI 响应之间的等待关系。
#[derive(Debug, Clone, Default)]
pub(crate) struct ConversationPermissionBroker {
    next_request_id: Arc<AtomicUsize>,
    context_generation: Arc<AtomicUsize>,
    pending: Arc<Mutex<HashMap<String, PermissionResponseSender>>>,
    rules: Arc<Mutex<ToolPermissionRuleSet>>,
}

impl ConversationPermissionBroker {
    pub(crate) fn handler(
        &self,
        sender: mpsc::Sender<ConversationEvent>,
    ) -> ConversationToolPermissionHandler {
        ConversationToolPermissionHandler {
            broker: self.clone(),
            sender,
        }
    }

    pub(crate) fn respond_permission(
        &self,
        request_id: &str,
        option_id: Option<String>,
    ) -> Result<(), String> {
        let sender = self.pending_guard().remove(request_id).ok_or_else(|| {
            format!("Conversation permission request is not pending: {request_id}")
        })?;
        sender.send(option_id).map_err(|_| {
            format!("Conversation permission request is no longer waiting: {request_id}")
        })
    }

    pub(crate) fn cancel_all(&self) {
        self.invalidate_pending(false);
    }

    pub(crate) fn clear_permission_context(&self) {
        self.invalidate_pending(true);
    }

    fn context_generation(&self) -> usize {
        self.context_generation.load(Ordering::Acquire)
    }

    fn evaluate(&self, request: &ToolPermissionRequest) -> Option<ToolPermissionRuleBehavior> {
        self.rules_guard().evaluate(request)
    }

    fn insert_rule_if_active(
        &self,
        generation: usize,
        cancellation: &CancellationToken,
        rule: ToolPermissionRule,
    ) -> bool {
        let mut rules = self.rules_guard();
        if cancellation.is_cancelled()
            || self.context_generation.load(Ordering::Acquire) != generation
        {
            return false;
        }
        rules.insert(rule);
        true
    }

    fn next_request_id(&self) -> String {
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        format!("{CONVERSATION_PERMISSION_REQUEST_PREFIX}-{id}")
    }

    fn register(
        &self,
        generation: usize,
        request_id: String,
        sender: PermissionResponseSender,
    ) -> bool {
        let mut pending = self.pending_guard();
        if self.context_generation.load(Ordering::Acquire) != generation {
            return false;
        }
        pending.insert(request_id, sender);
        true
    }

    fn remove(&self, request_id: &str) {
        self.pending_guard().remove(request_id);
    }

    fn invalidate_pending(&self, should_clear_rules: bool) {
        // generation 推进必须与 rule 插入和 pending 注册使用同一组锁形成原子边界，
        // 否则迟到的 Always 响应可能在失效检查后重新写入 approval context。
        let mut rules = self.rules_guard();
        let mut pending_guard = self.pending_guard();
        self.context_generation.fetch_add(1, Ordering::AcqRel);
        if should_clear_rules {
            rules.clear();
        }
        let pending = std::mem::take(&mut *pending_guard);
        drop(pending_guard);
        drop(rules);

        for (_, sender) in pending {
            let _ = sender.send(None);
        }
    }

    fn pending_guard(&self) -> MutexGuard<'_, HashMap<String, PermissionResponseSender>> {
        match self.pending.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn rules_guard(&self) -> MutexGuard<'_, ToolPermissionRuleSet> {
        match self.rules.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// `ConversationToolPermissionHandler` 在工具执行前把 Ask 请求转交给 TUI。
pub(crate) struct ConversationToolPermissionHandler {
    broker: ConversationPermissionBroker,
    sender: mpsc::Sender<ConversationEvent>,
}

impl ToolPermissionHandler for ConversationToolPermissionHandler {
    fn request_permission<'a>(
        &'a self,
        request: ToolPermissionRequest,
        cancellation: &'a CancellationToken,
    ) -> ToolPermissionFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return deny_permission(&request.definition.name, "permission request cancelled");
            }
            let context_generation = self.broker.context_generation();
            let stored_behavior = self.broker.evaluate(&request);
            // rule 求值可能与 context reset 并发；授权前再次观察 cancellation，
            // 避免 reset 已开始后仍消费旧 context 中的 allow rule。
            if cancellation.is_cancelled() || self.broker.context_generation() != context_generation
            {
                return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
            }
            if let Some(behavior) = stored_behavior {
                return decision_for_rule(&request.definition.name, behavior);
            }

            let request_id = self.broker.next_request_id();
            let (response_sender, response_receiver) = oneshot::channel();
            if !self
                .broker
                .register(context_generation, request_id.clone(), response_sender)
            {
                return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
            }

            let session_rule =
                ToolPermissionRule::from_request(&request, ToolPermissionRuleBehavior::Allow);
            let runtime_options = conversation_runtime_permission_options(session_rule.is_some());
            let runtime_request = conversation_runtime_permission_request(
                &request_id,
                &request,
                runtime_options.clone(),
            );
            if self
                .sender
                .send(ConversationEvent::PermissionRequested {
                    request: runtime_request,
                })
                .is_err()
            {
                self.broker.remove(&request_id);
                return deny_permission(&request.definition.name, "runtime is unavailable");
            }

            let option_id = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    self.broker.remove(&request_id);
                    None
                }
                response = response_receiver => response.ok().flatten(),
            };

            if cancellation.is_cancelled() {
                return deny_permission(&request.definition.name, "permission request cancelled");
            }
            if self.broker.context_generation() != context_generation {
                return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
            }

            let selected_kind = option_id.as_deref().and_then(|option_id| {
                runtime_options
                    .iter()
                    .find(|option| option.option_id == option_id)
                    .map(|option| option.kind)
            });
            match selected_kind {
                Some(RuntimePermissionOptionKind::AllowOnce) => ToolPermissionDecision::Allow,
                Some(RuntimePermissionOptionKind::AllowAlways) => {
                    if let Some(rule) = session_rule
                        && !self.broker.insert_rule_if_active(
                            context_generation,
                            cancellation,
                            rule,
                        )
                    {
                        return deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL);
                    }
                    ToolPermissionDecision::Allow
                }
                Some(RuntimePermissionOptionKind::RejectOnce) => {
                    deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL)
                }
                Some(RuntimePermissionOptionKind::RejectAlways) => {
                    if let Some(rule) = session_rule {
                        self.broker.insert_rule_if_active(
                            context_generation,
                            cancellation,
                            rule.with_behavior(ToolPermissionRuleBehavior::Deny),
                        );
                    }
                    deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL)
                }
                _ => deny_permission(&request.definition.name, USER_REJECTED_TOOL_CALL),
            }
        })
    }
}

fn conversation_runtime_permission_request(
    request_id: &str,
    request: &ToolPermissionRequest,
    options: Vec<RuntimePermissionOption>,
) -> RuntimePermissionRequest {
    let tool_activity =
        runtime_tool_activity_update_from_permission_request(&request.call.call_id, request);
    RuntimePermissionRequest::new(request_id.to_string(), tool_activity.title.clone(), options)
        .with_tool_activity(tool_activity)
}

fn conversation_runtime_permission_options(can_remember: bool) -> Vec<RuntimePermissionOption> {
    let mut options = vec![RuntimePermissionOption::new(
        ALLOW_ONCE_OPTION_ID,
        "Yes",
        RuntimePermissionOptionKind::AllowOnce,
    )];
    if can_remember {
        options.push(RuntimePermissionOption::new(
            ALLOW_ALWAYS_OPTION_ID,
            "Yes, allow similar requests during this session",
            RuntimePermissionOptionKind::AllowAlways,
        ));
    }
    options.push(RuntimePermissionOption::new(
        REJECT_ONCE_OPTION_ID,
        "No",
        RuntimePermissionOptionKind::RejectOnce,
    ));
    if can_remember {
        options.push(RuntimePermissionOption::new(
            REJECT_ALWAYS_OPTION_ID,
            "No, reject similar requests during this session",
            RuntimePermissionOptionKind::RejectAlways,
        ));
    }
    options
}

fn decision_for_rule(
    tool_name: &str,
    behavior: ToolPermissionRuleBehavior,
) -> ToolPermissionDecision {
    match behavior {
        ToolPermissionRuleBehavior::Allow => ToolPermissionDecision::Allow,
        ToolPermissionRuleBehavior::Deny => {
            deny_permission(tool_name, "a stored permission rule rejected the tool call")
        }
    }
}

fn deny_permission(tool_name: &str, reason: &str) -> ToolPermissionDecision {
    ToolPermissionDecision::Deny {
        message: format!("{TOOL_PERMISSION_DENIED}: {tool_name} {reason}"),
    }
}

#[cfg(test)]
mod tests;
