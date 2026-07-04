mod grapheme;
mod image_attachment;
mod layout;
mod message;
mod mouse;
mod mouse_interaction;
mod render;
mod viewport;

#[cfg(test)]
mod tests;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime_domain::session::{
    TranscriptCustomPromptBinding, TranscriptSkillBinding, TranscriptUserAttachment,
};

use self::{
    grapheme::{
        grapheme_clusters, grapheme_range_at_or_after_cursor, grapheme_range_before_cursor,
        grapheme_target_left, grapheme_target_right, logical_column_for_visual_offset,
        measure_width,
    },
    image_attachment::ComposerImageAttachment,
    layout::{visual_line_count, visual_lines_for_text},
    render::{DocumentRenderResult, render_document},
    viewport::{calculate_cursor_visual_position, sync_viewport_offset_for_cursor},
};
use super::{style_mode::StyleMode, theme::TerminalPalette};
use crate::terminal_text::sanitize_terminal_text;

pub(crate) use self::message::ComposerSourceMessage;
pub(crate) use self::mouse::{
    cursor_position_for_line_anchor_click, move_cursor_to_logical_position,
    selection_end_char_for_line_anchor, selection_start_char_for_line_anchor,
};
pub(crate) use self::mouse_interaction::{ComposerMouseOutcome, PendingComposerCursorClick};
pub(crate) use self::render::LineAnchor;
#[cfg(test)]
use self::render::{RenderResult, render};
#[cfg(test)]
pub(crate) use self::{
    layout::{reset_visual_lines_call_count, visual_lines_call_count},
    render::{render_document_call_count, reset_render_document_call_count},
};

const PLACEHOLDER: &str = "Enter to send Prompt";
const COMPOSER_RIGHT_PADDING_WIDTH: u16 = 2;
pub const DEFAULT_COMPOSER_UNDO_LIMIT: usize = 50;
pub const MAX_COMPOSER_UNDO_LIMIT: usize = 200;
const WORD_SEPARATORS: &str = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?";

/// `Composer` 管理底部输入区的文本、光标和自定义 viewport。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    value: String,
    skill_bindings: Vec<TranscriptSkillBinding>,
    custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
    image_attachments: Vec<ComposerImageAttachment>,
    cursor: usize,
    width: u16,
    height: u16,
    viewport_y: usize,
    content_revision: usize,
    cursor_revision: usize,
    style_mode: StyleMode,
    kill_buffer: String,
    undo_history: Vec<ComposerSnapshot>,
    undo_limit: usize,
    has_active_grapheme_undo_group: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComposerSnapshot {
    value: String,
    skill_bindings: Vec<TranscriptSkillBinding>,
    custom_prompt_bindings: Vec<TranscriptCustomPromptBinding>,
    image_attachments: Vec<ComposerImageAttachment>,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComposerReplaceUndoMode {
    Record,
    Reset,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new(StyleMode::Cx)
    }
}

impl Composer {
    /// `new` 创建指定样式模式的输入框状态。
    pub fn new(style_mode: StyleMode) -> Self {
        Self::new_with_undo_limit(style_mode, DEFAULT_COMPOSER_UNDO_LIMIT)
    }

    /// `new_with_undo_limit` 创建指定 undo 容量的输入框状态。
    pub fn new_with_undo_limit(style_mode: StyleMode, undo_limit: usize) -> Self {
        Self {
            value: String::new(),
            skill_bindings: Vec::new(),
            custom_prompt_bindings: Vec::new(),
            image_attachments: Vec::new(),
            cursor: 0,
            width: 1,
            height: 1,
            viewport_y: 0,
            content_revision: 1,
            cursor_revision: 1,
            style_mode: style_mode.normalized(),
            kill_buffer: String::new(),
            undo_history: Vec::new(),
            undo_limit: undo_limit.clamp(1, MAX_COMPOSER_UNDO_LIMIT),
            has_active_grapheme_undo_group: false,
        }
    }

    /// `set_width` 更新 composer 的总渲染宽度。
    pub fn set_width(&mut self, width: u16) {
        self.width = width.max(1);
        self.clamp_viewport();
    }

    /// `set_height` 更新 composer viewport 的可视高度。
    pub fn set_height(&mut self, height: u16) {
        self.height = height.max(1);
        self.clamp_viewport();
    }

    /// `visible_height` 返回当前 viewport 的可视高度。
    #[cfg(test)]
    pub fn visible_height(&self) -> u16 {
        self.height.max(1)
    }

    /// `full_height` 返回 composer 完整内容的视觉高度。
    pub fn full_height(&self) -> u16 {
        self.full_height_for_value_at_width(&self.value, self.width)
    }

    /// `value` 返回当前输入内容。
    pub fn value(&self) -> &str {
        &self.value
    }

    /// `clear` 清空输入内容并复位光标与 viewport。
    pub fn clear(&mut self) {
        self.has_active_grapheme_undo_group = false;
        if self.value.is_empty() {
            self.clear_structured_bindings();
            self.set_cursor(0);
            self.viewport_y = 0;
            return;
        }

        self.value.clear();
        self.clear_structured_bindings();
        self.set_cursor(0);
        self.viewport_y = 0;
        self.undo_history.clear();
        self.bump_content_revision();
    }

    /// `clear_for_edit` 清空用户草稿，并保留一次可撤销编辑。
    pub(crate) fn clear_for_edit(&mut self) {
        if self.value.is_empty() {
            self.finish_grapheme_undo_group();
            self.clear_structured_bindings();
            self.set_cursor(0);
            self.viewport_y = 0;
            return;
        }

        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        self.value.clear();
        self.clear_structured_bindings();
        self.set_cursor(0);
        self.viewport_y = 0;
        self.bump_content_revision();
    }

    /// `replace_text_and_move_to_end_for_edit` 用用户触发的整段编辑替换草稿，并记录 undo。
    ///
    /// 外部编辑器回写与命令补全都属于用户可感知的草稿编辑，`Ctrl+Z` 应该回到替换前文本。
    pub(crate) fn replace_text_and_move_to_end_for_edit(&mut self, value: impl Into<String>) {
        self.replace_text_and_move_to_end_with_undo_mode(value, ComposerReplaceUndoMode::Record);
    }

