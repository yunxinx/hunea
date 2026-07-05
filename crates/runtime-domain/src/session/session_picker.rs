/// `SessionPickerRow` 是 TUI session picker 展示与选择所需的 session 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPickerRow {
    pub session_id: String,
    pub title: String,
    pub first_user_message: String,
    pub last_assistant_message: String,
    pub updated_at_ms: i64,
    pub work_dir: String,
    pub size_bytes: Option<u64>,
    pub model: Option<String>,
}
