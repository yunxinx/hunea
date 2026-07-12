/// `MotionMode` 控制TUI装饰性动画的呈现策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionMode {
    #[default]
    Full,
    Reduced,
}

impl MotionMode {
    pub(crate) const fn allows_animation(self) -> bool {
        matches!(self, Self::Full)
    }
}
