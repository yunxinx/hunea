use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// 工作区数据目录名（相对 working dir）。
///
/// 与全局 `~/.config/hunea/` 对称：config / models / phrases / session 都落在此目录。
pub const WORKSPACE_HUNEA_DIRNAME: &str = ".hunea";

/// 应用主配置文件名（相对 data dir）。
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// 模型目录配置文件名（相对 data dir）。
pub const MODELS_FILE_NAME: &str = "models.toml";

/// 状态行文案配置文件名（相对 data dir）。
pub const PHRASES_FILE_NAME: &str = "phrases.toml";

/// `hunea_config_dir` 返回 hunea 用户级配置根目录。
pub fn hunea_config_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "hunea").map(|dirs| dirs.config_dir().to_path_buf())
}

/// `DataDirResolution` 描述预检阶段决定的数据目录落点。
///
/// 全局模式使用 `~/.config/hunea/`；便携模式 fallback 到工作区 `.hunea/`。
/// 当前 hunea 无独立 cache/data dir，config 与 session data 共用同一目录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataDirResolution {
    /// 全局配置目录 `~/.config/hunea/`
    Global(PathBuf),
    /// 工作区便携目录 `<working_dir>/.hunea/`
    Portable(PathBuf),
}

impl DataDirResolution {
    /// 返回 `config.toml` 所在目录。
    pub fn config_dir(&self) -> &Path {
        match self {
            Self::Global(path) | Self::Portable(path) => path,
        }
    }

    /// 返回 session data 所在目录。
    ///
    /// 当前单目录设计下与 `config_dir` 相同。拆成两个 accessor 是为了
    /// 调用点语义清晰（session store 读 data_dir，配置读 config_dir），
    /// 不是预留分叉逻辑——有真实 cache/data 拆分需求前不要在这里加分支。
    pub fn data_dir(&self) -> &Path {
        self.config_dir()
    }

    /// 返回便携模式是否激活。
    pub const fn is_portable(&self) -> bool {
        matches!(self, Self::Portable(_))
    }

    /// 有序配置文件搜索路径（后加载覆盖先加载）。
    ///
    /// 这是 config / models / phrases 共用的**唯一**路径决议入口，避免三处各自拼路径后漂移。
    ///
    /// - 全局：`[data_dir/<file>, <working_dir>/.hunea/<file>]`；
    ///   `working_dir` 为 `None`（cwd 不可用）时只有 data_dir，不再尝试工作区叠加
    /// - 便携：`[data_dir/<file>]`——便携目录本身就是工作区 `.hunea/`，
    ///   再叠一层工作区路径会重复读同一文件
    ///
    /// 不读工作区根下的同名文件；只认 data dir / `.hunea/` 布局。
    pub fn layered_config_file_paths(
        &self,
        working_dir: Option<&Path>,
        file_name: &str,
    ) -> Vec<PathBuf> {
        match self {
            Self::Global(global_dir) => {
                let mut paths = Vec::with_capacity(2);
                paths.push(global_dir.join(file_name));
                if let Some(working_dir) = working_dir {
                    paths.push(working_dir.join(WORKSPACE_HUNEA_DIRNAME).join(file_name));
                }
                paths
            }
            // 便携模式下 data_dir 已是工作区 `.hunea/`，无需再拼 working_dir。
            Self::Portable(portable_dir) => vec![portable_dir.join(file_name)],
        }
    }
}

/// `resolve_data_dir` 根据工作区与便携标记解析数据目录。
///
/// `portable_marker_present` 由上层预检阶段通过 I/O 探测后传入；
/// 本函数纯类型计算，不做文件系统访问。
///
/// 返回 `None` 当便携模式未激活且全局配置目录无法解析（如无 HOME 环境变量）。
pub fn resolve_data_dir(
    working_dir: &Path,
    portable_marker_present: bool,
) -> Option<DataDirResolution> {
    if portable_marker_present {
        Some(DataDirResolution::Portable(
            working_dir.join(WORKSPACE_HUNEA_DIRNAME),
        ))
    } else {
        hunea_config_dir().map(DataDirResolution::Global)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_resolution_uses_workspace_hunea_dir() {
        let working_dir = Path::new("/tmp/hunea-test-workspace");
        let resolution = resolve_data_dir(working_dir, true).expect("portable mode resolves");

        assert!(resolution.is_portable());
        assert_eq!(
            resolution.config_dir(),
            Path::new("/tmp/hunea-test-workspace/.hunea")
        );
        assert_eq!(resolution.data_dir(), resolution.config_dir());
    }

    #[test]
    fn global_resolution_uses_hunea_config_dir_when_available() {
        let working_dir = Path::new("/tmp/hunea-test-workspace");
        let resolution = resolve_data_dir(working_dir, false);

        if let Some(resolution) = resolution {
            assert!(!resolution.is_portable());
            assert_eq!(resolution.config_dir(), resolution.data_dir());
            assert_eq!(
                resolution.config_dir(),
                hunea_config_dir().expect("global dir should match hunea_config_dir")
            );
        }
    }

    #[test]
    fn portable_marker_dominates_global_lookup() {
        let working_dir = Path::new("/workspace/with-marker");
        let resolution = resolve_data_dir(working_dir, true).expect("portable resolves");

        assert!(resolution.is_portable());
        assert_eq!(
            resolution.config_dir(),
            Path::new("/workspace/with-marker/.hunea")
        );
    }

    #[test]
    fn config_dir_and_data_dir_coincide_under_current_single_dir_design() {
        let portable = DataDirResolution::Portable(PathBuf::from("/ws/.hunea"));
        assert_eq!(portable.config_dir(), portable.data_dir());

        let global = DataDirResolution::Global(PathBuf::from("/home/u/.config/hunea"));
        assert_eq!(global.config_dir(), global.data_dir());
    }

    #[test]
    fn is_portable_distinguishes_variants() {
        assert!(DataDirResolution::Portable(PathBuf::from("/ws/.hunea")).is_portable());
        assert!(!DataDirResolution::Global(PathBuf::from("/home/u/.config/hunea")).is_portable());
    }

    #[test]
    fn layered_paths_global_include_workspace_overlay_when_working_dir_present() {
        let resolution = DataDirResolution::Global(PathBuf::from("/home/u/.config/hunea"));
        let paths = resolution.layered_config_file_paths(Some(Path::new("/ws")), CONFIG_FILE_NAME);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/u/.config/hunea/config.toml"),
                PathBuf::from("/ws/.hunea/config.toml"),
            ]
        );
    }

    #[test]
    fn layered_paths_global_omit_workspace_when_working_dir_absent() {
        let resolution = DataDirResolution::Global(PathBuf::from("/home/u/.config/hunea"));
        let paths = resolution.layered_config_file_paths(None, MODELS_FILE_NAME);
        assert_eq!(
            paths,
            vec![PathBuf::from("/home/u/.config/hunea/models.toml")]
        );
    }

    #[test]
    fn layered_paths_portable_only_uses_data_dir() {
        let resolution = DataDirResolution::Portable(PathBuf::from("/ws/.hunea"));
        let paths = resolution.layered_config_file_paths(Some(Path::new("/ws")), PHRASES_FILE_NAME);
        assert_eq!(paths, vec![PathBuf::from("/ws/.hunea/phrases.toml")]);
    }
}
