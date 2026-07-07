use provider_protocol::ProviderError;

/// `OpenAiSseDecoder` 将任意 byte chunk 解码为完整的 SSE `data:` frame。
#[derive(Debug, Default)]
pub(crate) struct OpenAiSseDecoder {
    pending: Vec<u8>,
    event_name: Option<String>,
    event_data: Vec<String>,
}

impl OpenAiSseDecoder {
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>, ProviderError> {
        self.pending.extend_from_slice(chunk);
        let mut frames = Vec::new();

        while let Some(newline_index) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line = self.pending.drain(..=newline_index).collect::<Vec<_>>();
            let line = trim_line_end(&line);
            self.apply_line(line, &mut frames)?;
        }

        Ok(frames)
    }

    pub(crate) fn finish(&mut self) -> Result<Vec<String>, ProviderError> {
        let mut frames = Vec::new();
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let line = trim_line_end(&pending);
            self.apply_line(line, &mut frames)?;
        }
        self.emit_event_if_complete(&mut frames);
        Ok(frames)
    }

    fn apply_line(&mut self, line: &[u8], frames: &mut Vec<String>) -> Result<(), ProviderError> {
        if line.is_empty() {
            self.emit_event_if_complete(frames);
            return Ok(());
        }

        let line = std::str::from_utf8(line).map_err(|source| {
            ProviderError::Protocol(format!("invalid SSE UTF-8 line: {source}"))
        })?;
        if line.starts_with(':') {
            return Ok(());
        }

        if line == "data" {
            self.event_data.push(String::new());
            return Ok(());
        }
        if line == "event" {
            self.event_name = Some(String::new());
            return Ok(());
        }
        let Some(data) = line.strip_prefix("data:") else {
            if let Some(event_name) = line.strip_prefix("event:") {
                self.event_name = Some(sse_field_value(event_name).to_string());
            }
            return Ok(());
        };
        self.event_data.push(sse_field_value(data).to_string());
        Ok(())
    }

    fn emit_event_if_complete(&mut self, frames: &mut Vec<String>) {
        let event_name = self.event_name.take();
        if self.event_data.is_empty() {
            return;
        }
        let data = std::mem::take(&mut self.event_data).join("\n");
        if event_name.as_deref() != Some("keepalive") {
            frames.push(data);
        }
    }
}

fn trim_line_end(mut line: &[u8]) -> &[u8] {
    while matches!(line.last(), Some(b'\n' | b'\r')) {
        line = &line[..line.len() - 1];
    }
    line
}

fn sse_field_value(value: &str) -> &str {
    value.strip_prefix(' ').unwrap_or(value)
}
