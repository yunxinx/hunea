use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    rc::Rc,
};

#[cfg(test)]
use std::cell::Cell;

use mo_core::session::ChatMessage;
use ratatui::text::Line;

use super::{
    Sender, StyleMode,
    selection::SelectableLineRange,
    styled_text::{lines_to_ansi_text, lines_to_plain_text},
    theme::TerminalPalette,
    transcript::{ItemLineAnchor, TranscriptFastEstimate, TranscriptItemMetrics},
};

mod assistant;
mod assistant_estimate;
mod assistant_projection;
mod user;
mod user_estimate;
mod user_projection;

pub(crate) use self::assistant::{assistant_message_content_width, assistant_message_visual_inset};
pub(crate) use self::assistant_projection::AssistantMessageRenderProjection;
#[cfg(test)]
use self::user_estimate::{
    estimate_hard_wrap_line_count, estimate_hard_wrap_visible_text,
    estimate_wrapped_line_count_by_display_width,
};
pub(crate) use self::user_projection::UserMessageRenderProjection;
#[cfg(test)]
pub(crate) use self::user_projection::{
    reset_user_message_projection_plain_line_len_call_count,
    user_message_projection_plain_line_len_call_count,
};
use self::{
    assistant::{
        estimate_assistant_message_metrics_fast, render_assistant_message,
        render_assistant_message_metrics,
    },
    user::{render_user_message_lines, render_user_plain_text},
    user_estimate::{estimate_user_message_metrics_fast, measure_user_message_metrics},
};
#[cfg(test)]
use super::transcript::{TranscriptEstimateSource, wrap_prompt_visual_lines};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct UserMessageRenderLayout {
    frame_width: usize,
    content_width: usize,
    line_prefix_width: usize,
    shows_prefix: bool,
    shows_frame: bool,
}

/// `MessageItem` 表示 transcript 中的一条对话消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageItem {
    sender: Sender,
    content: Rc<str>,
    source_message: Option<ChatMessage>,
    style_mode: StyleMode,
    render_cache_key: u64,
}

#[cfg(test)]
thread_local! {
    static MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT: Cell<usize> = const { Cell::new(0) };
}

impl MessageItem {
    /// `new` 创建一条消息项。
    #[cfg(test)]
    pub fn new(sender: Sender, content: impl Into<String>) -> Self {
        Self::new_with_style_mode(sender, content, StyleMode::Cx)
    }

    /// `new_with_style_mode` 创建一条带指定样式模式的消息项。
    #[cfg(test)]
    pub fn new_with_style_mode(
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
    ) -> Self {
        Self::new_with_style_mode_and_source(sender, content, style_mode, None)
    }

    /// `new_with_style_mode_and_source` 创建一条带指定源消息的消息项。
    pub fn new_with_style_mode_and_source(
        sender: Sender,
        content: impl Into<String>,
        style_mode: StyleMode,
        source_message: Option<ChatMessage>,
    ) -> Self {
        let style_mode = style_mode.normalized();
        let content = content.into();
        let render_cache_key = message_item_render_cache_key(sender, &content, style_mode);
        let content = Rc::from(content);
        Self {
            sender,
            content,
            source_message,
            style_mode,
            render_cache_key,
        }
    }

