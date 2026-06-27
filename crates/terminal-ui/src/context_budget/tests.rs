use crossterm::event::KeyCode;
use runtime_domain::session::ContextBudgetDisplayPayload;

use crate::{Model, ModelOptions, StartupBannerOptions, update::AppEffect, update::AppEvent};

fn ready_model() -> Model {
    Model::new_with_options(StartupBannerOptions::default(), ModelOptions::default())
}

#[test]
fn context_command_emits_open_effect() {
    let mut model = ready_model();
    model.update(AppEvent::Key(KeyCode::Char('/').into()));
    for ch in "context".chars() {
        model.update(AppEvent::Key(KeyCode::Char(ch).into()));
    }
    let effect = model.update(AppEvent::Key(KeyCode::Enter.into()));
    assert_eq!(effect, Some(AppEffect::OpenContextBudget));
}

#[test]
fn context_overlay_header_relative_question_mark() {
    use crate::context_budget::state::header_summary;
    let text = header_summary(
        "local/qwen3",
        ContextBudgetDisplayPayload::Relative { used: 1_200 },
    );
    assert!(text.contains("/ ?"));
}
