use std::io::{BufRead, BufReader, Read};

use serde::Deserialize;

use super::OpenAiCompatibleError;

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamEvent {
    choices: Vec<ChatCompletionStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamChoice {
    delta: ChatCompletionStreamDelta,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionStreamDelta {
    content: Option<String>,
}

/// `collect_chat_completion_stream` 从 SSE 响应中聚合 `delta.content`。
pub fn collect_chat_completion_stream(reader: impl Read) -> Result<String, OpenAiCompatibleError> {
    let mut content = String::new();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(OpenAiCompatibleError::ReadStream)?;
        if bytes == 0 {
            break;
        }

        let Some(payload) = line
            .trim_end_matches(['\r', '\n'])
            .strip_prefix("data:")
            .map(str::trim)
        else {
            continue;
        };
        if payload.is_empty() {
            continue;
        }
        if payload == "[DONE]" {
            break;
        }

        let event: ChatCompletionStreamEvent =
            serde_json::from_str(payload).map_err(|_| OpenAiCompatibleError::InvalidStreamEvent)?;
        for choice in event.choices {
            if let Some(delta) = choice.delta.content {
                content.push_str(&delta);
            }
        }
    }

    Ok(content)
}
