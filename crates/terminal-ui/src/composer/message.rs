//! Composer prompt message assembly.

use std::path::Path;

/// TUI 内部保存的用户输入源消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerSourceMessage {
    content: String,
}

impl ComposerSourceMessage {
    /// 创建用户输入源消息。
    pub(crate) fn user_text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
        }
    }

    /// 返回原始用户输入文本。
    pub(crate) fn content(&self) -> &str {
        &self.content
    }

    /// 消费并返回原始用户输入文本。
    pub(crate) fn into_content(self) -> String {
        self.content
    }
}

/// `source_message_from_composer_text` 保留用户输入原文。
///
/// `@path` 是给模型看的路径引用，不在 TUI 层展开文件内容；模型需要内容时应显式调用
/// `read` 工具，避免把文件快照和用户指令混进同一个 prompt。
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