    /// `reset_text_and_move_to_end` 用内部状态切换结果替换草稿，并清空 undo 历史。
    ///
    /// message revisit、面板切换和预览初始化这类路径会同步改变草稿外的状态；
    /// 只撤回 composer 文本会制造和 transcript / panel 状态不一致的假历史，因此这里必须重置 undo。
    /// 发送与显式清空走 `clear`，但语义同样是重置而不是可撤销编辑。
    pub(crate) fn reset_text_and_move_to_end(&mut self, value: impl Into<String>) {
        self.replace_text_and_move_to_end_with_undo_mode(value, ComposerReplaceUndoMode::Reset);
    }

    /// `reset_source_message_and_move_to_end` 用完整源消息恢复草稿。
    pub(crate) fn reset_source_message_and_move_to_end(&mut self, message: ComposerSourceMessage) {
        let message = message.into_transcript_user_message();
        let value = sanitized_owned_text(message.content);
        let image_attachments = message
            .attachments
            .into_iter()
            .enumerate()
            .map(|(index, attachment)| ComposerImageAttachment::new(index + 1, attachment))
            .collect::<Vec<_>>();
        let changed = self.value != value
            || self.skill_bindings != message.skill_bindings
            || self.custom_prompt_bindings != message.custom_prompt_bindings
            || self.image_attachments != image_attachments;

        self.undo_history.clear();
        self.has_active_grapheme_undo_group = false;
        self.value = value;
        self.skill_bindings = message.skill_bindings;
        self.custom_prompt_bindings = message.custom_prompt_bindings;
        self.image_attachments = image_attachments;
        self.reconcile_structured_bindings();
        if changed {
            self.bump_content_revision();
        }
        self.set_cursor(total_chars(&self.value));
        self.sync_viewport_to_cursor();
    }

    fn replace_text_and_move_to_end_with_undo_mode(
        &mut self,
        value: impl Into<String>,
        undo_mode: ComposerReplaceUndoMode,
    ) {
        let value = sanitized_owned_text(value.into());
        let changed = self.value != value;
        match undo_mode {
            ComposerReplaceUndoMode::Record if changed => {
                self.finish_grapheme_undo_group();
                self.push_undo_snapshot();
            }
            ComposerReplaceUndoMode::Reset => {
                self.undo_history.clear();
                self.has_active_grapheme_undo_group = false;
                self.clear_structured_bindings();
            }
            ComposerReplaceUndoMode::Record => {}
        }

        if changed {
            self.value = value;
            if matches!(undo_mode, ComposerReplaceUndoMode::Record) {
                self.reconcile_structured_bindings();
            }
            self.bump_content_revision();
        }
        self.set_cursor(total_chars(&self.value));
        self.sync_viewport_to_cursor();
    }

    /// `insert_newline` 在当前光标位置插入显式换行。
    pub fn insert_newline(&mut self) {
        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        self.insert_char_without_undo('\n');
        self.sync_viewport_to_cursor();
    }

    /// `insert_text` 在当前光标位置插入一段文本。
    pub fn insert_text(&mut self, text: &str) {
        let sanitized_text = sanitize_terminal_text(text);
        let text = sanitized_text.as_ref();
        if text.is_empty() {
            return;
        }

        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        let byte_index = char_to_byte_index(&self.value, self.cursor);
        self.value.insert_str(byte_index, text);
        self.reconcile_structured_bindings();
        self.set_cursor(self.cursor + total_chars(text));
        self.bump_content_revision();
        self.sync_viewport_to_cursor();
    }

    /// `replace_char_range` 用指定文本替换字符范围，并记录一次 undo。
    pub(crate) fn replace_char_range(
        &mut self,
        start: usize,
        end: usize,
        replacement: &str,
    ) -> bool {
        if start >= end {
            return false;
        }

        let sanitized_replacement = sanitize_terminal_text(replacement);
        let replacement = sanitized_replacement.as_ref();
        let value_char_count = total_chars(&self.value);
        let start = start.min(value_char_count);
        let end = end.min(value_char_count);
        if start >= end {
            return false;
        }

        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        let byte_start = char_to_byte_index(&self.value, start);
        let byte_end = char_to_byte_index(&self.value, end);
        self.value.replace_range(byte_start..byte_end, replacement);
        self.reconcile_structured_bindings();
        self.set_cursor(start + total_chars(replacement));
        self.bump_content_revision();
        self.sync_viewport_to_cursor();
        true
    }

    /// `kill_char_range` 删除字符范围，并把删除文本放入 kill buffer。
    pub(crate) fn kill_char_range(&mut self, start: usize, end: usize) -> bool {
        let total_chars = total_chars(&self.value);
        let start = start.min(total_chars);
        let end = end.min(total_chars);
        if start >= end {
            return false;
        }

        let byte_start = char_to_byte_index(&self.value, start);
        let byte_end = char_to_byte_index(&self.value, end);
        let killed_text = self.value[byte_start..byte_end].to_string();
        if killed_text.is_empty() {
            return false;
        }

        self.kill_buffer = killed_text;
        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        self.delete_absolute_range_without_undo(start, end);
        self.sync_viewport_to_cursor();
        true
    }

    /// `replace_char_range_with_kill_buffer` 用 kill buffer 替换字符范围。
    pub(crate) fn replace_char_range_with_kill_buffer(&mut self, start: usize, end: usize) -> bool {
        if self.kill_buffer.is_empty() {
            return false;
        }

        let text = self.kill_buffer.clone();
        self.replace_char_range(start, end, &text)
    }

    /// `finish_current_undo_group` 结束当前普通输入 undo 分组。
    ///
    /// 有些高层事件不会进入 `Composer::handle_key`，例如 Ctrl+G 外部编辑器或 Enter 发送。
    /// 这些事件应当成为 undo 边界，避免后续输入和之前的 grapheme 组合误合并。
    pub(crate) fn finish_current_undo_group(&mut self) {
        self.finish_grapheme_undo_group();
    }

    /// `source_message` 返回当前草稿对应的 transcript-visible 用户消息。
    pub(crate) fn source_message(&self) -> ComposerSourceMessage {
        ComposerSourceMessage::user_text_with_bindings_and_attachments(
            self.value.clone(),
            self.skill_bindings.clone(),
            self.custom_prompt_bindings.clone(),
            self.source_image_attachments(),
        )
    }

    /// `current_prefixed_token_value` 返回当前前缀 token，不含 sigil。
    pub(crate) fn current_prefixed_token_value(&self, prefix: char) -> Option<String> {
        self.current_prefixed_token(prefix).map(|token| token.query)
    }

