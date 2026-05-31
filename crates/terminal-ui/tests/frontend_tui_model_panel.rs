use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier},
};
use runtime_domain::model_catalog::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource,
};
use runtime_domain::{model_catalog::ProviderSyncRequest, provider::ProviderKind};
use terminal_ui::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions, theme::default_palette,
};

const MODEL_PANEL_FOOTER_HINT: &str =
    "Enter select · U refresh · Esc clear/exit · ←→/Tab providers · ↑↓ navigate";

#[test]
fn models_command_replaces_composer_with_model_panel() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    let line_row =
        find_row_containing(&buffer, "━").expect("model panel should render a blue separator line");
    assert_blue_bold_row(&buffer, line_row);

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Providers:")
            && row.contains("[Local]")
            && row.contains("DeepSeek")),
        "expected providers row, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("[DeepSeek]")),
        "inactive provider should not render bracket chrome: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Available Models(Type to Search):")),
        "expected model list heading, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("qwen3")),
        "expected configured model entry, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "composer prompt should be hidden while model panel replaces the input area: {rows:?}"
    );
}

#[test]
fn model_panel_left_right_switch_provider_tabs() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    model.update(AppEvent::Key(KeyCode::Right.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Providers:")
            && row.contains("Local")
            && row.contains("[DeepSeek]")),
        "right arrow should switch to the next provider, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("deepseek-chat")),
        "right arrow should show the next provider model list, got: {rows:?}"
    );

    model.update(AppEvent::Key(KeyCode::Left.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Providers:")
            && row.contains("[Local]")
            && row.contains("DeepSeek")),
        "left arrow should switch back to the previous provider, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("qwen3")),
        "left arrow should restore the previous provider model list, got: {rows:?}"
    );
}

#[test]
fn model_panel_search_title_filters_models_and_enter_selects_filtered_model() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    type_text(&mut model, "qwen2");

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Search: qwen2")),
        "typing should replace the available-models title with search text, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("qwen2")),
        "search should keep matching models visible, got: {rows:?}"
    );
    let search_model_rows = model_panel_rows_after_heading(&rows, "Search: qwen2");
    assert!(
        search_model_rows.iter().all(|row| !row.contains("qwen3")),
        "search should hide non-matching models in the current provider list, got: {rows:?}"
    );

    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("local", "qwen2"))
    );
    assert_eq!(
        effect,
        Some(AppEffect::PersistSelectedModel {
            selection: ModelSelection::new("local", "qwen2")
        })
    );
}

#[test]
fn model_panel_search_first_character_updates_after_cached_render() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    let _cached_rows = render_trimmed_rows(&mut model, 72, 18);

    model.update(AppEvent::Key(KeyCode::Char('q').into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Search: q")),
        "first search character should render immediately after an existing cache, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("qwen3")),
        "first search character should apply filtering immediately, got: {rows:?}"
    );
}

#[test]
fn model_panel_backspace_removes_search_characters() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    type_text(&mut model, "qwe");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Backspace)));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Search: qw")),
        "Backspace should remove the last search character, got: {rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('h'),
        KeyModifiers::CONTROL,
    )));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Search: q")),
        "Ctrl+H should remove the last search character for terminals that report Backspace that way, got: {rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('\u{0008}'),
        KeyModifiers::NONE,
    )));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter()
            .any(|row| row.contains("Available Models(Type to Search):")),
        "raw C0 Backspace should clear the final search character, got: {rows:?}"
    );
}

#[test]
fn model_panel_ignores_paste_without_changing_hidden_composer() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    model.update(AppEvent::Paste("qwen2".to_string()));

    assert_eq!(
        model.composer_text(),
        "",
        "paste while model panel is active should not modify hidden composer"
    );
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter()
            .any(|row| row.contains("Available Models(Type to Search):")),
        "paste should not become a model search query, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Search: qwen2")),
        "paste should be ignored instead of rendered as search text, got: {rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(
        model.composer_text(),
        "",
        "ignored paste should not appear after closing the model panel"
    );
}

