//! Composer prompt message assembly.

#[cfg(test)]
use std::path::Path;

use runtime_domain::session::{
    TranscriptCustomPromptBinding, TranscriptSkillBinding, TranscriptUserAttachment,
    TranscriptUserMessage, transcript_image_label_ranges,
};

/// TUI 内部保存的用户输入源消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerSourceMessage {
    message: TranscriptUserMessage,
}

impl ComposerSourceMessage {
    /// 创建用户输入源消息。
    pub(crate) fn user_text(content: impl Into<String>) -> Self {
        Self {
            message: TranscriptUserMessage {
                content: content.into(),
                attachments: Vec::new(),
                skill_bindings: Vec::new(),
                custom_prompt_bindings: Vec::new(),
            },
        }
    }

    /// 使用显式结构化绑定创建用户输入源消息。
    #[cfg(test)]
    pub(crate) fn user_text_with_bindings(
        content: impl Into<String>,
        skill_bindings: Vec<TranscriptSkillBinding>,
        custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
    ) -> Self {
        Self::user_text_with_bindings_and_attachments(
            content,
            skill_bindings,
            custom_prompt_bindings,
            Vec::new(),
        )
    }

    /// 使用显式结构化绑定和附件创建用户输入源消息。
    pub(crate) fn user_text_with_bindings_and_attachments(
        content: impl Into<String>,
        skill_bindings: Vec<TranscriptSkillBinding>,
        custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
        attachments: Vec<TranscriptUserAttachment>,
    ) -> Self {
        Self {
            message: TranscriptUserMessage {
                content: content.into(),
                attachments,
                skill_bindings,
                custom_prompt_bindings,
            },
        }
    }

    /// 使用完整 transcript user message 创建源消息。
    pub(crate) fn from_transcript_user_message(message: TranscriptUserMessage) -> Self {
        Self { message }
    }

    /// 返回原始用户输入文本。
    pub(crate) fn content(&self) -> &str {
        &self.message.content
    }

    /// 返回当前消息里的 skill 绑定。
    pub(crate) fn skill_bindings(&self) -> &[TranscriptSkillBinding] {
        &self.message.skill_bindings
    }

    /// 返回当前消息里的 custom prompt 绑定。
    pub(crate) fn custom_prompt_bindings(&self) -> &[TranscriptCustomPromptBinding] {
        &self.message.custom_prompt_bindings
    }

    /// 返回当前消息里的结构化附件。
    #[cfg(test)]
    pub(crate) fn attachments(&self) -> &[TranscriptUserAttachment] {
        &self.message.attachments
    }

    /// 返回用户消息里需要着色的附件占位符范围。
    pub(crate) fn attachment_highlight_ranges(&self) -> Vec<(usize, usize)> {
        image_attachment_ranges_in_text(&self.message.content, &self.message.attachments)
    }

    /// 返回该消息是否有需要 projection 渲染的结构化片段。
    pub(crate) fn has_structured_highlights(&self) -> bool {
        !self.message.skill_bindings.is_empty()
            || !self.message.custom_prompt_bindings.is_empty()
            || !self.message.attachments.is_empty()
    }

    /// 返回完整 transcript-visible 用户消息。
    pub(crate) fn as_transcript_user_message(&self) -> &TranscriptUserMessage {
        &self.message
    }

    /// 消费并返回原始用户输入文本。
    #[cfg(test)]
    pub(crate) fn into_content(self) -> String {
        self.message.content
    }

    /// 消费并返回 transcript-visible 用户消息。
    pub(crate) fn into_transcript_user_message(self) -> TranscriptUserMessage {
        self.message
    }
}

fn image_attachment_ranges_in_text(
    content: &str,
    attachments: &[TranscriptUserAttachment],
) -> Vec<(usize, usize)> {
    transcript_image_label_ranges(content)
        .into_iter()
        .take(attachments.len())
        .collect()
}

/// `source_message_from_composer_text` 保留用户输入原文。
///
/// `@path` 是给模型看的路径引用，不在 TUI 层展开文件内容；模型需要内容时应显式调用
/// `read` 工具，避免把文件快照和用户指令混进同一个 prompt。
#[cfg(test)]
pub(crate) fn source_message_from_composer_text(
    text: &str,
    _current_dir: impl AsRef<Path>,
) -> ComposerSourceMessage {
    ComposerSourceMessage::user_text(text)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{ComposerSourceMessage, source_message_from_composer_text};

    #[test]
    fn composer_message_builder_preserves_at_file_references_as_text() {
        let root = temp_root("composer-message-at-text");
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("src/code.py"), b"print('hi')\n").expect("write text fixture");

        let message = source_message_from_composer_text("review @src/code.py", &root);

        assert_eq!(message.content(), "review @src/code.py");
        cleanup(&root);
    }

    #[test]
    fn composer_message_builder_preserves_at_image_references_as_text() {
        let root = temp_root("composer-message-at-image");
        fs::create_dir_all(root.join("assets")).expect("create assets dir");
        fs::write(root.join("assets/sample.png"), [0x89, b'P', b'N', b'G'])
            .expect("write image fixture");

        let message = source_message_from_composer_text("inspect @assets/sample.png", &root);

        assert_eq!(message.content(), "inspect @assets/sample.png");
        cleanup(&root);
    }

    #[test]
    fn composer_source_message_can_return_owned_content() {
        let message = ComposerSourceMessage::user_text("hello");

        assert_eq!(message.into_content(), "hello");
    }

    fn temp_root(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("hunea-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
