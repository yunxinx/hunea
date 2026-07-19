use std::path::Path;

use super::{render_markdown_lines, render_markdown_metrics, render_reasoning_markdown_lines};
use crate::{
    styled_text::{line_plain_text_len, lines_to_ansi_text, lines_to_plain_text},
    theme::{default_palette, palette_from_background, terminal_default_palette},
    transcript::markdown_highlight::{
        highlight_code_chunks_call_count, reset_highlight_code_chunks_call_count,
    },
};
use ratatui::style::{Color, Modifier};

#[test]
fn render_markdown_uses_codex_style_heading_markers() {
    let lines = render_markdown_lines("# Overview of the API", 80, default_palette(), None);
    assert_eq!(lines_to_plain_text(&lines), "# Overview of the API");
}

#[test]
fn render_markdown_removes_emphasis_markers() {
    let lines = render_markdown_lines("__init__", 20, default_palette(), None);
    assert_eq!(lines_to_plain_text(&lines), "init");
}

#[test]
fn render_markdown_strikethrough_applies_crossed_out_style() {
    let lines = render_markdown_lines("keep ~~drop~~ now", 80, default_palette(), None);
    let strike_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "drop")
        .expect("strikethrough text should render as a separate styled span");

    assert_eq!(lines_to_plain_text(&lines), "keep drop now");
    assert!(
        strike_span
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT),
        "删除线文本应使用 Ratatui 的 CROSSED_OUT 样式: {strike_span:?}"
    );
}

#[test]
fn render_markdown_blockquote_uses_quote_style() {
    let palette = default_palette();
    let lines = render_markdown_lines("> quoted text", 80, palette, None);

    assert_eq!(lines_to_plain_text(&lines), "> quoted text");
    for span in &lines[0].spans {
        if span.content.is_empty() {
            continue;
        }
        assert_eq!(
            span.style.fg,
            Some(palette.quote),
            "引用块前缀和正文都应使用 quote 颜色: {span:?}"
        );
        assert!(
            span.style.add_modifier.contains(Modifier::ITALIC),
            "引用块前缀和正文都应使用 italic 样式: {span:?}"
        );
    }
}