#[test]
fn model_panel_esc_clears_search_before_closing_panel() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    type_text(&mut model, "qwen2");

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter()
            .any(|row| row.contains("Available Models(Type to Search):")),
        "first Esc should clear the search and keep the panel open, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("qwen3"))
            && rows.iter().any(|row| row.contains("qwen2")),
        "clearing search should restore all current-provider models, got: {rows:?}"
    );

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().all(|row| !row.contains("Available Models")),
        "second Esc should close the model panel, got: {rows:?}"
    );
}

#[test]
fn model_panel_tab_switches_provider_tabs() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    model.update(AppEvent::Key(KeyCode::Tab.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("deepseek-chat")),
        "tab should switch to the next provider's model list, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Providers:")
            && row.contains("Local")
            && row.contains("[DeepSeek]")),
        "provider tabs should track the active provider, got: {rows:?}"
    );
    assert!(
        rows.iter()
            .find(|row| row.contains("Providers:"))
            .is_some_and(|row| !row.contains("[Local]")),
        "inactive provider should drop brackets after tab switch, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("• Provider")),
        "overview should not repeat the active provider, got: {rows:?}"
    );
}

#[test]
fn model_panel_enter_selects_model_and_restores_composer() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Tab.into()));
    model.update(AppEvent::Key(KeyCode::Down.into()));

    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));

    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("deepseek", "deepseek-reasoner"))
    );
    assert_eq!(
        effect,
        Some(AppEffect::PersistSelectedModel {
            selection: ModelSelection::new("deepseek", "deepseek-reasoner")
        })
    );
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().all(|row| !row.contains("Available Models")),
        "panel should close after selecting a model: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Model selected: [DeepSeek] deepseek-reasoner")),
        "selection notice should use the provider display name, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("deepseek/deepseek")),
        "selection notice should not use provider/model machine formatting, got: {rows:?}"
    );
}

#[test]
fn model_panel_u_requests_refresh_for_current_provider() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let effect = model.update(AppEvent::Key(KeyCode::Char('U').into()));

    assert_eq!(
        effect,
        Some(AppEffect::RefreshModelProvider {
            request: ProviderSyncRequest {
                provider_id: "local".to_string(),
                kind: ProviderKind::OpenAiCompatible,
                display_name: "Local".to_string(),
                base_url: Some("http://127.0.0.1:1234/v1".to_string()),
                api_key: None,
                api_key_env: None,
            }
        })
    );
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("Available Models")),
        "refresh should keep the model panel open, got: {rows:?}"
    );
}

#[test]
fn model_panel_esc_closes_without_changing_selection() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Tab.into()));

    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Esc)));

    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("local", "qwen3"))
    );
    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().all(|row| !row.contains("Available Models")),
        "panel should close after Esc: {rows:?}"
    );
}

#[test]
fn model_panel_shows_empty_state_without_enabled_models() {
    let mut model = ready_model(
        72,
        18,
        ModelOptions {
            model_catalog: ModelCatalog::default(),
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/models");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| row.contains("No enabled models")),
        "expected empty model panel state, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains('›')),
        "composer prompt should be hidden for empty model panel: {rows:?}"
    );
}

#[test]
fn model_panel_shows_sync_error_for_auto_synced_provider() {
    let mut model = ready_model(
        72,
        18,
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![
                ModelProvider::new(
                    "local",
                    ProviderKind::OpenAiCompatible,
                    "Local",
                    Some("http://127.0.0.1:1234/v1".to_string()),
                    ModelSource::Synced,
                    Vec::new(),
                )
                .with_sync_error("cannot reach http://localhos:1234/v1/models"),
            ]),
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/models");

    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let rows = render_trimmed_rows(&mut model, 72, 18);
    assert!(
        rows.iter().any(|row| {
            row.contains("Sync failed:") && row.contains("http://localhos:1234/v1/models")
        }),
        "expected provider sync error in model panel, got: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| !row.contains("Failed to sync /v1/models")),
        "sync error should avoid repeating /v1/models in the prefix: {rows:?}"
    );
}

fn model_options_with_catalog() -> ModelOptions {
    ModelOptions {
        model_catalog: ModelCatalog::new(vec![
            ModelProvider::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::Configured,
                vec![
                    ModelEntry::new(
                        "qwen3",
                        Some("Local reasoning model".to_string()),
                        ModelSource::Configured,
                    ),
                    ModelEntry::new(
                        "qwen2",
                        Some("Smaller local model".to_string()),
                        ModelSource::Configured,
                    ),
                ],
            ),
            ModelProvider::new(
                "deepseek",
                ProviderKind::OpenAiCompatible,
                "DeepSeek",
                Some("https://api.deepseek.com/v1".to_string()),
                ModelSource::Configured,
                vec![
                    ModelEntry::new(
                        "deepseek-chat",
                        Some("General chat model".to_string()),
                        ModelSource::Configured,
                    ),
                    ModelEntry::new(
                        "deepseek-reasoner",
                        Some("Reasoning model".to_string()),
                        ModelSource::Configured,
                    ),
                ],
            ),
        ]),
        selected_model: Some(ModelSelection::new("local", "qwen3")),
        ..ModelOptions::default()
    }
}

