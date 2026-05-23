use serde_json::Value;

const TOOL_RESULT_RAW_VALUE_MARKER: &str = "__lumos_tool_result";
const TOOL_RESULT_RAW_VALUE_VERSION: &str = "v1";
const TOOL_RESULT_RAW_VALUE_CONTENT: &str = "content";
const TOOL_RESULT_RAW_VALUE_DETAILS: &str = "details";

/// `RuntimeToolKind` 是 runtime tool activity 的稳定工具分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeToolKind {
    Read,
    Write,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

/// `RuntimeToolActivityStatus` 是 runtime tool activity 的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuntimeToolActivityStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// `RuntimeToolActivityLocation` 表示 tool activity 关联的文件位置。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeToolActivityLocation {
    pub path: String,
    pub line: Option<u32>,
}

/// `RuntimeToolActivityContent` 表示 tool activity 的富内容片段。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RuntimeToolActivityContent {
    Text(String),
    Image {
        mime_type: String,
        uri: Option<String>,
    },
    Audio {
        mime_type: String,
    },
    ResourceLink {
        uri: String,
        name: String,
        title: Option<String>,
    },
    Resource {
        uri: String,
        mime_type: Option<String>,
        text: Option<String>,
    },
    Diff {
        path: String,
        old_text: Option<String>,
        new_text: String,
    },
    Terminal {
        terminal_id: String,
    },
    Unknown(String),
}

/// `RuntimeToolActivityRawValue` 保留 runtime tool activity 原始 JSON 语义。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeToolActivityRawValue {
    value: Value,
}

impl RuntimeToolActivityRawValue {
    /// `new` 从 JSON value 创建原始值。
    pub fn new(value: Value) -> Self {
        Self { value }
    }

    /// `tool_result` 保留工具原始文本输出，同时附带内部 metadata。
    pub fn tool_result(content: impl Into<String>, details: Option<Value>) -> Self {
        let mut value = serde_json::Map::new();
        value.insert(
            TOOL_RESULT_RAW_VALUE_MARKER.to_string(),
            Value::String(TOOL_RESULT_RAW_VALUE_VERSION.to_string()),
        );
        value.insert(
            TOOL_RESULT_RAW_VALUE_CONTENT.to_string(),
            Value::String(content.into()),
        );
        if let Some(details) = details {
            value.insert(TOOL_RESULT_RAW_VALUE_DETAILS.to_string(), details);
        }

        Self::new(Value::Object(value))
    }

    /// `as_json` 返回未格式化的原始 JSON value。
    pub fn as_json(&self) -> &Value {
        &self.value
    }

    /// `tool_result_details` 返回工具结果携带的内部 metadata。
    pub fn tool_result_details(&self) -> Option<&Value> {
        if !self.is_tool_result_raw_value() {
            return None;
        }

        self.value.get(TOOL_RESULT_RAW_VALUE_DETAILS)
    }

    /// `display_text` 返回适合 transcript 展示的文本。
    pub fn display_text(&self) -> Option<String> {
        if let Some(content) = self.tool_result_content() {
            return (!content.is_empty()).then(|| content.to_string());
        }

        match &self.value {
            Value::Null => None,
            Value::String(value) => (!value.is_empty()).then(|| value.clone()),
            value => {
                Some(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
            }
        }
    }

    /// `token_text` 返回适合 token 估算投影使用的文本。
    pub fn token_text(&self) -> Option<String> {
        self.display_text()
    }

    /// `display_byte_len` 返回展示文本的字节长度。
    pub fn display_byte_len(&self) -> usize {
        self.display_text().map(|text| text.len()).unwrap_or(0)
    }

    /// `string_field` 从对象中读取第一个匹配的字符串字段。
    pub fn string_field(&self, keys: &[&str]) -> Option<String> {
        keys.iter()
            .find_map(|key| self.value.get(*key).and_then(Value::as_str))
            .map(str::to_string)
    }

    fn tool_result_content(&self) -> Option<&str> {
        if !self.is_tool_result_raw_value() {
            return None;
        }

        self.value
            .get(TOOL_RESULT_RAW_VALUE_CONTENT)
            .and_then(Value::as_str)
    }

    fn is_tool_result_raw_value(&self) -> bool {
        self.value
            .get(TOOL_RESULT_RAW_VALUE_MARKER)
            .and_then(Value::as_str)
            == Some(TOOL_RESULT_RAW_VALUE_VERSION)
    }
}

impl From<Value> for RuntimeToolActivityRawValue {
    fn from(value: Value) -> Self {
        Self::new(value)
    }
}

impl From<String> for RuntimeToolActivityRawValue {
    fn from(value: String) -> Self {
        match serde_json::from_str(&value) {
            Ok(json) => Self::new(json),
            Err(_) => Self::new(Value::String(value)),
        }
    }
}

impl From<&str> for RuntimeToolActivityRawValue {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}

/// `RuntimeToolActivity` 表示一次可渲染、可更新的 runtime tool activity。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeToolActivity {
    pub activity_id: String,
    pub title: String,
    pub kind: RuntimeToolKind,
    pub status: RuntimeToolActivityStatus,
    pub content: Vec<RuntimeToolActivityContent>,
    pub locations: Vec<RuntimeToolActivityLocation>,
    pub raw_input: Option<RuntimeToolActivityRawValue>,
    pub raw_output: Option<RuntimeToolActivityRawValue>,
}

/// `RuntimeToolActivityUpdate` 表示 tool activity 的增量更新。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct RuntimeToolActivityUpdate {
    pub activity_id: String,
    pub title: Option<String>,
    pub kind: Option<RuntimeToolKind>,
    pub status: Option<RuntimeToolActivityStatus>,
    pub content: Option<Vec<RuntimeToolActivityContent>>,
    pub locations: Option<Vec<RuntimeToolActivityLocation>>,
    pub raw_input: Option<RuntimeToolActivityRawValue>,
    pub raw_output: Option<RuntimeToolActivityRawValue>,
}

/// `RuntimeTerminalExitStatus` 表示 runtime terminal 命令退出状态。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeTerminalExitStatus {
    pub exit_code: Option<u32>,
    pub signal: Option<String>,
}

/// `RuntimeTerminalSnapshot` 表示 UI 渲染 terminal 嵌入块所需的当前输出快照。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuntimeTerminalSnapshot {
    pub terminal_id: String,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub output: String,
    pub truncated: bool,
    pub exit_status: Option<RuntimeTerminalExitStatus>,
    pub released: bool,
}