#[test]
fn render_markdown_renders_fenced_code_without_fence_markers() {
    let lines = render_markdown_lines(
        "```go\nif err != nil {\n\treturn err\n}\n```",
        20,
        default_palette(),
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert!(!rendered.contains("```"));
    assert!(rendered.contains("if err != nil {"));
    assert!(rendered.contains("return err"));
}

#[test]
fn render_markdown_preserves_intentional_trailing_blank_line_in_code_block() {
    let lines = render_markdown_lines("```rust\nfn main() {}\n\n```", 80, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "fn main() {}\n");
    assert_eq!(lines.len(), 2);
}

#[test]
fn render_markdown_splits_embedded_text_newlines_into_real_lines() {
    let lines = render_markdown_lines(
        "# 简单文档\n这是一个示例 Markdown 文件。\n## 列表\n- 项目一\n- 项目二\n\n```rust\nfn main() {\n    println!(\"Hello, world!\");\n}\n```\n\n[示例链接](https://example.com)",
        80,
        default_palette(),
        None,
    );

    for line in &lines {
        for span in &line.spans {
            assert!(
                !span.content.contains('\n'),
                "Ratatui Line/Span 不能包含内嵌换行，必须提前拆成独立视觉行: {:?}",
                span.content
            );
        }
    }

    let rendered = lines_to_plain_text(&lines);
    assert!(rendered.contains("fn main() {\n"));
    assert!(rendered.contains("println!(\"Hello, world!\");\n"));
    assert!(!rendered.contains("```rustfn main()"));
    assert!(!rendered.contains("项目一- 项目二"));
}

#[test]
fn render_markdown_preserves_blank_line_between_list_and_following_heading() {
    let lines = render_markdown_lines(
        "- 当前共识：仍待验证。\n\n### 💡 为什么重要？\n1. 算力策略：提示",
        80,
        default_palette(),
        None,
    );

    assert_eq!(
        lines_to_plain_text(&lines),
        "- 当前共识：仍待验证。\n\n### 💡 为什么重要？\n\n1. 算力策略：提示"
    );
}

#[test]
fn render_markdown_preserves_blank_line_between_heading_and_following_list() {
    let lines = render_markdown_lines(
        "### 1. 构词逻辑\n*   **Q**：取自英文单词 **Question**（问题）。",
        80,
        default_palette(),
        None,
    );

    assert_eq!(
        lines_to_plain_text(&lines),
        "### 1. 构词逻辑\n\n- Q：取自英文单词 Question（问题）。"
    );
}

#[test]
fn render_markdown_preserves_link_destinations() {
    let lines = render_markdown_lines(
        "[main.go](<cmd/hunea/main.go>)",
        40,
        default_palette(),
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains("cmd/hunea/main.go"));
}

#[test]
fn render_markdown_local_link_uses_normalized_target_not_label() {
    let cwd = Path::new("/workspace/hunea");
    let target = cwd.join("src/frontend/tui/transcript/markdown_render.rs");
    let markdown = format!("[custom label](<{}:74:3-76:9>)", target.display());

    let lines = render_markdown_lines(&markdown, 120, default_palette(), Some(cwd));

    assert_eq!(
        lines_to_plain_text(&lines),
        "src/frontend/tui/transcript/markdown_render.rs:74:3-76:9"
    );
}

#[test]
fn explicit_working_directory_keeps_render_and_metrics_link_shape_identical() {
    let working_dir = Path::new("/workspace/project");
    let markdown = "[report](</workspace/project/reports/current.md>)";
    let palette = default_palette();

    let lines = render_markdown_lines(markdown, 80, palette, Some(working_dir));
    let metrics = render_markdown_metrics(markdown, 80, palette, Some(working_dir));

    assert_eq!(lines_to_plain_text(&lines), "reports/current.md");
    assert_eq!(
        metrics,
        (lines.len(), lines.iter().map(line_plain_text_len).sum())
    );
}

#[test]
fn render_markdown_file_url_hash_location_is_normalized() {
    let cwd = Path::new("/workspace/hunea");
    let target = cwd.join("src/frontend/tui/transcript/markdown_render.rs");
    let markdown = format!("[ignored](file://{}#L74C3-L76C9)", target.display());

    let lines = render_markdown_lines(&markdown, 120, default_palette(), Some(cwd));

    assert_eq!(
        lines_to_plain_text(&lines),
        "src/frontend/tui/transcript/markdown_render.rs:74:3-76:9"
    );
}

#[test]
fn render_markdown_decodes_percent_encoded_local_link_target() {
    let cwd = Path::new("/workspace/hunea");
    let markdown = format!(
        "[report](<{}/Example%20Folder/R%C3%A9sum%C3%A9/report.md>)",
        cwd.display()
    );

    let lines = render_markdown_lines(&markdown, 120, default_palette(), Some(cwd));

    assert_eq!(
        lines_to_plain_text(&lines),
        "Example Folder/Résumé/report.md"
    );
}

#[test]
fn render_markdown_local_file_link_soft_break_before_colon_stays_inline() {
    let cwd = Path::new("/workspace/hunea");
    let target = cwd.join("README.md");
    let markdown = format!(
        "- [binary](<{}:93>)\n  : core owns the runtime behavior.",
        target.display()
    );

    let lines = render_markdown_lines(&markdown, 120, default_palette(), Some(cwd));

    assert_eq!(
        lines_to_plain_text(&lines),
        "- README.md:93: core owns the runtime behavior."
    );
}

#[test]
fn render_markdown_web_link_keeps_label_and_destination() {
    let lines = render_markdown_lines(
        "[Example](https://example.com)",
        80,
        default_palette(),
        None,
    );

    assert_eq!(lines_to_plain_text(&lines), "Example (https://example.com)");
}

#[test]
fn render_reasoning_image_alt_text_does_not_close_outer_link() {
    let lines = render_reasoning_markdown_lines(
        "[![diagram](image.png) docs](https://example.com)",
        80,
        default_palette(),
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert_eq!(rendered, "diagram docs (https://example.com)");
    assert!(
        !rendered.contains("image.png"),
        "Reasoning Content 应忽略 image target，只保留 alt text: {rendered:?}"
    );

    let docs_span = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.contains("docs"))
        .expect("外层 link 中 image 之后的文本应继续渲染");
    assert!(
        docs_span.style.add_modifier.contains(Modifier::UNDERLINED),
        "End(Image) 不应提前关闭外层 link 样式: {lines:?}"
    );
}

#[test]
fn render_markdown_renders_inline_html_as_literal_text() {
    let lines = render_markdown_lines("Press <kbd>Ctrl</kbd> now", 80, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "Press <kbd>Ctrl</kbd> now");
}

#[test]
fn render_markdown_renders_block_html_lines_as_literal_text() {
    let lines = render_markdown_lines(
        "<details>\n<summary>More</summary>\n</details>\n\nAfter",
        80,
        default_palette(),
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains("<details>"));
    assert!(rendered.contains("<summary>More</summary>"));
    assert!(rendered.contains("</details>"));
    assert!(rendered.contains("After"));
}

#[test]
fn render_markdown_highlights_known_fenced_code_language() {
    let lines = render_markdown_lines(
        "```rust\nfn main() { let value = 42; }\n```",
        120,
        default_palette(),
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert_eq!(rendered, "fn main() { let value = 42; }");
    assert!(
        lines[0].spans.len() > 1,
        "known languages should produce syntax-level spans, got {:?}",
        lines[0].spans
    );
    let mut styles = lines[0]
        .spans
        .iter()
        .map(|span| span.style)
        .collect::<Vec<_>>();
    styles.sort_by_key(|style| format!("{style:?}"));
    styles.dedup();
    assert!(
        styles.len() > 1,
        "syntax highlighting should use more than one style, got {:?}",
        lines[0].spans
    );
}

#[test]
fn render_markdown_highlights_two_face_extra_language() {
    let lines = render_markdown_lines(
        "```typescript\nconst answer: number = 42;\n```",
        120,
        default_palette(),
        None,
    );

    assert_eq!(lines_to_plain_text(&lines), "const answer: number = 42;");
    assert!(
        lines[0].spans.len() > 1,
        "two_face 扩展语法集应识别 TypeScript 并产生语法级 span: {:?}",
        lines[0].spans
    );
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg.is_none()),
        "已识别语言的 two_face 高亮代码块不应叠加背景色: {lines:?}"
    );
}

