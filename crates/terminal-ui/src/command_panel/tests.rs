use super::*;
use crate::{
    AppEvent, ModelOptions, Sender, StartupBannerOptions, StatusLineItem, StyleMode,
    document::DocumentAnchorRegion, theme::default_palette,
};
use ratatui::{buffer::Buffer, layout::Rect, style::Modifier};

#[test]
fn single_unmatched_character_keeps_command_panel_active() {
    assert_eq!(command_panel_query("/h"), Some("h".to_string()));
    assert_eq!(command_panel_query("/he"), None);
    assert_eq!(command_panel_query("/ "), None);
}

#[test]
fn non_prefix_subsequence_query_matches_command_name() {
    assert_eq!(command_panel_query("/del"), Some("del".to_string()));

    let names = base_command_panel_items_for_query("del")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/models"]);
}

#[test]
fn command_panel_ranks_contiguous_subsequence_above_spread_subsequence() {
    // /se 在 /resend 中是连续子串（score 0），在 /resume 中是离散子序列（score 2），
    // 连续匹配应排在前面。
    let names = base_command_panel_items_for_query("se")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/resend", "/resume"]);
}

#[test]
fn command_panel_ranks_exact_match_above_prefix_match() {
    // "exit" 精确匹配 /exit（score -1100），应排在所有其他命令前面。
    let names = base_command_panel_items_for_query("exit")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names.first(), Some(&"/exit".to_string()));
}

#[test]
fn command_panel_match_score_uses_alias_when_primary_does_not_match() {
    // /qu 不匹配 /exit 的主名 "exit"，但匹配别名 "quit" 的前缀。
    let names = base_command_panel_items_for_query("qu")
        .into_iter()
        .map(|item| item.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["/exit"]);
}

#[test]
fn current_status_notice_still_renders_below_command_panel() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.set_window(24, 6);
    model.show_transient_status_notice("Press Esc again to interrupt");
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();
    model.sync_composer_height();

    let status_line = model.current_status_line_render_result();

    assert!(status_line.has_content);
    let panel = model.current_inline_command_panel_render_result();
    assert!(panel.has_content);
}

#[test]
fn command_panel_rows_are_inserted_into_document_before_status_notice() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            style_mode: StyleMode::Cx,
            status_line_items: vec![StatusLineItem::GitBranch],
            ..ModelOptions::default()
        },
    );
    model.set_window(32, 8);
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();
    model.show_transient_status_notice("Press Esc again to interrupt");

    let layout = model.build_document_layout(crate::frame_time::FrameRenderContext::capture());
    let plain_lines = layout.all_plain_lines(crate::frame_time::FrameRenderContext::capture());
    let panel_line = plain_lines
        .iter()
        .position(|line| line.contains("/exit"))
        .expect("command panel row should exist");
    let notice_line = plain_lines
        .iter()
        .position(|line| line.contains("Press Esc again to interrupt"))
        .expect("status notice should exist");
    let command_panel_rows = layout
        .all_line_anchors(crate::frame_time::FrameRenderContext::capture())
        .into_iter()
        .filter(|anchor| anchor.region == DocumentAnchorRegion::CommandPanel)
        .count();

    assert!(panel_line < notice_line);
    assert_eq!(command_panel_rows, model.command_panel_list_visible_rows());
}

#[test]
fn command_panel_completion_is_undoable_to_the_query() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('/'))));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));
    assert_eq!(model.composer_text(), "/exit");

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('z'),
        KeyModifiers::CONTROL,
    )));

    assert_eq!(model.composer_text(), "/");
}

#[test]
fn plain_esc_dismisses_command_panel_without_changing_composer() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.composer.reset_text_and_move_to_end("/mod");
    model.sync_command_panel_navigation();
    assert!(model.command_panel_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(effect, None);
    assert_eq!(model.composer_text(), "/mod");
    assert!(!model.command_panel_active());
}

