const APPROX_BYTES_PER_TOKEN: usize = 4;
const FALLBACK_ENCODING: &str = "o200k_base";
const LEGACY_GPT_ENCODING: &str = "cl100k_base";

pub(crate) fn estimate_text_tokens(model_id: &str, text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    estimate_text_tokens_with_encoding_name(encoding_name_for_model(model_id), text)
}

fn estimate_text_tokens_with_encoding_name(encoding_name: &str, text: &str) -> usize {
    tiktoken::get_encoding(encoding_name)
        .or_else(|| tiktoken::get_encoding(FALLBACK_ENCODING))
        .map(|encoding| encoding.count(text))
        .unwrap_or_else(|| approximate_tokens_from_bytes(text.len()))
}

pub(crate) fn encoding_name_for_model(model_id: &str) -> &'static str {
    if let Some(encoding) = tiktoken::model_to_encoding(model_id) {
        return encoding;
    }

    alias_encoding_for_model(model_id).unwrap_or(FALLBACK_ENCODING)
}

fn alias_encoding_for_model(model_id: &str) -> Option<&'static str> {
    let lower = model_id.to_ascii_lowercase();
    if lower.contains("qwen") {
        return Some("qwen2");
    }
    if lower.contains("deepseek") {
        return Some("deepseek_v3");
    }
    if lower.contains("llama") {
        return Some("llama3");
    }
    if lower.contains("mistral") || lower.contains("mixtral") || lower.contains("codestral") {
        return Some("mistral_v3");
    }
    if lower.contains("gpt-3.5") || contains_legacy_gpt4_alias(&lower) {
        return Some(LEGACY_GPT_ENCODING);
    }
    if lower.contains("gpt")
        || lower.starts_with("o1")
        || lower.starts_with("o3")
        || lower.starts_with("o4")
    {
        return Some(FALLBACK_ENCODING);
    }
    None
}

fn contains_legacy_gpt4_alias(model_id: &str) -> bool {
    model_id.contains("gpt-4")
        && !model_id.contains("gpt-4.1")
        && !model_id.contains("gpt-4o")
        && !model_id.contains("gpt-4.5")
}

fn approximate_tokens_from_bytes(bytes: usize) -> usize {
    bytes.saturating_add(APPROX_BYTES_PER_TOKEN - 1) / APPROX_BYTES_PER_TOKEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_text_tokens_uses_tiktoken_for_known_model() {
        assert_eq!(estimate_text_tokens("gpt-4o", "hello world"), 2);
    }

    #[test]
    fn estimate_text_tokens_falls_back_for_local_model_aliases() {
        assert!(estimate_text_tokens("local/qwen3", "你好，lumos") > 0);
        assert!(estimate_text_tokens("custom-deepseek-chat", "hello lumos") > 0);
    }

    #[test]
    fn unavailable_alias_encoding_falls_back_to_o200k() {
        let unavailable_encoding = "definitely_missing_encoding";
        let text = "hello from lumos";

        assert!(tiktoken::get_encoding(unavailable_encoding).is_none());
        assert_eq!(
            estimate_text_tokens_with_encoding_name(unavailable_encoding, text),
            estimate_text_tokens_with_encoding_name(FALLBACK_ENCODING, text)
        );
    }

    #[test]
    fn encoding_name_for_model_uses_o200k_for_modern_gpt_aliases() {
        assert_eq!(encoding_name_for_model("gpt-5.4"), "o200k_base");
        assert_eq!(encoding_name_for_model("gpt-5.3-codex-spark"), "o200k_base");
        assert_eq!(encoding_name_for_model("gpt-4.1-mini"), "o200k_base");
        assert_eq!(encoding_name_for_model("gpt-4o-mini"), "o200k_base");
        assert_eq!(encoding_name_for_model("local-gpt-4.1"), "o200k_base");
        assert_eq!(encoding_name_for_model("local-gpt-4o"), "o200k_base");
        assert_eq!(encoding_name_for_model("gpt-oss-120b"), "o200k_base");
        assert_eq!(encoding_name_for_model("custom-gpt-local"), "o200k_base");
    }

    #[test]
    fn encoding_name_for_model_keeps_legacy_gpt_models_on_cl100k() {
        assert_eq!(encoding_name_for_model("gpt-4"), "cl100k_base");
        assert_eq!(encoding_name_for_model("gpt-4-0613"), "cl100k_base");
        assert_eq!(encoding_name_for_model("gpt-3.5-turbo"), "cl100k_base");
    }

    #[test]
    fn encoding_name_for_model_falls_back_to_o200k_for_unknown_models() {
        assert_eq!(encoding_name_for_model("unknown-local-model"), "o200k_base");
    }
}