#[test]
fn render_markdown_highlighted_fenced_code_does_not_use_block_background() {
    let lines = render_markdown_lines("```rust\nfn main() {}\n```", 80, default_palette(), None);

    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg.is_none()),
        "已识别语言的语法高亮代码块不应再叠加背景色: {lines:?}"
    );
}

#[test]
fn render_markdown_unknown_fenced_code_language_stays_plain_text() {
    let palette = default_palette();
    let lines = render_markdown_lines("```not-a-real-language\nhello\n```", 80, palette, None);

    assert_eq!(lines_to_plain_text(&lines), "hello");
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0].spans[0].style.bg, None,
        "未识别语言的代码块不应叠加背景色，应与纯 ``` 代码块保持一致"
    );
}

#[test]
fn render_markdown_text_fenced_code_matches_plain_fence_without_styling() {
    let palette = default_palette();
    let text_lines = render_markdown_lines("```text\nhello world\n```", 80, palette, None);
    let plain_lines = render_markdown_lines("```\nhello world\n```", 80, palette, None);

    assert_eq!(lines_to_plain_text(&text_lines), "hello world");
    assert!(
        text_lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg.is_none()),
        "`text` 代码块不应叠加背景色: {text_lines:?}"
    );
    assert_eq!(
        text_lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.style)
            .collect::<Vec<_>>(),
        plain_lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.style)
            .collect::<Vec<_>>(),
        "`text` 代码块应与纯 ``` 代码块渲染样式完全一致"
    );
}

#[test]
fn render_markdown_inline_code_uses_command_accent_foreground() {
    let palette = default_palette();
    let lines = render_markdown_lines("use `cargo test` first", 80, palette, None);
    let code_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "cargo test")
        .expect("inline code span should render separately");

    assert_eq!(
        code_span.style.fg,
        Some(palette.command_accent),
        "行内代码应改为使用 command_accent 前景色"
    );
    assert_eq!(code_span.style.bg, None, "行内代码不应再使用背景色");
}

