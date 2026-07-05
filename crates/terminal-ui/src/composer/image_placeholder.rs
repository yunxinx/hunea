use runtime_domain::session::{TranscriptUserAttachment, transcript_image_label_text};

/// `ComposerImageAttachment` 保存 composer 内一个图片占位符对应的附件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComposerImageAttachment {
    placeholder: String,
    attachment: TranscriptUserAttachment,
}

impl ComposerImageAttachment {
    pub(crate) fn new(
        label_number: usize,
        attachment: TranscriptUserAttachment,
    ) -> ComposerImageAttachment {
        Self {
            placeholder: transcript_image_label_text(label_number),
            attachment,
        }
    }

    pub(crate) fn placeholder(&self) -> &str {
        &self.placeholder
    }

    pub(crate) fn attachment(&self) -> &TranscriptUserAttachment {
        &self.attachment
    }
}
