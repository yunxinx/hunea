use super::*;

impl Model {
    pub(in crate::prompt_overlay) fn core_system_editor_body_for_scope(
        &self,
        scope: PromptAssemblyScope,
    ) -> String {
        match scope {
            PromptAssemblyScope::Global => self
                .prompt_assembly
                .core_system
                .global_override
                .clone()
                .unwrap_or_else(|| self.prompt_assembly.core_system.builtin_body.clone()),
            PromptAssemblyScope::Project => self
                .prompt_assembly
                .core_system
                .project_override
                .clone()
                .or_else(|| self.prompt_assembly.core_system.global_override.clone())
                .unwrap_or_else(|| self.prompt_assembly.core_system.builtin_body.clone()),
        }
    }

    pub(in crate::prompt_overlay) fn skill_discovery_editor_body_for_scope(
        &self,
        scope: PromptAssemblyScope,
    ) -> String {
        let origin = Some(match scope {
            PromptAssemblyScope::Global => PromptSourceOrigin::Global,
            PromptAssemblyScope::Project => PromptSourceOrigin::Project,
        });
        let body = self
            .prompt_assembly
            .sources
            .preview
            .iter()
            .find(|source| {
                source.reference_id == "skill-discovery"
                    && source.kind == PromptSourceKind::SkillDiscovery
                    && source.origin == origin
            })
            .and_then(|source| source.body.clone())
            .unwrap_or_default();
        if body.is_empty() {
            return format!("{SKILL_DISCOVERY_GENERATED_START}\n{SKILL_DISCOVERY_GENERATED_END}\n");
        }
        if body.contains(SKILL_DISCOVERY_GENERATED_START)
            && body.contains(SKILL_DISCOVERY_GENERATED_END)
        {
            return body;
        }
        format!("{SKILL_DISCOVERY_GENERATED_START}\n{body}\n{SKILL_DISCOVERY_GENERATED_END}\n")
    }

    pub(in crate::prompt_overlay) fn tool_guidelines_editor_body(&self) -> String {
        let body = self
            .prompt_assembly
            .sources
            .preview
            .iter()
            .find(|source| {
                source.reference_id == "tool-guidelines"
                    && source.kind == PromptSourceKind::ToolGuidelines
            })
            .and_then(|source| source.body.clone())
            .unwrap_or_default();
        if body.is_empty() {
            return format!("{TOOL_GUIDELINES_GENERATED_START}\n{TOOL_GUIDELINES_GENERATED_END}\n");
        }
        if body.contains(TOOL_GUIDELINES_GENERATED_START)
            && body.contains(TOOL_GUIDELINES_GENERATED_END)
        {
            return body;
        }
        format!("{TOOL_GUIDELINES_GENERATED_START}\n{body}\n{TOOL_GUIDELINES_GENERATED_END}\n")
    }
}