    /// `current_prefixed_token_start_char` 返回当前前缀 token 的起点字符偏移。
    pub(crate) fn current_prefixed_token_start_char(&self, prefix: char) -> Option<usize> {
        self.current_prefixed_token(prefix)
            .map(|token| token.start_char)
    }

    /// `replace_current_prefixed_token` 替换当前前缀 token，并把光标移动到替换文本末尾。
    pub(crate) fn replace_current_prefixed_token(
        &mut self,
        prefix: char,
        replacement: &str,
    ) -> bool {
        let Some(token) = self.current_prefixed_token(prefix) else {
            return false;
        };

        let byte_start = char_to_byte_index(&self.value, token.start_char);
        let byte_end = char_to_byte_index(&self.value, token.end_char);
        if self.value.get(byte_start..byte_end) == Some(replacement) {
            self.finish_grapheme_undo_group();
            self.set_cursor(token.start_char + total_chars(replacement));
            self.sync_viewport_to_cursor();
            return true;
        }

        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        self.value.replace_range(byte_start..byte_end, replacement);
        self.reconcile_structured_bindings();
        self.set_cursor(token.start_char + total_chars(replacement));
        self.bump_content_revision();
        self.sync_viewport_to_cursor();
        true
    }

    /// `current_at_token` 返回当前光标所在的 `@` 文件 token，不含前导 `@`。
    pub(crate) fn current_at_token(&self) -> Option<String> {
        self.current_prefixed_token_value('@')
    }

    /// `current_at_token_start_char` 返回当前 `@` token 起点的字符偏移。
    pub(crate) fn current_at_token_start_char(&self) -> Option<usize> {
        self.current_prefixed_token_start_char('@')
    }

    /// `replace_current_at_token` 替换当前 `@` token，并把光标移动到替换文本末尾。
    pub(crate) fn replace_current_at_token(&mut self, replacement: &str) -> bool {
        self.replace_current_prefixed_token('@', replacement)
    }

    /// `replace_current_at_token_with_image_attachment` 把当前 `@` token 替换成图片占位符。
    pub(crate) fn replace_current_at_token_with_image_attachment(
        &mut self,
        attachment: TranscriptUserAttachment,
    ) -> bool {
        let placeholder =
            runtime_domain::session::transcript_image_label_text(self.image_attachments.len() + 1);
        let replacement = format!("{placeholder} ");
        if !self.replace_current_prefixed_token('@', &replacement) {
            return false;
        }

        self.image_attachments.push(ComposerImageAttachment::new(
            self.image_attachments.len() + 1,
            attachment,
        ));
        true
    }

    /// `current_skill_token` 返回当前光标所在的 `$skill` token，不含前导 `$`。
    pub(crate) fn current_skill_token(&self) -> Option<String> {
        self.current_prefixed_token_value('$')
    }

    /// `current_skill_token_start_char` 返回当前 `$skill` token 起点的字符偏移。
    pub(crate) fn current_skill_token_start_char(&self) -> Option<usize> {
        self.current_prefixed_token_start_char('$')
    }

    /// `current_custom_prompt_token` 返回当前光标所在的 `#prompt` token，不含前导 `#`。
    pub(crate) fn current_custom_prompt_token(&self) -> Option<String> {
        self.current_prefixed_token_value('#')
    }

    /// `current_custom_prompt_token_start_char` 返回当前 `#prompt` token 起点的字符偏移。
    pub(crate) fn current_custom_prompt_token_start_char(&self) -> Option<usize> {
        self.current_prefixed_token_start_char('#')
    }

    /// `replace_current_skill_token` 替换当前 `$` token，建立绑定，并把光标移动到替换文本末尾。
    pub(crate) fn replace_current_skill_token(
        &mut self,
        skill_name: &str,
        skill_path: &str,
        origin: runtime_domain::prompt_assembly::PromptSourceOrigin,
    ) -> bool {
        let Some(token) = self.current_prefixed_token('$') else {
            return false;
        };
        let visible_token = format!("${skill_name}");
        let replacement = format!("{visible_token} ");
        if !self.replace_current_prefixed_token('$', &replacement) {
            return false;
        }

        self.skill_bindings.retain(|binding| {
            !(binding.start_char >= token.start_char && binding.end_char <= token.end_char)
        });
        self.skill_bindings.push(TranscriptSkillBinding {
            skill_name: skill_name.to_string(),
            origin,
            skill_path: skill_path.to_string(),
            start_char: token.start_char,
            end_char: token.start_char + total_chars(&visible_token),
        });
        self.skill_bindings
            .sort_by_key(|binding| binding.start_char);
        true
    }

    /// `replace_current_custom_prompt_token` 替换当前 `#` token，建立绑定，并把光标移动到替换文本末尾。
    pub(crate) fn replace_current_custom_prompt_token(
        &mut self,
        reference_id: &str,
        origin: runtime_domain::prompt_assembly::PromptSourceOrigin,
    ) -> bool {
        let Some(token) = self.current_prefixed_token('#') else {
            return false;
        };
        let visible_token = format!("#{reference_id}");
        let replacement = format!("{visible_token} ");
        if !self.replace_current_prefixed_token('#', &replacement) {
            return false;
        }

        self.custom_prompt_bindings.retain(|binding| {
            !(binding.start_char >= token.start_char && binding.end_char <= token.end_char)
        });
        self.custom_prompt_bindings
            .push(TranscriptCustomPromptBinding {
                reference_id: reference_id.to_string(),
                origin,
                start_char: token.start_char,
                end_char: token.start_char + total_chars(&visible_token),
            });
        self.custom_prompt_bindings
            .sort_by_key(|binding| binding.start_char);
        true
    }

    pub(crate) fn current_skill_binding(&self) -> Option<TranscriptSkillBinding> {
        let token = self.current_prefixed_token('$')?;
        self.skill_bindings
            .iter()
            .find(|binding| {
                binding.start_char == token.start_char && binding.end_char == token.end_char
            })
            .cloned()
    }

    pub(crate) fn current_custom_prompt_binding(&self) -> Option<TranscriptCustomPromptBinding> {
        let token = self.current_prefixed_token('#')?;
        self.custom_prompt_bindings
            .iter()
            .find(|binding| {
                binding.start_char == token.start_char && binding.end_char == token.end_char
            })
            .cloned()
    }

