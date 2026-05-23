use provider_protocol::{ModelDescriptor, ProviderError};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModel>,
}

#[derive(Debug, Deserialize)]
struct OpenAiModel {
    id: String,
}

pub(crate) async fn model_descriptors_from_response(
    response: reqwest::Response,
) -> Result<Vec<ModelDescriptor>, ProviderError> {
    let body = response
        .json::<OpenAiModelsResponse>()
        .await
        .map_err(|source| ProviderError::Protocol(format!("invalid /models response: {source}")))?;

    Ok(model_descriptors_from_body(body))
}

fn model_descriptors_from_body(body: OpenAiModelsResponse) -> Vec<ModelDescriptor> {
    body.data
        .into_iter()
        .map(|model| ModelDescriptor::new(model.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{OpenAiModelsResponse, model_descriptors_from_body};

    #[test]
    fn models_response_projects_model_ids() {
        let body = serde_json::from_str::<OpenAiModelsResponse>(
            r#"{"object":"list","data":[{"id":"gpt-4.1"},{"id":"qwen3"}]}"#,
        )
        .expect("models response should parse");

        let models = model_descriptors_from_body(body);

        assert_eq!(
            models.into_iter().map(|model| model.id).collect::<Vec<_>>(),
            vec!["gpt-4.1", "qwen3"]
        );
    }
}
