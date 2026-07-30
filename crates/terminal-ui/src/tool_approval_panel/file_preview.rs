use ratatui::{
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::Paragraph,
};
use runtime_domain::session::{
    RuntimeToolActivity, RuntimeToolActivityContent, RuntimeToolActivityStatus, RuntimeToolKind,
};

#[cfg(test)]
use std::cell::Cell;

use crate::{
    Model,
    inline_panel::inline_panel_rule_line,
    render_frame::RenderFrame,
    runtime::tool_activity_preview::ToolApprovalPreview,
    styled_text::render_line_with_full_width_background,
    theme::{
        TerminalPalette, muted_text_style, primary_text_style, subtle_rule_line,
        tertiary_text_style,
    },
    tool_result::{ToolActivityRenderMode, ToolResultItem},
    transcript_overlay::build_percentage_rule,
};

const FILE_PREVIEW_FULLSCREEN_HEADER_ROWS: usize = 2;
const FILE_PREVIEW_FULLSCREEN_FOOTER_ROWS: usize = 2;

/// Inline preview 在 diff 内容之外固定包含 rule、header、两条 separator 与 command bar。
const FILE_PREVIEW_INLINE_FRAME_ROWS: usize = 5;

#[cfg(test)]
thread_local! {
    static FILE_PREVIEW_ITEM_BUILD_COUNT: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
pub(super) fn reset_file_preview_item_build_count() {
    FILE_PREVIEW_ITEM_BUILD_COUNT.set(0);
}

#[cfg(test)]
pub(super) fn file_preview_item_build_count() -> usize {
    FILE_PREVIEW_ITEM_BUILD_COUNT.get()
}

#[cfg(test)]
fn record_file_preview_item_build() {
    FILE_PREVIEW_ITEM_BUILD_COUNT.with(|count| count.set(count.get() + 1));
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilePreviewRenderCache {
    width: usize,
    palette_version: usize,
    header_line: Line<'static>,
    lines: Vec<Line<'static>>,
}

/// `FilePreviewState` 将审批源数据、唯一的 diff item 与其渲染状态绑定为同一 revision。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FilePreviewState {
    preview: ToolApprovalPreview,
    item: ToolResultItem,
    pub(super) is_fullscreen: bool,
    pub(super) scroll_offset: usize,
    render_cache: Option<FilePreviewRenderCache>,
}

impl FilePreviewState {
    pub(super) fn new(preview: ToolApprovalPreview) -> Self {
        let item = build_file_preview_item(&preview);
        Self {
            preview,
            item,
            is_fullscreen: false,
            scroll_offset: 0,
            render_cache: None,
        }
    }

    pub(super) fn preview(&self) -> &ToolApprovalPreview {
        &self.preview
    }

    fn render_lines(&self, width: usize, palette: TerminalPalette) -> Vec<Line<'static>> {
        // 审批 marker 是静态标签；固定在 item 起始帧，避免最终行 cache 捕获瞬态 blink。
        let marker_now = self
            .item
            .active_marker_started_at()
            // File preview 始终由 Pending item 构造；缺失起始时间表示构造约束已被破坏。
            .expect("file preview item must retain its active-marker start instant");
        self.item.render_lines_at(width as u16, palette, marker_now)
    }
}

pub(super) fn build_file_preview_panel_lines(
    model: &mut Model,
    width: usize,
) -> Vec<Line<'static>> {
    let width = width.max(1);
    let (header_line, diff_lines) = {
        let cache = file_preview_content_cache(model, width);
        (cache.header_line.clone(), cache.lines.clone())
    };
    let mut lines = vec![
        inline_panel_rule_line(width, model.palette),
        header_line,
        file_preview_separator_line(width, model.palette),
    ];
    lines.extend(diff_lines);
    lines.push(file_preview_separator_line(width, model.palette));
    lines.push(file_preview_command_bar_line(model, false));
    lines
}

