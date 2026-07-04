use std::{fs, path::Path};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use runtime_domain::session::TranscriptUserAttachment;

const MAX_IMAGE_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;

/// `load_image_attachment` 读取并校验一个本地图片附件。
pub(crate) fn load_image_attachment(
    uri: &str,
    path: &Path,
) -> Result<TranscriptUserAttachment, ImageAttachmentLoadError> {
    let mime_type = supported_image_mime_type_for_path(path)
        .ok_or(ImageAttachmentLoadError::UnsupportedImageType)?;
    let metadata = fs::metadata(path).map_err(ImageAttachmentLoadError::Metadata)?;
    if !metadata.is_file() {
        return Err(ImageAttachmentLoadError::NotFile);
    }
    if metadata.len() > MAX_IMAGE_ATTACHMENT_BYTES {
        return Err(ImageAttachmentLoadError::TooLarge {
            bytes: metadata.len(),
            limit: MAX_IMAGE_ATTACHMENT_BYTES,
        });
    }

    let bytes = fs::read(path).map_err(ImageAttachmentLoadError::Read)?;
    if !image_signature_matches(mime_type, &bytes) {
        return Err(ImageAttachmentLoadError::SignatureMismatch { mime_type });
    }

    Ok(TranscriptUserAttachment::local_image(
        BASE64_STANDARD.encode(bytes),
        mime_type,
        Some(uri.to_string()),
    ))
}

pub(crate) fn is_supported_image_path(path: &Path) -> bool {
    supported_image_mime_type_for_path(path).is_some()
}

#[derive(Debug)]
pub(crate) enum ImageAttachmentLoadError {
    UnsupportedImageType,
    Metadata(std::io::Error),
    NotFile,
    TooLarge { bytes: u64, limit: u64 },
    Read(std::io::Error),
    SignatureMismatch { mime_type: &'static str },
}

impl std::fmt::Display for ImageAttachmentLoadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedImageType => {
                write!(formatter, "unsupported image type")
            }
            Self::Metadata(error) => write!(formatter, "could not read file metadata: {error}"),
            Self::NotFile => write!(formatter, "path is not a file"),
            Self::TooLarge { bytes, limit } => {
                write!(
                    formatter,
                    "image is too large ({bytes} bytes, limit {limit} bytes)"
                )
            }
            Self::Read(error) => write!(formatter, "could not read image file: {error}"),
            Self::SignatureMismatch { mime_type } => {
                write!(formatter, "file does not look like {mime_type}")
            }
        }
    }
}

fn supported_image_mime_type_for_path(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

fn image_signature_matches(mime_type: &str, bytes: &[u8]) -> bool {
    match mime_type {
        "image/png" => bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]),
        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
        "image/gif" => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
        "image/webp" => bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP",
        _ => false,
    }
}
