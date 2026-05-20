use std::{fs, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use mo_core::session::{ChatMessage, ChatMessageBlock};
use url::Url;

use crate::path_resolve::resolve_path_token;

const MAX_EMBEDDED_TEXT_BYTES: u64 = 512 * 1024;
const MAX_EMBEDDED_BINARY_BYTES: u64 = 5 * 1024 * 1024;

/// `build_native_chat_message_from_composer_text` 将 composer 文本归一化为 native chat message。
pub(crate) fn build_native_chat_message_from_composer_text(
    text: &str,
    current_dir: impl AsRef<Path>,
) -> ChatMessage {
    let root = current_dir.as_ref();
    let mut blocks = Vec::new();
    let mut text_start = 0usize;
    let mut cursor = 0usize;
    let mut transformed_any = false;

    while let Some((at_offset, _)) = text[cursor..].char_indices().find(|(_, ch)| *ch == '@') {
        let at_index = cursor + at_offset;
        if !is_token_boundary_before(text, at_index) {
            cursor = at_index + '@'.len_utf8();
            continue;
        }

        let token_end = token_end_after_at(text, at_index);
        let token = &text[at_index + '@'.len_utf8()..token_end];
        if token.is_empty() {
            cursor = token_end;
            continue;
        }

        let Some(block) = prompt_block_for_path_token(token, root) else {
            cursor = token_end;
            continue;
        };

        push_text_block(&mut blocks, &text[text_start..at_index]);
        blocks.push(block);
        transformed_any = true;
        text_start = token_end;
        cursor = token_end;
    }

    push_text_block(&mut blocks, &text[text_start..]);
    if transformed_any {
        ChatMessage::user_with_blocks(text.to_string(), Some(blocks))
    } else {
        ChatMessage::user(text.to_string())
    }
}

fn prompt_block_for_path_token(token: &str, root: &Path) -> Option<ChatMessageBlock> {
    let path = resolve_path_token(root, token);
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    let mime_type = mime_type_for_path(&path);
    let uri = file_uri(&path);

    if is_image_mime(mime_type.as_deref()) && metadata.len() <= MAX_EMBEDDED_BINARY_BYTES {
        let data = fs::read(&path).ok()?;
        return Some(ChatMessageBlock::Image {
            data_base64: STANDARD.encode(data),
            mime_type: mime_type?,
            uri,
        });
    }

    if is_audio_mime(mime_type.as_deref()) && metadata.len() <= MAX_EMBEDDED_BINARY_BYTES {
        let data = fs::read(&path).ok()?;
        return Some(ChatMessageBlock::Audio {
            data_base64: STANDARD.encode(data),
            mime_type: mime_type?,
            uri,
        });
    }

    if matches!(mime_type.as_deref(), Some("application/pdf"))
        && metadata.len() <= MAX_EMBEDDED_BINARY_BYTES
    {
        let data = fs::read(&path).ok()?;
        return Some(ChatMessageBlock::Document {
            data_base64: STANDARD.encode(data),
            mime_type: "application/pdf".to_string(),
            filename: path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            uri,
        });
    }

    if metadata.len() > MAX_EMBEDDED_TEXT_BYTES {
        return None;
    }

    let text = read_embeddable_text(&path)?;
    Some(ChatMessageBlock::Text(render_embedded_text_file(
        token,
        mime_type.as_deref(),
        &text,
    )))
}

fn read_embeddable_text(path: &Path) -> Option<String> {
    let text = String::from_utf8(fs::read(path).ok()?).ok()?;
    looks_like_plain_text(&text).then_some(text)
}

fn render_embedded_text_file(token: &str, mime_type: Option<&str>, text: &str) -> String {
    match mime_type {
        Some(mime_type) => format!("[Attached file: {token} ({mime_type})]\n{text}"),
        None => format!("[Attached file: {token}]\n{text}"),
    }
}

fn looks_like_plain_text(text: &str) -> bool {
    text.chars()
        .all(|ch| ch == '\n' || ch == '\r' || ch == '\t' || !ch.is_control())
}

fn push_text_block(blocks: &mut Vec<ChatMessageBlock>, text: &str) {
    if !text.is_empty() {
        blocks.push(ChatMessageBlock::Text(text.to_string()));
    }
}

fn is_token_boundary_before(text: &str, at_index: usize) -> bool {
    text[..at_index]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

fn token_end_after_at(text: &str, at_index: usize) -> usize {
    text[at_index + '@'.len_utf8()..]
        .char_indices()
        .find_map(|(offset, ch)| {
            ch.is_whitespace()
                .then_some(at_index + '@'.len_utf8() + offset)
        })
        .unwrap_or(text.len())
}

fn file_uri(path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    Url::from_file_path(absolute)
        .ok()
        .map(|url| url.to_string())
}

fn mime_type_for_path(path: &Path) -> Option<String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())?
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "svg" => "image/svg+xml",
        "wav" => "audio/wav",
        "mp3" => "audio/mp3",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/m4a",
        "aac" => "audio/aac",
        "opus" => "audio/ogg",
        "pdf" => "application/pdf",
        "rs" => "text/plain",
        "py" => "text/x-python",
        "js" | "mjs" | "cjs" => "text/x-javascript",
        "ts" | "tsx" | "jsx" => "text/plain",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "md" | "markdown" => "text/markdown",
        "txt" | "text" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "xml" => "text/xml",
        "csv" => "text/csv",
        "sh" | "bash" | "zsh" => "text/plain",
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" | "java" | "go" | "kt" | "kts" | "swift"
        | "php" | "rb" | "lua" | "sql" => "text/plain",
        _ => return None,
    };
    Some(mime.to_string())
}