/// 返回 inline preview 的完整高度；只读取已同步 cache 的行数，不 clone 最终行。
pub(super) fn file_preview_panel_line_count(model: &mut Model, width: usize) -> usize {
    file_preview_content_line_count(model, width).saturating_add(FILE_PREVIEW_INLINE_FRAME_ROWS)
}

pub(super) fn file_preview_fullscreen_content_height(model: &Model) -> usize {
    file_preview_fullscreen_content_height_for(usize::from(model.height.max(1)))
}

pub(super) fn file_preview_fullscreen_max_offset(model: &mut Model) -> usize {
    let width = usize::from(model.width.max(1));
    let content_height = file_preview_fullscreen_content_height(model);
    let total_lines = file_preview_content_line_count(model, width);
    total_lines.saturating_sub(content_height)
}

impl Model {
    pub(crate) fn render_tool_approval_fullscreen_preview(
        &mut self,
        frame: &mut RenderFrame<'_>,
        area: Rect,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let width = usize::from(area.width.max(1));
        let palette = self.palette;
        let available_height = usize::from(area.height);
        let footer_height = FILE_PREVIEW_FULLSCREEN_FOOTER_ROWS.min(available_height);
        let header_height =
            FILE_PREVIEW_FULLSCREEN_HEADER_ROWS.min(available_height.saturating_sub(footer_height));
        let content_height = available_height.saturating_sub(header_height + footer_height);
        let total_lines = file_preview_content_line_count(self, width);
        let max_offset = total_lines.saturating_sub(content_height);
        let scroll_offset = self
            .tool_approval_panel
            .file_preview
            .as_ref()
            .map(|state| state.scroll_offset.min(max_offset))
            .unwrap_or_default();
        if let Some(state) = self.tool_approval_panel.file_preview.as_mut() {
            state.scroll_offset = scroll_offset;
        }
        let header_line = file_preview_fullscreen_header_line(self, width);
        let percentage = scroll_percentage(scroll_offset, max_offset);
        let progress_line = build_percentage_rule(area.width, percentage, palette);
        let footer_line = file_preview_command_bar_line(self, true);

        render_file_preview_header(frame, area, palette, header_line, header_height, width);
        let content_y = area
            .y
            .saturating_add(u16::try_from(header_height).unwrap_or(u16::MAX));
        render_file_preview_content_window(
            frame,
            Rect::new(area.x, content_y, area.width, area.height),
            palette,
            file_preview_visible_content_lines(self, width, scroll_offset, content_height),
            content_height,
        );

        render_file_preview_footer(frame, area, progress_line, footer_line, footer_height);
    }
}

fn file_preview_separator_line(width: usize, palette: TerminalPalette) -> Line<'static> {
    subtle_rule_line(width, palette)
}

fn build_file_preview_content_cache(model: &Model, width: usize) -> FilePreviewRenderCache {
    let width = width.max(1);
    let palette_version = model.palette_version;
    let mut lines = render_file_preview_activity_lines(model, width);
    let header_line = take_file_preview_header_line(model, &mut lines);

    FilePreviewRenderCache {
        width,
        palette_version,
        header_line,
        lines,
    }
}

fn build_file_preview_item(preview: &ToolApprovalPreview) -> ToolResultItem {
    let activity = RuntimeToolActivity {
        activity_id: "tool-approval-preview".to_string(),
        title: preview.path().to_string(),
        kind: if preview.old_text().is_some() {
            RuntimeToolKind::Edit
        } else {
            RuntimeToolKind::Write
        },
        status: RuntimeToolActivityStatus::Pending,
        content: vec![RuntimeToolActivityContent::Diff {
            path: preview.path().to_string(),
            old_text: preview.old_text().map(str::to_string),
            new_text: preview.content().to_string(),
            is_truncated: preview.is_truncated(),
        }],
        locations: Vec::new(),
        raw_input: None,
        raw_output: None,
    };
    #[cfg(test)]
    record_file_preview_item_build();
    ToolResultItem::from_runtime_tool_activity(activity, ToolActivityRenderMode::Detailed)
}

