use super::*;

pub(in crate::prompt_assembly) fn format_tool_guidelines_body(
    tools: &[(String, String)],
) -> String {
    if tools.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "Tool usage guidelines — prefer dedicated tools over shell equivalents.".to_string(),
        String::new(),
        "<tool_guidelines>".to_string(),
    ];
    for (name, guidelines) in tools {
        lines.push(format!("  <tool name=\"{}\">", escape_xml(name)));
        lines.push(format!("    {}", escape_xml(guidelines)));
        lines.push("  </tool>".to_string());
    }
    lines.push("</tool_guidelines>".to_string());
    lines.join("\n")
}

pub(in crate::prompt_assembly) fn render_tool_guidelines_generated_body(
    tool_state: &[PersistedToolSelectionEntry],
    tool_enablement: &[PersistedToolEnablementEntry],
    tool_defs: &[ToolDefinition],
) -> String {
    let disabled_tools = disabled_tool_names(tool_enablement);
    let tools = tool_state
        .iter()
        .filter(|entry| entry.enabled)
        .filter(|entry| !disabled_tools.contains(entry.tool_name.as_str()))
        .filter_map(|entry| {
            tool_defs
                .iter()
                .find(|def| def.name == entry.tool_name)
                .and_then(|def| def.prompt_guidelines.as_deref())
                .map(|guidelines| (entry.tool_name.clone(), guidelines.to_string()))
        })
        .collect::<Vec<_>>();
    format_tool_guidelines_body(&tools)
}

pub(in crate::prompt_assembly) fn resolve_tool_guidelines_body(
    scope: PromptAssemblyScope,
    tool_state: &[PersistedToolSelectionEntry],
    tool_enablement: &[PersistedToolEnablementEntry],
    tool_defs: &[ToolDefinition],
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> String {
    let generated_body =
        render_tool_guidelines_generated_body(tool_state, tool_enablement, tool_defs);
    let override_body = match scope {
        PromptAssemblyScope::Global => global_state.tool_guidelines_override(),
        PromptAssemblyScope::Project => project_state
            .tool_guidelines_override()
            .or(global_state.tool_guidelines_override()),
    };
    match override_body {
        Some(override_body) => rebuild_tool_guidelines_override(override_body, &generated_body),
        None => generated_body,
    }
}

pub(in crate::prompt_assembly) fn rebuild_tool_guidelines_override(
    existing: &str,
    generated_body: &str,
) -> String {
    if let Some((_, suffix)) = parse_tool_guidelines_override(existing) {
        let mut sections = vec![
            TOOL_GUIDELINES_GENERATED_START.to_string(),
            generated_body.to_string(),
            TOOL_GUIDELINES_GENERATED_END.to_string(),
        ];
        if !suffix.is_empty() {
            sections.push(suffix.to_string());
        }
        return sections.join("\n");
    }
    existing.to_string()
}

pub(in crate::prompt_assembly) fn parse_tool_guidelines_override(
    content: &str,
) -> Option<(&str, &str)> {
    let (_, after_start) = content.split_once(TOOL_GUIDELINES_GENERATED_START)?;
    let (_, after_end) = after_start.split_once(TOOL_GUIDELINES_GENERATED_END)?;
    Some((after_start, after_end.trim_start_matches('\n')))
}

pub(in crate::prompt_assembly) fn ensure_tool_guidelines_entry_exists(
    state: &mut PromptAssemblyScopeState,
) {
    if state
        .entries()
        .iter()
        .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        return;
    }
    let requested_order = default_tool_guidelines_requested_order(state.entries());
    state.upsert_entry(PersistedPromptAssemblyEntry {
        reference_id: "tool-guidelines".to_string(),
        kind: PromptSourceKind::ToolGuidelines,
        title: "Tool guidelines".to_string(),
        enabled: true,
        requested_order: Some(requested_order),
    });
}