#[test]
fn dismissing_command_panel_does_not_prime_chat_interrupt() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            esc_interrupt_presses: 2,
            ..ModelOptions::default()
        },
    );
    model.show_stream_activity("qwen3");
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();

    let dismiss_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    let first_interrupt_effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(dismiss_effect, None);
    assert_eq!(first_interrupt_effect, None);
    assert!(model.current_status_notice_text().contains("Esc again"));
}

#[test]
fn editing_dismissed_command_query_reopens_panel() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.command_panel_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('m'))));

    assert_eq!(model.composer_text(), "/m");
    assert!(model.command_panel_active());
}

#[test]
fn leaving_command_query_clears_dismissed_query() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));
    assert!(!model.command_panel_active());

    model.composer.reset_text_and_move_to_end("plain text");
    model.sync_command_panel_navigation();
    model.composer.reset_text_and_move_to_end("/");
    model.sync_command_panel_navigation();

    assert!(model.command_panel_active());
}

#[test]
fn rendered_command_panel_line_respects_available_width() {
    let model = Model::new(StartupBannerOptions::default());

    let item = base_command_panel_items_for_query("exit")
        .into_iter()
        .next()
        .expect("exit item should exist");
    let (_, plain_line, selectable) =
        model.render_command_panel_line(&item, "", true, 8, display_width(&item.name));

    assert_eq!(display_width(&plain_line), 8);
    assert_eq!(
        selectable.content_columns().map(|(start, _)| start),
        Some(2)
    );
}

#[test]
fn command_panel_highlights_subsequence_matches_in_command_name() {
    let mut model = Model::new(StartupBannerOptions::default());
    model.set_palette(default_palette(), true);
    model.set_window(80, 12);
    model.composer.reset_text_and_move_to_end("/md");
    model.sync_command_panel_navigation();

    let panel = model.current_inline_command_panel_render_result();
    let command_line_index = panel
        .plain_lines
        .iter()
        .position(|line| line.contains("/models"))
        .expect("models command should render");
    let command_line = &panel.lines[command_line_index];

    let highlighted_text = command_line
        .spans
        .iter()
        .filter(|span| {
            span.style.bg == default_palette().surface
                || span.style.add_modifier.contains(Modifier::REVERSED)
        })
        .map(|span| span.content.as_ref())
        .collect::<String>();

    assert_eq!(highlighted_text, "md");
}

#[test]
fn sends_back_command_opens_coarse_rewind_when_esc_mode_is_entry() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            esc_rewind_mode: crate::EscRewindMode::Entry,
            ..ModelOptions::default()
        },
    );
    model
        .transcript_mut()
        .append_message(Sender::User, "first question");
    model
        .transcript_mut()
        .append_message(Sender::Assistant, "first answer");
    model.composer.reset_text_and_move_to_end("/sends-back");
    model.sync_command_panel_navigation();

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert!(model.transcript_overlay_active());
    assert_eq!(model.composer_text(), "");
}

fn model_with_command_menu_mode(mode: crate::CommandMenuMode) -> Model {
    Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            command_menu_mode: mode,
            ..ModelOptions::default()
        },
    )
}

#[test]
fn floating_mode_disables_inline_slash_command_panel() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.composer.reset_text_and_move_to_end("/mod");
    model.sync_command_panel_navigation();

    assert!(!model.command_panel_active());
    assert!(
        !model
            .current_inline_command_panel_render_result()
            .has_content
    );
}

#[test]
fn both_mode_keeps_inline_slash_command_panel() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Both);
    model.composer.reset_text_and_move_to_end("/mod");
    model.sync_command_panel_navigation();

    assert!(model.command_panel_active());
}

#[test]
fn both_mode_floating_menu_hides_inline_slash_panel() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Both);
    model.composer.reset_text_and_move_to_end("/mod");
    model.sync_command_panel_navigation();
    assert!(model.command_panel_active());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    // 悬浮菜单打开时内联斜杠面板不得同时 active，避免双 UI 叠层。
    assert!(model.floating_command_menu_active());
    assert!(!model.command_panel_active());
    assert!(
        !model
            .current_inline_command_panel_render_result()
            .has_content
    );
}

