use super::*;
use crate::precheck::accessibility::Accessibility;
use crate::precheck::config_probe::PortableMarkerProbe;
use crate::precheck::step::{PrecheckStep, StepState};
use std::path::PathBuf;
use terminal_ui::theme::default_palette;

fn make_ctx(
    portable_marker: PortableMarkerProbe,
    global_accessibility: Accessibility,
) -> PrecheckContext {
    PrecheckContext {
        working_dir: PathBuf::from("/tmp/hunea-test-workspace"),
        portable_marker,
        global_accessibility,
    }
}

fn count_steps_by_variant(steps: &[PrecheckStep]) -> (usize, usize, usize) {
    let mut config = 0;
    let mut confirm = 0;
    let mut recovery = 0;
    for step in steps {
        match step {
            PrecheckStep::ConfigAccessibility(_) => config += 1,
            PrecheckStep::PortableModeConfirm(_) => confirm += 1,
            PrecheckStep::PortableModeRecovery(_) => recovery += 1,
        }
    }
    (config, confirm, recovery)
}

#[test]
fn plan_steps_absent_available_yields_no_steps() {
    let ctx = make_ctx(PortableMarkerProbe::Absent, Accessibility::Available);
    let steps = plan_steps(&ctx, default_palette());
    assert!(steps.is_empty(), "no steps needed when all available");
}

#[test]
fn plan_steps_absent_unavailable_yields_config_then_confirm() {
    let ctx = make_ctx(
        PortableMarkerProbe::Absent,
        Accessibility::Unavailable {
            read_error: Some("denied".to_string()),
            write_error: None,
        },
    );
    let steps = plan_steps(&ctx, default_palette());
    let (config, confirm, recovery) = count_steps_by_variant(&steps);
    assert_eq!(config, 1, "should have ConfigAccessibility");
    assert_eq!(confirm, 1, "should have PortableModeConfirm");
    assert_eq!(recovery, 0, "should not have PortableModeRecovery");
}

#[test]
fn plan_steps_present_available_yields_recovery() {
    let ctx = make_ctx(PortableMarkerProbe::Present, Accessibility::Available);
    let steps = plan_steps(&ctx, default_palette());
    let (config, confirm, recovery) = count_steps_by_variant(&steps);
    assert_eq!(config, 0);
    assert_eq!(confirm, 0);
    assert_eq!(recovery, 1, "should have PortableModeRecovery");
}

#[test]
fn plan_steps_present_unavailable_yields_no_steps() {
    let ctx = make_ctx(
        PortableMarkerProbe::Present,
        Accessibility::Unavailable {
            read_error: None,
            write_error: None,
        },
    );
    let steps = plan_steps(&ctx, default_palette());
    assert!(
        steps.is_empty(),
        "portable mode continues silently when global still unavailable"
    );
}

#[test]
fn plan_steps_workspace_inaccessible_yields_no_steps() {
    let ctx = make_ctx(
        PortableMarkerProbe::WorkspaceInaccessible,
        Accessibility::Available,
    );
    let steps = plan_steps(&ctx, default_palette());
    assert!(steps.is_empty(), "workspace inaccessible yields no steps");
}

#[test]
fn plan_steps_order_config_before_confirm() {
    let ctx = make_ctx(
        PortableMarkerProbe::Absent,
        Accessibility::Unavailable {
            read_error: None,
            write_error: None,
        },
    );
    let steps = plan_steps(&ctx, default_palette());
    assert!(matches!(steps[0], PrecheckStep::ConfigAccessibility(_)));
    assert!(matches!(steps[1], PrecheckStep::PortableModeConfirm(_)));
}

#[test]
fn available_global_screen_needs_no_interaction() {
    let ctx = make_ctx(PortableMarkerProbe::Absent, Accessibility::Available);
    let screen = PrecheckScreen::new(
        &ctx,
        DataDirResolution::Global(PathBuf::from("/tmp/global-hunea")),
    );
    assert!(
        !screen.needs_interaction(),
        "normal startup must not open precheck TUI"
    );
}

#[test]
fn all_steps_start_in_progress() {
    // 所有加入的 step 初始为 InProgress（参见 codex 模式）。
    let ctx = make_ctx(
        PortableMarkerProbe::Absent,
        Accessibility::Unavailable {
            read_error: None,
            write_error: None,
        },
    );
    let steps = plan_steps(&ctx, default_palette());
    for step in &steps {
        assert_eq!(
            step.step_state(),
            StepState::InProgress,
            "step should start InProgress"
        );
    }
}