    /// `handle_key` 处理输入编辑、导航与分页相关按键。
    pub fn handle_key(&mut self, key: KeyEvent) {
        let is_plain_input = matches!(key.code, KeyCode::Char(_))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        if !is_plain_input {
            self.finish_grapheme_undo_group();
        }

        match key.code {
            KeyCode::Char('z') if is_ctrl_only(key.modifiers) => self.undo(),
            KeyCode::Char('k') if is_ctrl_only(key.modifiers) => self.kill_to_end_of_line(),
            KeyCode::Char('w') if is_ctrl_only(key.modifiers) => self.delete_backward_word(),
            KeyCode::Char('y') if is_ctrl_only(key.modifiers) => self.yank(),
            KeyCode::Char('h') if is_ctrl_only(key.modifiers) => self.backspace(),
            KeyCode::Backspace if has_word_modifier(key.modifiers) => self.delete_backward_word(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Char('d') if key.modifiers == KeyModifiers::ALT => self.delete_forward_word(),
            KeyCode::Char('d') if is_ctrl_only(key.modifiers) => self.delete_forward(),
            KeyCode::Delete if has_word_modifier(key.modifiers) => self.delete_forward_word(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Char('b') if is_ctrl_only(key.modifiers) => self.move_left(),
            KeyCode::Char('b') if key.modifiers == KeyModifiers::ALT => self.move_word_left(),
            KeyCode::Left if has_word_modifier(key.modifiers) => self.move_word_left(),
            KeyCode::Left => self.move_left(),
            KeyCode::Char('f') if is_ctrl_only(key.modifiers) => self.move_right(),
            KeyCode::Char('f') if key.modifiers == KeyModifiers::ALT => self.move_word_right(),
            KeyCode::Right if has_word_modifier(key.modifiers) => self.move_word_right(),
            KeyCode::Right => self.move_right(),
            KeyCode::Char('p') if is_ctrl_only(key.modifiers) => self.move_vertical(-1),
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Char('n') if is_ctrl_only(key.modifiers) => self.move_vertical(1),
            KeyCode::Down => self.move_vertical(1),
            KeyCode::Char('u') if is_ctrl_only(key.modifiers) => {
                self.delete_current_line_before_cursor()
            }
            KeyCode::Char('a') if is_ctrl_only(key.modifiers) => self.move_line_start(),
            KeyCode::Home if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_line_start()
            }
            KeyCode::Char('e') if is_ctrl_only(key.modifiers) => self.move_line_end(),
            KeyCode::End if !key.modifiers.contains(KeyModifiers::CONTROL) => self.move_line_end(),
            KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => self.move_to_begin(),
            KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => self.move_to_end(),
            KeyCode::PageUp => self.page_move(-1),
            KeyCode::PageDown => self.page_move(1),
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_plain_input_char(character);
            }
            _ => {}
        }

        self.sync_viewport_to_cursor();
    }

    #[cfg(test)]
    pub(crate) fn render(&self, palette: TerminalPalette) -> RenderResult {
        render(self, palette)
    }

    pub(crate) fn render_document(&self, palette: TerminalPalette) -> DocumentRenderResult {
        render_document(self, palette)
    }

    pub(crate) fn content_width(&self) -> usize {
        composer_content_width(self.width)
    }

    pub(crate) fn prompt(&self) -> &str {
        match self.style_mode {
            StyleMode::Cx => "› ",
            StyleMode::Cc => "❯ ",
            StyleMode::Ms => "┃ ",
        }
    }

    pub(crate) fn placeholder(&self) -> &str {
        PLACEHOLDER
    }

    pub(crate) fn style_mode(&self) -> StyleMode {
        self.style_mode
    }

    pub(crate) fn content_revision(&self) -> usize {
        self.content_revision
    }

    pub(crate) fn has_image_attachments(&self) -> bool {
        !self.image_attachments.is_empty()
    }

    pub(crate) fn cursor_revision(&self) -> usize {
        self.cursor_revision
    }

    /// 当前光标在 `value` 中的字符索引（盲回溯行边界门控用）。
    pub(crate) fn cursor_char_index(&self) -> usize {
        self.cursor
    }

    pub(crate) fn viewport_offset(&self) -> usize {
        self.viewport_y
    }

    pub(crate) fn set_viewport_offset(&mut self, offset: usize) {
        self.viewport_y = self.clamp_viewport_offset(offset);
    }

    pub(crate) fn viewport_height(&self) -> usize {
        usize::from(self.height.max(1))
    }

    pub(crate) fn cursor_position(&self) -> (usize, usize) {
        logical_position(&self.value, self.cursor)
    }

    pub(crate) fn cursor_visual_position_for_anchors(
        &self,
        anchors: &[LineAnchor],
    ) -> Option<(u16, usize)> {
        let (logical_line, logical_column) = self.cursor_position();
        self.visual_position_for_logical_position(logical_line, logical_column, anchors)
    }

    /// `visual_position_for_char_in_anchors` 基于已渲染 anchors 计算字符的视觉坐标。
    pub(crate) fn visual_position_for_char_in_anchors(
        &self,
        char_index: usize,
        anchors: &[LineAnchor],
    ) -> Option<(u16, usize)> {
        let (logical_line, logical_column) = logical_position(&self.value, char_index);
        self.visual_position_for_logical_position(logical_line, logical_column, anchors)
    }

    fn visual_position_for_logical_position(
        &self,
        logical_line: usize,
        logical_column: usize,
        anchors: &[LineAnchor],
    ) -> Option<(u16, usize)> {
        let prompt_width = measure_width(self.prompt());
        let (visual_line, visual_x) = cursor_visual_position_for_anchors(
            self.value(),
            anchors,
            logical_line,
            logical_column,
            prompt_width,
        )?;
        Some((u16::try_from(visual_x).unwrap_or(u16::MAX), visual_line))
    }

    pub(crate) fn line(&self) -> usize {
        self.cursor_position().0
    }

    pub(crate) fn column(&self) -> usize {
        self.cursor_position().1
    }

    pub(crate) fn move_to_begin(&mut self) {
        self.finish_grapheme_undo_group();
        self.set_cursor(0);
        self.sync_viewport_to_cursor();
    }

    pub(crate) fn move_to_end(&mut self) {
        self.finish_grapheme_undo_group();
        self.set_cursor(total_chars(&self.value));
        self.sync_viewport_to_cursor();
    }

