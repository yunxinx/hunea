//! Composer prompt message assembly.

use std::path::Path;

use runtime_domain::session::ChatMessage;

/// `chat_message_from_composer_text` 保留用户输入原文。
///
/// `@path` 是给模型看的路径引用，不在 TUI 层展开文件内容；模型需要内容时应显式调用
/// `read` 工具，避免把文件快照和用户指令混进同一个 prompt。
pub(crate) fn chat_message_from_composer_text(
    text: &str,
    _current_dir: impl AsRef<Path>,
) -> ChatMessage {
    ChatMessage::user(text.to_string())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::chat_message_from_composer_text;

    #[test]
    fn composer_message_builder_preserves_at_file_references_as_text() {
        let root = temp_root("composer-message-at-text");
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("src/code.py"), b"print('hi')\n").expect("write text fixture");

        let message = chat_message_from_composer_text("review @src/code.py", &root);

        assert_eq!(message.content, "review @src/code.py");
        assert!(message.blocks.is_none());
        cleanup(&root);
    }

    #[test]
    fn composer_message_builder_preserves_at_image_references_as_text() {
        let root = temp_root("composer-message-at-image");
        fs::create_dir_all(root.join("assets")).expect("create assets dir");
        fs::write(root.join("assets/sample.png"), [0x89, b'P', b'N', b'G'])
            .expect("write image fixture");

        let message = chat_message_from_composer_text("inspect @assets/sample.png", &root);

        assert_eq!(message.content, "inspect @assets/sample.png");
        assert!(message.blocks.is_none());
        cleanup(&root);
    }

    fn temp_root(prefix: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("lumos-{prefix}-{}-{stamp}", std::process::id()));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }
}
