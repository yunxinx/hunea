use std::{
    fs,
    path::{Path, PathBuf},
};

use agent_client_protocol::schema::{
    AudioContent, ContentBlock, EmbeddedResource, EmbeddedResourceResource, ImageContent,
    ResourceLink, TextContent, TextResourceContents,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use url::Url;

use super::AcpAgentIdentity;

const MAX_EMBEDDED_TEXT_BYTES: u64 = 512 * 1024;
const MAX_EMBEDDED_BINARY_BYTES: u64 = 5 * 1024 * 1024;

/// `AcpPrompt` 是 Lumos 内部使用的结构化 ACP prompt 表示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpPrompt {
    blocks: Vec<AcpPromptBlock>,
}

impl AcpPrompt {
    /// `from_text` 创建仅包含一个文本块的 prompt。
    pub fn from_text(text: impl Into<String>) -> Self {
        let text = text.into();
        if text.is_empty() {
            Self { blocks: Vec::new() }
        } else {
            Self {
                blocks: vec![AcpPromptBlock::Text(text)],
            }
        }
    }

    /// `from_blocks` 从已经拆分好的 block 创建 prompt。
    pub fn from_blocks(blocks: Vec<AcpPromptBlock>) -> Self {
        Self { blocks }
    }

    /// `blocks` 返回内部 prompt block 列表。
    pub fn blocks(&self) -> &[AcpPromptBlock] {
        &self.blocks
    }

    pub(crate) fn into_content_blocks(self) -> Vec<ContentBlock> {
        self.blocks.into_iter().map(Into::into).collect()
    }

    /// `to_content_blocks` 转换为 ACP SDK 的 `ContentBlock`。
    #[cfg(test)]
    pub(crate) fn to_content_blocks(&self) -> Vec<ContentBlock> {
        self.blocks
            .iter()
            .cloned()
            .map(ContentBlock::from)
            .collect()
    }
}

/// `AcpPromptBlock` 表示 Lumos prompt 链路可传输的结构化内容块。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcpPromptBlock {
    Text(String),
    ResourceLink {
        name: String,
        uri: String,
        mime_type: Option<String>,
        size: Option<i64>,
    },
    ResourceText {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    Image {
        data_base64: String,
        mime_type: String,
        uri: Option<String>,
    },
    Audio {
        data_base64: String,
        mime_type: String,
    },
}

impl From<AcpPromptBlock> for ContentBlock {
    fn from(block: AcpPromptBlock) -> Self {
        match block {
            AcpPromptBlock::Text(text) => ContentBlock::Text(TextContent::new(text)),
            AcpPromptBlock::ResourceLink {
                name,
                uri,
                mime_type,
                size,
            } => ContentBlock::ResourceLink(
                ResourceLink::new(name, uri).mime_type(mime_type).size(size),
            ),
            AcpPromptBlock::ResourceText {
                uri,
                mime_type,
                text,
            } => ContentBlock::Resource(EmbeddedResource::new(
                EmbeddedResourceResource::TextResourceContents(
                    TextResourceContents::new(text, uri).mime_type(mime_type),
                ),
            )),
            AcpPromptBlock::Image {
                data_base64,
                mime_type,
                uri,
            } => ContentBlock::Image(ImageContent::new(data_base64, mime_type).uri(uri)),
            AcpPromptBlock::Audio {
                data_base64,
                mime_type,
            } => ContentBlock::Audio(AudioContent::new(data_base64, mime_type)),
        }
    }
}

