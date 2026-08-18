use crate::model_family::classify_model_family;
use crate::token_count::approximate_tokens_from_bytes;

const FALLBACK_ENCODING: &str = "o200k_base";

/// `TokenEncoding` 缓存一次 model 解析得到的 encoding 选择，供重复估算复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenEncoding {
    encoding_name: &'static str,
}

impl TokenEncoding {
    /// `for_model` 解析 model 对应的 token encoding。
    pub fn for_model(model_id: &str) -> Self {
        Self {
            encoding_name: encoding_name_for_model(model_id),
        }
    }

    /// `estimate_text` 使用已解析的 encoding 估算文本 token 数。
    pub fn estimate_text(self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }

        estimate_text_tokens_with_encoding_name(self.encoding_name, text)
    }
}

pub fn estimate_text_tokens(model_id: &str, text: &str) -> usize {
    TokenEncoding::for_model(model_id).estimate_text(text)
}

fn estimate_text_tokens_with_encoding_name(encoding_name: &str, text: &str) -> usize {
    tiktoken::get_encoding(encoding_name)
        .or_else(|| tiktoken::get_encoding(FALLBACK_ENCODING))
        .map(|encoding| encoding.count(text))
        .unwrap_or_else(|| approximate_tokens_from_bytes(text.len()))
}

pub(crate) fn encoding_name_for_model(model_id: &str) -> &'static str {
    encoding_from_tiktoken_catalog(model_id)
        .or_else(|| alias_encoding_for_model(model_id))
        .unwrap_or(FALLBACK_ENCODING)
}

/// tiktoken 的 `model_to_encoding` 只认 prefix/exact。`local/qwen3` 这种
/// `provider/model` 写法先剥最后一段，才能命中 4.x 目录而不是掉进 o200k 兜底。
fn encoding_from_tiktoken_catalog(model_id: &str) -> Option<&'static str> {
    tiktoken::model_to_encoding(model_id).or_else(|| {
        let basename = model_id.rsplit('/').next().unwrap_or(model_id);
        if basename == model_id {
            None
        } else {
            tiktoken::model_to_encoding(basename)
        }
    })
}

fn alias_encoding_for_model(model_id: &str) -> Option<&'static str> {
    classify_model_family(model_id).preferred_encoding()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_token_encoding_matches_direct_model_estimation() {
        let encoding = TokenEncoding::for_model("gpt-4o");

        assert_eq!(
            encoding.estimate_text("hello world"),
            estimate_text_tokens("gpt-4o", "hello world")
        );
    }

    #[test]
    fn resolved_token_encoding_keeps_alias_fallbacks() {
        let encoding = TokenEncoding::for_model("local/qwen3");

        assert!(encoding.estimate_text("你好，hunea") > 0);
    }

    #[test]
    fn estimate_text_tokens_uses_tiktoken_for_known_model() {
        assert_eq!(estimate_text_tokens("gpt-4o", "hello world"), 2);
    }

    #[test]
    fn estimate_text_tokens_falls_back_for_local_model_aliases() {
        assert!(estimate_text_tokens("local/qwen3", "你好，hunea") > 0);
        assert!(estimate_text_tokens("custom-deepseek-chat", "hello hunea") > 0);
    }

    #[test]
    fn unavailable_alias_encoding_falls_back_to_o200k() {
        let unavailable_encoding = "definitely_missing_encoding";
        let text = "hello from hunea";

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
        assert_eq!(encoding_name_for_model("custom-gpt-local"), "o200k_base");
    }

    #[test]
    fn encoding_name_for_model_uses_harmony_for_gpt_oss_aliases() {
        assert_eq!(encoding_name_for_model("gpt-oss-120b"), "o200k_harmony");
        assert_eq!(encoding_name_for_model("gpt-oss-20b"), "o200k_harmony");
    }

    #[test]
    fn gpt_oss_plain_text_estimates_match_o200k_base() {
        let text = "plain text stays on the same BPE path";

        assert_eq!(
            estimate_text_tokens("gpt-oss-120b", text),
            estimate_text_tokens_with_encoding_name(FALLBACK_ENCODING, text)
        );
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

    #[test]
    fn encoding_name_for_model_uses_tiktoken_catalog_instead_of_o200k_fallback() {
        // 4.x 目录 / 别名能给出专用 encoding 的模型，不应再落到 o200k。
        let cases = [
            ("qwen3", "qwen2"),
            ("local/qwen3", "qwen2"),
            ("deepseek-chat", "deepseek_v4"),
            ("deepseek-reasoner", "deepseek_v4"),
            ("local/deepseek-chat", "deepseek_v4"),
            ("custom-deepseek-chat", "deepseek_v4"),
            ("deepseek-r1-distill", "deepseek_v3"),
            ("deepseek-v4-flash", "deepseek_v4"),
            ("kimi-k2", "kimi_k2"),
            ("kimi-k2.6", "kimi_k2"),
            ("local/kimi-k2", "kimi_k2"),
            ("custom-kimi-k2", "kimi_k2"),
            ("kimi-k3", "kimi_k3"),
            ("kimi-latest", "kimi_k3"),
            ("glm-4.5", "glm4"),
            ("local/glm-4.5", "glm4"),
            ("glm-5.2", "glm5"),
            ("minimax-m2.7", "minimax_m2"),
            ("local/minimax-m2", "minimax_m2"),
            ("custom-minimax-m2", "minimax_m2"),
            ("pixtral-12b", "mistral_v3"),
            ("custom-pixtral-12b", "mistral_v3"),
            ("llama-3.3", "llama3"),
            ("local/llama3", "llama3"),
        ];

        for (model_id, encoding) in cases {
            assert_eq!(
                encoding_name_for_model(model_id),
                encoding,
                "{model_id} should use {encoding} rather than o200k fallback"
            );
        }
    }

    #[test]
    fn encoding_name_for_model_keeps_o200k_when_tiktoken_has_no_tokenizer() {
        // Claude / Gemini 在 tiktoken 里只有计价，没有词表；o200k 仍是估算兜底。
        assert_eq!(encoding_name_for_model("claude-sonnet-4-5"), "o200k_base");
        assert_eq!(encoding_name_for_model("gemini-2.5-pro"), "o200k_base");
        assert_eq!(encoding_name_for_model("unknown-local-model"), "o200k_base");
    }

    #[test]
    fn estimate_pins_newline_and_cjk_counts_for_tiktoken_4() {
        // 3.8 起 o200k 把 ".\n/" 收成 1 token；换行 / CJK 是 3.6 起会漂的路径。
        assert_eq!(
            estimate_text_tokens_with_encoding_name("o200k_base", ".\n/"),
            1
        );
        assert_eq!(estimate_text_tokens("gpt-4o", "hello\nworld"), 3);
        assert_eq!(estimate_text_tokens("gpt-4", "hello\nworld"), 3);
        assert_eq!(estimate_text_tokens("local/qwen3", "你好，hunea"), 5);
    }
}
