//! genai 驱动的 Lumos native agent runtime。
//!
//! 当前阶段先建立单轮 agent 请求边界；工具执行器会在公共工具层具备执行语义后接入。

mod client;
mod error;
mod request;

pub use client::{NativeAgentResponse, send_agent_turn_with_cancellation};
pub use error::NativeAgentError;
pub use request::NativeAgentRequest;