#[test]
fn render_markdown_inline_math_uses_code_background() {
    let palette = default_palette();
    let lines = render_markdown_lines("energy $E = mc^2$ now", 80, palette, None);
    let math_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "E = mc^2")
        .expect("inline math should render as a separate styled span");

    assert_eq!(lines_to_plain_text(&lines), "energy E = mc^2 now");
    assert_eq!(
        math_span.style.bg, palette.surface,
        "行内 math 应使用 code_style 的 surface 背景"
    );
}

#[test]
fn render_markdown_display_math_uses_literal_code_background() {
    let palette = default_palette();
    let lines = render_markdown_lines("$$\nE = mc^2\n$$", 80, palette, None);

    assert_eq!(lines_to_plain_text(&lines), "E = mc^2");
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg == palette.surface),
        "块级 math 应使用 code_style 的 surface 背景: {lines:?}"
    );
}

#[test]
fn render_markdown_does_not_enable_footnote_definitions() {
    let lines = render_markdown_lines("[^n]: note", 80, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "");
}

#[test]
fn render_markdown_keeps_heading_attributes_literal() {
    let lines = render_markdown_lines("# Title {#custom-id .lead}", 80, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "# Title {#custom-id .lead}");
}

#[test]
fn render_markdown_unwraps_markdown_fence_containing_table() {
    let markdown = "```markdown\n| A | B |\n|---|---|\n| 1 | 2 |\n```\n";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(rendered.contains('━'));
    assert!(
        rendered
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["1", "2"]),
        "markdown table fence should render as a native table: {rendered}"
    );
    assert!(
        !rendered.contains("```"),
        "unwrapped table output should not contain fence markers: {rendered}"
    );
}

#[test]
fn render_markdown_unwraps_markdown_fence_containing_table_without_outer_pipes() {
    let markdown = "```md\nCol A | Col B | Col C\n--- | --- | ---\nx | y | z\n```\n";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(rendered.contains('━'));
    assert!(
        rendered.contains("Col A") && rendered.contains("Col B") && rendered.contains("Col C"),
        "markdown table fence without outer pipes should render as a native table: {rendered}"
    );
    assert!(
        !rendered
            .lines()
            .any(|line| line.trim() == "Col A | Col B | Col C"),
        "unwrapped no-outer-pipe table should not remain literal code: {rendered}"
    );
}

#[test]
fn render_markdown_unwraps_blockquoted_markdown_fence_containing_table() {
    let markdown = "> ```md\n> | A | B |\n> |---|---|\n> | 1 | 2 |\n> ```\n";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(rendered.lines().all(|line| line.starts_with("> ")));
    assert!(
        rendered.contains('━'),
        "blockquoted markdown table fence should render as a native table: {rendered}"
    );
}

#[test]
fn render_markdown_keeps_markdown_fence_without_table_as_code() {
    let lines = render_markdown_lines("```markdown\n**bold**\n```", 80, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "**bold**");
    assert!(
        lines[0]
            .spans
            .iter()
            .any(|span| span.content.contains("**")),
        "markdown fences without table structure should remain literal code blocks"
    );
}

#[test]
fn render_markdown_keeps_non_markdown_fence_containing_table_as_code() {
    let markdown = "```rust\n| A | B |\n|---|---|\n| 1 | 2 |\n```";
    let lines = render_markdown_lines(markdown, 80, default_palette(), None);
    let rendered = lines_to_plain_text(&lines);

    assert_eq!(rendered, "| A | B |\n|---|---|\n| 1 | 2 |");
    assert!(
        !rendered.contains('━'),
        "non-markdown fences should not be unwrapped as tables: {rendered}"
    );
}

