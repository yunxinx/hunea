use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::{ToolKind, ToolPermissionRequest};

/// `ToolPermissionRuleBehavior` 描述一条内存权限规则命中后的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPermissionRuleBehavior {
    Allow,
    Deny,
}

/// 描述一条规则适用的请求范围。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolPermissionRuleMatcher {
    /// 相对 workspace 的文件路径使用当前 workspace 作为范围。
    WorkspaceRelativeFile,
    /// 绝对文件路径使用目标文件的父目录作为范围。
    AbsoluteDirectory(PathBuf),
    /// execute 工具要求命令和工作目录都完全匹配。
    ExactCommand {
        command: String,
        workdir: Option<String>,
    },
    /// 其他工具按结构化参数完全匹配。
    ExactArguments(Value),
}

/// `ToolPermissionRule` 将工具身份、动作分类和具体范围绑定在一起。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPermissionRule {
    behavior: ToolPermissionRuleBehavior,
    tool_name: String,
    tool_kind: ToolKind,
    matcher: ToolPermissionRuleMatcher,
}

impl ToolPermissionRule {
    /// 从一次审批请求创建安全的规则；无法证明范围安全时返回 `None`。
    pub fn from_request(
        request: &ToolPermissionRequest,
        behavior: ToolPermissionRuleBehavior,
    ) -> Option<Self> {
        if request.call.name != request.definition.name || request.definition.name.trim().is_empty()
        {
            return None;
        }

        let matcher = matcher_from_request(request)?;
        Some(Self {
            behavior,
            tool_name: request.definition.name.clone(),
            tool_kind: request.definition.kind,
            matcher,
        })
    }

    /// 复制规则并替换命中行为。
    pub const fn with_behavior(mut self, behavior: ToolPermissionRuleBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    fn matches(&self, request: &ToolPermissionRequest) -> bool {
        if request.call.name != self.tool_name
            || request.definition.name != self.tool_name
            || request.definition.kind != self.tool_kind
        {
            return false;
        }

        match &self.matcher {
            ToolPermissionRuleMatcher::WorkspaceRelativeFile => {
                matches_relative_file_request(request)
            }
            ToolPermissionRuleMatcher::AbsoluteDirectory(directory) => {
                matches_absolute_file_request(request, directory)
            }
            ToolPermissionRuleMatcher::ExactCommand { command, workdir } => {
                matches_exact_command_request(request, command, workdir.as_deref())
            }
            ToolPermissionRuleMatcher::ExactArguments(arguments) => {
                request.call.arguments == *arguments
            }
        }
    }
}

/// `ToolPermissionRuleSet` 保存当前 runtime 的临时规则并进行 deny-first 求值。
#[derive(Debug, Clone, Default)]
pub struct ToolPermissionRuleSet {
    rules: Vec<ToolPermissionRule>,
}

impl ToolPermissionRuleSet {
    /// 添加规则；相同规则不会重复保存。
    pub fn insert(&mut self, rule: ToolPermissionRule) {
        if !self.rules.contains(&rule) {
            self.rules.push(rule);
        }
    }

    /// 评估一次请求。匹配的 deny 永远优先于匹配的 allow。
    pub fn evaluate(&self, request: &ToolPermissionRequest) -> Option<ToolPermissionRuleBehavior> {
        let mut matched_allow = false;
        for rule in &self.rules {
            if !rule.matches(request) {
                continue;
            }
            match rule.behavior {
                ToolPermissionRuleBehavior::Allow => matched_allow = true,
                ToolPermissionRuleBehavior::Deny => {
                    return Some(ToolPermissionRuleBehavior::Deny);
                }
            }
        }
        matched_allow.then_some(ToolPermissionRuleBehavior::Allow)
    }

    /// 清除当前 runtime 的全部临时规则。
    pub fn clear(&mut self) {
        self.rules.clear();
    }