fn render_file_preview_activity_lines(model: &Model, width: usize) -> Vec<Line<'static>> {
    model
        .tool_approval_panel
        .file_preview
        .as_ref()
        .expect("preview panel lines should only be built when preview exists")
        .render_lines(width, model.palette)
}

fn take_file_preview_header_line(model: &Model, lines: &mut Vec<Line<'static>>) -> Line<'static> {
    if lines.is_empty() {
        return file_preview_path_header_line(model);
    }

    lines.remove(0)
}

fn file_preview_path_header_line(model: &Model) -> Line<'static> {
    let preview = model
        .tool_approval_panel
        .file_preview
        .as_ref()
        .expect("preview content lines should only be built when preview exists")
        .preview();
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            preview.path().to_string(),
            primary_text_style(model.palette).add_modifier(Modifier::BOLD),
        ),
    ])
}

fn ensure_file_preview_content_cache(model: &mut Model, width: usize) {
    let width = width.max(1);
    let palette_version = model.palette_version;
    let is_cache_current = model
        .tool_approval_panel
        .file_preview
        .as_ref()
        .and_then(|state| state.render_cache.as_ref())
        .is_some_and(|cache| cache.width == width && cache.palette_version == palette_version);
    if is_cache_current {
        return;
    }

    let render_cache = build_file_preview_content_cache(model, width);
    if let Some(state) = model.tool_approval_panel.file_preview.as_mut() {
        state.render_cache = Some(render_cache);
    }
}

fn file_preview_content_cache(model: &mut Model, width: usize) -> &FilePreviewRenderCache {
    ensure_file_preview_content_cache(model, width);
    model
        .tool_approval_panel
        .file_preview
        .as_ref()
        .and_then(|state| state.render_cache.as_ref())
        .expect("file preview cache must exist after synchronization")
}

fn file_preview_content_line_count(model: &mut Model, width: usize) -> usize {
    file_preview_content_cache(model, width).lines.len()
}

fn file_preview_visible_content_lines(
    model: &mut Model,
    width: usize,
    scroll_offset: usize,
    content_height: usize,
) -> Vec<Line<'static>> {
    if content_height == 0 {
        return Vec::new();
    }

    file_preview_content_cache(model, width)
        .lines
        .iter()
        .skip(scroll_offset)
        .take(content_height)
        .cloned()
        .collect()
}

fn file_preview_fullscreen_header_line(model: &mut Model, width: usize) -> Line<'static> {
    file_preview_content_cache(model, width).header_line.clone()
}

fn file_preview_command_bar_line(model: &Model, has_scroll_hints: bool) -> Line<'static> {
    let hint_style = tertiary_text_style(model.palette).add_modifier(Modifier::ITALIC);
    let mut spans = Vec::new();
    if let Some(preview) = model
        .tool_approval_panel
        .file_preview
        .as_ref()
        .map(FilePreviewState::preview)
    {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            preview.question(),
            primary_text_style(model.palette),
        ));
        spans.push(Span::raw("  "));
    }
    spans.push(Span::styled("y/Enter approve", hint_style));
    spans.push(Span::styled(" · n reject", hint_style));
    spans.push(Span::styled(" · Esc cancel", hint_style));
    if has_scroll_hints {
        spans.push(Span::styled(" · ↑↓ scroll", hint_style));
        spans.push(Span::styled(" · PgUp/PgDn", hint_style));
        spans.push(Span::styled(" · Home/End", hint_style));
    }
    Line::from(spans)
}

fn file_preview_fullscreen_content_height_for(height: usize) -> usize {
    height.saturating_sub(FILE_PREVIEW_FULLSCREEN_HEADER_ROWS + FILE_PREVIEW_FULLSCREEN_FOOTER_ROWS)
}

fn render_file_preview_header(
    frame: &mut RenderFrame<'_>,
    area: Rect,
    palette: TerminalPalette,
    header_line: Line<'static>,
    header_height: usize,
    width: usize,
) {
    if header_height == 0 {
        return;
    }
    render_line_with_full_width_background(
        &header_line,
        Rect::new(area.x, area.y, area.width, 1),
        frame.buffer_mut(),
    );
    if header_height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(file_preview_separator_line(width, palette)),
        Rect::new(area.x, area.y.saturating_add(1), area.width, 1),
    );
}

