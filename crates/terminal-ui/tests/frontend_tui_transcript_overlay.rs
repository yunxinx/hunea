use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use runtime_domain::model_catalog::ModelSelection;
use terminal_ui::{AppEvent, Model, ModelOptions, ScrollAnimationMode, StartupBannerOptions};

mod common;

use common::single_model_catalog;

fn ready_model(width: u16, height: u16) -> Model {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            // 本测试关注 overlay 与主视图的滚动同步而非滚轮动力学：
            // 关闭平滑滚动让滚轮瞬时位移（drain 推进点在 runner，集成测试无法驱动）。
            scroll_animation: ScrollAnimationMode::Off,
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::StartupReadyTimeout);
    model
}

fn render_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);

    buffer_rows(&buffer)
}

fn buffer_rows(buffer: &Buffer) -> Vec<String> {
    let mut rows = Vec::with_capacity(buffer.area.height as usize);

    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rows.push(rendered);
    }

    rows
}

#[test]
fn transcript_overlay_renders_content_and_footer() {
    let mut model = ready_model(40, 10);

    // 发送一条消息，使 transcript 中有内容
    for character in "Hello world".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // Ctrl+T 打开覆盖层
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    let rows = render_rows(&mut model, 40, 10);

    // 倒数第二行应为百分比分隔线，最后一行为提示
    let rule_row = &rows[8];
    let footer_row = &rows[9];
    assert!(
        rule_row.contains('%'),
        "rule row should contain percentage: {:?}",
        rule_row
    );
    assert!(
        footer_row.contains("scroll")
            || footer_row.contains("close")
            || footer_row.contains("exit"),
        "footer hint should contain navigation tips: {:?}",
        footer_row
    );
}

#[test]
fn transcript_overlay_footer_omits_non_footer_shortcuts() {
    let mut model = ready_model(40, 10);

    for character in "Hello world".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    let rows = render_rows(&mut model, 40, 10);
    let footer_row = &rows[9];
    assert!(
        footer_row.contains("Esc close") && !footer_row.contains("Esc/q"),
        "footer should advertise only Esc as the close hint: {:?}",
        footer_row
    );
    assert!(
        !footer_row.contains("Ctrl+T")
            && !footer_row.contains("Ctrl+O")
            && !footer_row.contains("Ctrl+C"),
        "footer should not advertise hidden/toggle/ctrl-c shortcuts: {:?}",
        footer_row
    );
}

#[test]
fn transcript_overlay_hides_composer_and_panels() {
    let mut model = ready_model(30, 8);

    // 发送一条消息
    for character in "Test".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // 在 composer 中输入草稿
    for character in "draft".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    // 正常模式下应能看到 composer 内容
    let rows_before = render_rows(&mut model, 30, 8);
    assert!(
        rows_before.iter().any(|row| row.contains("draft")),
        "normal mode should show composer text"
    );

    // 打开覆盖层
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    let rows = render_rows(&mut model, 30, 8);

    // 覆盖层模式下，不应有 composer 内容
    for (i, row) in rows.iter().enumerate() {
        assert!(
            !row.contains("draft"),
            "row {i} should not contain composer text in overlay mode: {row:?}"
        );
    }

    // 覆盖层模式下第一行应为 transcript 内容（启动欢迎块或消息）
    assert!(
        !rows.iter().any(|row| row.contains("draft")),
        "overlay should not show composer text"
    );
}

#[test]
fn transcript_overlay_scrolls_and_closes() {
    let mut model = ready_model(20, 10);

    // 发送一条多行消息（通过粘贴带换行的文本）
    model.update(AppEvent::Paste(
        "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12"
            .to_string(),
    ));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // 发送消息后主界面处于 follow_bottom（底部），先将主界面滚动到顶部，
    // 使打开覆盖层时 scroll_offset 同步为 0，确保后续 End/Home 测试有意义
    model.update(AppEvent::MouseWheel { delta_lines: -100 });

    // 打开覆盖层
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    // 初始渲染，记录顶部内容
    let rows_top = render_rows(&mut model, 20, 10);

    // 按 End 跳到底部（内容应改变）
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::End)));
    let rows_bottom = render_rows(&mut model, 20, 10);
    assert_ne!(
        rows_top, rows_bottom,
        "scrolling to end should change visible content"
    );

    // 按 Home 回到顶部（内容应与初始相同）
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Home)));
    let rows_back_top = render_rows(&mut model, 20, 10);
    assert_eq!(
        rows_top, rows_back_top,
        "scrolling back to top should restore initial view"
    );

    // q 不再关闭 overlay，避免和文本输入习惯混淆。
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('q'))));
    let rows_after_q = render_rows(&mut model, 20, 10);
    assert_eq!(
        rows_back_top, rows_after_q,
        "q should be ignored as a close key while overlay is active"
    );

    // Esc 关闭
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    // 关闭后 composer 应重新出现
    for character in "x".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let rows_after = render_rows(&mut model, 20, 10);
    assert!(
        rows_after.iter().any(|row| row.contains("x")),
        "after closing overlay, composer should be visible again"
    );
}

