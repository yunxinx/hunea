use super::*;
use crate::precheck::accessibility::Accessibility;
use crate::precheck::config_probe::PortableMarkerProbe;
use std::path::Path;

#[test]
fn resolve_non_interactive_present_marker_yields_portable() {
    let working_dir = Path::new("/tmp/hunea-test-workspace");
    let result = resolve_non_interactive(
        Some(working_dir),
        &PortableMarkerProbe::Present,
        &Accessibility::Unavailable {
            read_error: None,
            write_error: None,
        },
    )
    .expect("present marker should resolve to portable");

    assert!(!result.should_exit);
    assert!(result.data_dir_resolution.is_portable());
    assert_eq!(result.working_dir.as_deref(), Some(working_dir));
    assert_eq!(
        result.data_dir_resolution.data_dir(),
        working_dir.join(".hunea")
    );
}

#[test]
fn resolve_non_interactive_absent_marker_with_available_global_yields_global() {
    let working_dir = Path::new("/tmp/hunea-test-workspace");
    let result = resolve_non_interactive(
        Some(working_dir),
        &PortableMarkerProbe::Absent,
        &Accessibility::Available,
    )
    .expect("absent + available should resolve");

    assert!(!result.should_exit);
    assert_eq!(result.working_dir.as_deref(), Some(working_dir));
    assert!(!result.data_dir_resolution.is_portable());
}

#[test]
fn resolve_non_interactive_absent_marker_with_unavailable_global_errors() {
    let working_dir = Path::new("/tmp/hunea-test-workspace");
    let result = resolve_non_interactive(
        Some(working_dir),
        &PortableMarkerProbe::Absent,
        &Accessibility::Unavailable {
            read_error: Some("permission denied".to_string()),
            write_error: None,
        },
    );

    assert!(result.is_err(), "absent + unavailable should error");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("non-interactive"),
        "error should mention non-interactive: {err}"
    );
}

#[test]
fn resolve_non_interactive_workspace_inaccessible_errors() {
    let working_dir = Path::new("/tmp/hunea-test-workspace");
    let result = resolve_non_interactive(
        Some(working_dir),
        &PortableMarkerProbe::WorkspaceInaccessible,
        &Accessibility::Available,
    );

    assert!(result.is_err(), "workspace inaccessible should error");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("workspace"),
        "error should mention workspace: {err}"
    );
}

#[test]
fn resolve_non_interactive_present_marker_with_available_global_still_portable() {
    let working_dir = Path::new("/tmp/hunea-test-workspace");
    let result = resolve_non_interactive(
        Some(working_dir),
        &PortableMarkerProbe::Present,
        &Accessibility::Available,
    )
    .expect("present marker should resolve to portable regardless of global");

    assert!(!result.should_exit);
    assert!(result.data_dir_resolution.is_portable());
}

#[test]
fn resolve_non_interactive_without_working_dir_and_available_global_uses_global() {
    let result = resolve_non_interactive(
        None,
        &PortableMarkerProbe::Absent,
        &Accessibility::Available,
    )
    .expect("cwd missing + global available should resolve");

    assert!(!result.should_exit);
    assert!(result.working_dir.is_none());
    assert!(!result.data_dir_resolution.is_portable());
}

#[test]
fn resolve_non_interactive_without_working_dir_and_unavailable_global_errors() {
    let result = resolve_non_interactive(
        None,
        &PortableMarkerProbe::Absent,
        &Accessibility::Unavailable {
            read_error: Some("denied".to_string()),
            write_error: None,
        },
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("working directory is unavailable"),
        "error should mention missing working directory: {err}"
    );
}