/// `build_acp_prompt_from_composer_text` 从 composer 文本构造结构化 ACP prompt。
pub fn build_acp_prompt_from_composer_text(
    text: &str,
    current_dir: impl AsRef<Path>,
    identity: &AcpAgentIdentity,
) -> AcpPrompt {
    let root = current_dir.as_ref();
    let mut blocks = Vec::new();
    let mut text_start = 0usize;
    let mut cursor = 0usize;
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

        let Some(block) = prompt_block_for_path_token(token, root, identity) else {
            cursor = token_end;
            continue;
        };

        push_text_block(&mut blocks, &text[text_start..at_index]);
        blocks.push(block);
        text_start = token_end;
        cursor = token_end;
    }

    push_text_block(&mut blocks, &text[text_start..]);
    AcpPrompt::from_blocks(blocks)
}

fn prompt_block_for_path_token(
    token: &str,
    root: &Path,
    identity: &AcpAgentIdentity,
) -> Option<AcpPromptBlock> {
    let path = resolve_path_token(root, token);
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }

    let uri = file_uri(&path)?;
    let name = resource_name(token, &path);
    let mime_type = mime_type_for_path(&path);
    let size = i64::try_from(metadata.len()).ok();
    let link = || AcpPromptBlock::ResourceLink {
        name: name.clone(),
        uri: uri.clone(),
        mime_type: mime_type.clone(),
        size,
    };

    if is_image_mime(mime_type.as_deref()) {
        if identity.supports_image() && metadata.len() <= MAX_EMBEDDED_BINARY_BYTES {
            return match fs::read(&path) {
                Ok(data) => Some(AcpPromptBlock::Image {
                    data_base64: STANDARD.encode(data),
                    mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                    uri: Some(uri),
                }),
                Err(_) => Some(link()),
            };
        }
        return Some(link());
    }

    if is_audio_mime(mime_type.as_deref()) {
        if identity.supports_audio() && metadata.len() <= MAX_EMBEDDED_BINARY_BYTES {
            return match fs::read(&path) {
                Ok(data) => Some(AcpPromptBlock::Audio {
                    data_base64: STANDARD.encode(data),
                    mime_type: mime_type.unwrap_or_else(|| "application/octet-stream".to_string()),
                }),
                Err(_) => Some(link()),
            };
        }
        return Some(link());
    }

    if identity.supports_embedded_context() && metadata.len() <= MAX_EMBEDDED_TEXT_BYTES {
        match fs::read_to_string(&path) {
            Ok(text) => {
                return Some(AcpPromptBlock::ResourceText {
                    uri,
                    mime_type: mime_type.or_else(|| Some("text/plain".to_string())),
                    text,
                });
            }
            Err(_) => return Some(link()),
        }
    }

    Some(link())
}

fn push_text_block(blocks: &mut Vec<AcpPromptBlock>, text: &str) {
    if !text.is_empty() {
        blocks.push(AcpPromptBlock::Text(text.to_string()));
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

fn resolve_path_token(root: &Path, token: &str) -> PathBuf {
    if token == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from(token));
    }
    if let Some(rest) = token.strip_prefix("~/") {
        return home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(token));
    }

    let path = Path::new(token);
    if path.is_absolute() {
        return path.to_path_buf();
    }
    root.join(path)
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

fn resource_name(token: &str, path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(token)
        .to_string()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
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
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "opus" => "audio/opus",
        "webm" => "audio/webm",
        "rs" => "text/x-rust",
        "py" => "text/x-python",
        "js" | "mjs" | "cjs" => "text/javascript",
        "ts" => "text/typescript",
        "tsx" => "text/tsx",
        "jsx" => "text/jsx",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "md" | "markdown" => "text/markdown",
        "txt" | "text" | "log" => "text/plain",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "sh" | "bash" | "zsh" => "application/x-sh",
        "c" | "h" => "text/x-c",
        "cc" | "cpp" | "cxx" | "hpp" => "text/x-c++",
        "java" => "text/x-java-source",
        "go" => "text/x-go",
        "kt" | "kts" => "text/x-kotlin",
        "swift" => "text/x-swift",
        "php" => "application/x-httpd-php",
        "rb" => "text/x-ruby",
        "lua" => "text/x-lua",
        "sql" => "application/sql",
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
