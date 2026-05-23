use crate::{
    document::{DocumentAnchorRegion, DocumentLineAnchor},
    transcript::LineAnchorKind,
};

pub(crate) fn preserves_blank_selection(anchor: &DocumentLineAnchor) -> bool {
    match anchor.region {
        DocumentAnchorRegion::Transcript => {
            !matches!(anchor.transcript.item_anchor.kind, LineAnchorKind::ItemGap)
        }
        DocumentAnchorRegion::Composer => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::ItemLineAnchor;

    fn transcript_anchor(kind: LineAnchorKind) -> DocumentLineAnchor {
        DocumentLineAnchor {
            region: DocumentAnchorRegion::Transcript,
            transcript: crate::transcript::LineAnchor {
                item_anchor: ItemLineAnchor {
                    kind,
                    ..ItemLineAnchor::default()
                },
                ..crate::transcript::LineAnchor::default()
            },
            ..DocumentLineAnchor::default()
        }
    }

    #[test]
    fn transcript_content_preserves_blank_selection() {
        assert!(preserves_blank_selection(&transcript_anchor(
            LineAnchorKind::RenderedLine
        )));
        assert!(preserves_blank_selection(&transcript_anchor(
            LineAnchorKind::LogicalPosition
        )));
    }

    #[test]
    fn transcript_item_gap_does_not_preserve_blank_selection() {
        assert!(!preserves_blank_selection(&transcript_anchor(
            LineAnchorKind::ItemGap
        )));
    }

    #[test]
    fn composer_preserves_blank_selection() {
        let anchor = DocumentLineAnchor {
            region: DocumentAnchorRegion::Composer,
            ..DocumentLineAnchor::default()
        };

        assert!(preserves_blank_selection(&anchor));
    }

    #[test]
    fn non_content_regions_do_not_preserve_blank_selection() {
        for region in [
            DocumentAnchorRegion::None,
            DocumentAnchorRegion::StatusLine,
            DocumentAnchorRegion::CommandPanel,
            DocumentAnchorRegion::ModelPanel,
            DocumentAnchorRegion::ToolApprovalPanel,
            DocumentAnchorRegion::ComposerPadding,
            DocumentAnchorRegion::TranscriptComposerGap,
        ] {
            let anchor = DocumentLineAnchor {
                region,
                ..DocumentLineAnchor::default()
            };

            assert!(!preserves_blank_selection(&anchor));
        }
    }
}