    pub(crate) fn handle_page_key(&mut self, direction: isize) -> bool {
        if !matches!(direction, -1 | 1) {
            return false;
        }

        self.finish_grapheme_undo_group();
        self.page_move(direction);
        true
    }

    pub(crate) fn bottom_viewport_offset(&self) -> usize {
        self.total_visual_lines()
            .saturating_sub(self.viewport_height().max(1))
    }

    pub(crate) fn full_height_for_value_at_width(&self, value: &str, width: u16) -> u16 {
        if value.is_empty() {
            return 1;
        }

        let prompt_width = usize::from(prompt_width());
        let content_width = composer_content_width(width);
        let line_count = visual_line_count(value, content_width, prompt_width);

        u16::try_from(line_count.max(1)).unwrap_or(u16::MAX)
    }

    #[cfg(test)]
    pub(crate) fn set_text_for_test(&mut self, value: impl Into<String>) {
        let value = sanitized_owned_text(value.into());
        self.has_active_grapheme_undo_group = false;
        if self.value != value {
            self.value = value;
            self.clear_structured_bindings();
            self.bump_content_revision();
        }
        self.set_cursor(total_chars(&self.value));
        self.sync_viewport_to_cursor();
    }

    #[cfg(test)]
    pub(crate) fn move_to_begin_for_test(&mut self) {
        self.set_cursor(0);
        self.sync_viewport_to_cursor();
    }

    fn insert_plain_input_char(&mut self, character: char) {
        if !self.plain_input_char_extends_active_grapheme(character) {
            self.push_undo_snapshot();
        }
        self.has_active_grapheme_undo_group = true;
        self.insert_char_without_undo(character);
    }

    fn plain_input_char_extends_active_grapheme(&self, character: char) -> bool {
        if !self.has_active_grapheme_undo_group || self.cursor == 0 {
            return false;
        }

        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        let Some(line) = lines.get(row) else {
            return false;
        };
        if column == 0 {
            return false;
        }

        let Some((start, end)) = grapheme_range_before_cursor(line.text, column) else {
            return false;
        };
        if end != column {
            return false;
        }

        let previous_grapheme = line
            .text
            .chars()
            .skip(start)
            .take(end - start)
            .collect::<String>();
        let expected_char_count = total_chars(&previous_grapheme) + 1;
        let mut proposed_grapheme = previous_grapheme;
        proposed_grapheme.push(character);

        // crossterm 只暴露逐个 `KeyCode::Char`，没有 IME commit 边界。
        // 因此不能把连续中文输入猜成同一次上屏；这里仅合并新增 scalar
        // 仍属于同一个 extended grapheme cluster 的情况，避免 undo 恢复半个 emoji /
        // variation selector / combining sequence。
        let clusters = grapheme_clusters(&proposed_grapheme);
        clusters.len() == 1 && clusters[0].end_char == expected_char_count
    }

    fn insert_char_without_undo(&mut self, character: char) {
        let byte_index = char_to_byte_index(&self.value, self.cursor);
        self.value.insert(byte_index, character);
        self.reconcile_structured_bindings();
        self.set_cursor(self.cursor + 1);
        self.bump_content_revision();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        if column == 0 {
            self.delete_absolute_range(self.cursor - 1, self.cursor);
            return;
        }

        let line = lines[row];
        let Some((start, end)) = grapheme_range_before_cursor(line.text, column) else {
            return;
        };

        self.delete_absolute_range(line.start_char + start, line.start_char + end);
    }

    fn delete_forward(&mut self) {
        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        if column >= line.len_chars() {
            if row + 1 < lines.len() {
                self.delete_absolute_range(self.cursor, self.cursor + 1);
            }
            return;
        }

        let Some((start, end)) = grapheme_range_at_or_after_cursor(line.text, column) else {
            return;
        };

        self.delete_absolute_range(line.start_char + start, line.start_char + end);
    }

    fn delete_current_line_before_cursor(&mut self) {
        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        if column == 0 {
            if line.start_char > 0 {
                self.kill_absolute_range(line.start_char - 1, line.start_char);
            }
            return;
        }

        self.kill_absolute_range(line.start_char, line.start_char + column);
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        if column == 0 {
            self.set_cursor(self.cursor - 1);
            return;
        }

        let line = lines[row];
        if let Some(target) = grapheme_target_left(line.text, column) {
            self.set_cursor(line.start_char + target);
        }
    }

    fn move_right(&mut self) {
        if self.cursor >= total_chars(&self.value) {
            return;
        }

        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        if column >= line.len_chars() {
            if row + 1 < lines.len() {
                self.set_cursor(self.cursor + 1);
            }
            return;
        }

        if let Some(target) = grapheme_target_right(line.text, column) {
            self.set_cursor(line.start_char + target);
        }
    }

    fn move_vertical(&mut self, direction: isize) {
        if self.value.is_empty() {
            return;
        }

        let lines = logical_lines(&self.value);
        let visual_lines = visual_lines_for_text(
            &self.value,
            self.content_width(),
            usize::from(prompt_width()),
        );
        let (row, column) = logical_position(&self.value, self.cursor);
        let (current_visual_line, current_visual_column) =
            calculate_cursor_visual_position(&visual_lines, row, column, 0);

        let Some(target_visual_line) =
            offset_index(current_visual_line, direction, visual_lines.len())
        else {
            return;
        };

        let target_line = &visual_lines[target_visual_line];
        let target_column = logical_column_for_visual_offset(
            target_line,
            current_visual_column,
            self.content_width(),
        );
        self.set_cursor(absolute_cursor_for_position(
            &lines,
            target_line.logical_line,
            target_column,
        ));
    }

    fn move_line_start(&mut self) {
        let lines = logical_lines(&self.value);
        let (row, _) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        self.set_cursor(lines[row].start_char);
    }

    fn move_line_end(&mut self) {
        let lines = logical_lines(&self.value);
        let (row, _) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        self.set_cursor(lines[row].start_char + lines[row].len_chars());
    }

    fn move_word_left(&mut self) {
        self.set_cursor(self.beginning_of_previous_word());
    }

    fn move_word_right(&mut self) {
        self.set_cursor(self.end_of_next_word());
    }

    fn delete_backward_word(&mut self) {
        let start = self.beginning_of_previous_word();
        self.kill_absolute_range(start, self.cursor);
    }

