//! 全局配置目录可读 + 可写检测。

use std::{
    fs, io,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use runtime_domain::paths::hunea_config_dir;

/// 目录可访问性检测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Accessibility {
    /// 可读 + 可写
    Available,
    /// 不可读或不可写
    Unavailable {
        read_error: Option<String>,
        write_error: Option<String>,
    },
}

/// 探测全局配置目录（`~/.config/hunea/`）的可访问性。
///
/// 预检只关心**目录**能否读写，不读其中的 config.toml 内容。
/// 单文件权限问题由后续 `load_with_resolution` 降级为 warning 处理。
/// 若全局配置目录无法解析（如 HOME 未设置），返回 `Unavailable`。
pub(crate) fn probe_global_config_dir_accessibility() -> Accessibility {
    let Some(dir) = hunea_config_dir() else {
        return Accessibility::Unavailable {
            read_error: Some("cannot resolve global config directory (HOME not set?)".to_string()),
            write_error: None,
        };
    };
    probe_dir_accessibility(&dir)
}

/// 探测指定目录的可读 + 可写性。供测试与 `probe_global_config_dir_accessibility` 复用。
///
/// 目录不存在时尝试 `create_dir_all`（与 session store `open_in` 首启建目录的语义一致）：
/// 创建成功即视为 `Available`，创建失败（如父目录无写权限）才判为 `Unavailable`。
/// 已存在但不可读/不可写时直接判为 `Unavailable`。
pub(crate) fn probe_dir_accessibility(dir: &Path) -> Accessibility {
    let read_error = match fs::read_dir(dir) {
        Ok(_) => None,
        Err(err) if err.kind() == io::ErrorKind::NotFound => match fs::create_dir_all(dir) {
            Ok(()) => match fs::read_dir(dir) {
                Ok(_) => None,
                Err(err) => Some(format_io_error(&err)),
            },
            Err(err) => {
                return Accessibility::Unavailable {
                    read_error: None,
                    write_error: Some(format!("cannot create directory: {err}")),
                };
            }
        },
        Err(err) => Some(format_io_error(&err)),
    };

    let write_error = match probe_writable(dir) {
        Ok(()) => None,
        Err(err) => Some(format_io_error(&err)),
    };

    if read_error.is_none() && write_error.is_none() {
        Accessibility::Available
    } else {
        Accessibility::Unavailable {
            read_error,
            write_error,
        }
    }
}

/// 在目录下创建临时文件验证可写性，成功后立即删除。
///
/// 文件名带 pid + 纳秒时间戳，降低并发探测冲突概率。
/// 这是尽力而为检测（存在 TOCTOU），覆盖“目录只读挂载”等常见场景即可。
fn probe_writable(dir: &Path) -> io::Result<()> {
    let pid = std::process::id();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe_filename = format!(".hunea.access_probe.{pid}.{timestamp}");
    let probe_path = dir.join(&probe_filename);

    fs::write(&probe_path, b"1")?;

    // 清理失败不把探测本身判失败：写成功已证明可写；残留文件最多是噪音。
    if let Err(err) = fs::remove_file(&probe_path) {
        eprintln!(
            "warning: failed to clean up accessibility probe file {}: {err}",
            probe_path.display()
        );
    }

    Ok(())
}

fn format_io_error(err: &io::Error) -> String {
    // Display 已含 "Permission denied (os error 13)" 等完整文案，不再前缀 kind。
    err.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// chmod 0o000 对 root 无效，权限相关测试在 root 下会假绿，故跳过。
    /// 仅测试内联，不进公共 API。
    #[cfg(unix)]
    fn process_euid_is_root() -> bool {
        // SAFETY: geteuid 无参数、无内存副作用。
        unsafe { libc::geteuid() == 0 }
    }

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let unique = format!(
            "{}-{}-{}",
            prefix,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn probe_available_for_existing_writable_dir() {
        let dir = temp_dir("access-available");
        let result = probe_dir_accessibility(&dir);
        assert_eq!(result, Accessibility::Available);
    }

    #[test]
    fn probe_creates_and_reports_available_for_nonexistent_creatable_dir() {
        let parent = temp_dir("access-create-parent");
        let dir = parent.join("hunea-nonexistent-creatable");
        // 确保目标目录不存在
        let _ = fs::remove_dir_all(&dir);

        let result = probe_dir_accessibility(&dir);
        assert_eq!(result, Accessibility::Available);
        // 探测应已创建目录（与 session store 首启建目录语义一致）
        assert!(dir.is_dir(), "dir should be created after probe");
    }

    #[cfg(unix)]
    #[test]
    fn probe_unavailable_when_parent_not_writable() {
        if process_euid_is_root() {
            eprintln!("skipping permission test under root");
            return;
        }

        let parent = temp_dir("access-readonly-parent");
        let dir = parent.join("child");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();

        let result = probe_dir_accessibility(&dir);

        // 恢复权限以便 tempdir 清理
        let _ = fs::set_permissions(&parent, fs::Permissions::from_mode(0o755));

        assert!(
            matches!(result, Accessibility::Unavailable { .. }),
            "expected Unavailable when parent not writable, got {result:?}"
        );
        assert!(
            !dir.exists(),
            "dir should not be created when parent is readonly"
        );
    }

    #[cfg(unix)]
    #[test]
    fn probe_unavailable_for_readonly_dir() {
        if process_euid_is_root() {
            eprintln!("skipping readonly test under root");
            return;
        }

        let dir = temp_dir("access-readonly");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();

        let result = probe_dir_accessibility(&dir);

        // 恢复权限以便 tempdir 清理
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));

        assert!(
            matches!(result, Accessibility::Unavailable { .. }),
            "expected Unavailable for readonly dir, got {result:?}"
        );
    }
}
