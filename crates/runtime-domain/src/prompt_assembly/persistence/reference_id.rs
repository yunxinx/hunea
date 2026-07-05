use std::path::{Component, Path};

pub const PROJECT_PROMPT_ASSEMBLY_FILE_NAME: &str = "prompt-assembly.toml";
pub const PROJECT_PROMPTS_DIR_NAME: &str = "prompts";
pub const PROJECT_CUSTOM_PROMPTS_DIR_NAME: &str = "custom";
pub const PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME: &str = "__core-system__.md";
pub const PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME: &str = "__skill-discovery__.md";
pub const PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME: &str = "__tool-guidelines__.md";

const PROJECT_CORE_SYSTEM_OVERRIDE_REFERENCE_ID: &str = "__core-system__";
const PROJECT_SKILL_DISCOVERY_OVERRIDE_REFERENCE_ID: &str = "__skill-discovery__";
const PROJECT_TOOL_GUIDELINES_OVERRIDE_REFERENCE_ID: &str = "__tool-guidelines__";

const PROJECT_RESERVED_PROMPTS: [ProjectReservedPrompt; 3] = [
    ProjectReservedPrompt {
        reference_id: PROJECT_CORE_SYSTEM_OVERRIDE_REFERENCE_ID,
        file_name: PROJECT_CORE_SYSTEM_OVERRIDE_FILE_NAME,
    },
    ProjectReservedPrompt {
        reference_id: PROJECT_SKILL_DISCOVERY_OVERRIDE_REFERENCE_ID,
        file_name: PROJECT_SKILL_DISCOVERY_OVERRIDE_FILE_NAME,
    },
    ProjectReservedPrompt {
        reference_id: PROJECT_TOOL_GUIDELINES_OVERRIDE_REFERENCE_ID,
        file_name: PROJECT_TOOL_GUIDELINES_OVERRIDE_FILE_NAME,
    },
];

#[derive(Debug, Clone, Copy)]
struct ProjectReservedPrompt {
    reference_id: &'static str,
    file_name: &'static str,
}

/// `InvalidProjectReferenceId` 描述不能安全映射到项目 prompt 文件名的 reference id。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("project prompt reference id must be a single safe file component: {reference_id}")]
pub struct InvalidProjectReferenceId {
    reference_id: String,
}

impl InvalidProjectReferenceId {
    /// 返回被拒绝的 reference id。
    #[must_use]
    pub fn reference_id(&self) -> &str {
        &self.reference_id
    }
}

pub(super) fn project_extra_prompt_file_name(
    reference_id: &str,
) -> Result<String, InvalidProjectReferenceId> {
    validate_project_reference_id(reference_id)?;
    Ok(format!("{reference_id}.md"))
}

pub(super) fn project_extra_prompt_reference_id_from_file_name(file_name: &str) -> Option<&str> {
    if is_reserved_project_prompt_file_name(file_name) {
        return None;
    }

    let reference_id = file_name.strip_suffix(".md")?;
    validate_project_reference_id(reference_id).ok()?;
    Some(reference_id)
}

pub(super) fn validate_project_reference_id(
    reference_id: &str,
) -> Result<(), InvalidProjectReferenceId> {
    if reference_id.is_empty() || is_reserved_project_prompt_reference_id(reference_id) {
        return Err(invalid_project_reference_id(reference_id));
    }

    let path = Path::new(reference_id);
    let mut components = path.components();
    let Some(Component::Normal(_)) = components.next() else {
        return Err(invalid_project_reference_id(reference_id));
    };
    if components.next().is_some() {
        return Err(invalid_project_reference_id(reference_id));
    }

    Ok(())
}

fn is_reserved_project_prompt_reference_id(reference_id: &str) -> bool {
    PROJECT_RESERVED_PROMPTS
        .iter()
        .any(|reserved| reserved.reference_id == reference_id)
}

fn is_reserved_project_prompt_file_name(file_name: &str) -> bool {
    PROJECT_RESERVED_PROMPTS
        .iter()
        .any(|reserved| reserved.file_name == file_name)
}

fn invalid_project_reference_id(reference_id: &str) -> InvalidProjectReferenceId {
    InvalidProjectReferenceId {
        reference_id: reference_id.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_prompt_metadata_drives_file_filter_and_reference_validation() {
        for reserved in PROJECT_RESERVED_PROMPTS {
            assert!(validate_project_reference_id(reserved.reference_id).is_err());
            assert_eq!(
                project_extra_prompt_reference_id_from_file_name(reserved.file_name),
                None
            );
        }
    }

    #[test]
    fn project_reference_id_must_be_single_safe_path_component() {
        for reference_id in ["", ".", "..", "nested/name", "/absolute"] {
            assert!(validate_project_reference_id(reference_id).is_err());
        }

        assert_eq!(
            project_extra_prompt_file_name("review-rules")
                .expect("valid reference id should produce a markdown file name"),
            "review-rules.md"
        );
        assert_eq!(
            project_extra_prompt_reference_id_from_file_name("review-rules.md"),
            Some("review-rules")
        );
    }
}
