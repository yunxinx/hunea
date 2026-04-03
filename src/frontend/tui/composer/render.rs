use ratatui::text::{Line, Span};

use super::{
    Composer,
    grapheme::{grapheme_clusters, is_space_cluster, measure_width},
    layout::{VisualLine, placeholder_visual_lines_for_text, visual_lines_for_text},
    viewport::calculate_cursor_visual_position,
};
use crate::frontend::tui::theme::{TerminalPalette, muted_text_style, secondary_text_style};

#[cfg(test)]
use super::viewport::visible_viewport_lines;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LineAnchor {
    pub(crate) logical_line: usize,
    pub(crate) visible_start_char: usize,
    pub(crate) end_char: usize,
}

#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct RenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: u16,
}

#[derive(Debug, Clone)]
pub(crate) struct DocumentRenderResult {
    pub(crate) lines: Vec<Line<'static>>,
    pub(crate) plain_lines: Vec<String>,
    pub(crate) anchors: Vec<LineAnchor>,
    pub(crate) cursor_x: u16,
    pub(crate) cursor_y: usize,
}

#[cfg(test)]
pub(crate) fn render(composer: &Composer, palette: TerminalPalette) -> RenderResult {
    let document = render_document(composer, palette);
    let (visible_lines, resolved_offset) = visible_viewport_lines(
        &document.lines,
        composer.viewport_offset(),
        composer.viewport_height(),
    );
    let max_cursor_y = visible_lines.len().saturating_sub(1);
    let cursor_y = document
        .cursor_y
        .saturating_sub(resolved_offset)
        .min(max_cursor_y);

    RenderResult {
        lines: visible_lines.to_vec(),
        cursor_x: document.cursor_x,
        cursor_y: u16::try_from(cursor_y).unwrap_or(u16::MAX),
    }
}

pub(crate) fn render_document(
    composer: &Composer,
    palette: TerminalPalette,
) -> DocumentRenderResult {
    let prompt_width = measure_width(composer.prompt());
    let prompt_style = secondary_text_style(palette);
    let text_style = muted_text_style(palette);
    let placeholder_style = secondary_text_style(palette);

    if composer.value().is_empty() {
        let placeholder_lines = placeholder_visual_lines_for_text(
            composer.placeholder(),
            composer.content_width(),
            prompt_width,
        );
        let plain_lines = rendered_plain_lines(
            &placeholder_lines,
            composer.prompt(),
            prompt_width,
            composer.content_width(),
            true,
        );
        let lines = render_visual_lines(
            &placeholder_lines,
            composer.prompt(),
            prompt_width,
            prompt_style,
            placeholder_style,
            composer.content_width(),
            true,
        );

        return DocumentRenderResult {
            anchors: line_anchors_for_visual_lines(&placeholder_lines),
            lines,
            plain_lines,
            cursor_x: u16::try_from(prompt_width).unwrap_or(u16::MAX),
            cursor_y: 0,
        };
    }

    let visual_lines =
        visual_lines_for_text(composer.value(), composer.content_width(), prompt_width);
    let (row, column) = composer.cursor_position();
    let (cursor_y, cursor_visual_x) =
        calculate_cursor_visual_position(&visual_lines, row, column, prompt_width);
    DocumentRenderResult {
        anchors: line_anchors_for_visual_lines(&visual_lines),
        plain_lines: rendered_plain_lines(
            &visual_lines,
            composer.prompt(),
            prompt_width,
            composer.content_width(),
            false,
        ),
        lines: render_visual_lines(
            &visual_lines,
            composer.prompt(),
            prompt_width,
            prompt_style,
            text_style,
            composer.content_width(),
            false,
        ),
        cursor_x: u16::try_from(cursor_visual_x).unwrap_or(u16::MAX),
        cursor_y,
    }
}

fn render_visual_lines(
    lines: &[VisualLine],
    prompt: &str,
    prompt_width: usize,
    prompt_style: ratatui::style::Style,
    text_style: ratatui::style::Style,
    content_width: usize,
    trim_overflow_spaces: bool,
) -> Vec<Line<'static>> {
    lines
        .iter()
        .map(|line| {
            let (line_prefix, line_text) = rendered_line_parts(
                line,
                prompt,
                prompt_width,
                content_width,
                trim_overflow_spaces,
            );
            Line::default().spans([
                Span::styled(line_prefix, prompt_style),
                Span::styled(line_text, text_style),
            ])
        })
        .collect()
}

fn rendered_plain_lines(
    lines: &[VisualLine],
    prompt: &str,
    prompt_width: usize,
    content_width: usize,
    trim_overflow_spaces: bool,
) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            let (line_prefix, line_text) = rendered_line_parts(
                line,
                prompt,
                prompt_width,
                content_width,
                trim_overflow_spaces,
            );
            format!("{line_prefix}{line_text}")
        })
        .collect()
}

fn line_anchors_for_visual_lines(lines: &[VisualLine]) -> Vec<LineAnchor> {
    let anchors = lines
        .iter()
        .map(|line| LineAnchor {
            logical_line: line.logical_line,
            visible_start_char: line.visible_start_char,
            end_char: line.end_char,
        })
        .collect::<Vec<_>>();

    if anchors.is_empty() {
        vec![LineAnchor::default()]
    } else {
        anchors
    }
}

fn rendered_line_parts(
    line: &VisualLine,
    prompt: &str,
    prompt_width: usize,
    content_width: usize,
    trim_overflow_spaces: bool,
) -> (String, String) {
    let line_prefix = if line.is_continuation {
        " ".repeat(prompt_width)
    } else {
        prompt.to_string()
    };
    let line_text = if trim_overflow_spaces {
        trim_overflow_boundary_spaces(&line.text, content_width)
    } else {
        line.text.clone()
    };

    (line_prefix, line_text)
}