#[test]
fn render_markdown_renders_tables_with_codex_row_separators() {
    let markdown = "| 名称 | 类型 | 版本 | 启用 |\n| --- | --- | ---: | :---: |\n| hunea | 应用 | 0.1.0 | 是 |\n| ratatui | 依赖 | 0.24 | 否 |";

    let lines = render_markdown_lines(markdown, 80, default_palette(), None);

    assert_eq!(
        lines_to_plain_text(&lines),
        [
            " 名称       类型     版本    启用",
            "━━━━━━━━━  ━━━━━━  ━━━━━━━  ━━━━━━",
            " hunea      应用    0.1.0     是",
            "─────────  ──────  ───────  ──────",
            " ratatui    依赖     0.24     否",
        ]
        .join("\n")
    );
}

#[test]
fn render_markdown_table_header_uses_table_header_accent() {
    let palette = default_palette();
    let lines = render_markdown_lines(
        "| Name | Status |\n| --- | --- |\n| hunea | ready |",
        80,
        palette,
        None,
    );

    let header_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("Name"))
        .expect("header row should contain the table header text");

    assert_eq!(header_span.style.fg, Some(palette.table_header));
    assert!(header_span.style.add_modifier.contains(Modifier::BOLD));
    assert!(
        lines_to_plain_text(&lines)
            .lines()
            .nth(1)
            .is_some_and(|line| line.contains('━')),
        "header separator should keep the codex-style heavy rule glyph"
    );
    assert!(
        lines[1]
            .spans
            .iter()
            .all(|span| span.style.fg != Some(palette.table_header)),
        "separator style should not reuse the table header accent"
    );
}

#[test]
fn render_markdown_wraps_table_cells_without_box_borders_or_ellipsis() {
    let markdown =
        "| 名称 | 说明 |\n| --- | --- |\n| hunea | 一个基于 Rust 和 Ratatui 的 TUI 客户端 |";

    let lines = render_markdown_lines(markdown, 24, default_palette(), None);
    let rendered = lines_to_plain_text(&lines);

    assert!(
        rendered.contains('━'),
        "表头下方应使用 codex-style 分隔线: {rendered}"
    );
    for box_border in ['┌', '┬', '┐', '│', '├', '┼', '┤', '└', '┴', '┘'] {
        assert!(
            !rendered.contains(box_border),
            "表格不应继续使用 box border {box_border}: {rendered}"
        );
    }
    assert!(
        rendered.contains("Ratatui"),
        "窄窗口表格必须换行保留内容，而不是省略: {rendered}"
    );
    for token in [
        "一个",
        "基于",
        "Rust",
        "和",
        "Ratatui",
        "的",
        "TUI",
        "客户端",
    ] {
        assert!(
            rendered.contains(token),
            "窄窗口表格必须完整保留 cell 内容，缺少 {token}: {rendered}"
        );
    }
    assert!(
        !rendered.contains('…'),
        "窄窗口表格不应使用省略号截断内容: {rendered}"
    );
    assert!(
        lines.len() > 3,
        "长 cell 应该增加表格行高以完整显示内容: {rendered}"
    );
}

#[test]
fn render_markdown_table_alignment_respects_markers() {
    let markdown = "| Left | Center | Right |\n|:-----|:------:|------:|\n| a | b | c |";
    let lines = render_markdown_lines(markdown, 80, default_palette(), None);
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains(" Left"));
    assert!(rendered.contains("Center"));
    assert!(rendered.contains("Right"));
    assert!(rendered.contains(" a"));
    assert!(rendered.contains(" b"));
    assert!(rendered.contains(" c"));
}

#[test]
fn render_markdown_table_falls_back_to_key_value_records_when_grid_cannot_fit() {
    let markdown = "| c1 | c2 | c3 | c4 | c5 | c6 | c7 | c8 | c9 | c10 |\n|---|---|---|---|---|---|---|---|---|---|\n| 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |";
    let lines = render_markdown_lines(markdown, 20, default_palette(), None);
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains("c1"));
    assert!(rendered.contains("10"));
    assert!(!rendered.contains('━'));
    assert!(!rendered.contains('│'));
}

