use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
};

/// `ExternalEditor` 描述一个可执行的外部编辑器命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalEditor {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub display_name: String,
}

/// `ExternalEditorError` 描述外部编辑器解析或校验失败的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalEditorError {
    NoExternalEditorAvailable,
    ExternalEditorNotFound { command: String },
    ExternalEditorMustWait { command: String },
}

impl fmt::Display for ExternalEditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoExternalEditorAvailable => write!(f, "no external editor available"),
            Self::ExternalEditorNotFound { command } => {
                write!(f, "external editor not found: {command}")
            }
            Self::ExternalEditorMustWait { command } => {
                write!(f, "external editor must wait for close: {command}")
            }
        }
    }
}

impl std::error::Error for ExternalEditorError {}

/// `resolve_external_editor` 根据显式配置或系统环境解析可用的外部编辑器。
pub fn resolve_external_editor(
    configured: &[String],
) -> Result<ExternalEditor, ExternalEditorError> {
    resolve_external_editor_with(
        configured,
        |key| env::var(key).ok(),
        |command| {
            look_path(command)
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "editor not found"))
        },
        env::consts::OS,
    )
}

/// `validate_configured_external_editor` 校验显式配置的编辑器命令是否会等待关闭。
pub fn validate_configured_external_editor(command: &[String]) -> Result<(), ExternalEditorError> {
    validate_configured_external_editor_with(command, env::consts::OS)
}

fn validate_configured_external_editor_with(
    command: &[String],
    os_name: &str,
) -> Result<(), ExternalEditorError> {
    if command.is_empty() {
        return Ok(());
    }

    let executable = command[0].trim();
    if executable.is_empty() {
        return Ok(());
    }

    if !command_blocks_until_close(executable, &command[1..], os_name) {
        return Err(ExternalEditorError::ExternalEditorMustWait {
            command: executable.to_string(),
        });
    }

    Ok(())
}

fn resolve_external_editor_with(
    configured: &[String],
    getenv: impl Fn(&str) -> Option<String>,
    look_path: impl Fn(&str) -> io::Result<PathBuf>,
    os_name: &str,
) -> Result<ExternalEditor, ExternalEditorError> {
    let mut rejected_environment_editor = false;

    if !configured.is_empty() {
        let editor = resolve_editor_argv(configured, &look_path, true)?;
        if !editor_blocks_until_close(&editor, os_name) {
            return Err(ExternalEditorError::ExternalEditorMustWait {
                command: configured[0].clone(),
            });
        }

        return Ok(editor);
    }

    for key in ["VISUAL", "EDITOR"] {
        let Some(command_line) = getenv(key) else {
            continue;
        };
        let command_line = command_line.trim();
        if command_line.is_empty() {
            continue;
        }

        let argv = match split_command_line(command_line, os_name) {
            Ok(argv) if !argv.is_empty() => argv,
            _ => {
                rejected_environment_editor = true;
                continue;
            }
        };

        let editor = resolve_editor_argv(&argv, &look_path, false)
            .or_else(|_| resolve_editor_argv_with_spaced_executable(&argv, &look_path));
        let Ok(editor) = editor else {
            rejected_environment_editor = true;
            continue;
        };
        if !editor_blocks_until_close(&editor, os_name) {
            rejected_environment_editor = true;
            continue;
        }

        return Ok(editor);
    }

    for candidate in platform_editor_candidates(os_name) {
        let editor = resolve_editor_argv(&candidate, &look_path, false);
        let Ok(editor) = editor else {
            continue;
        };
        if rejected_environment_editor && is_environment_editor_wrapper(&editor.display_name) {
            continue;
        }
        if !editor_blocks_until_close(&editor, os_name) {
            continue;
        }

        return Ok(editor);
    }

    Err(ExternalEditorError::NoExternalEditorAvailable)
}

