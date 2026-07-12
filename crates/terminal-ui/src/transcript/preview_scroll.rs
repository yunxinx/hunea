use super::Transcript;

/// 计算 transcript preview 跟随底部时的精确 offset。
pub(crate) fn latest_preview_offset(transcript: &mut Transcript, content_height: usize) -> usize {
    let content_height = content_height.max(1);
    let mut index = transcript.progressive_item_metrics_index();
    if index.line_count == 0 {
        return 0;
    }

    let mut offset = index.line_count.saturating_sub(content_height);
    let mut remaining_exactization_passes = index.metrics.len().saturating_add(1);
    while remaining_exactization_passes > 0 {
        let effective_total = index.line_count;
        if effective_total == 0 {
            return 0;
        }

        let next_offset = effective_total.saturating_sub(content_height);
        let visible_line_count = content_height.min(effective_total.saturating_sub(next_offset));
        let window = transcript.materialize_line_window(
            next_offset,
            visible_line_count,
            crate::frame_time::FrameRenderContext::capture(),
        );
        let exact_offset = window.index.line_count.saturating_sub(content_height);
        if exact_offset == offset {
            return exact_offset;
        }

        offset = exact_offset;
        index = window.index;
        remaining_exactization_passes -= 1;
    }

    offset
}

/// 计算 preview 翻页后的 offset，底页使用精确 bottom offset。
pub(crate) fn preview_page_offset(
    transcript: &mut Transcript,
    content_height: usize,
    current_offset: usize,
    direction: isize,
) -> usize {
    let content_height = content_height.max(1);
    let latest_offset = latest_preview_offset(transcript, content_height);
    let index = transcript.progressive_item_metrics_index();
    let total_lines = index.line_count;
    if total_lines == 0 {
        return 0;
    }

    let page_count = total_lines.saturating_sub(1) / content_height + 1;
    let current_page = if current_offset >= latest_offset {
        page_count
    } else {
        current_offset / content_height + 1
    };
    let next_page = if direction.is_negative() {
        current_page.saturating_sub(1).max(1)
    } else {
        current_page.saturating_add(1).min(page_count)
    };

    if next_page >= page_count {
        latest_offset
    } else {
        (next_page - 1) * content_height
    }
}
