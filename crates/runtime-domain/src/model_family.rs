#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelFamily {
    OpenAiGpt4oMini,
    OpenAiGpt4o,
    ClaudeSonnet4,
    ClaudeOpus4,
    Qwen,
    Deepseek,
    Llama,
    MistralV3,
    OpenAiLegacy,
    OpenAiGptOss,
    OpenAiModern,
    Unknown,
}

impl ModelFamily {
    pub(crate) fn built_in_context_limit(self) -> Option<usize> {
        match self {
            Self::OpenAiGpt4oMini | Self::OpenAiGpt4o => Some(128_000),
            Self::ClaudeSonnet4 | Self::ClaudeOpus4 => Some(200_000),
            Self::Qwen
            | Self::Deepseek
            | Self::Llama
            | Self::MistralV3
            | Self::OpenAiLegacy
            | Self::OpenAiGptOss
            | Self::OpenAiModern
            | Self::Unknown => None,
        }
    }

    pub(crate) fn preferred_encoding(self) -> Option<&'static str> {
        match self {
            Self::Qwen => Some("qwen2"),
            Self::Deepseek => Some("deepseek_v3"),
            Self::Llama => Some("llama3"),
            Self::MistralV3 => Some("mistral_v3"),
            Self::OpenAiLegacy => Some("cl100k_base"),
            Self::OpenAiGptOss => Some("o200k_harmony"),
            Self::OpenAiGpt4o | Self::OpenAiGpt4oMini | Self::OpenAiModern => Some("o200k_base"),
            Self::ClaudeSonnet4 | Self::ClaudeOpus4 | Self::Unknown => None,
        }
    }
}

pub(crate) fn classify_model_family(model_id: &str) -> ModelFamily {
    let normalized = model_id.trim().to_ascii_lowercase();

    if model_family_matches(&normalized, "gpt-4o-mini") {
        return ModelFamily::OpenAiGpt4oMini;
    }
    if model_family_matches(&normalized, "gpt-4o") {
        return ModelFamily::OpenAiGpt4o;
    }
    if model_family_matches(&normalized, "claude-sonnet-4") {
        return ModelFamily::ClaudeSonnet4;
    }
    if model_family_matches(&normalized, "claude-opus-4") {
        return ModelFamily::ClaudeOpus4;
    }
    if normalized.contains("qwen") {
        return ModelFamily::Qwen;
    }
    if normalized.contains("deepseek") {
        return ModelFamily::Deepseek;
    }
    if normalized.contains("llama") {
        return ModelFamily::Llama;
    }
    if normalized.contains("mistral")
        || normalized.contains("mixtral")
        || normalized.contains("codestral")
    {
        return ModelFamily::MistralV3;
    }
    if normalized.contains("gpt-3.5") || contains_legacy_gpt4_alias(&normalized) {
        return ModelFamily::OpenAiLegacy;
    }
    if normalized.contains("gpt-oss") {
        return ModelFamily::OpenAiGptOss;
    }
    if normalized.contains("gpt")
        || normalized.starts_with("o1")
        || normalized.starts_with("o3")
        || normalized.starts_with("o4")
    {
        return ModelFamily::OpenAiModern;
    }

    ModelFamily::Unknown
}

fn model_family_matches(model_id: &str, family: &str) -> bool {
    model_id == family
        || model_id
            .strip_prefix(family)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

fn contains_legacy_gpt4_alias(model_id: &str) -> bool {
    model_id.contains("gpt-4")
        && !model_id.contains("gpt-4.1")
        && !model_id.contains("gpt-4o")
        && !model_id.contains("gpt-4.5")
}

#[cfg(test)]
mod tests {
    use super::{ModelFamily, classify_model_family};

    #[test]
    fn classify_distinguishes_context_limit_families_from_generic_gpt_aliases() {
        assert_eq!(classify_model_family("gpt-4o"), ModelFamily::OpenAiGpt4o);
        assert_eq!(
            classify_model_family("my-gpt-4o-wrapper"),
            ModelFamily::OpenAiModern
        );
        assert_eq!(
            classify_model_family("claude-sonnet-4-20250514"),
            ModelFamily::ClaudeSonnet4
        );
    }

    #[test]
    fn classify_covers_local_alias_families_used_for_token_estimation() {
        assert_eq!(classify_model_family("local/qwen3"), ModelFamily::Qwen);
        assert_eq!(
            classify_model_family("custom-deepseek-chat"),
            ModelFamily::Deepseek
        );
        assert_eq!(
            classify_model_family("gpt-oss-120b-local"),
            ModelFamily::OpenAiGptOss
        );
    }
}
