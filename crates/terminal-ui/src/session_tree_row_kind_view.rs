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

const USER_KIND_PREFIX: &str = "user      ";
const ASSISTANT_KIND_PREFIX: &str = "assistant ";
const TOOL_KIND_PREFIX: &str = "tool      ";
const CENTERED_TOOL_KIND_PREFIX: &str = "  tool    ";
const REASONING_KIND_PREFIX: &str = "reasoning ";

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

pub(crate) fn session_tree_row_kind_prefix(
    kind: SessionTreeRowKind,
    alignment: TreeRowKindPrefixAlignment,
) -> &'static str {
    match (kind, alignment) {
        (SessionTreeRowKind::Tool, TreeRowKindPrefixAlignment::CenterTool) => {
            CENTERED_TOOL_KIND_PREFIX
        }
        (SessionTreeRowKind::User, _) => USER_KIND_PREFIX,
        (SessionTreeRowKind::Assistant, _) => ASSISTANT_KIND_PREFIX,
        (SessionTreeRowKind::Tool, _) => TOOL_KIND_PREFIX,
        (SessionTreeRowKind::Reasoning, _) => REASONING_KIND_PREFIX,
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
        let user_prefix: &'static str = session_tree_row_kind_prefix(
            SessionTreeRowKind::User,
            TreeRowKindPrefixAlignment::Left,
        );
        let tool_prefix: &'static str = session_tree_row_kind_prefix(
            SessionTreeRowKind::Tool,
            TreeRowKindPrefixAlignment::CenterTool,
        );

        assert_eq!(user_prefix, "user      ");
        assert_eq!(tool_prefix, "  tool    ");
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