pub(in crate::prompt_assembly) fn ensure_default_tool_guidelines_source(
    global_state: &mut PromptAssemblyScopeState,
    project_state: &mut PromptAssemblyScopeState,
) {
    if global_state
        .entries()
        .iter()
        .chain(project_state.entries().iter())
        .any(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
    {
        return;
    }

    let target = if tool_guidelines_scope(global_state, project_state, PromptAssemblyScope::Global)
        == PromptAssemblyScope::Project
    {
        project_state
    } else {
        global_state
    };
    ensure_tool_guidelines_entry_exists(target);
}

pub(in crate::prompt_assembly) fn default_tool_guidelines_requested_order(
    entries: &[PersistedPromptAssemblyEntry],
) -> u16 {
    entries
        .iter()
        .find(|entry| entry.kind == PromptSourceKind::ToolGuidelines)
        .and_then(|entry| entry.requested_order)
        .unwrap_or(DEFAULT_TOOL_GUIDELINES_REQUESTED_ORDER)
}

pub(in crate::prompt_assembly) fn tool_guideline_definitions(
    tool_defs: &[ToolDefinition],
) -> impl Iterator<Item = &ToolDefinition> {
    tool_defs
        .iter()
        .filter(|definition| definition.prompt_guidelines.is_some())
}

pub(in crate::prompt_assembly) fn tool_has_prompt_guidelines(
    tool_defs: &[ToolDefinition],
    tool_name: &str,
) -> bool {
    tool_guideline_definitions(tool_defs).any(|definition| definition.name == tool_name)
}

pub(in crate::prompt_assembly) fn merged_tool_selection_state(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    tool_defs: &[ToolDefinition],
) -> Vec<PersistedToolSelectionEntry> {
    let mut state_by_name = HashMap::<String, PersistedToolSelectionEntry>::new();
    for entry in global_state.tool_selections() {
        state_by_name.insert(entry.tool_name.clone(), entry.clone());
    }
    for entry in project_state.tool_selections() {
        state_by_name.insert(entry.tool_name.clone(), entry.clone());
    }

    let mut state = tool_guideline_definitions(tool_defs)
        .enumerate()
        .map(|(index, def)| {
            state_by_name
                .get(&def.name)
                .cloned()
                .unwrap_or(PersistedToolSelectionEntry {
                    tool_name: def.name.clone(),
                    enabled: true,
                    requested_order: Some(u16::try_from(index + 1).unwrap_or(u16::MAX)),
                })
        })
        .collect::<Vec<_>>();
    state.sort_by(|left, right| {
        requested_order_sort_key(left.requested_order)
            .cmp(&requested_order_sort_key(right.requested_order))
            .then_with(|| natural_sort_text_cmp(&left.tool_name, &right.tool_name))
    });
    state
}

/// `merged_tool_enablement_state` 合并 global/project 的工具启停记录并覆盖全部注册工具。
///
/// project 按 tool_name 覆盖 global；未记录的工具默认启用。
/// 输出顺序跟随 `tool_defs`（registry 按名排序），保证确定性。
pub(in crate::prompt_assembly) fn merged_tool_enablement_state(
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
    tool_defs: &[ToolDefinition],
) -> Vec<PersistedToolEnablementEntry> {
    let mut enabled_by_name = HashMap::<&str, bool>::new();
    for entry in global_state.tool_enablement() {
        enabled_by_name.insert(entry.tool_name.as_str(), entry.enabled);
    }
    for entry in project_state.tool_enablement() {
        enabled_by_name.insert(entry.tool_name.as_str(), entry.enabled);
    }

    tool_defs
        .iter()
        .map(|def| PersistedToolEnablementEntry {
            tool_name: def.name.clone(),
            enabled: enabled_by_name
                .get(def.name.as_str())
                .copied()
                .unwrap_or(true),
        })
        .collect()
}

/// `disabled_tool_names` 投影出被禁用的工具名集合，供 guidelines 生成与 registry 过滤使用。
pub(in crate::prompt_assembly) fn disabled_tool_names(
    tool_enablement: &[PersistedToolEnablementEntry],
) -> HashSet<&str> {
    tool_enablement
        .iter()
        .filter(|entry| !entry.enabled)
        .map(|entry| entry.tool_name.as_str())
        .collect()
}

pub(in crate::prompt_assembly) fn tool_candidate_inventory(
    tool_defs: &[ToolDefinition],
    tool_state: &[PersistedToolSelectionEntry],
    tool_enablement: &[PersistedToolEnablementEntry],
    global_state: &PromptAssemblyScopeState,
    project_state: &PromptAssemblyScopeState,
) -> Vec<PromptAssemblyToolCandidate> {
    let merged_state_by_name = tool_state
        .iter()
        .map(|entry| (entry.tool_name.as_str(), entry))
        .collect::<HashMap<_, _>>();
    let selected_order_by_name = tool_state
        .iter()
        .enumerate()
        .map(|(index, entry)| (entry.tool_name.as_str(), index + 1))
        .collect::<HashMap<_, _>>();
    let enabled_by_name = tool_enablement
        .iter()
        .map(|entry| (entry.tool_name.as_str(), entry.enabled))
        .collect::<HashMap<_, _>>();
    let mut inventory = tool_defs
        .iter()
        .map(|def| {
            let has_guidelines = def.prompt_guidelines.is_some();
            PromptAssemblyToolCandidate {
                name: def.name.clone(),
                label: def.label.clone(),
                description: def.description.clone(),
                prompt_guidelines: def.prompt_guidelines.clone(),
                origin: PromptSourceOrigin::Builtin,
                selection_scope: tool_guidelines_scope(
                    global_state,
                    project_state,
                    PromptAssemblyScope::Global,
                ),
                tool_enabled: enabled_by_name
                    .get(def.name.as_str())
                    .copied()
                    .unwrap_or(true),
                selection: PromptAssemblySelectionState::from_parts(
                    has_guidelines,
                    has_guidelines
                        && merged_state_by_name
                            .get(def.name.as_str())
                            .map(|entry| entry.enabled)
                            .unwrap_or(true),
                    selected_order_by_name.get(def.name.as_str()).copied(),
                ),
            }
        })
        .collect::<Vec<_>>();
    inventory.sort_by(|left, right| {
        left.selection
            .selected_order()
            .unwrap_or(usize::MAX)
            .cmp(&right.selection.selected_order().unwrap_or(usize::MAX))
            .then_with(|| natural_sort_text_cmp(&left.name, &right.name))
    });
    inventory
}

pub(in crate::prompt_assembly) fn set_tool_selected(
    state: &mut PromptAssemblyScopeState,
    tool_name: &str,
    selected: bool,
) {
    let next_order = next_tool_requested_order(state.tool_selections());
    if let Some(entry) = state.tool_selection_mut(tool_name) {
        entry.enabled = selected;
        if selected && entry.requested_order.is_none() {
            entry.requested_order = Some(next_order);
        }
        return;
    }

    state.upsert_tool_selection(PersistedToolSelectionEntry {
        tool_name: tool_name.to_string(),
        enabled: selected,
        requested_order: Some(next_order),
    });
}

/// `set_tool_enabled` 记录单个工具本体的启停选择；显式保留 enabled=true 的记录。
pub(in crate::prompt_assembly) fn set_tool_enabled(
    state: &mut PromptAssemblyScopeState,
    tool_name: &str,
    enabled: bool,
) {
    state.upsert_tool_enablement(PersistedToolEnablementEntry {
        tool_name: tool_name.to_string(),
        enabled,
    });
}

pub(in crate::prompt_assembly) fn move_tool(
    state: &mut PromptAssemblyScopeState,
    tool_name: &str,
    direction: PromptAssemblyMoveDirection,
) -> Result<()> {
    let Some(position) = state
        .tool_selections()
        .iter()
        .position(|entry| entry.tool_name == tool_name)
    else {
        return Ok(());
    };
    let Some(neighbor) = (match direction {
        PromptAssemblyMoveDirection::Up => position.checked_sub(1),
        PromptAssemblyMoveDirection::Down => {
            (position + 1 < state.tool_selections().len()).then_some(position + 1)
        }
    }) else {
        return Ok(());
    };
    state.swap_tool_selections(position, neighbor);
    normalize_tool_requested_orders(state);
    Ok(())
}

pub(in crate::prompt_assembly) fn next_tool_requested_order(
    entries: &[PersistedToolSelectionEntry],
) -> u16 {
    entries
        .iter()
        .filter_map(|entry| entry.requested_order)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

pub(in crate::prompt_assembly) fn normalize_tool_requested_orders(
    state: &mut PromptAssemblyScopeState,
) {
    for index in 0..state.tool_selections().len() {
        if let Some(entry) = state.tool_selection_at_mut(index) {
            entry.requested_order = Some(u16::try_from(index + 1).unwrap_or(u16::MAX));
        }
    }
}
