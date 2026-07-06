pub mod persistence;

mod mutation;
mod resolution;
mod title;
mod types;

pub use mutation::*;
pub use resolution::{requested_order_sort_key, resolve_prompt_assembly};
pub use title::*;
pub use types::*;

/// 受管 skill-discovery 内容的 generated 区块起始标记。
pub const SKILL_DISCOVERY_GENERATED_START: &str = "<!-- hunea:skill-discovery generated:start -->";
/// 受管 skill-discovery 内容的 generated 区块结束标记。
pub const SKILL_DISCOVERY_GENERATED_END: &str = "<!-- hunea:skill-discovery generated:end -->";
/// 受管 tool-guidelines 内容的 generated 区块起始标记。
pub const TOOL_GUIDELINES_GENERATED_START: &str = "<!-- hunea:tool-guidelines generated:start -->";
/// 受管 tool-guidelines 内容的 generated 区块结束标记。
pub const TOOL_GUIDELINES_GENERATED_END: &str = "<!-- hunea:tool-guidelines generated:end -->";
const CORE_SYSTEM_REFERENCE_ID: &str = "core-system";
const CORE_SYSTEM_TITLE: &str = "Core system prompt";
const DEFAULT_EXTRA_PROMPT_TITLE_PREFIX: &str = "New prompt";

#[cfg(test)]
mod tests;
