use provider_protocol::ProviderError;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OpenAiErrorEnvelope {
    error: Option<OpenAiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorBody {
    message: Option<String>,
    #[serde(rename = "type")]
    error_type: Option<String>,
    code: Option<serde_json::Value>,
}

pub(crate) async fn provider_error_from_response(response: reqwest::Response) -> ProviderError {
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<OpenAiErrorEnvelope>(&text)
        .ok()
        .and_then(|envelope| envelope.error)
        .map(|error| {
            let mut parts = Vec::new();
            if let Some(message) = error.message.filter(|message| !message.trim().is_empty()) {
                parts.push(message);
            }
            if let Some(error_type) = error.error_type.filter(|value| !value.trim().is_empty()) {
                parts.push(format!("type={error_type}"));
            }
            if let Some(code) = error.code {
                parts.push(format!("code={code}"));
            }
            parts.join("; ")
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| {
            if text.trim().is_empty() {
                "empty provider error response".to_string()
            } else {
                text
            }
        });

    ProviderError::Provider {
        status: Some(status),
        message,
    }
}