fn resolve_editor_argv(
    argv: &[String],
    look_path: &impl Fn(&str) -> io::Result<PathBuf>,
    explicit: bool,
) -> Result<ExternalEditor, ExternalEditorError> {
    if argv.is_empty() || argv[0].trim().is_empty() {
        return Err(ExternalEditorError::NoExternalEditorAvailable);
    }

    let resolved_command = look_path(&argv[0]).map_err(|_| {
        if explicit {
            ExternalEditorError::ExternalEditorNotFound {
                command: argv[0].clone(),
            }
        } else {
            ExternalEditorError::NoExternalEditorAvailable
        }
    })?;

    Ok(ExternalEditor {
        command: resolved_command,
        args: argv[1..].to_vec(),
        display_name: external_editor_command_base(&argv[0]),
    })
}

fn resolve_editor_argv_with_spaced_executable(
    argv: &[String],
    look_path: &impl Fn(&str) -> io::Result<PathBuf>,
) -> Result<ExternalEditor, ExternalEditorError> {
    for executable_token_count in (1..=argv.len()).rev() {
        let executable = argv[..executable_token_count].join(" ");
        let Ok(resolved_command) = look_path(&executable) else {
            continue;
        };

        return Ok(ExternalEditor {
            command: resolved_command,
            args: argv[executable_token_count..].to_vec(),
            display_name: external_editor_command_base(&executable),
        });
    }

    Err(ExternalEditorError::NoExternalEditorAvailable)
}

fn editor_blocks_until_close(editor: &ExternalEditor, os_name: &str) -> bool {
    command_blocks_until_close(&editor.display_name, &editor.args, os_name)
}

fn command_blocks_until_close(command: &str, args: &[String], os_name: &str) -> bool {
    let normalized_command = normalize_external_editor_command_name(command);
    if normalized_command == "open" {
        return os_name == "darwin" && open_args_block_until_close(args);
    }
    if normalized_command == "emacsclient" {
        return !has_any_flag(args, &["-n", "--no-wait"]);
    }

    let Some(required_flags) = auto_detected_editor_wait_flags(&normalized_command, os_name) else {
        return true;
    };

    has_any_flag(args, &required_flags)
}

fn auto_detected_editor_wait_flags(command: &str, os_name: &str) -> Option<Vec<&'static str>> {
    match command {
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" | "zed" => {
            Some(vec!["-w", "--wait"])
        }
        "subl" | "sublime_text" | "mate" => Some(vec!["-w", "--wait"]),
        "gvim" if os_name != "windows" => Some(vec!["-f", "--nofork"]),
        "mvim" if os_name == "darwin" => Some(vec!["-f", "--nofork"]),
        _ => None,
    }
}

fn has_any_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|arg| {
        flags
            .iter()
            .any(|flag| arg == flag || combined_short_flag_contains(arg, flag))
    })
}

fn combined_short_flag_contains(arg: &str, flag: &str) -> bool {
    if flag.len() != 2 || !flag.starts_with('-') || flag.starts_with("--") {
        return false;
    }
    if !arg.starts_with('-') || arg.starts_with("--") || arg.len() <= 2 {
        return false;
    }

    arg[1..].contains(flag.chars().nth(1).unwrap_or_default())
}

fn open_args_block_until_close(args: &[String]) -> bool {
    let mut has_wait_flag = false;
    let mut has_new_instance_flag = false;

    for arg in args {
        match arg.as_str() {
            "-W" => {
                has_wait_flag = true;
                continue;
            }
            "-n" => {
                has_new_instance_flag = true;
                continue;
            }
            _ => {}
        }

        if !arg.starts_with('-') || arg.starts_with("--") {
            continue;
        }

        for flag in arg[1..].chars() {
            match flag {
                'W' => has_wait_flag = true,
                'n' => has_new_instance_flag = true,
                _ => {}
            }
        }
    }

    has_wait_flag && has_new_instance_flag
}

fn normalize_external_editor_command_name(command: &str) -> String {
    let base = external_editor_command_base(command);
    let lowercase = base.to_ascii_lowercase();
    lowercase
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .trim_end_matches(".bat")
        .trim_end_matches(".com")
        .to_string()
}

fn external_editor_command_base(command: &str) -> String {
    command
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_string()
}

