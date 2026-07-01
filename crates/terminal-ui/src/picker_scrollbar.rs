use ratatui::{buffer::Buffer, layout::Rect, style::Style};

const PICKER_SCROLLBAR_TRACK_SYMBOL: &str = "┃";
const PICKER_SCROLLBAR_THUMB_SYMBOL: &str = "█";

/// `PickerScrollbar` 用 picker 的 top-offset 语义渲染竖向滚动条。
#[derive(Debug, Clone, Copy)]
pub(crate) struct PickerScrollbar {
    item_count: usize,
    visible_rows: usize,
    scroll_top: usize,
    thumb_style: Style,
    track_style: Style,
}

impl PickerScrollbar {
    pub(crate) const fn new(
        item_count: usize,
        visible_rows: usize,
        scroll_top: usize,
        thumb_style: Style,
        track_style: Style,
    ) -> Self {
        Self {
            item_count,
            visible_rows,
            scroll_top,
            thumb_style,
            track_style,
        }
    }

    pub(crate) fn render(self, area: Rect, buf: &mut Buffer) {
        let Some(geometry) = PickerScrollbarGeometry::new(
            self.item_count,
            self.visible_rows,
            self.scroll_top,
            usize::from(area.height),
        ) else {
            return;
        };
        if area.width == 0 {
            return;
        }

        let x = area.right().saturating_sub(1);
        for row in 0..area.height {
            let y = area.y.saturating_add(row);
            let is_thumb_row = geometry.contains(usize::from(row));
            let cell = &mut buf[(x, y)];
            cell.set_symbol(if is_thumb_row {
                PICKER_SCROLLBAR_THUMB_SYMBOL
            } else {
                PICKER_SCROLLBAR_TRACK_SYMBOL
            });
            cell.set_style(if is_thumb_row {
                self.thumb_style
            } else {
                self.track_style
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PickerScrollbarGeometry {
    thumb_start: usize,
    thumb_length: usize,
}

impl PickerScrollbarGeometry {
    fn new(
        item_count: usize,
        visible_rows: usize,
        scroll_top: usize,
        track_length: usize,
    ) -> Option<Self> {
        if item_count == 0 || track_length == 0 {
            return None;
        }

        let visible_rows = visible_rows.max(1);
        if item_count <= visible_rows {
            return None;
        }

        let max_scroll = item_count.saturating_sub(visible_rows);
        let scroll_top = scroll_top.min(max_scroll);
        let thumb_length = rounded_divide(visible_rows.saturating_mul(track_length), item_count)
            .clamp(1, track_length);
        let available_track = track_length.saturating_sub(thumb_length);
        let thumb_start = if max_scroll == 0 {
            0
        } else {
            rounded_divide(scroll_top.saturating_mul(available_track), max_scroll)
                .min(available_track)
        };

        Some(Self {
            thumb_start,
            thumb_length,
        })
    }

    const fn contains(self, row: usize) -> bool {
        row >= self.thumb_start && row < self.thumb_start.saturating_add(self.thumb_length)
    }
}

const fn rounded_divide(numerator: usize, denominator: usize) -> usize {
    (numerator + denominator / 2) / denominator
}

#[cfg(test)]
mod tests {
    use super::PickerScrollbarGeometry;

    #[test]
    fn geometry_starts_at_top_for_first_page() {
        let geometry =
            PickerScrollbarGeometry::new(10, 5, 0, 5).expect("overflowing picker should render");

        assert_eq!(geometry.thumb_start, 0);
    }

    #[test]
    fn geometry_reaches_bottom_on_last_page_without_chrome_rows() {
        let geometry =
            PickerScrollbarGeometry::new(10, 5, 5, 5).expect("overflowing picker should render");

        assert_eq!(geometry.thumb_start + geometry.thumb_length, 5);
    }

    #[test]
    fn geometry_reaches_bottom_on_last_page_with_chrome_rows() {
        let geometry =
            PickerScrollbarGeometry::new(8, 3, 5, 6).expect("overflowing picker should render");

        assert_eq!(geometry.thumb_start + geometry.thumb_length, 6);
    }
}
