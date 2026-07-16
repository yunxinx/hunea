use super::*;
use runtime_domain::prompt_assembly::persistence::{
    PersistedPromptAssemblyEntry, PersistedSkillDiscoverySkillEntry, PersistedToolSelectionEntry,
    PromptAssemblyScope, PromptAssemblyScopeState, StoredPromptBody,
    save_project_prompt_assembly_state,
};
use session_store::InMemorySessionStore;

macro_rules! scope_state {
    (scope: $scope:expr, $($field:ident $(: $value:expr)?),* $(,)?) => {{
        let mut state = PromptAssemblyScopeState::new($scope);
        $(scope_state!(@assign state, $field $(: $value)?);)*
        state
    }};
    (@assign $state:ident, core_system_override : $value:expr) => {
        $state.set_core_system_override($value);
    };
    (@assign $state:ident, skill_discovery_override : $value:expr) => {
        $state.set_skill_discovery_override($value);
    };
    (@assign $state:ident, tool_guidelines_override : $value:expr) => {
        $state.set_tool_guidelines_override($value);
    };
    (@assign $state:ident, entries : $value:expr) => {
        $state.set_entries($value);
    };
    (@assign $state:ident, skill_discovery_skills : $value:expr) => {
        $state.set_skill_discovery_skills($value);
    };
    (@assign $state:ident, tool_selections : $value:expr) => {
        $state.set_tool_selections($value);
    };
    (@assign $state:ident, tool_enablement : $value:expr) => {
        $state.set_tool_enablement($value);
    };
    (@assign $state:ident, dynamic_environment_sources : $value:expr) => {
        $state.set_dynamic_environment_sources($value);
    };
    (@assign $state:ident, extra_prompts : $value:expr) => {
        $state.set_extra_prompts($value);
    };
    (@assign $state:ident, $field:ident) => {
        scope_state!(@assign $state, $field : $field);
    };
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "hunea-terminal-app-prompt-assembly-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("temp dir should exist");
    dir
}

fn write_project_skill(work_dir: &Path, skill_name: &str) {
    let skill_dir = work_dir.join(".agents").join("skills").join(skill_name);
    fs::create_dir_all(&skill_dir).expect("skill dir should exist");
    fs::write(
        skill_dir.join(SKILL_FILE_NAME),
        format!(
            "---\nname: {skill_name}\ndescription: Test skill\ndisable-model-invocation: false\n---\n# {skill_name}\n\nUse this skill.\n"
        ),
    )
    .expect("skill file should exist");
}

fn builtin_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new("bash")
            .with_label("Bash")
            .with_description("Run shell commands")
            .with_prompt_guidelines("Prefer rg over grep."),
        ToolDefinition::new("read_file")
            .with_label("Read file")
            .with_description("Read workspace files")
            .with_prompt_guidelines("Use for direct file reads."),
    ]
}

fn tool_definitions_with_unguided_tool() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::new("authorize_search_download")
            .with_label("Authorize search download")
            .with_description("Install a managed search helper"),
        ToolDefinition::new("bash")
            .with_label("Bash")
            .with_description("Run shell commands")
            .with_prompt_guidelines("Prefer rg over grep."),
        ToolDefinition::new("read_file")
            .with_label("Read file")
            .with_description("Read workspace files")
            .with_prompt_guidelines("Use for direct file reads."),
    ]
}

mod attached_prompt;
mod discovery;
mod mutation;
mod persistence;
mod resolution;