    /// `render_lines` 将消息渲染为带样式的文本行。
    pub fn render_lines(&self, width: u16, palette: TerminalPalette) -> Vec<Line<'static>> {
        match self.sender {
            Sender::User => {
                render_user_message_lines(self.content.as_ref(), width, palette, self.style_mode)
            }
            Sender::Assistant => render_assistant_message(self.content.as_ref(), width, palette),
        }
    }

    /// `render_for_terminal_replay` 返回适合退出 AltScreen 后回放到终端的消息文本。
    pub fn render_for_terminal_replay(
        &self,
        width: u16,
        palette: TerminalPalette,
        preserve_ansi: bool,
    ) -> String {
        let lines = self.render_lines(width, palette);
        if preserve_ansi {
            lines_to_ansi_text(&lines)
        } else {
            lines_to_plain_text(&lines)
        }
    }

    /// `render_plain_text` 返回不带 ANSI 的纯文本消息内容。
    pub fn render_plain_text(&self, width: u16, palette: TerminalPalette) -> String {
        match self.sender {
            Sender::User => render_user_plain_text(self.content.as_ref(), width, self.style_mode),
            Sender::Assistant => lines_to_plain_text(&render_assistant_message(
                self.content.as_ref(),
                width,
                palette,
            )),
        }
    }

    pub(crate) fn is_assistant(&self) -> bool {
        self.sender == Sender::Assistant
    }

    pub(crate) fn sender(&self) -> Sender {
        self.sender
    }

    pub(crate) fn source_content(&self) -> &str {
        self.content.as_ref()
    }

    pub(crate) fn source_chat_message(&self) -> ChatMessage {
        self.source_message
            .clone()
            .unwrap_or_else(|| match self.sender {
                Sender::User => ChatMessage::user(self.content.as_ref().to_string()),
                Sender::Assistant => ChatMessage::assistant(self.content.as_ref().to_string()),
            })
    }

    pub(crate) fn render_cache_key(&self) -> u64 {
        self.render_cache_key
    }

    pub(crate) fn source_text_byte_len(&self) -> usize {
        self.content.len()
    }

    pub(crate) fn measure_render_metrics(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> (usize, usize) {
        match self.sender {
            Sender::User => {
                measure_user_message_metrics(self.content.as_ref(), width, palette, self.style_mode)
            }
            Sender::Assistant => {
                render_assistant_message_metrics(self.content.as_ref(), width, palette)
            }
        }
    }

    pub(crate) fn estimate_render_metrics_fast(
        &self,
        width: u16,
        palette: TerminalPalette,
        previous_metrics: Option<TranscriptItemMetrics>,
    ) -> TranscriptFastEstimate {
        let previous_metrics =
            previous_metrics.filter(|metrics| metrics.cache_key == self.render_cache_key);
        match self.sender {
            Sender::User => estimate_user_message_metrics_fast(
                self.content.as_ref(),
                width,
                palette,
                self.style_mode,
                previous_metrics,
            ),
            Sender::Assistant => estimate_assistant_message_metrics_fast(
                self.content.as_ref(),
                width,
                palette,
                previous_metrics,
            ),
        }
    }

    pub(crate) fn render_line_anchors(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<ItemLineAnchor> {
        if self.sender != Sender::User {
            return Vec::new();
        }

        user::render_user_message_line_anchors(
            self.content.as_ref(),
            width,
            palette,
            self.style_mode,
        )
    }

    pub(crate) fn render_selectable_line_ranges(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Vec<SelectableLineRange> {
        if self.sender != Sender::User {
            return Vec::new();
        }

        user_projection::render_user_message_selectable_line_ranges(
            self.content.as_ref(),
            width,
            palette,
            self.style_mode,
        )
    }

    pub(crate) fn render_projection(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Option<UserMessageRenderProjection> {
        (self.sender == Sender::User).then(|| {
            user_projection::render_user_message_projection(
                self.content.as_ref(),
                width,
                palette,
                self.style_mode,
            )
        })
    }

    pub(crate) fn render_assistant_projection(
        &self,
        width: u16,
        palette: TerminalPalette,
    ) -> Option<AssistantMessageRenderProjection> {
        (self.sender == Sender::Assistant)
            .then(|| {
                assistant_projection::render_assistant_message_projection(
                    Rc::clone(&self.content),
                    width,
                    palette,
                )
            })
            .flatten()
    }

    #[cfg(test)]
    fn render_plain_for_test(&self, width: u16) -> String {
        self.render_plain_text(width, crate::theme::default_palette())
    }
}

#[cfg(test)]
pub(crate) fn reset_message_item_render_cache_key_call_count() {
    MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT.set(0);
}

#[cfg(test)]
pub(crate) fn message_item_render_cache_key_call_count() -> usize {
    MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT.get()
}

fn message_item_render_cache_key(sender: Sender, content: &str, style_mode: StyleMode) -> u64 {
    #[cfg(test)]
    MESSAGE_ITEM_RENDER_CACHE_KEY_CALL_COUNT.with(|count| count.set(count.get() + 1));

    let mut hasher = DefaultHasher::new();
    if sender == Sender::User {
        style_mode.hash(&mut hasher);
    }
    content.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests;