fn ready_model(width: u16, height: u16, options: ModelOptions) -> Model {
    let mut model = Model::new_with_options(StartupBannerOptions::default(), options);
    model.update(AppEvent::Resized { width, height });
    model.update(AppEvent::DetectedPalette {
        palette: default_palette(),
        has_dark_background: true,
    });
    model
}

fn type_text(model: &mut Model, text: &str) {
    for character in text.chars() {
        model.update(AppEvent::Key(KeyCode::Char(character).into()));
    }
}

fn render_trimmed_rows(model: &mut Model, width: u16, height: u16) -> Vec<String> {
    trim_rows(&render_buffer(model, width, height))
}

fn render_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}

fn trim_rows(buffer: &Buffer) -> Vec<String> {
    let mut rows = Vec::with_capacity(buffer.area.height as usize);

    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rows.push(rendered.trim_end().to_string());
    }

    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }

    rows
}

fn find_row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
    for row in 0..buffer.area.height {
        let mut rendered = String::new();
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        if rendered.contains(needle) {
            return Some(row);
        }
    }

    None
}

fn assert_blue_bold_row(buffer: &Buffer, row: u16) {
    let palette = default_palette();
    let styled_cells = (0..buffer.area.width)
        .filter(|column| buffer[(*column, row)].symbol() == "━")
        .collect::<Vec<_>>();
    assert!(
        !styled_cells.is_empty(),
        "separator row should contain horizontal rule glyphs"
    );
    for column in styled_cells {
        let cell = &buffer[(column, row)];
        assert_eq!(cell.fg, palette.accent);
        assert!(
            cell.modifier.contains(Modifier::BOLD),
            "separator line should be bold at column {column}"
        );
    }
}

#[test]
fn model_panel_providers_label_uses_primary_text_color() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    assert_text_cells_use_color(&buffer, "Providers:", default_palette().main);
}

#[test]
fn model_panel_footer_hint_is_italic() {
    let mut model = ready_model(
        96,
        24,
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                None,
                ModelSource::Configured,
                vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 96, 24);

    assert_text_cells_are_italic(&buffer, MODEL_PANEL_FOOTER_HINT);
}

#[test]
fn model_panel_selected_provider_uses_surface_background_only_on_active_tab() {
    let palette = default_palette();
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);

    assert_text_cells_use_bg(&buffer, "[Local]", palette.surface);
    assert_text_cells_are_bold(&buffer, "[Local]");
    assert_text_cells_do_not_use_bg(&buffer, "DeepSeek", palette.surface);
}

#[test]
fn model_panel_splits_current_model_from_provider_details() {
    let palette = default_palette();
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    let rows = trim_rows(&buffer);

    let providers_row = row_containing(&buffer, "Providers:");
    let current_model_row = row_containing(&buffer, "Current Model:");
    let provider_details_row = row_containing(&buffer, "Provider Details:");
    assert!(
        providers_row < current_model_row && current_model_row < provider_details_row,
        "current model should be a standalone row between providers and provider details: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("Current Model:") && row.contains("[Local] qwen3")),
        "current model should render as provider chip plus model name, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("local/qwen3")),
        "current model should not use provider/model machine formatting, got: {rows:?}"
    );

    assert_text_cells_use_color_on_row(&buffer, "Current Model:", current_model_row, palette.main);
    assert_text_cells_are_not_bold_on_row(&buffer, "Current Model:", current_model_row);
    assert_text_cells_do_not_use_bg_on_row(&buffer, "[Local]", current_model_row, palette.surface);
    assert_text_cells_use_color_on_row(&buffer, "[Local]", current_model_row, palette.secondary);
    assert_text_cells_are_bold_on_row(&buffer, "[Local]", current_model_row);
    assert_text_cells_use_color_on_row(&buffer, "qwen3", current_model_row, palette.command_accent);
    assert_text_cells_are_not_bold_on_row(&buffer, "qwen3", current_model_row);
}

