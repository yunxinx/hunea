use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Clear, Paragraph, Widget},
};

use super::state::AttentionPillKind;
use crate::{Model, display_width::display_width, render_frame::RenderFrame};

pub(super) const TOOL_APPROVAL_PILL_TEXT: &str = "Tool waiting for approval";

fn new_message_pill_text(count: usize) -> String {
    if count == 1 {
        "1 new message ↓".to_string()
    } else {
        format!("{count} new messages ↓")
    }
}

/// pill 最多占用区域宽度的比例分母：宽度截到一半，避免与右侧 toast 抢位。
const PILL_MAX_WIDTH_DIVISOR: u16 = 2;

impl Model {
    /// 返回当前应展示的 pill 及其屏幕矩形与文案，供渲染与鼠标 hit-test 共用。
    ///
    /// 布局是状态与区域的纯函数，避免在 render 中回写 Model（Elm 约定），
    /// 鼠标处理侧用相同输入即可重建同一组矩形。
    pub(crate) fn attention_pill_hit_targets(
        &self,
        area: Rect,
    ) -> Vec<(AttentionPillKind, Rect, String)> {
        if area.is_empty() {
            return Vec::new();
        }

        // 审批在上（优先级高），新消息在下，左锚定垂直堆叠。
        let mut pills = Vec::new();
        if self.attention_pill.approval_pending {
            pills.push((
                AttentionPillKind::ToolApproval,
                TOOL_APPROVAL_PILL_TEXT.to_string(),
            ));
        }
        if let Some(count) = self.attention_pill.new_message_count {
            pills.push((AttentionPillKind::NewMessages, new_message_pill_text(count)));
        }

        let max_width = (area.width / PILL_MAX_WIDTH_DIVISOR).max(1);
        let mut targets = Vec::with_capacity(pills.len());
        for (row, (kind, text)) in pills.into_iter().enumerate() {
            let Some(offset) = u16::try_from(row)
                .ok()
                .filter(|offset| *offset < area.height)
            else {
                break;
            };
            let padded = format!(" {text} ");
            let width = u16::try_from(display_width(&padded))
                .unwrap_or(u16::MAX)
                .min(max_width);
            targets.push((
                kind,
                Rect::new(area.x, area.y.saturating_add(offset), width, 1),
                padded,
            ));
        }
        targets
    }

    /// 在帧左上角渲染常驻 pill；使用 accent 语义槽位反色，16 色终端自然降级。
    pub(crate) fn render_attention_pills(&self, frame: &mut RenderFrame<'_>, area: Rect) {
        let targets = self.attention_pill_hit_targets(area);
        if targets.is_empty() {
            return;
        }

        let style = Style::default()
            .fg(self.palette.accent)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD);
        for (_, rect, text) in targets {
            let rect = rect.intersection(frame.area());
            if rect.is_empty() {
                continue;
            }
            Clear.render(rect, frame.buffer_mut());
            Paragraph::new(text)
                .style(style)
                .render(rect, frame.buffer_mut());
        }
    }
}
