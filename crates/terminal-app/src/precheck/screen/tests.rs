use super::*;
use crate::precheck::accessibility::Accessibility;
use crate::precheck::config_probe::PortableMarkerProbe;
use crate::precheck::step::{PrecheckStep, StepState};
use runtime_domain::paths::DataDirResolution;
use std::path::PathBuf;
use terminal_ui::theme::default_palette;
use tool_runtime::builtin::ManagedSearchToolConfig;

fn make_ctx(
    portable_marker: PortableMarkerProbe,
    global_accessibility: Accessibility,
) -> PrecheckContext {
    PrecheckContext {
        working_dir: PathBuf::from("/tmp/hunea-test-workspace"),
        portable_marker,
        global_accessibility,
        managed_search_config: ManagedSearchToolConfig::default(),
        managed_root: PathBuf::from("/tmp/hunea-test-managed-root"),
    }
}

/// 只统计便携模式 step（ConfigAccessibility / PortableModeConfirm / PortableModeRecovery）。
/// SearchToolPrecheck step 不计入，因为它依赖运行环境的工具检测。
fn count_portable_steps(steps: &[PrecheckStep]) -> (usize, usize, usize) {
    let mut config = 0;
    let mut confirm = 0;
    let mut recovery = 0;
    for step in steps {
        match step {
            PrecheckStep::ConfigAccessibility(_) => config += 1,
            PrecheckStep::PortableModeConfirm(_) => confirm += 1,
            PrecheckStep::PortableModeRecovery(_) => recovery += 1,
            PrecheckStep::SearchToolPrecheck(_) => {}
        }
    }
    (config, confirm, recovery)
}

#[test]
fn plan_steps_absent_available_yields_no_portable_steps() {
    let ctx = make_ctx(PortableMarkerProbe::Absent, Accessibility::Available);
    let steps = plan_steps(&ctx, default_palette());
    let (config, confirm, recovery) = count_portable_steps(&steps);
    assert_eq!(config, 0);
    assert_eq!(confirm, 0);
    assert_eq!(recovery, 0);
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
    let (config, confirm, recovery) = count_portable_steps(&steps);
    assert_eq!(config, 1, "should have ConfigAccessibility");
    assert_eq!(confirm, 1, "should have PortableModeConfirm");
    assert_eq!(recovery, 0, "should not have PortableModeRecovery");
}

#[test]
fn plan_steps_present_available_yields_recovery() {
    let ctx = make_ctx(PortableMarkerProbe::Present, Accessibility::Available);
    let steps = plan_steps(&ctx, default_palette());
    let (config, confirm, recovery) = count_portable_steps(&steps);
    assert_eq!(config, 0);
    assert_eq!(confirm, 0);
    assert_eq!(recovery, 1, "should have PortableModeRecovery");
}

#[test]
fn plan_steps_present_unavailable_yields_no_portable_steps() {
    let ctx = make_ctx(
        PortableMarkerProbe::Present,
        Accessibility::Unavailable {
            read_error: None,
            write_error: None,
        },
    );
    let steps = plan_steps(&ctx, default_palette());
    let (config, confirm, recovery) = count_portable_steps(&steps);
    assert_eq!(config, 0);
    assert_eq!(confirm, 0);
    assert_eq!(recovery, 0);
}

#[test]
fn plan_steps_workspace_inaccessible_yields_no_portable_steps() {
    let ctx = make_ctx(
        PortableMarkerProbe::WorkspaceInaccessible,
        Accessibility::Available,
    );
    let steps = plan_steps(&ctx, default_palette());
    let (config, confirm, recovery) = count_portable_steps(&steps);
    assert_eq!(config, 0);
    assert_eq!(confirm, 0);
    assert_eq!(recovery, 0);
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
    // 便携模式 step 排在搜索工具 step 之前。
    let first_portable = steps
        .iter()
        .find(|s| !matches!(s, PrecheckStep::SearchToolPrecheck(_)));
    let second_portable = steps
        .iter()
        .filter(|s| !matches!(s, PrecheckStep::SearchToolPrecheck(_)))
        .nth(1);
    assert!(matches!(
        first_portable,
        Some(PrecheckStep::ConfigAccessibility(_))
    ));
    assert!(matches!(
        second_portable,
        Some(PrecheckStep::PortableModeConfirm(_))
    ));
}

#[test]
fn portable_mode_steps_start_in_progress() {
    let ctx = make_ctx(
        PortableMarkerProbe::Absent,
        Accessibility::Unavailable {
            read_error: None,
            write_error: None,
        },
    );
    let steps = plan_steps(&ctx, default_palette());
    for step in &steps {
        if !matches!(step, PrecheckStep::SearchToolPrecheck(_)) {
            assert_eq!(
                step.step_state(),
                StepState::InProgress,
                "portable mode step should start InProgress"
            );
        }
    }
}

#[test]
fn available_global_screen_needs_no_interaction() {
    // 全局可用 + 明确拒绝 managed 工具时，不应有任何 step（正常启动不弹 TUI）。
    // 用拒绝避免环境依赖：未拒绝时 detect_managed_tool_status 在无 rg/fd 环境返回
    // NeedsDownload 导致 search tool step 加入，使 needs_interaction() 误判为 true。
    let ctx = PrecheckContext {
        working_dir: PathBuf::from("/tmp/hunea-test-workspace"),
        portable_marker: PortableMarkerProbe::Absent,
        global_accessibility: Accessibility::Available,
        managed_search_config: ManagedSearchToolConfig {
            allow_managed_rg: Some(false),
            allow_managed_fd: Some(false),
        },
        managed_root: PathBuf::from("/tmp/hunea-test-managed-root"),
    };
    let screen = PrecheckScreen::new(
        &ctx,
        DataDirResolution::Global(PathBuf::from("/tmp/global-hunea")),
    );
    assert!(
        !screen.needs_interaction(),
        "normal startup must not open precheck TUI when managed tools are rejected"
    );
}