    fn delete_forward_word(&mut self) {
        let end = self.end_of_next_word();
        self.kill_absolute_range(self.cursor, end);
    }

    fn kill_to_end_of_line(&mut self) {
        let lines = logical_lines(&self.value);
        let (row, _) = logical_position(&self.value, self.cursor);
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        let line_end = line.start_char + line.len_chars();
        if self.cursor < line_end {
            self.kill_absolute_range(self.cursor, line_end);
        } else if row + 1 < lines.len() {
            self.kill_absolute_range(self.cursor, self.cursor + 1);
        }
    }

    fn yank(&mut self) {
        if self.kill_buffer.is_empty() {
            return;
        }

        let text = self.kill_buffer.clone();
        self.insert_text(&text);
    }

    fn page_move(&mut self, direction: isize) {
        let visual_lines = visual_lines_for_text(
            &self.value,
            self.content_width(),
            usize::from(prompt_width()),
        );
        if visual_lines.is_empty() {
            self.viewport_y = 0;
            return;
        }

        let lines = logical_lines(&self.value);
        let (row, column) = logical_position(&self.value, self.cursor);
        let (current_visual_line, current_visual_column) =
            calculate_cursor_visual_position(&visual_lines, row, column, 0);
        let current_offset = sync_viewport_offset_for_cursor(
            self.viewport_y,
            self.viewport_height(),
            visual_lines.len(),
            current_visual_line,
        );

        let target_visual_line = if direction < 0 {
            if current_visual_line > current_offset {
                current_offset
            } else {
                current_visual_line.saturating_sub(self.viewport_height())
            }
        } else {
            let last_visible_line =
                current_offset.saturating_add(self.viewport_height().saturating_sub(1));
            let last_visible_line = last_visible_line.min(visual_lines.len().saturating_sub(1));
            if current_visual_line < last_visible_line {
                last_visible_line
            } else {
                current_visual_line
                    .saturating_add(self.viewport_height())
                    .min(visual_lines.len().saturating_sub(1))
            }
        };

        let target_line = &visual_lines[target_visual_line];
        let target_column = logical_column_for_visual_offset(
            target_line,
            current_visual_column,
            self.content_width(),
        );
        self.set_cursor(absolute_cursor_for_position(
            &lines,
            target_line.logical_line,
            target_column,
        ));
        self.viewport_y = sync_viewport_offset_for_cursor(
            current_offset,
            self.viewport_height(),
            visual_lines.len(),
            target_visual_line,
        );
    }

    pub(crate) fn sync_viewport_to_cursor(&mut self) {
        if self.value.is_empty() {
            self.viewport_y = 0;
            return;
        }

        let visual_lines = visual_lines_for_text(
            &self.value,
            self.content_width(),
            usize::from(prompt_width()),
        );
        let (row, column) = logical_position(&self.value, self.cursor);
        let (cursor_visual_y, _) = calculate_cursor_visual_position(&visual_lines, row, column, 0);
        self.viewport_y = sync_viewport_offset_for_cursor(
            self.viewport_y,
            self.viewport_height(),
            visual_lines.len(),
            cursor_visual_y,
        );
    }

    fn clamp_viewport(&mut self) {
        self.viewport_y = self.clamp_viewport_offset(self.viewport_y);
    }

    fn clamp_viewport_offset(&self, offset: usize) -> usize {
        let total_lines = self.total_visual_lines();
        if total_lines == 0 {
            return 0;
        }

        offset.min(total_lines.saturating_sub(self.viewport_height().max(1)))
    }

    fn total_visual_lines(&self) -> usize {
        if self.value.is_empty() {
            return 1;
        }

        let prompt_width = usize::from(prompt_width());
        visual_line_count(&self.value, self.content_width(), prompt_width).max(1)
    }

    fn current_prefixed_token(&self, prefix: char) -> Option<PrefixedToken> {
        let chars = self.value.chars().collect::<Vec<_>>();
        if chars.is_empty() {
            return None;
        }

        let cursor = self.cursor.min(chars.len());
        let mut start = cursor;
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }

        let mut end = cursor;
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }

        if start >= end || chars.get(start).copied() != Some(prefix) {
            return None;
        }

        let query = chars[start + 1..end].iter().collect::<String>();
        Some(PrefixedToken {
            query,
            start_char: start,
            end_char: end,
            visible_text: chars[start..end].iter().collect(),
        })
    }

    fn delete_absolute_range(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }

        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        self.delete_absolute_range_without_undo(start, end);
    }

    fn kill_absolute_range(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }

        let byte_start = char_to_byte_index(&self.value, start);
        let byte_end = char_to_byte_index(&self.value, end);
        let killed_text = self.value[byte_start..byte_end].to_string();
        if killed_text.is_empty() {
            return;
        }

        self.kill_buffer = killed_text;
        self.finish_grapheme_undo_group();
        self.push_undo_snapshot();
        self.delete_absolute_range_without_undo(start, end);
    }

    fn delete_absolute_range_without_undo(&mut self, start: usize, end: usize) {
        let byte_start = char_to_byte_index(&self.value, start);
        let byte_end = char_to_byte_index(&self.value, end);
        self.value.drain(byte_start..byte_end);
        self.reconcile_structured_bindings();
        self.set_cursor(start.min(total_chars(&self.value)));
        self.bump_content_revision();
    }

    fn push_undo_snapshot(&mut self) {
        let snapshot = ComposerSnapshot {
            value: self.value.clone(),
            skill_bindings: self.skill_bindings.clone(),
            custom_prompt_bindings: self.custom_prompt_bindings.clone(),
            image_attachments: self.image_attachments.clone(),
            cursor: self.cursor,
        };
        if self.undo_history.last() == Some(&snapshot) {
            return;
        }

        if self.undo_history.len() >= self.undo_limit {
            self.undo_history.remove(0);
        }
        self.undo_history.push(snapshot);
    }

    fn finish_grapheme_undo_group(&mut self) {
        self.has_active_grapheme_undo_group = false;
    }

    fn undo(&mut self) {
        self.finish_grapheme_undo_group();
        let Some(snapshot) = self.undo_history.pop() else {
            return;
        };

        if self.value != snapshot.value {
            self.value = snapshot.value;
            self.skill_bindings = snapshot.skill_bindings;
            self.custom_prompt_bindings = snapshot.custom_prompt_bindings;
            self.image_attachments = snapshot.image_attachments;
            self.bump_content_revision();
        } else {
            self.skill_bindings = snapshot.skill_bindings;
            self.custom_prompt_bindings = snapshot.custom_prompt_bindings;
            self.image_attachments = snapshot.image_attachments;
        }
        self.set_cursor(snapshot.cursor.min(total_chars(&self.value)));
        self.sync_viewport_to_cursor();
    }

    fn beginning_of_previous_word(&self) -> usize {
        let chars = self.value.chars().collect::<Vec<_>>();
        let mut index = self.cursor.min(chars.len());
        while index > 0 && chars[index - 1].is_whitespace() {
            index -= 1;
        }
        if index == 0 {
            return 0;
        }

        let is_separator_run = is_word_separator(chars[index - 1]);
        while index > 0 {
            let previous = chars[index - 1];
            if previous.is_whitespace() || is_word_separator(previous) != is_separator_run {
                break;
            }
            index -= 1;
        }
        index
    }

    fn end_of_next_word(&self) -> usize {
        let chars = self.value.chars().collect::<Vec<_>>();
        let mut index = self.cursor.min(chars.len());
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index >= chars.len() {
            return chars.len();
        }

        let is_separator_run = is_word_separator(chars[index]);
        while index < chars.len() {
            let character = chars[index];
            if character.is_whitespace() || is_word_separator(character) != is_separator_run {
                break;
            }
            index += 1;
        }
        index
    }

    fn bump_content_revision(&mut self) {
        self.content_revision = self.content_revision.saturating_add(1);
    }

    fn set_cursor(&mut self, cursor: usize) {
        if self.cursor == cursor {
            return;
        }

        self.cursor = cursor;
        self.cursor_revision = self.cursor_revision.saturating_add(1);
    }

    fn clear_structured_bindings(&mut self) {
        self.skill_bindings.clear();
        self.custom_prompt_bindings.clear();
        self.image_attachments.clear();
    }

    fn reconcile_structured_bindings(&mut self) {
        self.reconcile_skill_bindings();
        self.reconcile_custom_prompt_bindings();
        self.reconcile_image_attachments();
    }

    pub(crate) fn reconcile_skill_bindings(&mut self) {
        let tokens = prefixed_tokens_in_text(&self.value, '$');
        self.skill_bindings = reconcile_bound_prefixed_tokens(
            &self.skill_bindings,
            &tokens,
            |binding| (binding.start_char, binding.end_char),
            TranscriptSkillBinding::visible_token_text,
            |binding, token| {
                binding.start_char = token.start_char;
                binding.end_char = token.end_char;
            },
        );
    }

    fn reconcile_custom_prompt_bindings(&mut self) {
        let tokens = prefixed_tokens_in_text(&self.value, '#');
        self.custom_prompt_bindings = reconcile_bound_prefixed_tokens(
            &self.custom_prompt_bindings,
            &tokens,
            |binding| (binding.start_char, binding.end_char),
            TranscriptCustomPromptBinding::visible_token_text,
            |binding, token| {
                binding.start_char = token.start_char;
                binding.end_char = token.end_char;
            },
        );
    }

    fn reconcile_image_attachments(&mut self) {
        if self.image_attachments.is_empty() {
            return;
        }

        let ranges = image_attachment_ranges_in_text(&self.value, &self.image_attachments);
        self.image_attachments = ranges
            .into_iter()
            .map(|range| self.image_attachments[range.attachment_index].clone())
            .collect();
    }

    fn source_image_attachments(&self) -> Vec<TranscriptUserAttachment> {
        let mut ranges = image_attachment_ranges_in_text(&self.value, &self.image_attachments);
        ranges.sort_unstable_by_key(|range| range.start_char);
        ranges
            .into_iter()
            .map(|range| {
                self.image_attachments[range.attachment_index]
                    .attachment()
                    .clone()
            })
            .collect()
    }

    pub(crate) fn image_attachment_highlight_ranges(&self) -> Vec<(usize, usize)> {
        let mut ranges = image_attachment_ranges_in_text(&self.value, &self.image_attachments)
            .into_iter()
            .map(|range| (range.start_char, range.end_char))
            .collect::<Vec<_>>();
        ranges.sort_unstable_by_key(|(start, _)| *start);
        ranges
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PrefixedToken {
    query: String,
    start_char: usize,
    end_char: usize,
    visible_text: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LogicalLine<'a> {
    pub(crate) text: &'a str,
    pub(crate) start_char: usize,
}

impl LogicalLine<'_> {
    pub(crate) fn len_chars(self) -> usize {
        self.text.chars().count()
    }
}

pub(crate) fn logical_lines(value: &str) -> Vec<LogicalLine<'_>> {
    let mut lines = Vec::new();
    let mut start_char = 0;

    for segment in value.split('\n') {
        lines.push(LogicalLine {
            text: segment,
            start_char,
        });
        start_char += segment.chars().count() + 1;
    }

    if lines.is_empty() {
        lines.push(LogicalLine {
            text: "",
            start_char: 0,
        });
    }

    lines
}

pub(crate) fn logical_position(value: &str, cursor: usize) -> (usize, usize) {
    let cursor = cursor.min(total_chars(value));
    let lines = logical_lines(value);

    for (row, line) in lines.iter().enumerate() {
        let line_end = line.start_char + line.len_chars();
        if cursor <= line_end {
            return (row, cursor.saturating_sub(line.start_char));
        }
    }

    let last_row = lines.len().saturating_sub(1);
    let last_line = lines[last_row];
    (last_row, last_line.len_chars())
}

