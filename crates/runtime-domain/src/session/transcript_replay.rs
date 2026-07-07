use crate::prompt_assembly::PromptSourceOrigin;
use provider_protocol::{ContentBlock, ConversationItem, ImageDetail};
use serde::{Deserialize, Serialize};

use super::activity::{RuntimeTerminalSnapshot, RuntimeToolActivity};

/// `transcript_image_label_text` 返回 transcript 中本地图片附件的可见占位符。
#[must_use]
pub fn transcript_image_label_text(label_number: usize) -> String {
    format!("[Image #{label_number}]")
}

/// `transcript_image_label_ranges` 返回 transcript 文本里的本地图片占位符字符范围。
#[must_use]
pub fn transcript_image_label_ranges(content: &str) -> Vec<(usize, usize)> {
    const IMAGE_LABEL_PREFIX: &str = "[Image #";

    let mut ranges = Vec::new();
    let mut byte_cursor = 0;
    let mut char_cursor = 0;
    while let Some(relative_start) = content[byte_cursor..].find(IMAGE_LABEL_PREFIX) {
        let byte_start = byte_cursor + relative_start;
        char_cursor += content[byte_cursor..byte_start].chars().count();
        let after_prefix = byte_start + IMAGE_LABEL_PREFIX.len();
        let Some(rest) = content.get(after_prefix..) else {
            break;
        };
        let digit_len = rest
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .map(char::len_utf8)
            .sum::<usize>();
        if digit_len == 0 || !content[after_prefix + digit_len..].starts_with(']') {
            let next_char_len = content[byte_start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
            byte_cursor = byte_start + next_char_len;
            char_cursor += 1;
            continue;
        }
        let byte_end = after_prefix + digit_len + ']'.len_utf8();
        let label_char_len = content[byte_start..byte_end].chars().count();
        ranges.push((char_cursor, char_cursor + label_char_len));
        byte_cursor = byte_end;
        char_cursor += label_char_len;
    }
    ranges
}

/// `TranscriptReplayItem` 表示从 canonical session history 重建 TUI transcript 的语义项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum TranscriptReplayItem {
    Message {
        role: TranscriptReplayRole,
        content: String,
    },
    BoundUserMessage {
        message: TranscriptUserMessage,
    },
    Reasoning {
        content: String,
    },
    ToolActivity {
        activity: RuntimeToolActivity,
    },
    TerminalSnapshot {
        snapshot: RuntimeTerminalSnapshot,
    },
    ToolResult {
        content: String,
    },
    System {
        content: String,
    },
}

impl TranscriptReplayItem {
    /// `content_text` 返回该 replay 项适合测试和搜索使用的主文本。
    pub fn content_text(&self) -> &str {
        match self {
            Self::Message { content, .. }
            | Self::BoundUserMessage {
                message: TranscriptUserMessage { content, .. },
            }
            | Self::Reasoning { content }
            | Self::ToolResult { content }
            | Self::System { content } => content,
            Self::ToolActivity { activity } => &activity.title,
            Self::TerminalSnapshot { snapshot } => snapshot
                .command
                .as_deref()
                .filter(|command| !command.is_empty())
                .or_else(|| (!snapshot.output.is_empty()).then_some(snapshot.output.as_str()))
                .unwrap_or(snapshot.terminal_id.as_str()),
        }
    }
}

/// `TranscriptSkillBinding` 表示一次 user transcript 中仍可恢复的 `$skill` 结构化绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSkillBinding {
    pub skill_name: String,
    pub origin: PromptSourceOrigin,
    pub skill_path: String,
    pub start_char: usize,
    pub end_char: usize,
}

impl TranscriptSkillBinding {
    /// `visible_token_text` 返回 transcript 中应当出现的 `$skill` 可见 token。
    #[must_use]
    pub fn visible_token_text(&self) -> String {
        format!("${}", self.skill_name)
    }
}

/// `TranscriptCustomPromptBinding` 表示一次 user transcript 中仍可恢复的 `#prompt` 结构化绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptCustomPromptBinding {
    pub reference_id: String,
    pub origin: PromptSourceOrigin,
    pub start_char: usize,
    pub end_char: usize,
}

impl TranscriptCustomPromptBinding {
    /// `visible_token_text` 返回 transcript 中应当出现的 `#prompt` 可见 token。
    #[must_use]
    pub fn visible_token_text(&self) -> String {
        format!("#{}", self.reference_id)
    }
}

/// `TranscriptUserMessage` 表示 transcript-visible 的用户消息及其可选结构化绑定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TranscriptUserMessage {
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<TranscriptUserAttachment>,
    #[serde(default)]
    pub skill_bindings: Vec<TranscriptSkillBinding>,
    #[serde(default)]
    pub custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
}

impl TranscriptUserMessage {
    /// `provider_content` 返回该用户消息对应的 provider-visible 结构化内容。
    #[must_use]
    pub fn provider_content(&self) -> Vec<ContentBlock> {
        self.provider_content_with_text(self.content.clone())
    }

