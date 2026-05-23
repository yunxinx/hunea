use super::{render_markdown_lines, render_markdown_metrics};
use crate::{
    styled_text::{line_plain_text_len, lines_to_ansi_text, lines_to_plain_text},
    theme::{default_palette, terminal_default_palette},
    transcript::markdown_highlight::{
        highlight_code_chunks_call_count, reset_highlight_code_chunks_call_count,
    },
};
use ratatui::style::Modifier;

#[test]
fn render_markdown_uses_codex_style_heading_markers() {
    let lines = render_markdown_lines("# Overview of the API", 80, default_palette());
    assert_eq!(lines_to_plain_text(&lines), "# Overview of the API");
}

#[test]
fn render_markdown_removes_emphasis_markers() {
    let lines = render_markdown_lines("__init__", 20, default_palette());
    assert_eq!(lines_to_plain_text(&lines), "init");
}

#[test]
fn render_markdown_strikethrough_applies_crossed_out_style() {
    let lines = render_markdown_lines("keep ~~drop~~ now", 80, default_palette());
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
    let lines = render_markdown_lines("> quoted text", 80, palette);

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
    );
    let rendered = lines_to_plain_text(&lines);

    assert!(!rendered.contains("```"));
    assert!(rendered.contains("if err != nil {"));
    assert!(rendered.contains("return err"));
}

#[test]
fn render_markdown_preserves_intentional_trailing_blank_line_in_code_block() {
    let lines = render_markdown_lines("```rust\nfn main() {}\n\n```", 80, default_palette());

    assert_eq!(lines_to_plain_text(&lines), "fn main() {}\n");
    assert_eq!(lines.len(), 2);
}

#[test]
fn render_markdown_splits_embedded_text_newlines_into_real_lines() {
    let lines = render_markdown_lines(
        "# 简单文档\n这是一个示例 Markdown 文件。\n## 列表\n- 项目一\n- 项目二\n\n```rust\nfn main() {\n    println!(\"Hello, world!\");\n}\n```\n\n[示例链接](https://example.com)",
        80,
        default_palette(),
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
fn render_markdown_preserves_link_destinations() {
    let lines = render_markdown_lines("[main.go](<cmd/lumos/main.go>)", 40, default_palette());
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains("cmd/lumos/main.go"));
}

#[test]
fn render_markdown_local_link_uses_normalized_target_not_label() {
    let cwd = std::env::current_dir().expect("test should run inside the workspace");
    let target = cwd.join("src/frontend/tui/transcript/markdown_render.rs");
    let markdown = format!("[custom label](<{}:74:3-76:9>)", target.display());

    let lines = render_markdown_lines(&markdown, 120, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "src/frontend/tui/transcript/markdown_render.rs:74:3-76:9"
    );
}

#[test]
fn render_markdown_file_url_hash_location_is_normalized() {
    let cwd = std::env::current_dir().expect("test should run inside the workspace");
    let target = cwd.join("src/frontend/tui/transcript/markdown_render.rs");
    let markdown = format!("[ignored](file://{}#L74C3-L76C9)", target.display());

    let lines = render_markdown_lines(&markdown, 120, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "src/frontend/tui/transcript/markdown_render.rs:74:3-76:9"
    );
}

#[test]
fn render_markdown_decodes_percent_encoded_local_link_target() {
    let cwd = std::env::current_dir().expect("test should run inside the workspace");
    let markdown = format!(
        "[report](<{}/Example%20Folder/R%C3%A9sum%C3%A9/report.md>)",
        cwd.display()
    );

    let lines = render_markdown_lines(&markdown, 120, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "Example Folder/Résumé/report.md"
    );
}

#[test]
fn render_markdown_local_file_link_soft_break_before_colon_stays_inline() {
    let cwd = std::env::current_dir().expect("test should run inside the workspace");
    let target = cwd.join("README.md");
    let markdown = format!(
        "- [binary](<{}:93>)\n  : core owns the runtime behavior.",
        target.display()
    );

    let lines = render_markdown_lines(&markdown, 120, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "- README.md:93: core owns the runtime behavior."
    );
}

#[test]
fn render_markdown_web_link_keeps_label_and_destination() {
    let lines = render_markdown_lines("[Example](https://example.com)", 80, default_palette());

    assert_eq!(lines_to_plain_text(&lines), "Example (https://example.com)");
}

#[test]
fn render_markdown_renders_inline_html_as_literal_text() {
    let lines = render_markdown_lines("Press <kbd>Ctrl</kbd> now", 80, default_palette());

    assert_eq!(lines_to_plain_text(&lines), "Press <kbd>Ctrl</kbd> now");
}

#[test]
fn render_markdown_renders_block_html_lines_as_literal_text() {
    let lines = render_markdown_lines(
        "<details>\n<summary>More</summary>\n</details>\n\nAfter",
        80,
        default_palette(),
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
    let lines = render_markdown_lines("```rust\nfn main() {}\n```", 80, default_palette());

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
    let lines = render_markdown_lines("```not-a-real-language\nhello\n```", 80, palette);

    assert_eq!(lines_to_plain_text(&lines), "hello");
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0].spans[0].style.bg, palette.surface,
        "未识别语言的代码块仍应保留背景色，帮助和普通正文区分"
    );
}

#[test]
fn render_markdown_inline_code_keeps_code_background() {
    let palette = default_palette();
    let lines = render_markdown_lines("use `cargo test` first", 80, palette);
    let code_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "cargo test")
        .expect("inline code span should render separately");

    assert_eq!(
        code_span.style.bg, palette.surface,
        "行内代码背景色不属于本次代码块背景调整范围"
    );
}

