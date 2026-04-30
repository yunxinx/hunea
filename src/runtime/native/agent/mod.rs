//! genai 驱动的 Lumos native agent runtime。
//!
//! 当前阶段建立单轮请求与最小工具回灌 loop；TUI 权限流会在后续接入。

mod client;
mod error;
mod request;
mod response;
mod session;
mod stream;
mod tool_loop;
mod tool_mapping;

pub use client::send_agent_loop_with_cancellation;
pub use error::NativeAgentError;
pub use request::NativeAgentRequest;
pub use response::NativeAgentResponse;
pub(crate) use session::{NativeAgentEvent, NativeAgentRuntimeState};
