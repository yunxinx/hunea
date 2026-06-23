/// `SessionLoadRequestId` 标识一次由 TUI 发起的 session 视图异步加载。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionLoadRequestId(u64);

impl SessionLoadRequestId {
    /// `new` 从调用方维护的单调序列创建请求标识。
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}