#[test]
fn render_markdown_inline_math_uses_code_background() {
    let palette = default_palette();
    let lines = render_markdown_lines("energy $E = mc^2$ now", 80, palette);
    let math_span = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "E = mc^2")
        .expect("inline math should render as a separate styled span");

    assert_eq!(lines_to_plain_text(&lines), "energy E = mc^2 now");
    assert_eq!(
        math_span.style.bg, palette.surface,
        "行内 math 应使用未识别语言代码块同款背景色"
    );
}

#[test]
fn render_markdown_display_math_uses_literal_code_background() {
    let palette = default_palette();
    let lines = render_markdown_lines("$$\nE = mc^2\n$$", 80, palette);

    assert_eq!(lines_to_plain_text(&lines), "E = mc^2");
    assert!(
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .all(|span| span.style.bg == palette.surface),
        "块级 math 应使用未识别语言代码块同款背景色: {lines:?}"
    );
}

#[test]
fn render_markdown_does_not_enable_footnote_definitions() {
    let lines = render_markdown_lines("[^n]: note", 80, default_palette());

    assert_eq!(lines_to_plain_text(&lines), "");
}

#[test]
fn render_markdown_keeps_heading_attributes_literal() {
    let lines = render_markdown_lines("# Title {#custom-id .lead}", 80, default_palette());

    assert_eq!(lines_to_plain_text(&lines), "# Title {#custom-id .lead}");
}

#[test]
fn render_markdown_renders_tables_with_connected_box_borders() {
    let markdown = "| 名称 | 类型 | 版本 | 启用 |\n| --- | --- | ---: | :---: |\n| lumos | 应用 | 0.1.0 | 是 |\n| ratatui | 依赖 | 0.24 | 否 |";

    let lines = render_markdown_lines(markdown, 80, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "┌─────────┬──────┬───────┬──────┐\n\
             │ 名称    │ 类型 │  版本 │ 启用 │\n\
             ├─────────┼──────┼───────┼──────┤\n\
             │ lumos   │ 应用 │ 0.1.0 │  是  │\n\
             │ ratatui │ 依赖 │  0.24 │  否  │\n\
             └─────────┴──────┴───────┴──────┘"
    );
}

#[test]
fn render_markdown_wraps_table_cells_in_narrow_width_without_ellipsis() {
    let markdown =
        "| 名称 | 说明 |\n| --- | --- |\n| lumos | 一个基于 Rust 和 Ratatui 的 TUI 客户端 |";

    let lines = render_markdown_lines(markdown, 24, default_palette());
    let rendered = lines_to_plain_text(&lines);

    assert!(rendered.contains("┌"));
    assert!(rendered.contains("┬"));
    assert!(rendered.contains("┼"));
    assert!(rendered.contains("┘"));
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
        lines.len() > 5,
        "长 cell 应该增加表格行高以完整显示内容: {rendered}"
    );
}

#[test]
fn render_markdown_keeps_non_table_pipe_text_plain() {
    let markdown = "苹果 | 10 | 有货\n香蕉 | 5 | 缺货";
    let lines = render_markdown_lines(markdown, 80, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "苹果 | 10 | 有货\n香蕉 | 5 | 缺货"
    );
}

#[test]
fn render_markdown_renders_task_list_markers() {
    let lines = render_markdown_lines("- [x] done\n- [ ] todo", 40, default_palette());

    assert_eq!(lines_to_plain_text(&lines), "- [x] done\n- [ ] todo");
}

#[test]
fn render_markdown_nested_lists_use_codex_style_indent() {
    let lines = render_markdown_lines("- outer\n  - inner\n    1. ordered", 80, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "- outer\n    - inner\n        1. ordered"
    );
}

#[test]
fn render_markdown_keeps_terminal_default_plain_text_unstyled() {
    let lines = render_markdown_lines("plain text", 20, terminal_default_palette());
    let rendered = lines_to_ansi_text(&lines);

    assert_eq!(rendered, "plain text");
}

#[test]
fn render_markdown_preserves_explicit_edge_blank_lines() {
    let lines = render_markdown_lines("\nhello\n", 20, default_palette());
    assert_eq!(lines_to_plain_text(&lines), "\nhello\n");
}

#[test]
fn render_markdown_does_not_insert_blank_row_before_wide_glyph() {
    let lines = render_markdown_lines("中", 1, default_palette());
    assert_eq!(lines_to_plain_text(&lines), "中");
    assert_eq!(lines.len(), 1);
}

#[test]
fn render_markdown_uses_cjk_breakpoints_in_mixed_prose() {
    let content = "你好，请你随意阅读一下当前目录下的目录和文件情况，不过最多读 10 个文件即可。我只是在测试我的工具，而不是关心你的分析结果内容";
    let lines = render_markdown_lines(content, 102, default_palette());

    assert_eq!(
        lines_to_plain_text(&lines),
        "你好，请你随意阅读一下当前目录下的目录和文件情况，不过最多读 10 个文件即可。我只是在测试我的工具，而不\n是关心你的分析结果内容"
    );
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

    let rendered = render_markdown_lines(markdown, width, palette);
    let rendered_line_count = rendered.len();
    let rendered_plain_text_len = rendered.iter().map(line_plain_text_len).sum::<usize>();

    reset_highlight_code_chunks_call_count();
    let metrics = render_markdown_metrics(markdown, width, palette);

    assert_eq!(metrics, (rendered_line_count, rendered_plain_text_len));
    assert_eq!(
        highlight_code_chunks_call_count(),
        0,
        "metrics-only Markdown measurement should not pay syntax highlighting cost"
    );

    reset_highlight_code_chunks_call_count();
    let _ = render_markdown_lines(markdown, width, palette);
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
        black_box(render_markdown_lines(&markdown, 72, default_palette()));
    }
}
