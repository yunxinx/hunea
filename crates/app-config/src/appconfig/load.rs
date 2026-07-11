use std::path::{Path, PathBuf};

use runtime_domain::paths::{CONFIG_FILE_NAME, DataDirResolution, WORKSPACE_HUNEA_DIRNAME};

use super::{
    error::AppConfigError,
    merge::{ConfigFileLoadOutcome, merge_config_file},
    types::{Config, UserInputStyle},
};

/// `load_from_paths` 使用给定目录快照加载配置，便于测试与非标准启动入口复用。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<Config, AppConfigError> {
    let (config, _warnings) = load_from_base_config(
        Config::default_config(),
        working_dir.map(Path::to_path_buf),
        user_config_dir.map(Path::to_path_buf),
    )?;
    Ok(config)
}

/// `load_with_resolution` 使用预检阶段决定的数据目录解析结果加载配置。
///
/// 全局模式下读全局 config + 工作区 config 覆盖；
/// 便携模式下只读工作区 config，不读全局（因全局可能不可访问）。
///
/// `working_dir` 为 `None` 时：不叠加工作区 config，并将默认 `user_input_style`
/// 设为 `Ms`（与历史 cwd 不可用行为一致；后续文件 merge 仍可覆盖）。
///
/// 返回加载后的 config 与收集到的可降级错误（如配置文件读取权限错误）。
/// 文件级 Read 错误永不 fatal——项目有内置默认配置；目录可访问性由预检负责。
pub fn load_with_resolution(
    working_dir: Option<&Path>,
    resolution: &DataDirResolution,
) -> Result<(Config, Vec<AppConfigError>), AppConfigError> {
    let mut config = Config::default_config();
    // cwd 不可用时先落到最朴素主题（无 frame 依赖），避免默认 Cx 在残缺环境下渲染异常；
    // 若全局 config.toml 显式配置了 style，后续 merge 仍可覆盖此默认。
    if working_dir.is_none() {
        config.tui.user_input_style = UserInputStyle::Ms;
    }
    let config_paths = resolution.layered_config_file_paths(working_dir, CONFIG_FILE_NAME);
    let warnings = load_from_config_paths(&mut config, config_paths)?;
    Ok((config, warnings))
}

fn load_from_base_config(
    mut config: Config,
    working_dir: Option<PathBuf>,
    user_config_dir: Option<PathBuf>,
) -> Result<(Config, Vec<AppConfigError>), AppConfigError> {
    let mut config_paths = Vec::with_capacity(2);
    if let Some(path) = user_config_dir {
        config_paths.push(path.join(CONFIG_FILE_NAME));
    }
    if let Some(path) = working_dir.as_ref() {
        config_paths.push(path.join(WORKSPACE_HUNEA_DIRNAME).join(CONFIG_FILE_NAME));
    }

    let warnings = load_from_config_paths(&mut config, config_paths)?;
    Ok((config, warnings))
}