#[test]
fn render_markdown_table_preserves_inline_styles_inside_cells() {
    let palette = default_palette();
    let lines = render_markdown_lines(
        "| Key | Content |\n| --- | --- |\n| item | [link](https://example.com) **bold** `code` |",
        80,
        palette,
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains("link (https://example.com)"));
    assert!(rendered.contains("bold"));
    assert!(rendered.contains("code"));
    assert!(
        lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
            (span.content.as_ref() == "link" || span.content.contains("link"))
                && span.style.add_modifier.contains(Modifier::UNDERLINED)
        }),
        "table cells should preserve markdown link styling: {lines:?}"
    );
    assert!(
        lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
            span.content.as_ref() == "bold" && span.style.add_modifier.contains(Modifier::BOLD)
        }),
        "table cells should preserve strong styling: {lines:?}"
    );
    assert!(
        lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
            span.content.as_ref() == "code"
                && span.style.fg == Some(palette.command_accent)
                && span.style.bg.is_none()
        }),
        "table cells should preserve inline code styling: {lines:?}"
    );
    let destination_span = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.contains("https://example.com"))
        .expect("table link destination should render as a separate styled span");
    assert_eq!(destination_span.style.fg, Some(palette.secondary));
    assert!(
        destination_span
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED),
        "table link destination should preserve link styling: {lines:?}"
    );
}

#[test]
fn render_markdown_table_header_style_is_base_for_inline_cell_styles() {
    let palette = default_palette();
    let lines = render_markdown_lines(
        "| Plain | [Docs](https://example.com) | `Code` |\n| --- | --- | --- |\n| value | value | value |",
        120,
        palette,
        None,
    );

    let plain_header = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("Plain"))
        .expect("plain table header should render");
    assert_eq!(plain_header.style.fg, Some(palette.table_header));
    assert!(plain_header.style.add_modifier.contains(Modifier::BOLD));

    let destination_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("https://example.com"))
        .expect("header link destination should render");
    assert_eq!(destination_span.style.fg, Some(palette.secondary));
    assert!(
        destination_span
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED),
        "header link destination should preserve link styling: {lines:?}"
    );

    let code_header = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "Code")
        .expect("header inline code should render");
    assert_eq!(code_header.style.fg, Some(palette.command_accent));
    assert_eq!(code_header.style.bg, None);
}

#[test]
fn render_markdown_table_preserves_escaped_pipe_inside_cell() {
    let lines = render_markdown_lines(
        "| Text |\n| --- |\n| a \\| b |",
        80,
        default_palette(),
        None,
    );
    let rendered = lines_to_plain_text(&lines);

    assert!(
        rendered.lines().any(|line| line.contains("a | b")),
        "escaped pipes should stay inside the cell text instead of splitting columns: {rendered}"
    );
    assert!(
        !rendered.contains("a \\| b"),
        "escaped table pipes should render as literal pipes: {rendered}"
    );
}

#[test]
fn render_markdown_table_keeps_spillover_text_outside_table() {
    let markdown = "| A | B |\n| --- | --- |\n| 1 | 2 |\ntrailing paragraph";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(rendered.contains(" 1"));
    assert!(rendered.ends_with("trailing paragraph"));
}

#[test]
fn render_markdown_table_keeps_prefixed_html_spillover_outside_table() {
    let markdown = "| A | B |\n| --- | --- |\n| 1 | 2 |\n| HTML follows <div>content</div> |";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(
        rendered
            .lines()
            .any(|line| line == "HTML follows <div>content</div>"),
        "HTML-like spillover with prose before the tag should be rendered as prose: {rendered}"
    );
    assert!(
        rendered
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == ["1", "2"]),
        "the real table row should still render as a table row: {rendered}"
    );
}

#[test]
fn render_markdown_table_does_not_treat_html_substring_label_as_spillover() {
    let markdown = "| Key | Value |\n| --- | --- |\n| nothtml: | |";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(
        rendered.lines().any(|line| line.starts_with(" nothtml:")),
        "labels only containing html as a substring should remain in the table: {rendered}"
    );
}