#[test]
fn model_panel_provider_details_uses_precise_source_label_without_provider_row() {
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    let rows = trim_rows(&buffer);

    assert!(
        rows.iter().any(|row| row.contains("Provider Details:")),
        "provider details heading should describe the active provider metadata, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("Overview:")),
        "provider metadata heading should not use vague Overview wording, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains("Model Source")),
        "provider details should use precise source wording, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("• Source")),
        "provider details should not use vague Source label, got: {rows:?}"
    );
    assert!(
        rows.iter().all(|row| !row.contains("• Provider")),
        "provider details should not repeat provider identity, got: {rows:?}"
    );
}

#[test]
fn model_panel_model_list_uses_focus_and_current_model_styles_without_current_suffix() {
    let palette = default_palette();
    let mut model = ready_model(72, 18, model_options_with_catalog());
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));
    model.update(AppEvent::Key(KeyCode::Down.into()));

    let buffer = render_buffer(&mut model, 72, 18);
    let rows = trim_rows(&buffer);

    assert!(
        rows.iter().all(|row| !row.contains("current")),
        "current model should be shown by color instead of suffix text: {rows:?}"
    );
    assert_text_cells_use_color_after(&buffer, "qwen3", "Available Models", palette.command_accent);
    assert_text_cells_are_not_bold_after(&buffer, "qwen3", "Available Models");
    assert_text_cells_use_color_after(&buffer, "qwen2", "Available Models", palette.main);
    assert_text_cells_are_bold_after(&buffer, "qwen2", "Available Models");
}

#[test]
fn model_panel_scrolls_long_model_list_without_hiding_footer_hint() {
    let mut model = ready_model(96, 24, model_options_with_many_models(10));
    type_text(&mut model, "/models");
    model.update(AppEvent::Key(KeyCode::Enter.into()));

    let rows = render_trimmed_rows(&mut model, 96, 24);
    assert_eq!(
        visible_many_model_rows(&rows),
        vec![
            "many-model-01",
            "many-model-02",
            "many-model-03",
            "many-model-04",
            "many-model-05",
            "many-model-06",
            "many-model-07",
        ],
        "model panel should show at most seven model rows before scrolling, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains(MODEL_PANEL_FOOTER_HINT)),
        "model panel footer hint should stay visible, got: {rows:?}"
    );

    for _ in 0..7 {
        model.update(AppEvent::Key(KeyCode::Down.into()));
    }

    let rows = render_trimmed_rows(&mut model, 96, 24);
    assert_eq!(
        visible_many_model_rows(&rows),
        vec![
            "many-model-02",
            "many-model-03",
            "many-model-04",
            "many-model-05",
            "many-model-06",
            "many-model-07",
            "many-model-08",
        ],
        "model panel should scroll internally to keep the focused model visible, got: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.contains(MODEL_PANEL_FOOTER_HINT)),
        "model panel footer hint should remain visible after scrolling, got: {rows:?}"
    );
}

fn model_options_with_many_models(count: usize) -> ModelOptions {
    let models = (1..=count)
        .map(|index| {
            ModelEntry::new(
                format!("many-model-{index:02}"),
                None,
                ModelSource::Configured,
            )
        })
        .collect::<Vec<_>>();

    ModelOptions {
        model_catalog: ModelCatalog::new(vec![ModelProvider::new(
            "many",
            ProviderKind::OpenAiCompatible,
            "Many",
            Some("https://api.example.com/v1".to_string()),
            ModelSource::Configured,
            models,
        )]),
        selected_model: Some(ModelSelection::new("many", "many-model-01")),
        ..ModelOptions::default()
    }
}

