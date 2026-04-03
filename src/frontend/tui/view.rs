use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Text,
    widgets::Paragraph,
};

use super::Model;

const TRANSCRIPT_COMPOSER_GAP: u16 = 1;

/// `render` 负责将模型状态映射为当前帧内容。
pub fn render(model: &Model, frame: &mut Frame<'_>) {
    if !model.is_ready() {
        return;
    }

    let area = frame.area();
    if area.is_empty() {
        return;
    }

    let composer_height = 1.min(area.height);
    let gap_height = if area.height > 1 {
        TRANSCRIPT_COMPOSER_GAP
    } else {
        0
    };
    let transcript_height = area.height.saturating_sub(composer_height + gap_height);

    let layout = Layout::vertical([
        Constraint::Length(transcript_height),
        Constraint::Length(gap_height),
        Constraint::Length(composer_height),
    ]);
    let [transcript_area, _, composer_area] = layout.areas(area);

    if transcript_height > 0 {
        let transcript = Paragraph::new(Text::from(model.transcript().render_lines()));
        frame.render_widget(transcript, transcript_area);
    }

    let composer = Paragraph::new(model.composer().render_line(*model.palette()));
    frame.render_widget(composer, composer_area);
    frame.set_cursor_position((
        composer_area.x + model.composer().cursor_offset(),
        composer_area.y,
    ));
}
