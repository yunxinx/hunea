//! 便携模式标记探测：检查 `.hunea/portable.marker` 是否存在。

use std::{fs, io, path::Path};

use runtime_domain::paths::WORKSPACE_HUNEA_DIRNAME;

/// 便携模式标记探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PortableMarkerProbe {
    /// 标记存在，进入便携模式
    Present,
    /// 标记不存在，走全局
    Absent,
    /// 工作区不可访问（连标记文件都无法探测）
    WorkspaceInaccessible,
}

impl PortableMarkerProbe {
    /// 是否检测到便携标记。
    pub(crate) fn is_present(&self) -> bool {
        matches!(self, Self::Present)
    }
}

/// 便携标记文件名。
pub(crate) const PORTABLE_MARKER_FILENAME: &str = "portable.marker";

/// 轻量探测：只判断标记文件是否存在，**不**解析 config.toml。
///
/// 标记独立于 config，就是为了打破“读 config 才能知道是否便携 / 便携才能决定读哪个 config”
/// 的循环依赖。探测结果：
/// - metadata 成功 → Present
/// - NotFound → Absent（含 `.hunea/` 尚未创建的常态）
/// - 其他 IO（权限等） → WorkspaceInaccessible
pub(crate) fn probe_portable_marker(working_dir: &Path) -> PortableMarkerProbe {
    let marker_path = working_dir
        .join(WORKSPACE_HUNEA_DIRNAME)
        .join(PORTABLE_MARKER_FILENAME);
    match fs::metadata(&marker_path) {
        Ok(_) => PortableMarkerProbe::Present,
        Err(err) if err.kind() == io::ErrorKind::NotFound => PortableMarkerProbe::Absent,
        Err(_) => PortableMarkerProbe::WorkspaceInaccessible,
    }
}

/// 创建 `<working_dir>/.hunea/` 目录并写入便携标记文件。
///
/// 成功后 `probe_portable_marker` 会返回 `Present`，后续启动将直接进入便携模式。
/// 已存在时幂等覆盖。
pub(crate) fn write_portable_marker(working_dir: &Path) -> io::Result<()> {
    let hunea_dir = working_dir.join(WORKSPACE_HUNEA_DIRNAME);
    fs::create_dir_all(&hunea_dir)?;
    let marker_path = hunea_dir.join(PORTABLE_MARKER_FILENAME);
    fs::write(&marker_path, b"")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(prefix: &str) -> PathBuf {
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
    fn probe_returns_absent_when_no_marker_exists() {
        let working_dir = temp_dir("probe-absent");
        let result = probe_portable_marker(&working_dir);
        assert_eq!(result, PortableMarkerProbe::Absent);
        assert!(!result.is_present());
    }

    #[test]
    fn probe_returns_present_when_marker_exists() {
        let working_dir = temp_dir("probe-present");
        let hunea_dir = working_dir.join(WORKSPACE_HUNEA_DIRNAME);
        fs::create_dir_all(&hunea_dir).unwrap();
        fs::write(hunea_dir.join(PORTABLE_MARKER_FILENAME), b"").unwrap();

        let result = probe_portable_marker(&working_dir);
        assert_eq!(result, PortableMarkerProbe::Present);
        assert!(result.is_present());
    }

    #[test]
    fn probe_returns_absent_when_hunea_dir_does_not_exist() {
        let working_dir = temp_dir("probe-absent-no-hunea-dir");
        // marker 路径的父目录会因 fs::metadata 不做创建而保持不存在；
        // 但 fs::metadata 对不存在路径返回 NotFound，所以这里是 Absent。
        let result = probe_portable_marker(&working_dir);
        assert_eq!(result, PortableMarkerProbe::Absent);
    }

    #[test]
    fn is_present_is_false_for_absent_and_inaccessible() {
        assert!(!PortableMarkerProbe::Absent.is_present());
        assert!(!PortableMarkerProbe::WorkspaceInaccessible.is_present());
    }

    #[test]
    fn write_marker_creates_hunea_dir_and_marker_file() {
        let working_dir = temp_dir("write-marker-create");
        // 初始无标记
        assert_eq!(
            probe_portable_marker(&working_dir),
            PortableMarkerProbe::Absent
        );

        write_portable_marker(&working_dir).expect("write marker");

        // 标记文件存在
        assert_eq!(
            probe_portable_marker(&working_dir),
            PortableMarkerProbe::Present
        );
        // 目录也被创建
        assert!(working_dir.join(WORKSPACE_HUNEA_DIRNAME).is_dir());
    }

    #[test]
    fn write_marker_is_idempotent() {
        let working_dir = temp_dir("write-marker-idempotent");
        write_portable_marker(&working_dir).expect("first write");
        write_portable_marker(&working_dir).expect("second write");

        assert_eq!(
            probe_portable_marker(&working_dir),
            PortableMarkerProbe::Present
        );
    }
}
