use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Clear, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthStr;

use super::{
    Model,
    document::{DocumentLayout, DocumentViewport},
    theme::{secondary_text_style, tertiary_text_style},
};

/// `FloatingLayer` 负责承载不参与 document flow 的后绘制浮窗。
#[derive(Debug, Clone, Default)]
pub(crate) struct FloatingLayer {
    surfaces: Vec<FloatingSurface>,
}

impl FloatingLayer {
    fn push_anchored_with_scrollbar(
        &mut self,
        anchor: FloatingAnchor,
        size: FloatingSize,
        lines: Vec<Line<'static>>,
        scrollbar: Option<FloatingScrollbar>,
    ) {
        if lines.is_empty() {
            return;
        }

        self.surfaces.push(FloatingSurface {
            placement: FloatingPlacement::Anchored { anchor, size },
            lines,
            scrollbar,
        });
    }
}

impl Widget for FloatingLayer {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() {
            return;
        }

        for surface in self.surfaces {
            surface.render(area, buf);
        }
    }
}

/// `FloatingAnchor` 表示浮窗锚点相对于当前 Frame 区域的屏幕坐标。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FloatingAnchor {
    x: u16,
    y: u16,
}

impl FloatingAnchor {
    fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }
}

/// `FloatingSize` 表示浮窗希望占用的未裁剪尺寸。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FloatingSize {
    width: u16,
    height: u16,
}

impl FloatingSize {
    const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }

    const fn full_width(height: u16) -> Self {
        Self::new(u16::MAX, height)
    }
}

#[derive(Debug, Clone)]
struct FloatingSurface {
    placement: FloatingPlacement,
    lines: Vec<Line<'static>>,
    scrollbar: Option<FloatingScrollbar>,
}

impl FloatingSurface {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let surface_area = self.placement.resolve(area);
        if surface_area.is_empty() {
            return;
        }

        clear_surface_area(area, surface_area, buf);

        for (row, line) in self
            .lines
            .iter()
            .take(usize::from(surface_area.height))
            .enumerate()
        {
            let y = surface_area.y + u16::try_from(row).unwrap_or(u16::MAX);
            buf.set_line(surface_area.x, y, line, surface_area.width);
        }

        if let Some(scrollbar) = self.scrollbar {
            scrollbar.render(surface_area, buf);
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FloatingScrollbar {
    content_length: usize,
    viewport_content_length: usize,
    position: usize,
    thumb_style: Style,
    track_style: Style,
}

impl FloatingScrollbar {
    const fn new(
        content_length: usize,
        viewport_content_length: usize,
        position: usize,
        thumb_style: Style,
        track_style: Style,
    ) -> Self {
        Self {
            content_length,
            viewport_content_length,
            position,
            thumb_style,
            track_style,
        }
    }

    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.content_length <= self.viewport_content_length || area.width == 0 {
            return;
        }

        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("┃"))
            .thumb_symbol("█")
            .thumb_style(self.thumb_style)
            .track_style(self.track_style);
        let mut state = ScrollbarState::new(self.content_length)
            .position(self.position)
            .viewport_content_length(self.viewport_content_length);
        scrollbar.render(area, buf, &mut state);
    }
}

fn clear_surface_area(bounds: Rect, surface_area: Rect, buf: &mut Buffer) {
    let clear_area = wide_character_safe_clear_area(bounds, surface_area, buf);
    Clear.render(clear_area, buf);
}