#[test]
fn slash_mode_ignores_ctrl_o_floating_command_menu() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Slash);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    assert!(!model.floating_command_menu_active());
}

#[test]
fn ctrl_alt_o_does_not_open_floating_command_menu() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    )));

    assert!(!model.floating_command_menu_active());
}

#[test]
fn ctrl_o_toggles_floating_command_menu_in_floating_mode() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(!model.floating_command_menu_active());
}

#[test]
fn esc_closes_floating_command_menu() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert!(!model.floating_command_menu_active());
}

#[test]
fn reset_to_initial_tui_state_closes_floating_command_menu() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    model.reset_to_initial_tui_state();

    assert!(!model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_tab_completes_selected_command_into_query() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    // 默认首项是 /exit；Tab 应把过滤框补全为裸命令名 exit，而非执行。
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Tab)));

    assert!(model.floating_command_menu_active());
    assert_eq!(model.composer_text(), "");

    let query = model
        .resolved_floating_command_menu_state()
        .map(|state| state.query);
    assert_eq!(query.as_deref(), Some("exit"));
}

#[test]
fn floating_command_menu_typing_does_not_reach_composer() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    for ch in "models".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }

    assert_eq!(model.composer_text(), "");
    assert!(model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_filters_then_executes_selected() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    for ch in "models".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, None);
    assert!(model.model_panel_active());
    assert!(!model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_down_navigation_executes_second_item() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, Some(AppEffect::OpenResumePicker));
    assert!(!model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_click_selects_row_then_executes() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.set_window(80, 24);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    let geometry = model
        .floating_command_menu_geometry(Rect::new(0, 0, 80, 24))
        .expect("菜单打开时应存在几何布局");
    // 点击第二条可见命令行（offset 1），应把选中项从默认的 0 移到 1。
    let column = geometry.popup_area.x + 2;
    let row = geometry.list_top + 1;
    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column,
        row,
    });

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(effect, Some(AppEffect::OpenResumePicker));
    assert!(!model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_click_outside_closes() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.set_window(80, 24);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    // 居中弹窗不会覆盖左上角，(0, 0) 落在浮窗外部，点击应关闭菜单。
    model.update(AppEvent::MouseDown {
        button: MouseButton::Left,
        column: 0,
        row: 0,
    });

    assert!(!model.floating_command_menu_active());
}

fn find_text_in_buffer(buffer: &Buffer, needle: &str) -> Option<(u16, u16)> {
    let area = buffer.area;
    for y in area.top()..area.bottom() {
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buffer[(x, y)].symbol());
        }
        if let Some(byte_index) = row.find(needle) {
            let column = row[..byte_index].chars().count();
            return Some((u16::try_from(column).unwrap_or(u16::MAX), y));
        }
    }
    None
}

#[test]
fn floating_command_menu_renders_as_centered_bordered_popup() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.set_palette(default_palette(), true);
    model.set_window(80, 24);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);

    // panel_block 使用圆角边框，弹窗应绘制出边框而非锚定式无边框浮层。
    let has_rounded_border = (area.top()..area.bottom()).any(|y| {
        (area.left()..area.right())
            .any(|x| matches!(buffer[(x, y)].symbol(), "╭" | "╮" | "╰" | "╯"))
    });
    assert!(has_rounded_border, "弹窗应带圆角边框");

    // 命令项去掉前导 `/`，显示裸命令名。
    let (exit_column, exit_row) =
        find_text_in_buffer(&buffer, "exit").expect("命令项应渲染裸命令名 exit");
    assert!(
        find_text_in_buffer(&buffer, "/exit").is_none(),
        "命令项不应再显示前导 /"
    );

    // 空查询时输入框显示占位提示，告知用户可键入筛选。
    assert!(
        find_text_in_buffer(&buffer, "type to filter").is_some(),
        "输入框应显示可筛选的占位提示"
    );

    // 输入行下方有一条横贯内框的分割线，与命令列表区隔（内联输入框，无盒子边框）。
    let geometry = model
        .floating_command_menu_geometry(area)
        .expect("菜单打开时应存在几何布局");
    let divider_row = geometry.inner_area.y + 1;
    let divider_is_rule = (geometry.inner_area.x
        ..geometry.inner_area.x + geometry.inner_area.width)
        .all(|x| buffer[(x, divider_row)].symbol() == "─");
    assert!(divider_is_rule, "输入行下方应有横贯内框的分割线");

    // 标题保持 1 空格前导的原始设计，命令内容使用 2 cell 左留白，
    // 故命令名比标题 `Commands` 的 `C` 再右移 1 列。
    let (commands_column, commands_row) =
        find_text_in_buffer(&buffer, "Commands").expect("弹窗标题 Commands 应渲染");
    assert_eq!(
        exit_column,
        commands_column + 1,
        "命令内容 2 cell 左留白应比 1 空格前导的标题再右移 1 列"
    );

    // 居中弹窗内容既不贴最左，也不贴顶。
    assert!(exit_column > 2, "弹窗内容应居中，而非左侧锚定");
    assert!(
        exit_row > 1 && commands_row >= 1,
        "弹窗内容应居中，而非顶部锚定"
    );
}