fn is_image_mime(mime_type: Option<&str>) -> bool {
    mime_type.is_some_and(|mime_type| mime_type.starts_with("image/"))
}

fn is_audio_mime(mime_type: Option<&str>) -> bool {
    mime_type.is_some_and(|mime_type| mime_type.starts_with("audio/"))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use mo_core::session::ChatMessageBlock;

    use super::build_native_chat_message_from_composer_text;

    #[test]
    fn native_prompt_builder_embeds_supported_files_into_blocks() {
        let root = temp_root("native-prompt-rich");
        fs::create_dir_all(root.join("assets")).expect("create assets dir");
        fs::create_dir_all(root.join("src")).expect("create src dir");
        fs::write(root.join("assets/sample.png"), [0x89, b'P', b'N', b'G'])
            .expect("write image fixture");
        fs::write(root.join("src/code.py"), b"print('hi')\n").expect("write text fixture");

        let message = build_native_chat_message_from_composer_text(
            "review @assets/sample.png @src/code.py",
            &root,
        );

        let blocks = message
            .blocks
            .expect("message should contain structured blocks");
        assert!(matches!(&blocks[0], ChatMessageBlock::Text(text) if text == "review "));
        assert!(matches!(
            &blocks[1],
            ChatMessageBlock::Image { mime_type, data_base64, .. }
                if mime_type == "image/png" && data_base64 == "iVBORw=="
        ));
        assert!(matches!(&blocks[2], ChatMessageBlock::Text(text) if text == " "));
        assert!(matches!(
            &blocks[3],
            ChatMessageBlock::Text(text)
                if text.contains("[Attached file: src/code.py (text/x-python)]")
                    && text.contains("print('hi')")
        ));

        cleanup(&root);
    }

    #[test]
    fn native_prompt_builder_leaves_unsupported_binary_as_plain_text() {
        let root = temp_root("native-prompt-unsupported");
        fs::write(root.join("archive.zip"), [0x50, 0x4b, 0x03, 0x04]).expect("write zip fixture");

        let message = build_native_chat_message_from_composer_text("inspect @archive.zip", &root);

        assert!(message.blocks.is_none());
        assert_eq!(message.content, "inspect @archive.zip");
        cleanup(&root);
    }

    #[test]
    fn native_prompt_builder_embeds_utf8_text_without_known_extension() {
        let root = temp_root("native-prompt-extensionless");
        fs::write(root.join("Dockerfile"), b"FROM rust:1.89\n").expect("write dockerfile");

        let message = build_native_chat_message_from_composer_text("inspect @Dockerfile", &root);

        let blocks = message
            .blocks
            .expect("message should contain structured blocks");
        assert!(matches!(&blocks[0], ChatMessageBlock::Text(text) if text == "inspect "));
        assert!(matches!(
            &blocks[1],
            ChatMessageBlock::Text(text)
                if text.contains("[Attached file: Dockerfile]")
                    && text.contains("FROM rust:1.89")
        ));
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