fn is_environment_editor_wrapper(command: &str) -> bool {
    matches!(
        normalize_external_editor_command_name(command).as_str(),
        "editor" | "sensible-editor"
    )
}

fn terminal_editor_candidates() -> Vec<Vec<String>> {
    [
        "nano", "nvim", "vim", "vi", "micro", "hx", "helix", "kak", "kakoune",
    ]
    .into_iter()
    .map(|command| vec![command.to_string()])
    .collect()
}

fn gui_editor_candidates() -> Vec<Vec<String>> {
    [
        ("code", "--wait"),
        ("code-insiders", "--wait"),
        ("codium", "--wait"),
        ("cursor", "--wait"),
        ("windsurf", "--wait"),
        ("zed", "--wait"),
        ("subl", "-w"),
        ("sublime_text", "-w"),
        ("mate", "-w"),
    ]
    .into_iter()
    .map(|(command, flag)| vec![command.to_string(), flag.to_string()])
    .collect()
}

fn gui_editor_candidates_for_os(os_name: &str) -> Vec<Vec<String>> {
    let mut candidates = gui_editor_candidates();
    if os_name != "windows" {
        candidates.push(vec!["gvim".to_string(), "-f".to_string()]);
    }
    if os_name == "darwin" {
        candidates.push(vec!["mvim".to_string(), "-f".to_string()]);
    }
    candidates
}

fn platform_editor_candidates(os_name: &str) -> Vec<Vec<String>> {
    match os_name {
        "windows" => {
            let mut candidates = gui_editor_candidates_for_os(os_name);
            candidates.push(vec!["notepad".to_string()]);
            candidates
        }
        "darwin" => {
            let mut candidates = terminal_editor_candidates();
            candidates.extend(gui_editor_candidates_for_os(os_name));
            candidates.push(vec![
                "open".to_string(),
                "-W".to_string(),
                "-n".to_string(),
                "-t".to_string(),
            ]);
            candidates
        }
        _ => {
            let mut candidates = terminal_editor_candidates();
            candidates.extend(gui_editor_candidates_for_os(os_name));
            candidates.push(vec!["sensible-editor".to_string()]);
            candidates.push(vec!["editor".to_string()]);
            candidates
        }
    }
}

fn split_command_line(command: &str, os_name: &str) -> Result<Vec<String>, &'static str> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut in_single_quotes = false;
    let mut in_double_quotes = false;
    let mut escaped = false;
    let mut token_started = false;
    let uses_backslash_escape = os_name != "windows";

    let flush = |argv: &mut Vec<String>, current: &mut String, token_started: &mut bool| {
        if !*token_started && current.is_empty() {
            return;
        }

        argv.push(std::mem::take(current));
        *token_started = false;
    };

    for character in command.chars() {
        match () {
            _ if escaped => {
                token_started = true;
                current.push(character);
                escaped = false;
            }
            _ if uses_backslash_escape && character == '\\' && !in_single_quotes => {
                token_started = true;
                escaped = true;
            }
            _ if character == '\'' && !in_double_quotes => {
                token_started = true;
                in_single_quotes = !in_single_quotes;
            }
            _ if character == '"' && !in_single_quotes => {
                token_started = true;
                in_double_quotes = !in_double_quotes;
            }
            _ if matches!(character, ' ' | '\t') && !in_single_quotes && !in_double_quotes => {
                flush(&mut argv, &mut current, &mut token_started);
            }
            _ => {
                token_started = true;
                current.push(character);
            }
        }
    }

    if escaped || in_single_quotes || in_double_quotes {
        return Err("unterminated editor command");
    }

    flush(&mut argv, &mut current, &mut token_started);
    Ok(argv)
}