pub(crate) fn absolute_cursor_for_position(
    lines: &[LogicalLine<'_>],
    row: usize,
    column: usize,
) -> usize {
    if lines.is_empty() {
        return 0;
    }

    let row = row.min(lines.len() - 1);
    let line = lines[row];
    line.start_char + column.min(line.len_chars())
}

pub(crate) fn char_to_byte_index(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or(value.len())
}

fn cursor_visual_position_for_anchors(
    value: &str,
    anchors: &[LineAnchor],
    logical_line: usize,
    logical_column: usize,
    prompt_width: usize,
) -> Option<(usize, usize)> {
    let (first_line, last_line) = line_anchor_bounds(anchors, logical_line)?;
    if logical_column == 0 {
        return Some((first_line, prompt_width));
    }

    let last_anchor = anchors[last_line];
    let logical_column = logical_column.min(last_anchor.end_char);
    for line_index in first_line..=last_line {
        let anchor = anchors[line_index];
        if logical_column == anchor.end_char && line_index < last_line {
            let next_anchor = anchors[line_index + 1];
            if next_anchor.visible_start_char <= logical_column
                && logical_column <= next_anchor.end_char
            {
                continue;
            }
            if next_anchor.visible_start_char == logical_column {
                return Some((line_index + 1, prompt_width));
            }
        }

        if logical_column > anchor.end_char {
            continue;
        }

        if logical_column <= anchor.visible_start_char {
            return Some((line_index, prompt_width));
        }

        return Some((
            line_index,
            prompt_width + visual_width_for_anchor_prefix(value, anchor, logical_column)?,
        ));
    }

    Some((
        last_line,
        prompt_width + visual_width_for_anchor_prefix(value, last_anchor, last_anchor.end_char)?,
    ))
}

fn line_anchor_bounds(anchors: &[LineAnchor], logical_line: usize) -> Option<(usize, usize)> {
    let mut first = None;
    let mut last = None;
    for (index, anchor) in anchors.iter().enumerate() {
        if anchor.logical_line != logical_line {
            if first.is_some() {
                break;
            }
            continue;
        }
        first.get_or_insert(index);
        last = Some(index);
    }
    match (first, last) {
        (Some(first), Some(last)) => Some((first, last)),
        _ => None,
    }
}

fn visual_width_for_anchor_prefix(
    value: &str,
    anchor: LineAnchor,
    logical_column: usize,
) -> Option<usize> {
    let lines = logical_lines(value);
    let line = lines.get(anchor.logical_line)?;
    let end_char = logical_column.min(anchor.end_char).min(line.len_chars());
    if end_char <= anchor.visible_start_char {
        return Some(0);
    }

    let text = line
        .text
        .chars()
        .skip(anchor.visible_start_char)
        .take(end_char - anchor.visible_start_char)
        .collect::<String>();
    Some(
        grapheme_clusters(&text)
            .iter()
            .map(|cluster| cluster.width)
            .sum(),
    )
}

fn total_chars(value: &str) -> usize {
    value.chars().count()
}

fn prefixed_tokens_in_text(value: &str, prefix: char) -> Vec<PrefixedToken> {
    let chars = value.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index >= chars.len() {
            break;
        }
        let start = index;
        while index < chars.len() && !chars[index].is_whitespace() {
            index += 1;
        }
        let end = index;
        if chars.get(start).copied() != Some(prefix) {
            continue;
        }
        let visible_text = chars[start..end].iter().collect::<String>();
        tokens.push(PrefixedToken {
            query: chars[start + 1..end].iter().collect(),
            start_char: start,
            end_char: end,
            visible_text,
        });
    }
    tokens
}

fn reconcile_bound_prefixed_tokens<T: Clone>(
    bindings: &[T],
    tokens: &[PrefixedToken],
    current_range: impl Fn(&T) -> (usize, usize),
    expected_visible_text: impl Fn(&T) -> String,
    mut update_range: impl FnMut(&mut T, &PrefixedToken),
) -> Vec<T> {
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut matched = vec![false; tokens.len()];
    let mut next_bindings = Vec::new();
    for binding in bindings {
        let expected = expected_visible_text(binding);
        let (start_char, end_char) = current_range(binding);
        let preferred = tokens
            .iter()
            .enumerate()
            .find(|(index, token)| {
                !matched[*index]
                    && token.start_char == start_char
                    && token.end_char == end_char
                    && token.visible_text == expected
            })
            .map(|(index, token)| (index, token.clone()))
            .or_else(|| {
                tokens
                    .iter()
                    .enumerate()
                    .find(|(index, token)| !matched[*index] && token.visible_text == expected)
                    .map(|(index, token)| (index, token.clone()))
            });
        let Some((index, token)) = preferred else {
            continue;
        };
        matched[index] = true;
        let mut binding = binding.clone();
        update_range(&mut binding, &token);
        next_bindings.push(binding);
    }
    next_bindings
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ImageAttachmentRange {
    attachment_index: usize,
    start_char: usize,
    end_char: usize,
}

fn image_attachment_ranges_in_text(
    value: &str,
    attachments: &[ComposerImageAttachment],
) -> Vec<ImageAttachmentRange> {
    let mut used_byte_starts = Vec::<usize>::new();
    let mut ranges = Vec::new();
    for (attachment_index, attachment) in attachments.iter().enumerate() {
        let Some((byte_start, placeholder)) =
            first_unused_placeholder_match(value, attachment.placeholder(), &used_byte_starts)
        else {
            continue;
        };
        used_byte_starts.push(byte_start);
        let start_char = byte_to_char_index(value, byte_start);
        ranges.push(ImageAttachmentRange {
            attachment_index,
            start_char,
            end_char: start_char + placeholder.chars().count(),
        });
    }
    ranges
}

fn first_unused_placeholder_match<'a>(
    value: &'a str,
    placeholder: &'a str,
    used_byte_starts: &[usize],
) -> Option<(usize, &'a str)> {
    value
        .match_indices(placeholder)
        .find(|(byte_start, _)| !used_byte_starts.contains(byte_start))
}

fn byte_to_char_index(value: &str, byte_index: usize) -> usize {
    value[..byte_index.min(value.len())].chars().count()
}

fn prompt_width() -> u16 {
    u16::try_from(measure_width("┃ ")).unwrap_or(u16::MAX)
}

fn composer_content_width(frame_width: u16) -> usize {
    usize::from(
        frame_width
            .saturating_sub(prompt_width())
            .saturating_sub(COMPOSER_RIGHT_PADDING_WIDTH),
    )
    .max(1)
}

fn sanitized_owned_text(value: String) -> String {
    match sanitize_terminal_text(&value) {
        std::borrow::Cow::Borrowed(_) => value,
        std::borrow::Cow::Owned(sanitized) => sanitized,
    }
}

fn is_ctrl_only(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
}

fn has_word_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) || modifiers.contains(KeyModifiers::ALT)
}

fn is_word_separator(character: char) -> bool {
    WORD_SEPARATORS.contains(character)
}

fn offset_index(current: usize, direction: isize, len: usize) -> Option<usize> {
    match direction {
        -1 if current > 0 => Some(current - 1),
        1 if current + 1 < len => Some(current + 1),
        _ => None,
    }
}
