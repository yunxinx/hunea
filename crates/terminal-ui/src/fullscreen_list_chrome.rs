use ratatui::layout::Rect;

pub(crate) const FULLSCREEN_LIST_HEADER_HEIGHT: u16 = 1;
pub(crate) const FULLSCREEN_LIST_HEADER_RULE_HEIGHT: u16 = 1;
pub(crate) const FULLSCREEN_LIST_PAGE_RULE_HEIGHT: u16 = 1;
pub(crate) const FULLSCREEN_LIST_FOOTER_HEIGHT: u16 = 1;
pub(crate) const FULLSCREEN_LIST_CHROME_HEIGHT: u16 = FULLSCREEN_LIST_HEADER_HEIGHT
    + FULLSCREEN_LIST_HEADER_RULE_HEIGHT
    + FULLSCREEN_LIST_PAGE_RULE_HEIGHT
    + FULLSCREEN_LIST_FOOTER_HEIGHT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FullscreenListChromeRects {
    pub(crate) header: Rect,
    pub(crate) header_rule: Rect,
    pub(crate) body: Rect,
    pub(crate) page_rule: Rect,
    pub(crate) footer: Rect,
}

pub(crate) fn fullscreen_list_page_size_for_height(height: u16) -> usize {
    usize::from(height.saturating_sub(FULLSCREEN_LIST_CHROME_HEIGHT)).max(1)
}

pub(crate) fn fullscreen_list_page_size_for_height_with_row_height(
    height: u16,
    row_height: usize,
) -> usize {
    let body_height = usize::from(height.saturating_sub(FULLSCREEN_LIST_CHROME_HEIGHT));
    (body_height / row_height.max(1)).max(1)
}

pub(crate) fn fullscreen_list_body_visible_offset_for_row(height: u16, row: u16) -> Option<usize> {
    if height < FULLSCREEN_LIST_CHROME_HEIGHT {
        return None;
    }

    let body_top = FULLSCREEN_LIST_HEADER_HEIGHT + FULLSCREEN_LIST_HEADER_RULE_HEIGHT;
    let body_height = height.saturating_sub(FULLSCREEN_LIST_CHROME_HEIGHT);
    (row >= body_top && row < body_top.saturating_add(body_height))
        .then(|| usize::from(row.saturating_sub(body_top)))
}

pub(crate) fn fullscreen_list_chrome_rects(area: Rect) -> Option<FullscreenListChromeRects> {
    if area.is_empty() || area.height < FULLSCREEN_LIST_CHROME_HEIGHT {
        return None;
    }

    let body_height = area.height.saturating_sub(FULLSCREEN_LIST_CHROME_HEIGHT);
    Some(FullscreenListChromeRects {
        header: Rect::new(area.x, area.y, area.width, FULLSCREEN_LIST_HEADER_HEIGHT),
        header_rule: Rect::new(
            area.x,
            area.y + FULLSCREEN_LIST_HEADER_HEIGHT,
            area.width,
            FULLSCREEN_LIST_HEADER_RULE_HEIGHT,
        ),
        body: Rect::new(
            area.x,
            area.y + FULLSCREEN_LIST_HEADER_HEIGHT + FULLSCREEN_LIST_HEADER_RULE_HEIGHT,
            area.width,
            body_height,
        ),
        page_rule: Rect::new(
            area.x,
            area.y
                + area.height.saturating_sub(
                    FULLSCREEN_LIST_PAGE_RULE_HEIGHT + FULLSCREEN_LIST_FOOTER_HEIGHT,
                ),
            area.width,
            FULLSCREEN_LIST_PAGE_RULE_HEIGHT,
        ),
        footer: Rect::new(
            area.x,
            area.y + area.height.saturating_sub(FULLSCREEN_LIST_FOOTER_HEIGHT),
            area.width,
            FULLSCREEN_LIST_FOOTER_HEIGHT,
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_visible_offset_maps_only_body_rows() {
        let height = FULLSCREEN_LIST_CHROME_HEIGHT + 3;

        assert_eq!(fullscreen_list_body_visible_offset_for_row(height, 0), None);
        assert_eq!(fullscreen_list_body_visible_offset_for_row(height, 1), None);
        assert_eq!(
            fullscreen_list_body_visible_offset_for_row(height, 2),
            Some(0)
        );
        assert_eq!(
            fullscreen_list_body_visible_offset_for_row(height, 3),
            Some(1)
        );
        assert_eq!(
            fullscreen_list_body_visible_offset_for_row(height, 4),
            Some(2)
        );
        assert_eq!(fullscreen_list_body_visible_offset_for_row(height, 5), None);
        assert_eq!(fullscreen_list_body_visible_offset_for_row(height, 6), None);
    }

    #[test]
    fn body_visible_offset_rejects_short_chrome() {
        assert_eq!(
            fullscreen_list_body_visible_offset_for_row(FULLSCREEN_LIST_CHROME_HEIGHT - 1, 0),
            None
        );
        assert_eq!(
            fullscreen_list_body_visible_offset_for_row(FULLSCREEN_LIST_CHROME_HEIGHT, 2),
            None
        );
    }

    #[test]
    fn page_size_with_row_height_uses_shared_chrome_body_height() {
        assert_eq!(
            fullscreen_list_page_size_for_height_with_row_height(12, 4),
            2
        );
        assert_eq!(
            fullscreen_list_page_size_for_height_with_row_height(20, 4),
            4
        );
        assert_eq!(
            fullscreen_list_page_size_for_height_with_row_height(7, 4),
            1
        );
    }
}
