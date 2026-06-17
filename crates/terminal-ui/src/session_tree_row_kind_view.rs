use ratatui::style::Style;
use runtime_domain::session::SessionTreeRowKind;

use crate::theme::{
    TerminalPalette, command_accent_text_style, muted_text_style, primary_text_style,
    tertiary_text_style,
};

pub(crate) const SESSION_TREE_ROW_KIND_WIDTH: usize = 9;
pub(crate) const SESSION_TREE_ROW_KIND_PREFIX_WIDTH: usize = SESSION_TREE_ROW_KIND_WIDTH + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TreeRowKindPrefixAlignment {
    Left,
    CenterTool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyableSessionTreeRowKind {
    User,
    Assistant,
}

impl CopyableSessionTreeRowKind {
    pub(crate) fn from_session_tree_kind(kind: SessionTreeRowKind) -> Option<Self> {
        match kind {
            SessionTreeRowKind::User => Some(Self::User),
            SessionTreeRowKind::Assistant => Some(Self::Assistant),
            SessionTreeRowKind::Tool | SessionTreeRowKind::Reasoning => None,
        }
    }

    pub(crate) fn session_tree_kind(self) -> SessionTreeRowKind {
        match self {
            Self::User => SessionTreeRowKind::User,
            Self::Assistant => SessionTreeRowKind::Assistant,
        }
    }
}

pub(crate) fn session_tree_row_kind_is_copyable(kind: SessionTreeRowKind) -> bool {
    CopyableSessionTreeRowKind::from_session_tree_kind(kind).is_some()
}

pub(crate) fn session_tree_row_kind_label(kind: SessionTreeRowKind) -> &'static str {
    match kind {
        SessionTreeRowKind::User => "user",
        SessionTreeRowKind::Assistant => "assistant",
        SessionTreeRowKind::Tool => "tool",
        SessionTreeRowKind::Reasoning => "reasoning",
    }
}

pub(crate) fn session_tree_row_kind_prefix(
    kind: SessionTreeRowKind,
    alignment: TreeRowKindPrefixAlignment,
) -> String {
    let label = session_tree_row_kind_label(kind);
    match (kind, alignment) {
        (SessionTreeRowKind::Tool, TreeRowKindPrefixAlignment::CenterTool) => {
            format!("{label:^SESSION_TREE_ROW_KIND_WIDTH$} ")
        }
        _ => format!("{label:<SESSION_TREE_ROW_KIND_WIDTH$} "),
    }
}

pub(crate) fn session_tree_row_kind_label_style(
    kind: SessionTreeRowKind,
    palette: TerminalPalette,
) -> Style {
    match kind {
        SessionTreeRowKind::User => command_accent_text_style(palette),
        SessionTreeRowKind::Assistant => primary_text_style(palette),
        SessionTreeRowKind::Tool => muted_text_style(palette),
        SessionTreeRowKind::Reasoning => tertiary_text_style(palette).italic(),
    }
}

#[cfg(test)]
mod tests {
    use runtime_domain::session::SessionTreeRowKind;

    use crate::theme::{command_accent_text_style, default_palette, primary_text_style};

    use super::*;

    #[test]
    fn row_kind_prefix_uses_shared_width_and_alignment() {
        assert_eq!(
            session_tree_row_kind_prefix(
                SessionTreeRowKind::User,
                TreeRowKindPrefixAlignment::Left
            ),
            "user      "
        );
        assert_eq!(
            session_tree_row_kind_prefix(
                SessionTreeRowKind::Tool,
                TreeRowKindPrefixAlignment::CenterTool
            ),
            "  tool    "
        );
    }

    #[test]
    fn row_kind_label_style_uses_shared_semantic_color() {
        let palette = default_palette();

        assert_eq!(
            session_tree_row_kind_label_style(SessionTreeRowKind::User, palette),
            command_accent_text_style(palette)
        );
        assert_eq!(
            session_tree_row_kind_label_style(SessionTreeRowKind::Assistant, palette),
            primary_text_style(palette)
        );
    }

    #[test]
    fn row_kind_copyable_policy_is_shared() {
        assert!(session_tree_row_kind_is_copyable(SessionTreeRowKind::User));
        assert!(session_tree_row_kind_is_copyable(
            SessionTreeRowKind::Assistant
        ));
        assert!(!session_tree_row_kind_is_copyable(SessionTreeRowKind::Tool));
        assert!(!session_tree_row_kind_is_copyable(
            SessionTreeRowKind::Reasoning
        ));
    }
}