    /// 返回当前是否没有规则。
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

fn matcher_from_request(request: &ToolPermissionRequest) -> Option<ToolPermissionRuleMatcher> {
    match request.definition.kind {
        ToolKind::Write | ToolKind::Edit => file_matcher_from_request(request),
        ToolKind::Execute => command_matcher_from_request(request),
        _ => Some(ToolPermissionRuleMatcher::ExactArguments(
            request.call.arguments.clone(),
        )),
    }
}

fn file_matcher_from_request(request: &ToolPermissionRequest) -> Option<ToolPermissionRuleMatcher> {
    let call_path = string_argument(&request.call.arguments, "path")?;
    let normalized_call_path = normalize_file_path(call_path)?;
    let preview_path = request.preview.as_ref()?.path.as_str();
    let normalized_preview_path = normalize_file_path(preview_path)?;
    if normalized_preview_path != normalized_call_path {
        return None;
    }

    if normalized_call_path.is_absolute() {
        let parent = normalized_call_path.parent()?.to_path_buf();
        Some(ToolPermissionRuleMatcher::AbsoluteDirectory(parent))
    } else {
        Some(ToolPermissionRuleMatcher::WorkspaceRelativeFile)
    }
}

fn command_matcher_from_request(
    request: &ToolPermissionRequest,
) -> Option<ToolPermissionRuleMatcher> {
    let command = string_argument_any(&request.call.arguments, &["command", "cmd"])?;
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let workdir = normalized_workdir(
        request
            .call
            .arguments
            .get("workdir")
            .or_else(|| request.call.arguments.get("cwd")),
    )?;
    Some(ToolPermissionRuleMatcher::ExactCommand {
        command: command.to_string(),
        workdir,
    })
}

fn matches_relative_file_request(request: &ToolPermissionRequest) -> bool {
    let Some(path) = string_argument(&request.call.arguments, "path") else {
        return false;
    };
    let Some(normalized_path) = normalize_file_path(path) else {
        return false;
    };
    if normalized_path.is_absolute() {
        return false;
    }
    preview_path_matches_call(request, &normalized_path)
}

fn matches_absolute_file_request(request: &ToolPermissionRequest, directory: &Path) -> bool {
    let Some(path) = string_argument(&request.call.arguments, "path") else {
        return false;
    };
    let Some(normalized_path) = normalize_file_path(path) else {
        return false;
    };
    if !normalized_path.is_absolute() || !normalized_path.starts_with(directory) {
        return false;
    }
    preview_path_matches_call(request, &normalized_path)
}

fn preview_path_matches_call(request: &ToolPermissionRequest, call_path: &Path) -> bool {
    request.preview.as_ref().is_some_and(|preview| {
        normalize_file_path(&preview.path).is_some_and(|preview_path| preview_path == call_path)
    })
}

fn matches_exact_command_request(
    request: &ToolPermissionRequest,
    expected_command: &str,
    expected_workdir: Option<&str>,
) -> bool {
    let Some(command) = string_argument_any(&request.call.arguments, &["command", "cmd"]) else {
        return false;
    };
    if command.trim() != expected_command {
        return false;
    }
    let Some(workdir) = normalized_workdir(
        request
            .call
            .arguments
            .get("workdir")
            .or_else(|| request.call.arguments.get("cwd")),
    ) else {
        return false;
    };
    workdir.as_deref() == expected_workdir
}

fn string_argument<'a>(arguments: &'a Value, key: &str) -> Option<&'a str> {
    arguments.get(key).and_then(Value::as_str)
}

fn string_argument_any<'a>(arguments: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| arguments.get(*key).and_then(Value::as_str))
}

fn normalize_file_path(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.trim() != path {
        return None;
    }
    normalize_path(Path::new(path))
}

fn normalize_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
                has_component = true;
            }
        }
    }
    has_component.then_some(normalized)
}

