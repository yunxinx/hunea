use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Text,
    widgets::Paragraph,
};

use super::Model;

/// `render` 负责将模型状态映射为当前帧内容。
pub fn render(model: &Model, frame: &mut Frame<'_>) {
    if !model.is_ready() {
        return;
    }

    let area = frame.area();
    if area.is_empty() {
        return;
    }

    let composer_height = model.composer().visible_height().min(area.height);
    let gap_height = if area.height > composer_height {
        model.composer_gap_height()
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

    let composer_result = model.composer().render(*model.palette());
    let composer = Paragraph::new(Text::from(composer_result.lines));
    frame.render_widget(composer, composer_area);
    frame.set_cursor_position((
        composer_area.x + composer_result.cursor_x,
        composer_area.y + composer_result.cursor_y,
    ));
}