fn look_path(command: &str) -> Option<PathBuf> {
    if command.is_empty() {
        return None;
    }

    let path = Path::new(command);
    if path.components().count() > 1 || path.is_absolute() {
        return is_executable_file(path).then(|| path.to_path_buf());
    }

    let search_paths = env::var_os("PATH")?;
    let executable_suffixes = executable_suffixes();
    for directory in env::split_paths(&search_paths) {
        for suffix in &executable_suffixes {
            let candidate = directory.join(format!("{command}{suffix}"));
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

#[cfg(windows)]
fn executable_suffixes() -> Vec<String> {
    let mut suffixes = vec![String::new()];
    if let Some(path_exts) = env::var_os("PATHEXT") {
        for extension in
            env::split_paths(&path_exts).flat_map(|path| path.to_str().map(str::to_string))
        {
            suffixes.push(extension);
        }
    }
    suffixes
}

#[cfg(not(windows))]
fn executable_suffixes() -> Vec<String> {
    vec![String::new()]
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_external_editor_uses_configured_command() {
        let editor = resolve_external_editor_with(
            &["code".into(), "--wait".into()],
            |_| None,
            |command| match command {
                "code" => Ok(PathBuf::from("/usr/bin/code")),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            },
            "linux",
        )
        .expect("configured editor should resolve");

        assert_eq!(editor.command, PathBuf::from("/usr/bin/code"));
        assert_eq!(editor.args, vec!["--wait"]);
        assert_eq!(editor.display_name, "code");
    }

    #[test]
    fn resolve_external_editor_rejects_non_blocking_configured_gui_editor() {
        let error = resolve_external_editor_with(
            &["code".into()],
            |_| None,
            |command| match command {
                "code" => Ok(PathBuf::from("/usr/bin/code")),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            },
            "linux",
        )
        .expect_err("missing wait flag should be rejected");

        assert_eq!(
            error,
            ExternalEditorError::ExternalEditorMustWait {
                command: "code".to_string(),
            }
        );
    }

    #[test]
    fn resolve_external_editor_parses_visual_with_arguments() {
        let editor = resolve_external_editor_with(
            &[],
            |key| {
                if key == "VISUAL" {
                    Some("code --wait --reuse-window".to_string())
                } else {
                    None
                }
            },
            |command| match command {
                "code" => Ok(PathBuf::from("/usr/bin/code")),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            },
            "linux",
        )
        .expect("VISUAL should resolve");

        assert_eq!(editor.args, vec!["--wait", "--reuse-window"]);
    }

    #[test]
    fn resolve_external_editor_parses_spaced_executable_path() {
        let editor_path = "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code";
        let editor = resolve_external_editor_with(
            &[],
            |key| {
                if key == "VISUAL" {
                    Some(format!("{editor_path} --wait"))
                } else {
                    None
                }
            },
            |command| match command {
                value if value == editor_path => Ok(PathBuf::from(editor_path)),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            },
            "darwin",
        )
        .expect("path with spaces should resolve");

        assert_eq!(editor.command, PathBuf::from(editor_path));
        assert_eq!(editor.args, vec!["--wait"]);
        assert_eq!(editor.display_name, "code");
    }

    #[test]
    fn resolve_external_editor_falls_back_to_platform_candidate() {
        let editor = resolve_external_editor_with(
            &[],
            |_| None,
            |command| match command {
                "nano" => Ok(PathBuf::from("/bin/nano")),
                _ => Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            },
            "linux",
        )
        .expect("platform fallback should resolve");

        assert_eq!(editor.command, PathBuf::from("/bin/nano"));
        assert_eq!(editor.display_name, "nano");
    }

    #[test]
    fn validate_configured_external_editor_accepts_open_with_combined_flags_on_darwin() {
        validate_configured_external_editor_with(&["open".into(), "-Wn".into()], "darwin")
            .expect("combined wait and new-instance flags should be accepted");
    }

    #[test]
    fn validate_configured_external_editor_rejects_open_without_new_instance_flag() {
        let error = validate_configured_external_editor_with(
            &["open".into(), "-W".into(), "-t".into()],
            "darwin",
        )
        .expect_err("open must include -n on macOS");

        assert_eq!(
            error,
            ExternalEditorError::ExternalEditorMustWait {
                command: "open".to_string(),
            }
        );
    }
}