fn visible_many_model_rows(rows: &[String]) -> Vec<&'static str> {
    const MODEL_IDS: [&str; 10] = [
        "many-model-01",
        "many-model-02",
        "many-model-03",
        "many-model-04",
        "many-model-05",
        "many-model-06",
        "many-model-07",
        "many-model-08",
        "many-model-09",
        "many-model-10",
    ];

    let start = rows
        .iter()
        .position(|row| row.contains("Available Models"))
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = rows
        .iter()
        .position(|row| row.contains("Enter select"))
        .unwrap_or(rows.len());
    let model_rows = &rows[start..end];

    MODEL_IDS
        .iter()
        .filter(|model_id| model_rows.iter().any(|row| row.contains(*model_id)))
        .copied()
        .collect()
}

fn model_panel_rows_after_heading<'a>(rows: &'a [String], heading: &str) -> &'a [String] {
    let start = rows
        .iter()
        .position(|row| row.contains(heading))
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = rows
        .iter()
        .position(|row| row.contains("Enter select"))
        .unwrap_or(rows.len());
    &rows[start..end]
}

fn assert_text_cells_use_color(buffer: &Buffer, text: &str, expected: Color) {
    let (row, column) = find_cell_containing(buffer, text);
    assert_text_cells_use_color_at(buffer, text, row, column, expected);
}

fn assert_text_cells_use_color_on_row(buffer: &Buffer, text: &str, row: u16, expected: Color) {
    let column = find_column_containing_on_row(buffer, text, row);
    assert_text_cells_use_color_at(buffer, text, row, column, expected);
}

