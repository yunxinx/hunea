use std::{
    env, io,
    path::{Path, PathBuf},
};

use super::{
    error::AppConfigError,
    merge::merge_config_file,
    paths::config_dir,
    types::{Config, UserInputStyle},
};

/// `load` 按“用户级配置 -> 当前目录覆盖”的顺序加载配置。
pub fn load() -> Result<Config, AppConfigError> {
    load_with_lookups(env::current_dir, config_dir)
}

/// `load_from_paths` 使用给定目录快照加载配置，便于测试与非标准启动入口复用。
pub fn load_from_paths(
    working_dir: Option<&Path>,
    user_config_dir: Option<&Path>,
) -> Result<Config, AppConfigError> {
    load_from_base_config(
        Config::default_config(),
        working_dir.map(Path::to_path_buf),
        user_config_dir.map(Path::to_path_buf),
    )
}

fn load_with_lookups(
    get_working_dir: impl FnOnce() -> io::Result<PathBuf>,
    get_user_config_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<Config, AppConfigError> {
    let mut config = Config::default_config();
    let working_dir = match get_working_dir() {
        Ok(path) => Some(path),
        Err(_) => {
            config.tui.user_input_style = UserInputStyle::Ms;
            None
        }
    };

    load_from_base_config(config, working_dir, get_user_config_dir())
}

fn load_from_base_config(
    mut config: Config,
    working_dir: Option<PathBuf>,
    user_config_dir: Option<PathBuf>,
) -> Result<Config, AppConfigError> {
    let mut config_paths = Vec::with_capacity(2);
    if let Some(path) = user_config_dir {
        config_paths.push(path.join("config.toml"));
    }
    if let Some(path) = working_dir.as_ref() {
        config_paths.push(path.join(".hunea").join("config.toml"));
    }

    let mut reasoning_content_display_configured = false;
    for path in config_paths {
        config = merge_config_file(config, &path, &mut reasoning_content_display_configured)?;
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::{load_from_paths, load_with_lookups};
    use crate::appconfig::UserInputStyle;
    use std::{
        fs, io,
        path::{Path, PathBuf},
    };

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
    fn load_falls_back_to_ms_when_working_directory_lookup_fails() {
        let config = load_with_lookups(
            || Err(io::Error::other("working directory unavailable")),
            || None,
        )
        .expect("missing working dir should fall back to ms");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Ms);
    }

    #[test]
    fn load_still_uses_user_config_when_working_directory_lookup_fails() {
        let user_config_dir = temp_test_dir("load-user-config-after-cwd-failure");
        write_config(
            &user_config_dir.join("config.toml"),
            "[tui]\nuser_input_style = \"cc\"\n",
        );

        let config = load_with_lookups(
            || Err(io::Error::other("working directory unavailable")),
            || Some(user_config_dir.clone()),
        )
        .expect("user config should still be used");

        assert_eq!(config.tui.user_input_style, UserInputStyle::Cc);
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