fn render_file_preview_content_window(
    frame: &mut RenderFrame<'_>,
    area: Rect,
    palette: TerminalPalette,
    visible_lines: Vec<Line<'static>>,
    content_height: usize,
) {
    let content_bottom = area
        .y
        .saturating_add(u16::try_from(content_height).unwrap_or(u16::MAX));
    let mut row = area.y;
    for line in visible_lines {
        if row >= content_bottom {
            break;
        }
        render_line_with_full_width_background(
            &line,
            Rect::new(area.x, row, area.width, 1),
            frame.buffer_mut(),
        );
        row = row.saturating_add(1);
    }

    let fill_style = muted_text_style(palette);
    while row < content_bottom {
        frame.render_widget(
            Paragraph::new(Line::styled("~", fill_style)),
            Rect::new(area.x, row, area.width, 1),
        );
        row = row.saturating_add(1);
    }
}

fn render_file_preview_footer(
    frame: &mut RenderFrame<'_>,
    area: Rect,
    progress_line: Line<'static>,
    footer_line: Line<'static>,
    footer_height: usize,
) {
    if footer_height == 0 {
        return;
    }
    let footer_y = area
        .y
        .saturating_add(area.height)
        .saturating_sub(u16::try_from(footer_height).unwrap_or(u16::MAX));
    if footer_height >= 2 {
        frame.render_widget(
            Paragraph::new(progress_line),
            Rect::new(area.x, footer_y, area.width, 1),
        );
    }
    let command_y = area.y.saturating_add(area.height).saturating_sub(1);
    frame.render_widget(
        Paragraph::new(footer_line),
        Rect::new(area.x, command_y, area.width, 1),
    );
}

fn scroll_percentage(scroll_offset: usize, max_offset: usize) -> usize {
    (scroll_offset * 100 + max_offset / 2)
        .checked_div(max_offset)
        .unwrap_or(0)
        .clamp(0, 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        StartupBannerOptions, theme::default_palette, tool_approval_panel::ToolApprovalSource,
    };

    #[test]
    fn preview_mode_sync_populates_the_final_line_cache() {
        let model = model_with_file_preview();

        let cache = model
            .tool_approval_panel
            .file_preview
            .as_ref()
            .and_then(|state| state.render_cache.as_ref())
            .expect("mode sync must materialize the final line cache");

        assert_eq!(cache.width, 72);
        assert_eq!(cache.palette_version, model.palette_version);
    }

    #[test]
    fn inline_panel_uses_the_matching_final_line_cache() {
        let mut model = model_with_file_preview();
        let cached_header = Line::raw("cached header");
        let cached_body = Line::raw("cached body");
        let palette_version = model.palette_version;
        model
            .tool_approval_panel
            .file_preview
            .as_mut()
            .expect("file preview must exist")
            .render_cache = Some(FilePreviewRenderCache {
            width: 72,
            palette_version,
            header_line: cached_header.clone(),
            lines: vec![cached_body.clone()],
        });

        let lines = build_file_preview_panel_lines(&mut model, 72);

        assert_eq!(lines[1], cached_header);
        assert_eq!(lines[3], cached_body);
        assert_eq!(lines.len(), file_preview_panel_line_count(&mut model, 72));
    }

    fn model_with_file_preview() -> Model {
        let mut model = Model::new(StartupBannerOptions::default());
        model.set_window(72, 80);
        model.set_palette(default_palette(), true);
        model.open_tool_approval_panel_with_preview(
            ToolApprovalSource::Preview,
            "WriteFile: cached.rs".to_string(),
            Vec::new(),
            Some(ToolApprovalPreview::create_file(
                "cached.rs",
                "first line\nsecond line\n",
            )),
        );
        model
    }
}
