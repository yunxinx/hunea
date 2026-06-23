use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use runtime_domain::model_catalog::{
    ModelCatalog, ModelEntry, ModelProvider, ModelSelection, ModelSource,
};
use runtime_domain::provider::{ProviderApiKey, ProviderKind};
use terminal_ui::{
    AppEffect, AppEvent, Model, ModelOptions, StartupBannerOptions,
    theme::{palette_from_background, terminal_default_palette},
};

#[test]
fn model_does_not_auto_select_first_catalog_model_without_default() {
    let model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            ..ModelOptions::default()
        },
    );

    assert_eq!(model.selected_model(), None);
}

#[test]
fn startup_banner_uses_the_selected_model_id() {
    let model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            ..ModelOptions::default()
        },
    );

    let startup_banner = model
        .transcript_plain_items()
        .into_iter()
        .next()
        .expect("startup banner should be present");

    assert!(startup_banner.contains("model:     qwen3   /models to change"));
}

#[test]
fn configured_default_model_is_kept_when_provider_models_are_not_loaded() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![ModelProvider::new(
                "local",
                ProviderKind::OpenAiCompatible,
                "Local",
                Some("http://127.0.0.1:1234/v1".to_string()),
                ModelSource::NotLoaded,
                Vec::new(),
            )]),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );

    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("local", "qwen3"))
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn {
        request,
    }) = effect
    else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert_eq!(request.provider_id(), "local");
    assert_eq!(request.model_id(), "qwen3");
}

#[test]
fn configured_default_model_is_trusted_when_it_is_outside_allowlist() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen4")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::DetectedPalette {
        palette: terminal_default_palette(),
        has_dark_background: true,
    });

    assert_eq!(
        model.selected_model(),
        Some(ModelSelection::new("local", "qwen4"))
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn {
        request,
    }) = effect
    else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert_eq!(request.provider_id(), "local");
    assert_eq!(request.model_id(), "qwen4");
}

#[test]
fn enter_with_required_empty_model_keeps_draft_unsent() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::DetectedPalette {
        palette: terminal_default_palette(),
        has_dark_background: true,
    });

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert!(effect.is_none());
    assert_eq!(model.composer_text(), "hello");
    assert_eq!(model.transcript_plain_items().len(), 1);
}

#[test]
fn enter_with_required_selected_model_sends_message() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "");
    assert!(
        model
            .transcript_plain_items()
            .iter()
            .any(|item| item.contains("hello"))
    );
}

#[test]
fn enter_with_selected_provider_model_returns_conversation_turn_effect() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn {
        request,
    }) = effect
    else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert_eq!(request.provider_id(), "local");
    assert_eq!(request.provider_kind(), ProviderKind::OpenAiCompatible);
    assert_eq!(request.model_id(), "qwen3");
    assert_eq!(request.base_url(), Some("http://127.0.0.1:1234/v1"));
    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "hello");
}

#[test]
fn enter_with_provider_api_key_returns_conversation_turn_effect_with_direct_key() {
    let provider = ModelProvider::new(
        "remote",
        ProviderKind::OpenAiCompatible,
        "Remote",
        Some("https://api.example.com/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )
    .with_api_key(Some(ProviderApiKey::new("sk-test-direct")));
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: ModelCatalog::new(vec![provider]),
            selected_model: Some(ModelSelection::new("remote", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn {
        request,
    }) = effect
    else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert_eq!(
        request.api_key().map(ProviderApiKey::as_str),
        Some("sk-test-direct")
    );
    assert_eq!(request.api_key_env(), None);
}

#[test]
fn enter_with_openai_compatible_provider_without_base_url_keeps_draft_unsent() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
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
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );
    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::DetectedPalette {
        palette: terminal_default_palette(),
        has_dark_background: true,
    });

    for character in "hello".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert!(effect.is_none());
    assert_eq!(model.composer_text(), "hello");
    assert_eq!(model.transcript_plain_items().len(), 1);
}

#[test]
fn clear_command_removes_previous_conversation_context() {
    let mut model = Model::new_with_options(
        StartupBannerOptions::default(),
        ModelOptions {
            model_catalog: single_model_catalog(),
            selected_model: Some(ModelSelection::new("local", "qwen3")),
            requires_model_selection: true,
            ..ModelOptions::default()
        },
    );

    for character in "old context".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    for character in "/clear".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));
    for character in "fresh question".chars() {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    let effect = model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    let Some(AppEffect::SendConversationTurn {
        request,
    }) = effect
    else {
        panic!("expected conversation turn effect, got {effect:?}");
    };
    assert!(request.is_user_message());
    assert_eq!(request.message_text(), "fresh question");
}

fn single_model_catalog() -> ModelCatalog {
    ModelCatalog::new(vec![ModelProvider::new(
        "local",
        ProviderKind::OpenAiCompatible,
        "Local",
        Some("http://127.0.0.1:1234/v1".to_string()),
        ModelSource::Configured,
        vec![ModelEntry::new("qwen3", None, ModelSource::Configured)],
    )])
}

#[test]
fn foreground_fallback_uses_light_palette_when_foreground_is_dark() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::ForegroundColorHint { is_dark: true });

    assert_eq!(model.palette(), &palette_from_background(false, None));
    assert!(model.has_palette());
}

#[test]
fn startup_timeout_allows_initial_frame_without_detected_palette() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });
    model.update(AppEvent::StartupReadyTimeout);

    assert_eq!(model.palette(), &terminal_default_palette());
    assert!(model.is_ready());

    let rendered = buffer_text(&render_model_buffer(&mut model, 80, 24));
    assert!(
        !rendered.trim().is_empty(),
        "ready model should produce visible frame content"
    );
}

#[test]
fn enter_submits_raw_message_and_clears_the_composer() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::Resized {
        width: 80,
        height: 24,
    });

    for character in [' ', ' ', 'h', 'i', ' ', ' '] {
        model.update(AppEvent::Key(KeyEvent::from(KeyCode::Char(character))));
    }
    model.update(AppEvent::Key(KeyEvent::from(KeyCode::Enter)));

    assert_eq!(model.composer_text(), "");

    let items = model.transcript_plain_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1], "›   hi  ");
}

#[test]
fn first_ctrl_c_shows_exit_confirmation_instead_of_quitting_immediately() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert!(!model.is_quitting());
}

#[test]
fn second_ctrl_c_quits_while_confirmation_is_active() {
    let mut model = Model::new(StartupBannerOptions::default());

    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    model.update(AppEvent::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));

    assert!(model.is_quitting());
}

#[test]
fn terminal_replay_items_use_the_current_width_for_the_startup_banner() {
    let mut model = Model::new(StartupBannerOptions {
        app_name: Some("L".repeat(120)),
        ..StartupBannerOptions::default()
    });

    model.update(AppEvent::Resized {
        width: 20,
        height: 8,
    });

    let items = model.terminal_replay_items(false);
    assert_eq!(items.len(), 1);

    for line in items[0].split('\n') {
        assert!(line.chars().count() <= 20, "line exceeded width: {line}");
    }
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut rendered = String::new();

    for row in 0..buffer.area.height {
        for column in 0..buffer.area.width {
            rendered.push_str(buffer[(column, row)].symbol());
        }
        rendered.push('\n');
    }

    rendered
}

fn render_model_buffer(model: &mut Model, width: u16, height: u16) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buffer = Buffer::empty(area);
    let _ = model.render_to_buffer(area, &mut buffer);
    buffer
}
