use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::text::{Line, Span};

use super::theme::{TerminalPalette, muted_text_style, secondary_text_style};

const PROMPT: &str = "┃ ";

/// `Composer` 管理底部单行输入框的文本与光标。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composer {
    value: String,
    cursor: usize,
    width: u16,
}

impl Composer {
    /// `set_width` 更新输入框的可用宽度。
    pub fn set_width(&mut self, width: u16) {
        self.width = width.max(1);
    }

    /// `value` 返回当前输入内容。
    pub fn value(&self) -> &str {
        &self.value
    }

    /// `clear` 清空输入框内容。
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// `insert_char` 在当前光标位置插入一个字符。
    pub fn insert_char(&mut self, character: char) {
        let byte_index = char_to_byte_index(&self.value, self.cursor);
        self.value.insert(byte_index, character);
        self.cursor += 1;
    }

    /// `handle_key` 处理输入编辑相关按键。
    pub fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.insert_char(character);
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.move_left(),
            KeyCode::Right => self.move_right(),
            KeyCode::Home => self.move_home(),
            KeyCode::End => self.move_end(),
            _ => {}
        }
    }

    /// `render_line` 返回当前输入框的带样式文本行。
    pub fn render_line(&self, palette: TerminalPalette) -> Line<'static> {
        let (visible_text, _) = self.visible_text();

        Line::default().spans([
            Span::styled(PROMPT.to_string(), secondary_text_style(palette)),
            Span::styled(visible_text, muted_text_style(palette)),
        ])
    }

    /// `cursor_offset` 返回当前光标在渲染行中的横向偏移量。
    pub fn cursor_offset(&self) -> u16 {
        let (_, visible_cursor) = self.visible_text();
        prompt_width().saturating_add(visible_cursor as u16)
    }
    fn visible_text(&self) -> (String, usize) {
        let available_width = self.width.saturating_sub(prompt_width()).max(1) as usize;
        let characters: Vec<char> = self.value.chars().collect();
        let start = self
            .cursor
            .saturating_sub(available_width.saturating_sub(1))
            .min(characters.len());
        let end = (start + available_width).min(characters.len());
        let visible_text = characters[start..end].iter().collect::<String>();
        let visible_cursor = self.cursor.saturating_sub(start);

        (visible_text, visible_cursor)
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let end = char_to_byte_index(&self.value, self.cursor);
        let start = char_to_byte_index(&self.value, self.cursor - 1);
        self.value.drain(start..end);
        self.cursor -= 1;
    }

    fn delete(&mut self) {
        if self.cursor >= self.value.chars().count() {
            return;
        }

        let start = char_to_byte_index(&self.value, self.cursor);
        let end = char_to_byte_index(&self.value, self.cursor + 1);
        self.value.drain(start..end);
    }

    fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    fn move_home(&mut self) {
        self.cursor = 0;
    }

    fn move_end(&mut self) {
        self.cursor = self.value.chars().count();
    }
}

fn prompt_width() -> u16 {
    PROMPT.chars().count() as u16
}

fn char_to_byte_index(value: &str, character_index: usize) -> usize {
    value
        .char_indices()
        .nth(character_index)
        .map(|(byte_index, _)| byte_index)
        .unwrap_or_else(|| value.len())
}