#[test]
fn transcript_overlay_toggles_with_ctrl_t() {
    let mut model = ready_model(20, 10);

    // 发送一条消息
    for character in "hi".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let rows_startup_frame = render_rows(&mut model, 20, 10);

    // Ctrl+T 打开
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    let rows_overlay = render_rows(&mut model, 20, 10);
    assert_ne!(
        rows_startup_frame, rows_overlay,
        "overlay should change the view: {:?}",
        rows_overlay
    );

    // 再次 Ctrl+T 关闭
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    let rows_closed = render_rows(&mut model, 20, 10);
    assert_ne!(
        rows_overlay, rows_closed,
        "closing overlay should leave overlay view"
    );
    assert!(
        rows_closed.iter().any(|row| row.contains("directory:")),
        "closing overlay should restore normal document view: {:?}",
        rows_closed
    );
    assert!(
        rows_closed.iter().any(|row| row.contains("› hi")),
        "closing overlay should restore submitted user message: {:?}",
        rows_closed
    );
}

#[test]
fn transcript_overlay_esc_closes() {
    let mut model = ready_model(20, 10);

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let _rows_startup_frame = render_rows(&mut model, 20, 10);

    // 打开覆盖层
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    // Esc 关闭
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    let rows_after = render_rows(&mut model, 20, 10);
    assert!(
        rows_after.iter().any(|row| row.contains("directory:")),
        "esc should close overlay and restore normal document view: {:?}",
        rows_after
    );
    assert!(
        rows_after.iter().any(|row| row.contains("› hello")),
        "esc should close overlay and restore submitted user message: {:?}",
        rows_after
    );
}

#[test]
fn transcript_overlay_ctrl_c_closes_without_clearing_composer() {
    let mut model = ready_model(30, 8);

    for character in "history".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    for character in "draft".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert!(
        !model.is_quitting(),
        "ctrl-c in overlay should not enter the global exit path"
    );
    let rows_after = render_rows(&mut model, 30, 8);
    assert!(
        rows_after.iter().any(|row| row.contains("draft")),
        "ctrl-c should close overlay and preserve the hidden composer draft: {:?}",
        rows_after
    );
}

#[test]
fn transcript_overlay_paste_does_not_modify_hidden_composer() {
    let mut model = ready_model(30, 8);

    for character in "history".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    model.update(AppEvent::Paste("hidden paste".to_string()));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    let rows_after = render_rows(&mut model, 30, 8);
    assert!(
        !rows_after.iter().any(|row| row.contains("hidden paste")),
        "paste while overlay is active should not change the hidden composer: {:?}",
        rows_after
    );
}

#[test]
fn transcript_overlay_excludes_startup_banner() {
    let mut model = ready_model(40, 10);

    // 打开覆盖层（默认模型包含启动欢迎块）
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    let rows = render_rows(&mut model, 40, 10);
    // 启动欢迎块内容（如 ">_ Hunea"）不应出现在覆盖层中
    assert!(
        !rows.iter().any(|r| r.contains(">_ Hunea")),
        "overlay should not show startup banner content: {:?}",
        rows
    );
}

#[test]
fn transcript_overlay_shows_percentage_rule_and_footer() {
    let mut model = ready_model(30, 10);

    // 发送一条长消息
    model.update(AppEvent::Paste(
        "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\nn\no\np".to_string(),
    ));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // 打开覆盖层
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));

    let rows = render_rows(&mut model, 30, 10);
    // 倒数第二行应为百分比分隔线（百分比在右侧）
    let rule_row = &rows[8];
    assert!(
        rule_row.contains('%'),
        "rule row should contain percentage on the right: {:?}",
        rule_row
    );
    assert!(
        rule_row.starts_with("─"),
        "rule row should start with rule line: {:?}",
        rule_row
    );
    // 最后一行应为提示，且应有空格前缀（与 model_panel 风格一致）
    let footer_row = &rows[9];
    assert!(
        footer_row.starts_with("  "),
        "footer row should have two-space indent like model_panel: {:?}",
        footer_row
    );
}

#[test]
fn transcript_overlay_syncs_scroll_with_main_view() {
    let mut model = ready_model(20, 10);

    // 发送一条多行 assistant 消息（通过粘贴模拟用户输入后由 assistant 回复）
    model.update(AppEvent::Paste(
        "alpha\nbravo\ncharlie\ndelta\necho\nfoxtrot\ngolf\nhotel\nindia\njuliet\nkilo\nlima"
            .to_string(),
    ));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // 主界面默认 follow_bottom，打开 overlay 应显示底部内容
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    let rows_bottom = render_rows(&mut model, 20, 10);
    assert!(
        rows_bottom.iter().any(|r| r.contains("lima")),
        "overlay should show bottom content when main view is at bottom"
    );
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    // 将主界面滚动到顶部
    model.update(AppEvent::MouseWheel { delta_lines: -100 });

    // 再次打开 overlay，应显示顶部内容
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('t'),
        KeyModifiers::CONTROL,
    )));
    let rows_top = render_rows(&mut model, 20, 10);
    // 主界面滚动到顶部后，overlay 应显示与底部不同的内容（验证同步生效）
    assert_ne!(
        rows_top, rows_bottom,
        "overlay should show different content when main view is scrolled to top"
    );
}
