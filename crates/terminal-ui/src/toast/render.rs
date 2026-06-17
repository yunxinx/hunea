use super::*;

pub(super) fn toast_rect(area: Rect, text: &str) -> Option<Rect> {
    if area.width < TOAST_MIN_WIDTH || area.height < TOAST_HEIGHT {
        return None;
    }

    let max_width = area.width;
    let desired_width = u16::try_from(
        display_width(text)
            .saturating_add(TOAST_HORIZONTAL_FRAME_WIDTH)
            .max(usize::from(TOAST_MIN_WIDTH)),
    )
    .unwrap_or(u16::MAX);
    let width = desired_width.min(max_width).max(TOAST_MIN_WIDTH);
    let x = area.x.saturating_add(area.width).saturating_sub(width);
    let y = area.y;

    Some(Rect::new(x, y, width, TOAST_HEIGHT))
}

pub(super) fn render_toast_notice(
    notice: &ToastNotice,
    toast_area: Rect,
    buffer: &mut Buffer,
    palette: TerminalPalette,
) {
    Clear.render(toast_area, buffer);

    let text_width = usize::from(toast_area.width).saturating_sub(TOAST_HORIZONTAL_FRAME_WIDTH);
    let text = truncate_display_width_with_ellipsis(&notice.text, text_width);
    let border_style = Style::default().fg(notice.severity.border_color(palette));
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    Paragraph::new(text)
        .style(primary_text_style(palette))
        .block(block)
        .render(toast_area, buffer);
}

pub(super) fn render_toast_transition(
    notice: &ToastNotice,
    toast_area: Rect,
    buffer: &mut Buffer,
    palette: TerminalPalette,
    kind: ToastAnimationKind,
    frame: ToastAnimationFrame,
    underlay: Option<&ToastUnderlaySnapshot>,
) {
    let mut toast_buffer = Buffer::empty(toast_area);
    render_toast_notice(notice, toast_area, &mut toast_buffer, palette);

    match kind {
        ToastAnimationKind::Enter => {
            render_enter_transition(toast_area, &toast_buffer, buffer, frame)
        }
        ToastAnimationKind::Exit => {
            render_exit_transition(toast_area, &toast_buffer, buffer, frame, underlay)
        }
    }
}

fn render_enter_transition(
    toast_area: Rect,
    toast_buffer: &Buffer,
    buffer: &mut Buffer,
    frame: ToastAnimationFrame,
) {
    let erase_start = toast_area.right().saturating_sub(frame.erased_columns);
    erase_columns(buffer, toast_area, erase_start, toast_area.right());

    let leading_blank_columns = if frame.is_complete {
        0
    } else {
        TOAST_ERASE_EDGE_WIDTH.min(frame.erased_columns)
    };
    let visible_columns = frame
        .visible_columns
        .min(toast_area.width.saturating_sub(leading_blank_columns));
    let destination_end = toast_area.right().saturating_sub(leading_blank_columns);
    let destination_start = destination_end.saturating_sub(visible_columns);
    copy_shifted_columns(
        toast_buffer,
        buffer,
        toast_area,
        toast_area.x,
        destination_start,
        visible_columns,
    );
}

fn render_exit_transition(
    toast_area: Rect,
    toast_buffer: &Buffer,
    buffer: &mut Buffer,
    frame: ToastAnimationFrame,
    underlay: Option<&ToastUnderlaySnapshot>,
) {
    let visible_end = toast_area.x.saturating_add(frame.visible_columns);
    copy_columns(toast_buffer, buffer, toast_area, toast_area.x, visible_end);

    let restore_start = toast_area.right().saturating_sub(frame.erased_columns);
    match underlay {
        Some(underlay) => underlay.restore_columns(buffer, restore_start, toast_area.right()),
        None => erase_columns(buffer, toast_area, restore_start, toast_area.right()),
    }
}

fn erase_columns(buffer: &mut Buffer, area: Rect, start: u16, end: u16) {
    let start = start.max(area.x);
    let end = end.min(area.right());
    for y in area.y..area.bottom() {
        for x in start..end {
            buffer[(x, y)].reset();
        }
    }
}

fn copy_columns(source: &Buffer, destination: &mut Buffer, area: Rect, start: u16, end: u16) {
    let start = start.max(area.x);
    let end = end.min(area.right());
    for y in area.y..area.bottom() {
        for x in start..end {
            destination[(x, y)] = source[(x, y)].clone();
        }
    }
}

fn copy_shifted_columns(
    source: &Buffer,
    destination: &mut Buffer,
    area: Rect,
    source_start: u16,
    destination_start: u16,
    columns: u16,
) {
    let source_start = source_start.max(area.x);
    let destination_start = destination_start.max(area.x);
    let columns = columns
        .min(area.right().saturating_sub(source_start))
        .min(area.right().saturating_sub(destination_start));
    for y in area.y..area.bottom() {
        for column in 0..columns {
            destination[(destination_start + column, y)] =
                source[(source_start + column, y)].clone();
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct ToastUnderlaySnapshot {
    area: Rect,
    cells: Vec<Cell>,
}

impl ToastUnderlaySnapshot {
    pub(super) fn capture(buffer: &Buffer, area: Rect) -> Self {
        let area = area.intersection(buffer.area);
        let mut cells = Vec::with_capacity(usize::from(area.width) * usize::from(area.height));
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                cells.push(buffer[(x, y)].clone());
            }
        }
        Self { area, cells }
    }

    fn restore_columns(&self, buffer: &mut Buffer, start: u16, end: u16) {
        let width = usize::from(self.area.width);
        if width == 0 {
            return;
        }
        debug_assert_eq!(
            self.cells.len(),
            width * usize::from(self.area.height),
            "toast underlay snapshot cell count must match captured area"
        );
        let start = start.max(self.area.x);
        let end = end.min(self.area.right());
        for y in self.area.y..self.area.bottom() {
            for x in start..end {
                if !rect_contains(buffer.area, x, y) {
                    continue;
                }
                let index = usize::from(y - self.area.y) * width + usize::from(x - self.area.x);
                match self.cells.get(index) {
                    Some(cell) => buffer[(x, y)] = cell.clone(),
                    None => buffer[(x, y)].reset(),
                }
            }
        }
    }
}

const fn rect_contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}
