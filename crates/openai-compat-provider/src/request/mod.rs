mod body;
mod content;
mod projection;
mod validation;

pub(crate) use body::chat_completion_request_body;
pub use projection::{
    PromptRequestProjection, prompt_request_projection, prompt_request_projection_from_parts,
};

#[cfg(test)]
mod tests;