    /// `provider_content_with_text` 使用替换后的文本与原附件构造 provider-visible 内容。
    #[must_use]
    pub fn provider_content_with_text(&self, text: impl Into<String>) -> Vec<ContentBlock> {
        let text = text.into();
        let mut content =
            Vec::with_capacity(usize::from(!text.is_empty()) + self.attachments.len());
        if !text.is_empty() {
            content.push(ContentBlock::Text(text));
        }
        content.extend(
            self.attachments
                .iter()
                .map(TranscriptUserAttachment::to_content_block),
        );
        content
    }

    /// `provider_message` 返回 provider-neutral user message。
    #[must_use]
    pub fn provider_message(&self) -> ConversationItem {
        ConversationItem::user(self.provider_content())
    }

    /// `display_content` 返回 transcript 恢复时可见的用户消息文本。
    #[must_use]
    pub fn display_content(&self) -> String {
        if self.attachments.is_empty() {
            return self.content.clone();
        }

        let labeled_attachment_count = transcript_image_label_ranges(&self.content)
            .len()
            .min(self.attachments.len());
        let unlabeled_attachment_blocks = self
            .attachments
            .iter()
            .skip(labeled_attachment_count)
            .map(TranscriptUserAttachment::to_content_block)
            .collect::<Vec<_>>();
        let unlabeled_attachment_summary =
            provider_protocol::summary_text_from_blocks(&unlabeled_attachment_blocks);

        if self.content.trim().is_empty() {
            return unlabeled_attachment_summary;
        }
        if unlabeled_attachment_summary.is_empty() {
            return self.content.clone();
        }

        let mut display_content = self.content.clone();
        if !display_content.ends_with('\n') {
            display_content.push('\n');
        }
        display_content.push_str(&unlabeled_attachment_summary);
        display_content
    }

    /// `requires_bound_replay` 判断该消息是否必须用结构化 replay 恢复。
    #[must_use]
    pub fn requires_bound_replay(&self) -> bool {
        !self.attachments.is_empty()
            || !self.skill_bindings.is_empty()
            || !self.custom_prompt_bindings.is_empty()
    }
}

/// `TranscriptUserAttachment` 表示一次用户消息携带的结构化附件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptUserAttachment {
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<ImageDetail>,
    },
}

impl TranscriptUserAttachment {
    /// `local_image` 创建用户从本地选择的图片附件。
    ///
    /// 默认使用 `High`，与 Codex 的本地图片默认细节等级保持一致；需要原始字节语义的
    /// 路径由 `view_image` 工具通过显式 detail 控制。
    #[must_use]
    pub fn local_image(
        data_base64: impl Into<String>,
        mime_type: impl Into<String>,
        uri: Option<String>,
    ) -> Self {
        Self::Image {
            data_base64: data_base64.into(),
            mime_type: mime_type.into(),
            uri,
            detail: Some(ImageDetail::High),
        }
    }

    /// `to_content_block` 将 transcript 附件转成 provider-neutral content block。
    #[must_use]
    pub fn to_content_block(&self) -> ContentBlock {
        match self {
            Self::Image {
                data_base64,
                mime_type,
                uri,
                detail,
            } => ContentBlock::Image {
                data_base64: data_base64.clone(),
                mime_type: mime_type.clone(),
                uri: uri.clone(),
                detail: *detail,
            },
        }
    }
}

/// `TranscriptReplayRole` 是恢复普通消息时可见消息的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptReplayRole {
    User,
    Assistant,
}

#[cfg(test)]
mod tests {
    use super::{TranscriptUserAttachment, TranscriptUserMessage};

    #[test]
    fn labeled_image_attachment_display_content_does_not_duplicate_attachment_summary() {
        let message = TranscriptUserMessage {
            content: "[Image #1] inspect".to_string(),
            attachments: vec![TranscriptUserAttachment::local_image(
                "iVBORw0KGgo=",
                "image/png",
                Some("assets/a.png".to_string()),
            )],
            skill_bindings: Vec::new(),
            custom_prompt_bindings: Vec::new(),
        };

        assert_eq!(message.display_content(), "[Image #1] inspect");
    }

    #[test]
    fn unlabeled_image_attachment_display_content_keeps_visible_summary() {
        let message = TranscriptUserMessage {
            content: "inspect".to_string(),
            attachments: vec![TranscriptUserAttachment::local_image(
                "iVBORw0KGgo=",
                "image/png",
                Some("assets/a.png".to_string()),
            )],
            skill_bindings: Vec::new(),
            custom_prompt_bindings: Vec::new(),
        };

        let display_content = message.display_content();

        assert!(display_content.contains("inspect"));
        assert!(display_content.contains("Attached image"));
        assert!(display_content.contains("assets/a.png"));
    }
}
