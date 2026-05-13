/// `RuntimePermissionRequest` 是 runtime 向 TUI 发出的通用权限确认请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePermissionRequest {
    pub request_id: String,
    pub title: Option<String>,
    pub options: Vec<RuntimePermissionOption>,
}

impl RuntimePermissionRequest {
    /// `new` 创建通用 runtime 权限确认请求。
    pub fn new(
        request_id: impl Into<String>,
        title: Option<String>,
        options: Vec<RuntimePermissionOption>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            title,
            options,
        }
    }

    /// `option_id_for` 返回指定类型的第一个 option id。
    pub fn option_id_for(&self, kind: RuntimePermissionOptionKind) -> Option<String> {
        self.options
            .iter()
            .find(|option| option.kind == kind)
            .map(|option| option.option_id.clone())
    }

    /// `reject_for_cancel` 返回取消/丢弃请求时应优先使用的拒绝选项。
    pub fn reject_for_cancel(&self) -> Option<String> {
        self.option_id_for(RuntimePermissionOptionKind::RejectOnce)
            .or_else(|| self.option_id_for(RuntimePermissionOptionKind::RejectAlways))
    }
}

/// `RuntimePermissionOption` 描述权限确认中的一个可选动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: RuntimePermissionOptionKind,
}

impl RuntimePermissionOption {
    /// `new` 创建通用 runtime 权限选项。
    pub fn new(
        option_id: impl Into<String>,
        name: impl Into<String>,
        kind: RuntimePermissionOptionKind,
    ) -> Self {
        Self {
            option_id: option_id.into(),
            name: name.into(),
            kind,
        }
    }
}

/// `RuntimePermissionOptionKind` 用于 TUI 识别允许/拒绝的语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimePermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    Unknown,
}
