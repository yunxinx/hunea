use std::{
    io::{self, BufRead, BufReader, Read},
    str,
};

use futures_util::Stream;
use futures_util::StreamExt as _;
use serde::Deserialize;

use super::{CancellationToken, OpenAiCompatibleError};

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
    collect_chat_completion_stream_with_cancellation(reader, &CancellationToken::default())
}

/// `collect_chat_completion_stream_with_cancellation` 从 SSE 响应中聚合内容并检查取消信号。
pub fn collect_chat_completion_stream_with_cancellation(
    reader: impl Read,
    cancellation: &CancellationToken,
) -> Result<String, OpenAiCompatibleError> {
    let mut content = String::new();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        if cancellation.is_cancelled() {
            return Err(OpenAiCompatibleError::Cancelled);
        }

        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(OpenAiCompatibleError::ReadStream)?;
        if cancellation.is_cancelled() {
            return Err(OpenAiCompatibleError::Cancelled);
        }
        if bytes == 0 {
            break;
        }

        if parse_chat_completion_stream_line(line.trim_end_matches(['\r', '\n']), &mut content)? {
            break;
        }
    }

    Ok(content)
}

/// `collect_chat_completion_stream_chunks_with_cancellation` 从 async SSE chunk 中聚合内容。
pub(crate) async fn collect_chat_completion_stream_chunks_with_cancellation<S>(
    chunks: S,
    cancellation: &CancellationToken,
) -> Result<String, OpenAiCompatibleError>
where
    S: Stream<Item = Result<Vec<u8>, io::Error>>,
{
    let mut content = String::new();
    let mut pending = Vec::new();
    futures_util::pin_mut!(chunks);

    loop {
        if cancellation.is_cancelled() {
            return Err(OpenAiCompatibleError::Cancelled);
        }

        let chunk = tokio::select! {
            _ = cancellation.cancelled() => return Err(OpenAiCompatibleError::Cancelled),
            chunk = chunks.next() => chunk,
        };
        let Some(chunk) = chunk else {
            break;
        };
        let chunk = chunk.map_err(OpenAiCompatibleError::ReadStream)?;
        pending.extend_from_slice(&chunk);

        while let Some(line_end) = pending.iter().position(|byte| *byte == b'\n') {
            let mut line = pending.drain(..=line_end).collect::<Vec<_>>();
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line =
                str::from_utf8(&line).map_err(|_| OpenAiCompatibleError::InvalidStreamEvent)?;
            if parse_chat_completion_stream_line(line, &mut content)? {
                return Ok(content);
            }
        }
    }

    if !pending.is_empty() {
        let line =
            str::from_utf8(&pending).map_err(|_| OpenAiCompatibleError::InvalidStreamEvent)?;
        let _ = parse_chat_completion_stream_line(line.trim_end_matches('\r'), &mut content)?;
    }

    Ok(content)
}

fn parse_chat_completion_stream_line(
    line: &str,
    content: &mut String,
) -> Result<bool, OpenAiCompatibleError> {
    let Some(payload) = line.strip_prefix("data:").map(str::trim) else {
        return Ok(false);
    };
    if payload.is_empty() {
        return Ok(false);
    }
    if payload == "[DONE]" {
        return Ok(true);
    }

    let event: ChatCompletionStreamEvent =
        serde_json::from_str(payload).map_err(|_| OpenAiCompatibleError::InvalidStreamEvent)?;
    for choice in event.choices {
        if let Some(delta) = choice.delta.content {
            content.push_str(&delta);
        }
    }

    Ok(false)
}

#[cfg(test)]
mod tests {
    use std::{io, time::Duration};

    use super::*;

    #[tokio::test]
    async fn async_stream_honors_cancellation_while_waiting_for_next_chunk() {
        let token = CancellationToken::default();
        let (sender, receiver) =
            tokio::sync::mpsc::unbounded_channel::<Result<Vec<u8>, io::Error>>();
        let stream = futures_util::stream::unfold(receiver, |mut receiver| async {
            receiver.recv().await.map(|chunk| (chunk, receiver))
        });

        sender
            .send(Ok(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n".to_vec(),
            ))
            .expect("first stream chunk should be queued");

        let task = tokio::spawn({
            let token = token.clone();
            async move { collect_chat_completion_stream_chunks_with_cancellation(stream, &token).await }
        });
        tokio::time::sleep(Duration::from_millis(10)).await;
        token.cancel();

        let error = tokio::time::timeout(Duration::from_millis(200), task)
            .await
            .expect("cancelled collector should return promptly")
            .expect("collector task should not panic")
            .expect_err("cancelled stream should not wait for another chunk");

        assert_eq!(error.to_string(), "chat completion cancelled");
    }
}