fn trim_overflow_boundary_spaces(text: &str, width: usize) -> String {
    if width == 0 || measure_width(text) <= width {
        return text.to_string();
    }

    let mut graphemes = grapheme_clusters(text);
    let mut total_width = graphemes.iter().map(|cluster| cluster.width).sum::<usize>();

    while total_width > width {
        let Some(last) = graphemes.last() else {
            break;
        };
        if !is_space_cluster(last.text) {
            break;
        }

        total_width = total_width.saturating_sub(last.width);
        graphemes.pop();
    }

    graphemes
        .into_iter()
        .map(|cluster| cluster.text)
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::{render_document, trim_overflow_boundary_spaces};
    use crate::frontend::tui::composer::Composer;
    use crate::frontend::tui::composer::grapheme::is_space_cluster;
    use crate::frontend::tui::theme::default_palette;

    #[test]
    fn trim_overflow_boundary_spaces_drops_trailing_spaces_only_when_needed() {
        assert_eq!(trim_overflow_boundary_spaces("ab  ", 2), "ab");
        assert_eq!(trim_overflow_boundary_spaces("ab ", 3), "ab ");
        assert_eq!(trim_overflow_boundary_spaces("ab", 2), "ab");
    }

    #[test]
    fn is_space_cluster_recognizes_whitespace_only_clusters() {
        assert!(is_space_cluster(" "));
        assert!(is_space_cluster("\t"));
        assert!(!is_space_cluster(" x"));
        assert!(!is_space_cluster(""));
    }

    #[test]
    fn render_document_preserves_invariants_across_seed_cases() {
        let cases = [
            ("hello world", 12, 0, 5),
            ("", 8, 0, 0),
            ("中文和 emoji 👨‍👩‍👧", 6, 0, 4),
        ];

        for (value, width, cursor_line, cursor_column) in cases {
            assert_render_document_invariants(value, width, cursor_line, cursor_column);
        }
    }

    #[test]
    fn render_document_preserves_invariants_across_generated_cases() {
        for (value, width, cursor_line, cursor_column) in generated_composer_cases() {
            assert_render_document_invariants(&value, width, cursor_line, cursor_column);
        }
    }

    #[test]
    #[ignore = "performance smoke test"]
    fn render_document_perf_smoke() {
        use std::hint::black_box;

        let draft = [
            "draft heading for transcript and composer benchmark",
            "",
            "soft wrap should stay stable under repeated rendering soft wrap should stay stable under repeated rendering",
            "    indented literal line with spaces",
            "\tindented literal line with tabs",
            "中文内容需要继续参与真实宽度计算。",
            "emoji cluster 👨‍👩‍👧 should keep cursor mapping correct",
            "line eight keeps the input tall enough to exercise viewport math",
            "line nine keeps the document renderer allocating multiple visual rows",
            "line ten keeps the cursor near the bottom of the draft",
            "benchmark final line with emoji 👨‍👩‍👧 and trailing text",
        ]
        .join("\n");

        let composer = composer_with_cursor(&draft, 64, 10, 48);
        for _ in 0..256 {
            black_box(render_document(&composer, default_palette()));
        }
    }

    fn assert_render_document_invariants(
        value: &str,
        content_width: usize,
        cursor_line: usize,
        cursor_column: usize,
    ) {
        let composer = composer_with_cursor(value, content_width, cursor_line, cursor_column);
        let result = render_document(&composer, default_palette());

        assert!(
            !result.lines.is_empty(),
            "rendered lines should not be empty"
        );
        assert_eq!(
            result.lines.len(),
            result.anchors.len(),
            "line count {} did not match anchors {}",
            result.lines.len(),
            result.anchors.len()
        );
        assert_eq!(result.lines.len(), result.plain_lines.len());
        assert!(
            result.cursor_y < result.lines.len(),
            "cursor_y {} was out of range for {} lines",
            result.cursor_y,
            result.lines.len()
        );
    }

    fn composer_with_cursor(
        value: &str,
        content_width: usize,
        cursor_line: usize,
        cursor_column: usize,
    ) -> Composer {
        let mut composer = Composer::default();
        composer.set_width((content_width + 2) as u16);
        composer.set_height(8);
        composer.set_text_for_test(value);

        let lines = super::super::logical_lines(composer.value());
        composer.cursor =
            super::super::absolute_cursor_for_position(&lines, cursor_line, cursor_column);
        composer.sync_viewport_to_cursor();
        composer
    }

    fn generated_composer_cases() -> Vec<(String, usize, usize, usize)> {
        let segments = ["a", "b", " ", "  ", "\t", "\n", "中", "文", "👨‍👩‍👧", "emoji"];
        let mut seed = 0xC0DE_u64;
        let mut cases = Vec::new();

        for _ in 0..48 {
            let len = next_u32(&mut seed) as usize % 24;
            let mut value = String::new();
            for _ in 0..len {
                let index = next_u32(&mut seed) as usize % segments.len();
                value.push_str(segments[index]);
            }

            let width = (next_u32(&mut seed) as usize % 32) + 1;
            let lines = value.split('\n').collect::<Vec<_>>();
            let cursor_line = if lines.is_empty() {
                0
            } else {
                next_u32(&mut seed) as usize % lines.len()
            };
            let cursor_column = lines
                .get(cursor_line)
                .map(|line| next_u32(&mut seed) as usize % (line.chars().count() + 1))
                .unwrap_or(0);

            cases.push((value, width, cursor_line, cursor_column));
        }

        cases
    }

    fn next_u32(seed: &mut u64) -> u32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        (*seed >> 32) as u32
    }
}