fn normalized_workdir(value: Option<&Value>) -> Option<Option<String>> {
    let Some(value) = value else {
        return Some(None);
    };
    let value = value.as_str()?;
    let value = value.trim();
    if value.is_empty() {
        return Some(None);
    }
    if Path::new(value)
        .components()
        .all(|component| matches!(component, Component::CurDir))
    {
        return Some(None);
    }
    let normalized = normalize_path(Path::new(value))?;
    if normalized.as_os_str().is_empty() {
        return Some(None);
    }
    Some(Some(normalized.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ToolCall, ToolDefinition, ToolKind, ToolPermissionPolicy, ToolPermissionPreview};

    fn request(tool_name: &str, kind: ToolKind, arguments: Value) -> ToolPermissionRequest {
        let request = ToolPermissionRequest::new(
            ToolCall::new("call-1", tool_name, arguments),
            ToolDefinition::new(tool_name)
                .with_kind(kind)
                .with_permission_policy(ToolPermissionPolicy::Ask),
        );
        let preview_path = request
            .call
            .arguments
            .get("path")
            .and_then(Value::as_str)
            .map(str::to_string);
        if matches!(kind, ToolKind::Write | ToolKind::Edit)
            && let Some(path) = preview_path
        {
            return request.with_preview(ToolPermissionPreview {
                path,
                old_text: None,
                new_text: String::new(),
                is_truncated: false,
                snapshot: None,
            });
        }
        request
    }

    #[test]
    fn relative_file_rule_matches_same_action_inside_workspace() {
        let original = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/./main.rs", "content": "one"}),
        );
        let later = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "docs/README.md", "content": "two"}),
        );
        let outside_form = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "../outside.txt", "content": "three"}),
        );
        let rule = ToolPermissionRule::from_request(&original, ToolPermissionRuleBehavior::Allow)
            .expect("relative file request should produce a rule");
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(rule);

        assert_eq!(
            rules.evaluate(&later),
            Some(ToolPermissionRuleBehavior::Allow)
        );
        assert_eq!(rules.evaluate(&outside_form), None);
    }

    #[test]
    fn empty_rule_set_does_not_match_requests() {
        let rules = ToolPermissionRuleSet::default();
        let request = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs", "content": "one"}),
        );

        assert_eq!(rules.evaluate(&request), None);
    }

    #[test]
    fn rule_requires_matching_tool_name_and_kind() {
        let original = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs", "content": "one"}),
        );
        let different_name = request(
            "write_file",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs", "content": "two"}),
        );
        let different_kind = request(
            "write",
            ToolKind::Edit,
            serde_json::json!({"path": "src/main.rs", "content": "two"}),
        );
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(
            ToolPermissionRule::from_request(&original, ToolPermissionRuleBehavior::Allow)
                .expect("valid write request should produce a rule"),
        );

        assert_eq!(rules.evaluate(&different_name), None);
        assert_eq!(rules.evaluate(&different_kind), None);
    }

    #[test]
    fn mismatched_preview_and_call_paths_do_not_create_file_rule() {
        let request = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs", "content": "one"}),
        )
        .with_preview(ToolPermissionPreview {
            path: "src/lib.rs".to_string(),
            old_text: None,
            new_text: "one".to_string(),
            is_truncated: false,
            snapshot: None,
        });

        assert!(
            ToolPermissionRule::from_request(&request, ToolPermissionRuleBehavior::Allow).is_none()
        );
    }

    #[test]
    fn file_rules_require_a_current_matching_preview() {
        let with_preview = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs", "content": "one"}),
        )
        .with_preview(ToolPermissionPreview {
            path: "src/main.rs".to_string(),
            old_text: None,
            new_text: "one".to_string(),
            is_truncated: false,
            snapshot: None,
        });
        let mut without_preview = with_preview.clone();
        without_preview.preview = None;

        assert!(
            ToolPermissionRule::from_request(&without_preview, ToolPermissionRuleBehavior::Allow,)
                .is_none(),
            "a file request without a preview must not create a reusable rule"
        );

        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(
            ToolPermissionRule::from_request(&with_preview, ToolPermissionRuleBehavior::Allow)
                .expect("a matching file preview should create a reusable rule"),
        );
        assert_eq!(
            rules.evaluate(&without_preview),
            None,
            "an existing file rule must not bypass a missing current preview"
        );
    }

    #[test]
    fn empty_file_path_and_command_do_not_create_rules() {
        let empty_path = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "  ", "content": "one"}),
        );
        let empty_command = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "  "}),
        );

        assert!(
            ToolPermissionRule::from_request(&empty_path, ToolPermissionRuleBehavior::Allow)
                .is_none()
        );
        assert!(
            ToolPermissionRule::from_request(&empty_command, ToolPermissionRuleBehavior::Allow)
                .is_none()
        );
    }

    #[test]
    fn invalid_execute_workdir_does_not_create_rule() {
        let non_string_workdir = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "cargo test", "workdir": 42}),
        );
        let parent_traversal = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "cargo test", "workdir": "../outside"}),
        );

        assert!(
            ToolPermissionRule::from_request(
                &non_string_workdir,
                ToolPermissionRuleBehavior::Allow
            )
            .is_none()
        );
        assert!(
            ToolPermissionRule::from_request(&parent_traversal, ToolPermissionRuleBehavior::Allow)
                .is_none()
        );
    }

    #[test]
    fn relative_and_absolute_file_paths_do_not_share_rules() {
        let relative = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "workspace/src/main.rs", "content": "one"}),
        );
        let absolute = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "/workspace/src/main.rs", "content": "two"}),
        );
        let relative_rule =
            ToolPermissionRule::from_request(&relative, ToolPermissionRuleBehavior::Allow)
                .expect("relative request should create a rule");
        let absolute_rule =
            ToolPermissionRule::from_request(&absolute, ToolPermissionRuleBehavior::Allow)
                .expect("absolute request should create a rule");
        let mut relative_rules = ToolPermissionRuleSet::default();
        relative_rules.insert(relative_rule);
        let mut absolute_rules = ToolPermissionRuleSet::default();
        absolute_rules.insert(absolute_rule);

        assert_eq!(relative_rules.evaluate(&absolute), None);
        assert_eq!(absolute_rules.evaluate(&relative), None);
    }

    #[test]
    fn file_rule_keeps_write_and_edit_actions_separate() {
        let write = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs", "content": "one"}),
        );
        let edit = request(
            "edit",
            ToolKind::Edit,
            serde_json::json!({"path": "src/main.rs", "edits": []}),
        );
        let rule = ToolPermissionRule::from_request(&write, ToolPermissionRuleBehavior::Allow)
            .expect("write request should produce a rule");
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(rule);

        assert_eq!(rules.evaluate(&edit), None);
    }

    #[test]
    fn absolute_file_rule_uses_path_components_for_directory_scope() {
        let original = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "/workspace/src/main.rs", "content": "one"}),
        );
        let child = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "/workspace/src/nested/lib.rs", "content": "two"}),
        );
        let sibling_with_shared_prefix = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "/workspace/src-next/lib.rs", "content": "three"}),
        );
        let rule = ToolPermissionRule::from_request(&original, ToolPermissionRuleBehavior::Allow)
            .expect("absolute file request should produce a rule");
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(rule);

        assert_eq!(
            rules.evaluate(&child),
            Some(ToolPermissionRuleBehavior::Allow)
        );
        assert_eq!(rules.evaluate(&sibling_with_shared_prefix), None);
    }

    #[test]
    fn execute_rule_requires_exact_command_and_workdir() {
        let original = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "cargo test", "workdir": "./"}),
        );
        let same = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "  cargo test  "}),
        );
        let different_command = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "cargo build"}),
        );
        let different_workdir = request(
            "bash",
            ToolKind::Execute,
            serde_json::json!({"command": "cargo test", "workdir": "src"}),
        );
        let rule = ToolPermissionRule::from_request(&original, ToolPermissionRuleBehavior::Allow)
            .expect("execute request should produce a rule");
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(rule);

        assert_eq!(
            rules.evaluate(&same),
            Some(ToolPermissionRuleBehavior::Allow)
        );
        assert_eq!(rules.evaluate(&different_command), None);
        assert_eq!(rules.evaluate(&different_workdir), None);
    }

    #[test]
    fn deny_rule_has_priority_over_allow_rule() {
        let request = request(
            "custom",
            ToolKind::Other,
            serde_json::json!({"path": "same", "value": 1}),
        );
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(
            ToolPermissionRule::from_request(&request, ToolPermissionRuleBehavior::Allow)
                .expect("allow rule should be created"),
        );
        rules.insert(
            ToolPermissionRule::from_request(&request, ToolPermissionRuleBehavior::Deny)
                .expect("deny rule should be created"),
        );

        assert_eq!(
            rules.evaluate(&request),
            Some(ToolPermissionRuleBehavior::Deny)
        );
    }

    #[test]
    fn invalid_paths_do_not_create_rules() {
        let request = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "../outside.txt", "content": "one"}),
        );

        assert!(
            ToolPermissionRule::from_request(&request, ToolPermissionRuleBehavior::Allow).is_none()
        );
    }

    #[test]
    fn paths_with_outer_whitespace_do_not_change_scope_during_normalization() {
        let relative_path_disguising_an_absolute_path = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": " /workspace/src/main.rs", "content": "one"}),
        );
        let trailing_whitespace = request(
            "write",
            ToolKind::Write,
            serde_json::json!({"path": "src/main.rs ", "content": "one"}),
        );

        assert!(
            ToolPermissionRule::from_request(
                &relative_path_disguising_an_absolute_path,
                ToolPermissionRuleBehavior::Allow,
            )
            .is_none()
        );
        assert!(
            ToolPermissionRule::from_request(
                &trailing_whitespace,
                ToolPermissionRuleBehavior::Allow,
            )
            .is_none()
        );
    }

    #[test]
    fn exact_arguments_compare_structured_json_without_call_ids() {
        let original = request(
            "custom",
            ToolKind::Other,
            serde_json::json!({"first": 1, "second": {"value": true}}),
        );
        let same_arguments = request(
            "custom",
            ToolKind::Other,
            serde_json::json!({"second": {"value": true}, "first": 1}),
        );
        let changed_arguments = request(
            "custom",
            ToolKind::Other,
            serde_json::json!({"first": 2, "second": {"value": true}}),
        );
        let rule = ToolPermissionRule::from_request(&original, ToolPermissionRuleBehavior::Allow)
            .expect("exact call should produce a rule");
        let mut rules = ToolPermissionRuleSet::default();
        rules.insert(rule);

        assert_eq!(
            rules.evaluate(&same_arguments),
            Some(ToolPermissionRuleBehavior::Allow)
        );
        assert_eq!(rules.evaluate(&changed_arguments), None);
    }
}