fn assert_text_cells_use_color_at(
    buffer: &Buffer,
    text: &str,
    row: u16,
    column: u16,
    expected: Color,
) {
    for offset in 0..text.chars().count() {
        assert_eq!(
            buffer[(column + offset as u16, row)].fg,
            expected,
            "expected {text:?} to use {expected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_use_color_after(buffer: &Buffer, text: &str, after: &str, expected: Color) {
    let (row, column) = find_cell_containing_after(buffer, text, after);
    for offset in 0..text.chars().count() {
        assert_eq!(
            buffer[(column + offset as u16, row)].fg,
            expected,
            "expected {text:?} after {after:?} to use {expected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_use_bg(buffer: &Buffer, text: &str, expected: Option<Color>) {
    let (row, column) = find_cell_containing(buffer, text);
    assert_text_cells_use_bg_at(buffer, text, row, column, expected);
}

fn assert_text_cells_use_bg_at(
    buffer: &Buffer,
    text: &str,
    row: u16,
    column: u16,
    expected: Option<Color>,
) {
    let expected = expected.unwrap_or(Color::Reset);
    for offset in 0..text.chars().count() {
        assert_eq!(
            buffer[(column + offset as u16, row)].bg,
            expected,
            "expected {text:?} to use bg {expected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_do_not_use_bg(buffer: &Buffer, text: &str, unexpected: Option<Color>) {
    let (row, column) = find_cell_containing(buffer, text);
    assert_text_cells_do_not_use_bg_at(buffer, text, row, column, unexpected);
}

fn assert_text_cells_do_not_use_bg_on_row(
    buffer: &Buffer,
    text: &str,
    row: u16,
    unexpected: Option<Color>,
) {
    let column = find_column_containing_on_row(buffer, text, row);
    assert_text_cells_do_not_use_bg_at(buffer, text, row, column, unexpected);
}

fn assert_text_cells_do_not_use_bg_at(
    buffer: &Buffer,
    text: &str,
    row: u16,
    column: u16,
    unexpected: Option<Color>,
) {
    let unexpected = unexpected.unwrap_or(Color::Reset);
    for offset in 0..text.chars().count() {
        assert_ne!(
            buffer[(column + offset as u16, row)].bg,
            unexpected,
            "expected {text:?} not to use bg {unexpected:?} at offset {offset}"
        );
    }
}

fn assert_text_cells_are_bold(buffer: &Buffer, text: &str) {
    let (row, column) = find_cell_containing(buffer, text);
    assert_text_cells_are_bold_at(buffer, text, row, column);
}

fn assert_text_cells_are_bold_on_row(buffer: &Buffer, text: &str, row: u16) {
    let column = find_column_containing_on_row(buffer, text, row);
    assert_text_cells_are_bold_at(buffer, text, row, column);
}

fn assert_text_cells_are_bold_at(buffer: &Buffer, text: &str, row: u16, column: u16) {
    for offset in 0..text.chars().count() {
        assert!(
            buffer[(column + offset as u16, row)]
                .modifier
                .contains(Modifier::BOLD),
            "expected {text:?} to be bold at offset {offset}"
        );
    }
}

fn assert_text_cells_are_italic(buffer: &Buffer, text: &str) {
    let (row, column) = find_cell_containing(buffer, text);
    for offset in 0..text.chars().count() {
        assert!(
            buffer[(column + offset as u16, row)]
                .modifier
                .contains(Modifier::ITALIC),
            "expected {text:?} to be italic at offset {offset}"
        );
    }
}

fn assert_text_cells_are_bold_after(buffer: &Buffer, text: &str, after: &str) {
    let (row, column) = find_cell_containing_after(buffer, text, after);
    for offset in 0..text.chars().count() {
        assert!(
            buffer[(column + offset as u16, row)]
                .modifier
                .contains(Modifier::BOLD),
            "expected {text:?} after {after:?} to be bold at offset {offset}"
        );
    }
}

fn assert_text_cells_are_not_bold_after(buffer: &Buffer, text: &str, after: &str) {
    let (row, column) = find_cell_containing_after(buffer, text, after);
    assert_text_cells_are_not_bold_at(buffer, text, row, column);
}

fn assert_text_cells_are_not_bold_on_row(buffer: &Buffer, text: &str, row: u16) {
    let column = find_column_containing_on_row(buffer, text, row);
    assert_text_cells_are_not_bold_at(buffer, text, row, column);
}

fn assert_text_cells_are_not_bold_at(buffer: &Buffer, text: &str, row: u16, column: u16) {
    for offset in 0..text.chars().count() {
        assert!(
            !buffer[(column + offset as u16, row)]
                .modifier
                .contains(Modifier::BOLD),
            "expected {text:?} not to be bold at offset {offset}"
        );
    }
}

fn find_cell_containing(buffer: &Buffer, needle: &str) -> (u16, u16) {
    find_cell_containing_from_row(buffer, needle, 0)
}

fn row_containing(buffer: &Buffer, needle: &str) -> u16 {
    find_cell_containing(buffer, needle).0
}

fn find_cell_containing_after(buffer: &Buffer, needle: &str, after: &str) -> (u16, u16) {
    let (after_row, _) = find_cell_containing(buffer, after);
    find_cell_containing_from_row(buffer, needle, after_row.saturating_add(1))
}

fn find_cell_containing_from_row(buffer: &Buffer, needle: &str, start_row: u16) -> (u16, u16) {
    for row in start_row..buffer.area.height {
        if let Some(column) = find_column_containing_on_row_opt(buffer, needle, row) {
            return (row, column);
        }
    }

    panic!(
        "could not find {needle:?} in rendered rows: {:?}",
        trim_rows(buffer)
    )
}

fn find_column_containing_on_row(buffer: &Buffer, needle: &str, row: u16) -> u16 {
    find_column_containing_on_row_opt(buffer, needle, row).unwrap_or_else(|| {
        panic!(
            "could not find {needle:?} on row {row}: {:?}",
            trim_rows(buffer)
        )
    })
}

fn find_column_containing_on_row_opt(buffer: &Buffer, needle: &str, row: u16) -> Option<u16> {
    let needle_symbols = needle
        .chars()
        .map(|character| character.to_string())
        .collect::<Vec<_>>();

    let symbols = (0..buffer.area.width)
        .map(|column| buffer[(column, row)].symbol().to_string())
        .collect::<Vec<_>>();
    for column in 0..=symbols.len().saturating_sub(needle_symbols.len()) {
        if symbols[column..column + needle_symbols.len()] == needle_symbols {
            return Some(column as u16);
        }
    }

    None
}
