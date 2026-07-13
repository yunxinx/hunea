//! 判定当前环境是否应抑制 kitty keyboard enhancement。
//!
//! `KeyboardEnhancementPreference::On`/`Off` 直接生效；`Auto` 时 WSL 内的 VSCode
//! 终端自动禁用——该组合对 CSI-u 序列的处理不可靠，且 VSCode 的 WSL shell 可能
//! 向 Linux 进程隐藏 `TERM_PROGRAM`，因此还需要经 WSL interop 探测 Windows 侧环境。

use crate::model::KeyboardEnhancementPreference;

/// 是否应跳过 keyboard enhancement 的 Push/Pop。
pub(crate) fn keyboard_enhancement_suppressed(preference: KeyboardEnhancementPreference) -> bool {
    match preference {
        KeyboardEnhancementPreference::On => false,
        KeyboardEnhancementPreference::Off => true,
        // && 短路：VSCode 探测可能 spawn interop 子进程，只在 WSL 下才值得付出这个成本。
        KeyboardEnhancementPreference::Auto => running_in_wsl() && running_in_vscode_terminal(),
    }
}

fn running_in_wsl() -> bool {
    wsl_detected(wsl_env_present(), linux_kernel_osrelease().as_deref())
}

fn wsl_env_present() -> bool {
    std::env::var_os("WSL_DISTRO_NAME").is_some() || std::env::var_os("WSL_INTEROP").is_some()
}

fn linux_kernel_osrelease() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/osrelease").ok()
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn wsl_detected(has_wsl_env: bool, kernel_osrelease: Option<&str>) -> bool {
    has_wsl_env
        || kernel_osrelease
            .is_some_and(|release| release.to_ascii_lowercase().contains("microsoft"))
}

fn running_in_vscode_terminal() -> bool {
    vscode_terminal_detected(std::env::var("TERM_PROGRAM").ok().as_deref(), || {
        windows_term_program()
    })
}

/// Windows 侧探测以闭包传入并短路：Linux 侧 `TERM_PROGRAM` 已命中时
/// 不再经 WSL interop spawn `cmd.exe`。
fn vscode_terminal_detected(
    linux_term_program: Option<&str>,
    windows_term_program: impl FnOnce() -> Option<String>,
) -> bool {
    term_program_is_vscode(linux_term_program)
        || term_program_is_vscode(windows_term_program().as_deref())
}

fn term_program_is_vscode(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("vscode"))
}

/// Windows 侧的 `TERM_PROGRAM`。探测要 spawn `cmd.exe`，结果按进程缓存，
/// 避免 suspend/resume 或多次进入终端会话时重复付出子进程开销。
fn windows_term_program() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        static WINDOWS_TERM_PROGRAM: std::sync::OnceLock<Option<String>> =
            std::sync::OnceLock::new();
        WINDOWS_TERM_PROGRAM
            .get_or_init(read_windows_term_program)
            .clone()
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(target_os = "linux")]
fn read_windows_term_program() -> Option<String> {
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/s", "/c", "set TERM_PROGRAM"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.trim_end_matches('\r')
                .strip_prefix("TERM_PROGRAM=")
                .map(str::to_string)
        })
        .filter(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        KeyboardEnhancementPreference, keyboard_enhancement_suppressed, term_program_is_vscode,
        vscode_terminal_detected, wsl_detected,
    };

    #[test]
    fn explicit_preference_overrides_environment_detection() {
        assert!(!keyboard_enhancement_suppressed(
            KeyboardEnhancementPreference::On
        ));
        assert!(keyboard_enhancement_suppressed(
            KeyboardEnhancementPreference::Off
        ));
    }

    #[test]
    fn wsl_detection_accepts_env_marker_or_microsoft_kernel_release() {
        assert!(wsl_detected(true, None));
        assert!(wsl_detected(
            false,
            Some("5.15.167.4-microsoft-standard-WSL2")
        ));
        assert!(!wsl_detected(false, Some("6.12.94+deb13-amd64")));
        assert!(!wsl_detected(false, None));
    }

    #[test]
    fn vscode_detection_uses_linux_or_windows_term_program() {
        assert!(vscode_terminal_detected(Some("vscode"), || None));
        assert!(vscode_terminal_detected(None, || Some(
            "vscode".to_string()
        )));
        assert!(!vscode_terminal_detected(None, || Some(
            "WindowsTerminal".to_string()
        )));
        assert!(!vscode_terminal_detected(None, || None));
    }

    #[test]
    fn vscode_detection_skips_windows_probe_when_linux_side_matches() {
        assert!(vscode_terminal_detected(Some("vscode"), || {
            panic!("Linux 侧已命中时不应触发 Windows 侧探测")
        }));
    }

    #[test]
    fn term_program_match_is_case_insensitive() {
        assert!(term_program_is_vscode(Some("VSCode")));
        assert!(!term_program_is_vscode(Some("vscode-insiders")));
        assert!(!term_program_is_vscode(None));
    }
}