#[test]
fn render_markdown_table_wraps_path_heavy_narrow_rows_without_truncation() {
    let path = "/home/archie/GoCodes/lumos_rust/crates/terminal-ui/src/transcript/markdown_render/table.rs";
    let markdown = format!("| File | Path |\n| --- | --- |\n| renderer | {path} |");
    let rendered = lines_to_plain_text(&render_markdown_lines(
        &markdown,
        28,
        default_palette(),
        None,
    ));
    let compact_rendered = rendered
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<String>();
    let compact_path = path
        .chars()
        .filter(|char| !char.is_whitespace())
        .collect::<String>();

    assert!(rendered.contains("File"));
    assert!(rendered.contains("renderer"));
    assert!(
        compact_rendered.contains(&compact_path),
        "path-heavy wrapping should preserve the complete path across visual lines: {rendered}"
    );
    assert!(
        !rendered.contains('…'),
        "path-heavy wrapping should not truncate with ellipsis: {rendered}"
    );
}

#[test]
fn render_markdown_table_falls_back_for_compact_systemic_fragmentation() {
    let markdown = "| Build | Test | Lint | Format |\n| --- | --- | --- | --- |\n| build-pipeline-20260529 | nextest-suite-20260529 | clippy-workspace-20260529 | rustfmt-check-20260529 |";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        34,
        default_palette(),
        None,
    ));

    for token in [
        "build-pipeline-20260529",
        "nextest-suite-20260529",
        "clippy-workspace-20260529",
        "rustfmt-check-20260529",
    ] {
        assert!(
            rendered.contains(token),
            "compact fragmented fallback should preserve {token}: {rendered}"
        );
    }
    assert!(
        !rendered.contains('━'),
        "systemic fragmentation should use record fallback rather than an unreadable grid: {rendered}"
    );
}

#[test]
fn render_markdown_table_inside_blockquote_keeps_quote_prefix() {
    let markdown = "> | A | B |\n> |---|---|\n> | 1 | 2 |";
    let rendered = lines_to_plain_text(&render_markdown_lines(
        markdown,
        80,
        default_palette(),
        None,
    ));

    assert!(rendered.lines().all(|line| line.starts_with("> ")));
    assert!(rendered.contains("━━━━━"));
}

#[test]
fn render_markdown_keeps_non_table_pipe_text_plain() {
    let markdown = "苹果 | 10 | 有货\n香蕉 | 5 | 缺货";
    let lines = render_markdown_lines(markdown, 80, default_palette(), None);

    assert_eq!(
        lines_to_plain_text(&lines),
        "苹果 | 10 | 有货\n香蕉 | 5 | 缺货"
    );
}

#[test]
fn render_markdown_renders_task_list_markers() {
    let lines = render_markdown_lines("- [x] done\n- [ ] todo", 40, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "- [x] done\n- [ ] todo");
}

#[test]
fn render_markdown_nested_lists_use_codex_style_indent() {
    let lines = render_markdown_lines(
        "- outer\n  - inner\n    1. ordered",
        80,
        default_palette(),
        None,
    );

    assert_eq!(
        lines_to_plain_text(&lines),
        "- outer\n    - inner\n        1. ordered"
    );
}

#[test]
fn render_markdown_keeps_terminal_default_plain_text_unstyled() {
    let lines = render_markdown_lines("plain text", 20, terminal_default_palette(), None);
    let rendered = lines_to_ansi_text(&lines);

    assert_eq!(rendered, "plain text");
}

#[test]
fn fenced_code_uses_a_syntax_theme_matching_the_terminal_background() {
    let markdown = "```rust\npub fn main() { let answer = true; }\n```";
    let dark_lines = render_markdown_lines(
        markdown,
        80,
        palette_from_background(true, Some(Color::Rgb(18, 24, 32))),
        None,
    );
    let light_lines = render_markdown_lines(
        markdown,
        80,
        palette_from_background(false, Some(Color::Rgb(242, 242, 242))),
        None,
    );
    let dark_foregrounds = syntax_rgb_foregrounds(&dark_lines);
    let light_foregrounds = syntax_rgb_foregrounds(&light_lines);

    assert!(!dark_foregrounds.is_empty());
    assert!(!light_foregrounds.is_empty());
    assert_ne!(dark_foregrounds, light_foregrounds);
}

