use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
enum PromptAttachment {
    ManualSkill {
        start_char: usize,
        use_item: ManualSkillPromptUse,
    },
    CustomPrompt {
        start_char: usize,
        use_item: CustomPromptUse,
    },
}

impl PromptAttachment {
    fn start_char(&self) -> usize {
        match self {
            Self::ManualSkill { start_char, .. } | Self::CustomPrompt { start_char, .. } => {
                *start_char
            }
        }
    }
}

/// `assemble_attached_prompt_message` 解析当前用户消息里的 `$skill` / `#prompt` 提及并拼装 provider-visible 文本。
pub(super) fn assemble_attached_prompt_message(
    manager: Option<&PromptAssemblyManagerSnapshot>,
    work_dir: &Path,
    user_message: &TranscriptUserMessage,
) -> AttachedPromptMessageAssembly {
    let discovered_skills = discover_skills(work_dir, None);
    let skills_by_locator = discovered_skills
        .iter()
        .map(|skill| {
            (
                (
                    skill.name.as_str(),
                    skill.origin,
                    skill.skill_path.as_path(),
                ),
                skill,
            )
        })
        .collect::<HashMap<_, _>>();
    let extra_prompts_by_locator = manager
        .filter(|_| !user_message.custom_prompt_bindings.is_empty())
        .map(|manager| {
            manager
                .candidates
                .extra_prompts
                .iter()
                .map(|prompt| ((prompt.reference_id.clone(), prompt.origin), prompt.clone()))
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();

    let mut attachments = Vec::new();
    for binding in &user_message.skill_bindings {
        let Some(skill) = skills_by_locator.get(&(
            binding.skill_name.as_str(),
            binding.origin,
            Path::new(binding.skill_path.as_str()),
        )) else {
            continue;
        };
        attachments.push(PromptAttachment::ManualSkill {
            start_char: binding.start_char,
            use_item: ManualSkillPromptUse {
                skill_name: skill.name.clone(),
                origin: skill.origin,
                skill_path: skill.skill_path.clone(),
                body: format_long_lived_skill_body(skill),
            },
        });
    }
    for binding in &user_message.custom_prompt_bindings {
        let Some(prompt) =
            extra_prompts_by_locator.get(&(binding.reference_id.clone(), binding.origin))
        else {
            continue;
        };
        attachments.push(PromptAttachment::CustomPrompt {
            start_char: binding.start_char,
            use_item: CustomPromptUse {
                reference_id: prompt.reference_id.clone(),
                origin: prompt.origin,
                title: prompt.title.clone(),
                body: prompt.body.clone(),
            },
        });
    }

    attachments.sort_by_key(PromptAttachment::start_char);
    let mut manual_skill_uses = Vec::new();
    let mut custom_prompt_uses = Vec::new();
    let mut seen_manual_skills = std::collections::HashSet::new();
    let mut seen_custom_prompts = std::collections::HashSet::new();
    let mut sections = Vec::new();
    for attachment in attachments {
        match attachment {
            PromptAttachment::ManualSkill { use_item, .. } => {
                let key = (
                    use_item.skill_name.clone(),
                    use_item.origin,
                    use_item.skill_path.clone(),
                );
                if !seen_manual_skills.insert(key) {
                    continue;
                }
                if !use_item.body.trim().is_empty() {
                    sections.push(use_item.body.clone());
                }
                manual_skill_uses.push(use_item);
            }
            PromptAttachment::CustomPrompt { use_item, .. } => {
                let key = (use_item.reference_id.clone(), use_item.origin);
                if !seen_custom_prompts.insert(key) {
                    continue;
                }
                custom_prompt_uses.push(use_item);
            }
        }
    }

    let expanded_custom_prompt_text = expand_custom_prompt_bindings(
        &user_message.content,
        &user_message.custom_prompt_bindings,
        &extra_prompts_by_locator,
    );
    let provider_visible_user_text = if sections.is_empty() {
        expanded_custom_prompt_text.unwrap_or_else(|| user_message.content.clone())
    } else {
        let visible_user_text = expanded_custom_prompt_text
            .as_deref()
            .unwrap_or(user_message.content.as_str());
        let trimmed_user_text = visible_user_text.trim();
        if !trimmed_user_text.is_empty() {
            sections.push(trimmed_user_text.to_string());
        }
        sections.join("\n\n")
    };

    AttachedPromptMessageAssembly {
        provider_visible_user_text,
        manual_skill_uses,
        custom_prompt_uses,
    }
}
