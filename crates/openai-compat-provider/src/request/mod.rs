mod body;
mod content;
mod projection;
mod validation;

pub(crate) use body::{chat_completion_request_body, responses_request_body};
pub use projection::{
    OpenAiRequestFormat, PromptRequestProjection, prompt_request_projection,
    prompt_request_projection_for_format, prompt_request_projection_from_parts,
    prompt_request_projection_from_parts_for_format,
};

#[cfg(test)]
mod tests;