/// `load_from_config_paths` 按顺序合并多个配置文件，收集可降级错误。
///
/// 错误分层（与预检职责分工）：
/// - **Decode/Validation** → `Err` fatal：用户写错了配置，必须修
/// - **Read（权限/IO，非 NotFound）** → warning：环境问题，继续尝试下一源
/// - **NotFound** → Skipped：文件可选，用默认值即可
///
/// 全部源都 Read 失败时**不** fatal：`Config::default_config()` 已是完整可用配置。
/// “目录能不能用”由预检（`Accessibility` + 便携模式）决定，不在文件加载层重复判死。
fn load_from_config_paths(
    config: &mut Config,
    config_paths: Vec<PathBuf>,
) -> Result<Vec<AppConfigError>, AppConfigError> {
    let mut warnings = Vec::new();
    let mut reasoning_content_display_configured = false;
    for path in config_paths {
        match merge_config_file(config, &path, &mut reasoning_content_display_configured)? {
            ConfigFileLoadOutcome::Loaded | ConfigFileLoadOutcome::Skipped => {}
            ConfigFileLoadOutcome::Downgradable(err) => warnings.push(err),
        }
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::{load_from_paths, load_with_resolution};
    use crate::appconfig::{AppConfigError, MotionMode, UserInputStyle};
    use runtime_domain::paths::DataDirResolution;
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    /// chmod 0o000 对 root 无效，权限相关测试在 root 下会假绿，故跳过。
    /// 仅测试内联，不进公共 API。
    #[cfg(unix)]
    fn process_euid_is_root() -> bool {
        // SAFETY: geteuid 无参数、无内存副作用。
        unsafe { libc::geteuid() == 0 }
    }

    #[test]
    fn load_defaults_to_cx_when_no_config_exists() {
        let working_dir = temp_test_dir("load-defaults-working");
        let user_config_dir = temp_test_dir("load-defaults-config");

        let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
            .expect("missing config files should fall back to defaults");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
    }

    #[test]
    fn load_project_config_overrides_user_config() {
        let working_dir = temp_test_dir("load-project-overrides-working");
        let user_config_dir = temp_test_dir("load-project-overrides-config");
        write_config(
            &user_config_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"ms\"\n",
        );
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"cx\"\n",
        );

        let config = load_from_paths(Some(working_dir.as_path()), Some(user_config_dir.as_path()))
            .expect("project config should override the user config");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
    }

    #[test]
    fn load_accepts_cc_style_mode() {
        let working_dir = temp_test_dir("load-accepts-cc-working");
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"cc\"\n",
        );

        let config = load_from_paths(Some(working_dir.as_path()), None)
            .expect("cc should be accepted as a valid style mode");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
    }

    #[test]
    fn load_defaults_motion_to_full_and_accepts_reduced() {
        let default_working_dir = temp_test_dir("load-default-motion-working");
        let default_config = load_from_paths(Some(default_working_dir.as_path()), None)
            .expect("missing motion config should use full motion");
        assert_eq!(default_config.tui.motion, MotionMode::Full);

        let reduced_working_dir = temp_test_dir("load-reduced-motion-working");
        write_config(
            &reduced_working_dir.join(".hunea").join("config.toml"),
            "[tui]\nmotion = \"reduced\"\n",
        );
        let reduced_config = load_from_paths(Some(reduced_working_dir.as_path()), None)
            .expect("reduced motion should be accepted");
        assert_eq!(reduced_config.tui.motion, MotionMode::Reduced);
    }

    #[test]
    fn load_rejects_unknown_motion_mode() {
        let working_dir = temp_test_dir("load-rejects-motion-working");
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nmotion = \"sometimes\"\n",
        );

        let error = load_from_paths(Some(working_dir.as_path()), None)
            .expect_err("unknown motion mode should be rejected");
        assert!(error.to_string().contains("unknown tui.motion"));
    }

    #[test]
    fn load_rejects_unknown_style_mode() {
        let working_dir = temp_test_dir("load-rejects-style-working");
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"weird\"\n",
        );

        let error = load_from_paths(Some(working_dir.as_path()), None)
            .expect_err("unknown style mode should be rejected");

        assert!(
            error.to_string().contains("unknown tui.user_input_style"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_rejects_unknown_keys() {
        let working_dir = temp_test_dir("load-rejects-keys-working");
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nunknown = true\n",
        );

        let error = load_from_paths(Some(working_dir.as_path()), None)
            .expect_err("unknown keys should fail");

        assert!(
            error.to_string().contains("unknown field"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn load_with_resolution_global_reads_global_then_workspace() {
        let working_dir = temp_test_dir("resolution-global-working");
        let global_dir = temp_test_dir("resolution-global-config");
        write_config(
            &global_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"ms\"\n",
        );
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"cx\"\n",
        );

        let resolution = DataDirResolution::Global(global_dir);
        let (config, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("global resolution should load");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
    }

    #[test]
    fn load_with_resolution_portable_reads_only_workspace() {
        let working_dir = temp_test_dir("resolution-portable-working");
        let global_dir = temp_test_dir("resolution-portable-config");
        write_config(
            &global_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"ms\"\n",
        );
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"cc\"\n",
        );

        let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
        let (config, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("portable resolution should load");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
        assert!(warnings.is_empty(), "no warnings expected: {warnings:?}");
    }

    #[test]
    fn load_with_resolution_portable_without_workspace_config_uses_defaults() {
        let working_dir = temp_test_dir("resolution-portable-defaults");

        let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
        let (config, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("portable resolution with no config should use defaults");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_with_resolution_without_working_dir_uses_ms_default_and_global_only() {
        let global_dir = temp_test_dir("resolution-no-cwd-config");
        write_config(
            &global_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"cc\"\n",
        );

        let resolution = DataDirResolution::Global(global_dir);
        let (config, warnings) =
            load_with_resolution(None, &resolution).expect("global-only load should work");

        // 全局文件覆盖 Ms 默认
        assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_with_resolution_without_working_dir_and_no_files_keeps_ms() {
        let global_dir = temp_test_dir("resolution-no-cwd-empty");
        let resolution = DataDirResolution::Global(global_dir);
        let (config, warnings) =
            load_with_resolution(None, &resolution).expect("defaults should load");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Ms);
        assert!(warnings.is_empty());
    }

    #[test]
    fn load_with_resolution_decode_error_still_fatal() {
        let working_dir = temp_test_dir("resolution-decode-fatal");
        write_config(&working_dir.join(".hunea").join("config.toml"), "[tui\n");

        let resolution = DataDirResolution::Portable(working_dir.join(".hunea"));
        let error = load_with_resolution(Some(&working_dir), &resolution)
            .expect_err("decode error should be fatal");

        assert!(
            error.to_string().contains("decode config file"),
            "unexpected error: {error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_with_resolution_global_permission_error_downgrades_and_continues() {
        if process_euid_is_root() {
            eprintln!("skipping permission test under root");
            return;
        }

        let working_dir = temp_test_dir("resolution-perm-working");
        let global_dir = temp_test_dir("resolution-perm-config");
        write_config(
            &global_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"ms\"\n",
        );
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"cx\"\n",
        );

        use std::os::unix::fs::PermissionsExt;
        let unreadable_path = global_dir.join("config.toml");
        fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o000))
            .expect("chmod should work");

        let resolution = DataDirResolution::Global(global_dir);
        let (config, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("permission error should downgrade, not fatal");

        let _ = fs::set_permissions(&unreadable_path, fs::Permissions::from_mode(0o644));

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
        assert_eq!(warnings.len(), 1, "expected one warning: {warnings:?}");
        assert!(matches!(warnings[0], AppConfigError::Read { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn load_with_resolution_all_sources_unreadable_uses_defaults_with_warnings() {
        if process_euid_is_root() {
            eprintln!("skipping permission test under root");
            return;
        }

        let working_dir = temp_test_dir("resolution-all-downgradable-working");
        let global_dir = temp_test_dir("resolution-all-downgradable-config");
        write_config(
            &global_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"ms\"\n",
        );
        write_config(
            &working_dir.join(".hunea").join("config.toml"),
            "[tui]\nuser_input_style = \"cx\"\n",
        );

        use std::os::unix::fs::PermissionsExt;
        let global_path = global_dir.join("config.toml");
        let workspace_path = working_dir.join(".hunea").join("config.toml");
        fs::set_permissions(&global_path, fs::Permissions::from_mode(0o000))
            .expect("chmod should work");
        fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o000))
            .expect("chmod should work");

        let resolution = DataDirResolution::Global(global_dir);
        let (config, warnings) = load_with_resolution(Some(&working_dir), &resolution)
            .expect("unreadable files should fall back to defaults");

        let _ = fs::set_permissions(&global_path, fs::Permissions::from_mode(0o644));
        let _ = fs::set_permissions(&workspace_path, fs::Permissions::from_mode(0o644));

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cx);
        assert_eq!(warnings.len(), 2, "expected two warnings: {warnings:?}");
        assert!(
            warnings
                .iter()
                .all(|w| matches!(w, AppConfigError::Read { .. }))
        );
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(format!("hunea-rust-{prefix}-{unique}"));
        fs::create_dir_all(&path).expect("temp test dir should be created");
        path
    }

    fn write_config(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("config parent dir should exist");
        }
        fs::write(path, content).expect("config file should be written");
    }
}