#[test]
fn terminal_default_fenced_code_does_not_emit_syntect_rgb_foregrounds() {
    let lines = render_markdown_lines(
        "```rust\npub fn main() { let answer = true; }\n```",
        80,
        terminal_default_palette(),
        None,
    );

    assert!(syntax_rgb_foregrounds(&lines).is_empty());
}

fn syntax_rgb_foregrounds(lines: &[ratatui::text::Line<'_>]) -> Vec<Color> {
    lines
        .iter()
        .flat_map(|line| &line.spans)
        .filter_map(|span| span.style.fg)
        .filter(|color| matches!(color, Color::Rgb(_, _, _)))
        .collect()
}

#[test]
fn render_markdown_preserves_explicit_edge_blank_lines() {
    let lines = render_markdown_lines("\nhello\n", 20, default_palette(), None);
    assert_eq!(lines_to_plain_text(&lines), "\nhello\n");
}

#[test]
fn render_markdown_lines_removes_terminal_control_sequences_before_parsing() {
    let lines = render_markdown_lines("a\u{1b}[31mb\u{1b}[0m", 20, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "ab");
}

#[test]
fn render_markdown_does_not_insert_blank_row_before_wide_glyph() {
    let lines = render_markdown_lines("中", 1, default_palette(), None);
    assert_eq!(lines_to_plain_text(&lines), "中");
    assert_eq!(lines.len(), 1);
}

#[test]
fn render_markdown_uses_cjk_breakpoints_in_mixed_prose() {
    let content = "你好，请你随意阅读一下当前目录下的目录和文件情况，不过最多读 10 个文件即可。我只是在测试我的工具，而不是关心你的分析结果内容";
    let lines = render_markdown_lines(content, 102, default_palette(), None);

    assert_eq!(
        lines_to_plain_text(&lines),
        "你好，请你随意阅读一下当前目录下的目录和文件情况，不过最多读 10 个文件即可。我只是在测试我的工具，而不\n是关心你的分析结果内容"
    );
}

#[test]
fn render_markdown_never_starts_wrapped_line_with_closing_punctuation() {
    let lines = render_markdown_lines("你，好", 4, default_palette(), None);

    assert_eq!(lines_to_plain_text(&lines), "你，\n好");
}

#[test]
fn render_markdown_keeps_cross_style_family_grapheme_on_one_line() {
    let lines = render_markdown_lines("👨‍`👩‍`👧", 2, default_palette(), None);

    assert_eq!(lines.len(), 1);
    assert_eq!(lines_to_plain_text(&lines), "👨‍👩‍👧");
}

#[test]
fn render_markdown_metrics_skip_code_highlighting_but_match_rendered_plain_text_shape() {
    let markdown = r#"# Title

```rust
fn main() {
    println!("hello");
}
```

- after code
"#;
    let width = 32;
    let palette = default_palette();

    let rendered = render_markdown_lines(markdown, width, palette, None);
    let rendered_line_count = rendered.len();
    let rendered_plain_text_len = rendered.iter().map(line_plain_text_len).sum::<usize>();

    reset_highlight_code_chunks_call_count();
    let metrics = render_markdown_metrics(markdown, width, palette, None);

    assert_eq!(metrics, (rendered_line_count, rendered_plain_text_len));
    assert_eq!(
        highlight_code_chunks_call_count(),
        0,
        "metrics-only Markdown measurement should not pay syntax highlighting cost"
    );

    reset_highlight_code_chunks_call_count();
    let _ = render_markdown_lines(markdown, width, palette, None);
    assert!(
        highlight_code_chunks_call_count() > 0,
        "visible rendering should still apply syntax highlighting"
    );
}

#[test]
#[ignore = "performance smoke test"]
fn render_markdown_perf_smoke() {
    use std::hint::black_box;

    let markdown = (0..6)
            .map(|index| {
                format!(
                    "## Section {index}\n\n- summarize the latest transcript cache behavior\n- explain why viewport anchors stay stable across resize\n- keep the markdown renderer width-aware\n\n```rust\nfn section_{index}() -> Result<(), &'static str> {{\n    Ok(())\n}}\n```\n"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

    for _ in 0..128 {
        black_box(render_markdown_lines(
            &markdown,
            72,
            default_palette(),
            None,
        ));
    }
}