fn model_with_floating_command_menu_rows(rows: u16) -> Model {
    Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            command_menu_mode: crate::CommandMenuMode::Floating,
            command_menu_rows: rows,
            ..ModelOptions::default()
        },
    )
}

fn floating_command_menu_has_scrollbar_column(model: &Model, buffer: &Buffer, area: Rect) -> bool {
    let geometry = model
        .floating_command_menu_geometry(area)
        .expect("菜单打开时应存在几何布局");
    // 滚动条画在弹窗右侧边框列上。
    let scrollbar_x = geometry.popup_area.right().saturating_sub(1);
    let list_bottom = geometry
        .list_top
        .saturating_add(u16::try_from(geometry.list_rows).unwrap_or(u16::MAX));
    (geometry.list_top..list_bottom).any(|y| matches!(buffer[(scrollbar_x, y)].symbol(), "┃" | "█"))
}

#[test]
fn floating_command_menu_caps_visible_rows_by_config() {
    // 默认命令数（9 条）超过默认可见行数上限 7，视口应被截断到 7 行。
    let mut model = model_with_floating_command_menu_rows(7);
    model.set_window(80, 40);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    let geometry = model
        .floating_command_menu_geometry(Rect::new(0, 0, 80, 40))
        .expect("菜单打开时应存在几何布局");
    let total = model
        .resolved_floating_command_menu_state()
        .map(|state| state.items.len())
        .unwrap_or_default();

    assert_eq!(geometry.list_rows, 7, "可见行数应被配置上限 7 截断");
    assert!(total > 7, "默认命令数应超过可见行数，才需要滚动");
}

#[test]
fn floating_command_menu_shows_scrollbar_when_overflowing() {
    let mut model = model_with_floating_command_menu_rows(7);
    model.set_palette(default_palette(), true);
    model.set_window(80, 40);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    let area = Rect::new(0, 0, 80, 40);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);

    assert!(
        floating_command_menu_has_scrollbar_column(&model, &buffer, area),
        "命令数超出可见行数时，应在内框右列绘制滚动条"
    );
}

#[test]
fn floating_command_menu_hides_scrollbar_when_all_rows_fit() {
    // 行数上限 21 足以容纳全部默认命令，不应出现滚动条。
    let mut model = model_with_floating_command_menu_rows(21);
    model.set_palette(default_palette(), true);
    model.set_window(80, 40);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    let area = Rect::new(0, 0, 80, 40);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);

    assert!(
        !floating_command_menu_has_scrollbar_column(&model, &buffer, area),
        "命令全部可见时不应绘制滚动条"
    );
}

#[test]
fn floating_command_menu_execution_keeps_composer_draft() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model
        .composer
        .reset_text_and_move_to_end("draft in progress");
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Down)));
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // 悬浮菜单查询独立于 composer，执行命令不得清空用户草稿。
    assert_eq!(effect, Some(AppEffect::OpenResumePicker));
    assert_eq!(model.composer_text(), "draft in progress");
}