fn wide_character_safe_clear_area(bounds: Rect, surface_area: Rect, buf: &Buffer) -> Rect {
    let clear_left = surface_area
        .x
        .checked_sub(1)
        .filter(|left| *left >= bounds.x)
        .is_some_and(|left| {
            (surface_area.top()..surface_area.bottom()).any(|y| buf[(left, y)].symbol().width() > 1)
        });
    let clear_right = surface_area.right() < bounds.right()
        && surface_area.width > 0
        && (surface_area.top()..surface_area.bottom())
            .any(|y| buf[(surface_area.right() - 1, y)].symbol().width() > 1);

    // Ratatui diff 会跳过双宽字符占用的后续单元；若浮窗边界切过双宽字符，
    // 需要把整块清理区域按矩形扩展，避免只在部分行出现锯齿状空白。
    let x = if clear_left {
        surface_area.x - 1
    } else {
        surface_area.x
    };
    let right = if clear_right {
        surface_area.right() + 1
    } else {
        surface_area.right()
    };
    Rect::new(x, surface_area.y, right - x, surface_area.height)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatingPlacement {
    Anchored {
        anchor: FloatingAnchor,
        size: FloatingSize,
    },
}

impl FloatingPlacement {
    fn resolve(self, bounds: Rect) -> Rect {
        if bounds.is_empty() {
            return Rect::ZERO;
        }

        match self {
            Self::Anchored { anchor, size } => resolve_anchored_area(bounds, anchor, size),
        }
    }
}

impl Model {
    pub(crate) fn has_current_floating_layer(&self) -> bool {
        self.file_picker.is_some()
    }

    pub(crate) fn current_floating_layer(
        &self,
        document: &DocumentLayout,
        viewport: &DocumentViewport,
    ) -> FloatingLayer {
        let mut layer = FloatingLayer::default();
        let file_picker = self.current_file_picker_render_result();
        if file_picker.has_content
            && let Some(anchor) = self.current_file_picker_floating_anchor(document, viewport)
        {
            let scrollbar = self.file_picker.as_ref().and_then(|state| {
                let visible_rows = self.file_picker_list_visible_rows();
                (state.items.len() > visible_rows).then_some(FloatingScrollbar::new(
                    state.items.len(),
                    visible_rows,
                    state.scroll,
                    secondary_text_style(self.palette),
                    tertiary_text_style(self.palette),
                ))
            });
            layer.push_anchored_with_scrollbar(
                FloatingAnchor::new(0, anchor.y),
                FloatingSize::full_width(self.file_picker_popup_height),
                file_picker.lines,
                scrollbar,
            );
        }
        layer
    }

    fn current_file_picker_floating_anchor(
        &self,
        document: &DocumentLayout,
        viewport: &DocumentViewport,
    ) -> Option<FloatingAnchor> {
        let token_start = self.composer.current_at_token_start_char()?;
        let tail_composer_start = document.tail.composer_slot.content_start_line;
        let tail_composer_end =
            tail_composer_start.saturating_add(document.tail.composer_slot.content_line_count);
        let composer_anchors = document
            .tail
            .anchors
            .get(tail_composer_start..tail_composer_end)?
            .iter()
            .map(|anchor| anchor.composer)
            .collect::<Vec<_>>();
        let (x, composer_y) = self
            .composer
            .visual_position_for_char_in_anchors(token_start, &composer_anchors)?;
        let document_y = document
            .composer_slot
            .content_start_line
            .saturating_add(composer_y);
        let viewport_y = document_y.checked_sub(viewport.resolved_offset)?;
        if viewport_y >= viewport.lines.len() {
            return None;
        }

        Some(FloatingAnchor::new(x, u16::try_from(viewport_y).ok()?))
    }
}

fn resolve_anchored_area(bounds: Rect, anchor: FloatingAnchor, size: FloatingSize) -> Rect {
    if anchor.x >= bounds.width || anchor.y >= bounds.height {
        return Rect::ZERO;
    }

    let anchor_x = bounds.x.saturating_add(anchor.x);
    let anchor_y = bounds.y.saturating_add(anchor.y);
    let (x, width) = resolve_horizontal_axis(bounds, anchor_x, size.width);
    let (y, height) = resolve_vertical_axis(bounds, anchor_y, size.height);
    Rect::new(x, y, width, height).intersection(bounds)
}

fn resolve_horizontal_axis(bounds: Rect, anchor_x: u16, width: u16) -> (u16, u16) {
    let target_width = width.min(bounds.width);
    if target_width == 0 {
        return (bounds.x, 0);
    }

    let right_space = bounds.right().saturating_sub(anchor_x);
    let left_space = anchor_x.saturating_add(1).saturating_sub(bounds.x);
    if right_space >= target_width {
        return (anchor_x, target_width);
    }
    if left_space >= target_width {
        return (
            anchor_x.saturating_add(1).saturating_sub(target_width),
            target_width,
        );
    }
    if right_space >= left_space {
        (anchor_x, right_space.min(target_width))
    } else {
        (bounds.x, left_space.min(target_width))
    }
}

fn resolve_vertical_axis(bounds: Rect, anchor_y: u16, height: u16) -> (u16, u16) {
    let target_height = height.min(bounds.height);
    if target_height == 0 {
        return (bounds.y, 0);
    }

    let below_start = anchor_y.saturating_add(1);
    let below_space = bounds.bottom().saturating_sub(below_start);
    let above_space = anchor_y.saturating_sub(bounds.y);
    if below_space >= target_height {
        return (below_start, target_height);
    }
    if above_space >= target_height {
        return (anchor_y.saturating_sub(target_height), target_height);
    }
    if below_space >= above_space {
        (below_start, below_space.min(target_height))
    } else {
        (bounds.y, above_space.min(target_height))
    }
}

#[cfg(test)]
mod tests;
