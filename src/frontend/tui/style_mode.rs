/// `StyleMode` 表示当前 TUI 采用的用户输入样式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum StyleMode {
    #[default]
    Cx,
    Cc,
    Ms,
}

impl StyleMode {
    /// `normalized` 将任意样式值收敛到当前支持的稳定集合。
    pub fn normalized(self) -> Self {
        match self {
            Self::Cc => Self::Cc,
            Self::Ms => Self::Ms,
            Self::Cx => Self::Cx,
        }
    }
}