#[test]
fn floating_command_menu_consumes_editing_keys_without_touching_composer() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.composer.reset_text_and_move_to_end("draft");
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    // Home / Delete / Shift+Enter 在菜单未独占键盘时会分别移动 composer 光标、
    // 前向删除、插入换行；菜单打开期间必须消费它们，不得改动下层草稿。
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Home)));
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Delete)));
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::SHIFT,
    )));

    assert_eq!(model.composer_text(), "draft");
    assert!(model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_paste_filters_query_instead_of_composer() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    // 换行等控制字符被剔除，其余进入过滤查询；composer 不接收粘贴。
    model.update(AppEvent::Paste("mod\nels".to_string()));

    assert_eq!(model.composer_text(), "");
    assert!(model.floating_command_menu_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    assert_eq!(effect, None);
    assert!(model.model_panel_active());
}

#[test]
fn ctrl_o_closes_file_picker_before_opening_floating_menu() {
    let root = std::env::temp_dir().join(format!(
        "hunea-floating-menu-file-picker-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create temp dir for file picker");
    std::fs::write(root.join("notes.md"), "x").expect("write temp file");

    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.current_dir = root.display().to_string();
    model.set_window(80, 24);
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char('@'))));
    assert!(model.file_picker_active());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));

    // 菜单独占键盘输入，composer 附属浮层留在下层只会冻结，打开时应一并关闭。
    assert!(!model.file_picker_active());
    assert!(model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_wheel_moves_selection_instead_of_document() {
    let mut model = model_with_floating_command_menu_rows(7);
    model.set_window(80, 40);
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    assert!(model.floating_command_menu_active());

    model.update(AppEvent::MouseWheel { delta_lines: 3 });

    // 与其他 picker 一致：滚轮每次移动一格选中项，事件被菜单消费。
    let selected = model
        .resolved_floating_command_menu_state()
        .map(|state| state.selected);
    assert_eq!(selected, Some(1));

    model.update(AppEvent::MouseWheel { delta_lines: -3 });

    let selected = model
        .resolved_floating_command_menu_state()
        .map(|state| state.selected);
    assert_eq!(selected, Some(0));
    assert!(model.floating_command_menu_active());
}

#[test]
fn floating_command_menu_models_execution_keeps_composer_draft() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model
        .composer
        .reset_text_and_move_to_end("draft in progress");
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    for ch in "models".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // `open_model_panel` 不再清 composer：悬浮路径执行 /models 保留用户草稿。
    assert_eq!(effect, None);
    assert!(model.model_panel_active());
    assert_eq!(model.composer_text(), "draft in progress");
}

#[test]
fn inline_models_command_clears_command_text_from_composer() {
    let mut model = Model::new(StartupBannerOptions::default());
    for ch in "/models".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(ch))));
    }
    assert!(model.command_panel_active());

    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    // 内联路径 composer 中是命令文本本身，执行后统一由
    // `clear_inline_command_query` 清空，行为与拆分前一致。
    assert_eq!(effect, None);
    assert!(model.model_panel_active());
    assert_eq!(model.composer_text(), "");
}

#[test]
fn floating_command_menu_hides_composer_cursor_while_open() {
    let mut model = model_with_command_menu_mode(crate::CommandMenuMode::Floating);
    model.set_palette(default_palette(), true);
    model.set_window(80, 24);

    let area = Rect::new(0, 0, 80, 24);
    let mut buffer = Buffer::empty(area);
    let cursor_before = model.render_to_buffer(area, &mut buffer);
    assert!(cursor_before.is_some(), "菜单未打开时 composer 光标应存在");

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('o'),
        KeyModifiers::CONTROL,
    )));
    let mut buffer = Buffer::empty(area);
    let cursor_while_open = model.render_to_buffer(area, &mut buffer);

    // 键盘焦点在菜单查询（自绘 caret），composer 终端光标不应再绘制。
    assert!(
        cursor_while_open.is_none(),
        "菜单打开时不应定位 composer 光标"
    );
}
