mod grapheme;
mod layout;
mod mouse;
mod render;
mod viewport;

#[cfg(test)]
mod tests;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use self::{
    grapheme::{
        grapheme_range_at_or_after_cursor, grapheme_range_before_cursor, grapheme_target_left,
        grapheme_target_right, logical_column_for_visual_offset, measure_width,
    },
    layout::{visual_line_count, visual_lines_for_text},
    render::{DocumentRenderResult, render_document},
    viewport::{calculate_cursor_visual_position, sync_viewport_offset_for_cursor},
};
use super::{style_mode::StyleMode, theme::TerminalPalette};

pub(crate) use self::mouse::{
    cursor_position_for_line_anchor_click, move_cursor_to_logical_position,
};
pub(crate) use self::render::LineAnchor;
#[cfg(test)]
use self::render::{RenderResult, render};

const PLACEHOLDER: &str = "Enter to send Prompt";

/// `Composer` 管理底部输入区的文本、光标和自定义 viewport。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    value: String,
    cursor: usize,
    width: u16,
    height: u16,
    viewport_y: usize,
    content_revision: usize,
    cursor_revision: usize,
    style_mode: StyleMode,
}

impl Default for Composer {
    fn default() -> Self {
        Self::new(StyleMode::Cx)
    }
}

impl Composer {
    /// `new` 创建指定样式模式的输入框状态。
    pub fn new(style_mode: StyleMode) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            width: 1,
            height: 1,
            viewport_y: 0,
            content_revision: 1,
            cursor_revision: 1,
            style_mode: style_mode.normalized(),
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
    #[allow(dead_code)]
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
        if self.value.is_empty() {
            self.set_cursor(0);
            self.viewport_y = 0;
            return;
        }

        self.value.clear();
        self.set_cursor(0);
        self.viewport_y = 0;
        self.bump_content_revision();
    }

    /// `replace_text_and_move_to_end` 用新内容替换当前草稿，并把光标移动到末尾。
    pub fn replace_text_and_move_to_end(&mut self, value: impl Into<String>) {
        let value = value.into();
        if self.value != value {
            self.value = value;
            self.bump_content_revision();
        }
        self.set_cursor(total_chars(&self.value));
        self.sync_viewport_to_cursor();
    }

    /// `insert_newline` 在当前光标位置插入显式换行。
    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
        self.sync_viewport_to_cursor();
    }

    /// `insert_text` 在当前光标位置插入一段文本。
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }

        let byte_index = char_to_byte_index(&self.value, self.cursor);
        self.value.insert_str(byte_index, text);
        self.set_cursor(self.cursor + total_chars(text));
        self.bump_content_revision();
        self.sync_viewport_to_cursor();
    }

    /// `handle_key` 处理输入编辑、导航与分页相关按键。
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') if is_ctrl_only(key.modifiers) => self.backspace(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Char('d') if is_ctrl_only(key.modifiers) => self.delete_forward(),
            KeyCode::Delete => self.delete_forward(),
            KeyCode::Char('b') if is_ctrl_only(key.modifiers) => self.move_left(),
            KeyCode::Left => self.move_left(),
            KeyCode::Char('f') if is_ctrl_only(key.modifiers) => self.move_right(),
            KeyCode::Right => self.move_right(),
            KeyCode::Char('p') if is_ctrl_only(key.modifiers) => self.move_vertical(-1),
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Char('n') if is_ctrl_only(key.modifiers) => self.move_vertical(1),
            KeyCode::Down => self.move_vertical(1),
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
                self.insert_char(character);
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
        usize::from(self.width.saturating_sub(prompt_width())).max(1)
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

    pub(crate) fn cursor_revision(&self) -> usize {
        self.cursor_revision
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

    pub(crate) fn line(&self) -> usize {
        self.cursor_position().0
    }

    pub(crate) fn column(&self) -> usize {
        self.cursor_position().1
    }

    pub(crate) fn move_to_begin(&mut self) {
        self.set_cursor(0);
        self.sync_viewport_to_cursor();
    }

    pub(crate) fn move_to_end(&mut self) {
        self.set_cursor(total_chars(&self.value));
        self.sync_viewport_to_cursor();
    }

    pub(crate) fn handle_page_key(&mut self, direction: isize) -> bool {
        if !matches!(direction, -1 | 1) {
            return false;
        }

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
        let content_width = usize::from(width.saturating_sub(prompt_width as u16)).max(1);
        let line_count = visual_line_count(value, content_width, prompt_width);

        u16::try_from(line_count.max(1)).unwrap_or(u16::MAX)
    }

    #[cfg(test)]
    pub(crate) fn set_text_for_test(&mut self, value: impl Into<String>) {
        let value = value.into();
        if self.value != value {
            self.value = value;
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

    fn insert_char(&mut self, character: char) {
        let byte_index = char_to_byte_index(&self.value, self.cursor);
        self.value.insert(byte_index, character);
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

    fn delete_absolute_range(&mut self, start: usize, end: usize) {
        if end <= start {
            return;
        }

        let byte_start = char_to_byte_index(&self.value, start);
        let byte_end = char_to_byte_index(&self.value, end);
        self.value.drain(byte_start..byte_end);
        self.set_cursor(start.min(total_chars(&self.value)));
        self.bump_content_revision();
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

fn total_chars(value: &str) -> usize {
    value.chars().count()
}

fn prompt_width() -> u16 {
    u16::try_from(measure_width("┃ ")).unwrap_or(u16::MAX)
}

fn is_ctrl_only(modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL) && !modifiers.contains(KeyModifiers::ALT)
}

fn offset_index(current: usize, direction: isize, len: usize) -> Option<usize> {
    match direction {
        -1 if current > 0 => Some(current - 1),
        1 if current + 1 < len => Some(current + 1),
        _ => None,
    }
}
