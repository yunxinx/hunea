use std::sync::Arc;

use mo_core::{provider::ProviderKind, tools::RuntimeToolExecutor};
use rig_core::client::CompletionClient;
use tokio_util::sync::CancellationToken;

use crate::{
    NativeAgentError, NativeAgentRequest,
    agent::{NativeAgentCompletion, NativeAgentProgress},
    llm::{
        NativeLlmError,
        provider::{
            anthropic_client_for_request, cohere_client_for_request, copilot_client_for_request,
            deepseek_client_for_request, gemini_client_for_request, groq_client_for_request,
            ollama_client_for_request, openai_compatible_model_for_request,
            openai_completions_client_for_request, together_client_for_request,
            xai_client_for_request, xiaomi_mimo_client_for_request, zai_client_for_request,
        },
        stream::run_rig_agent,
    },
};

/// `execute_rig_agent_for_request` 使用 Rig agent/streaming/multi-turn 执行一次 native turn。
pub(crate) async fn execute_rig_agent_for_request<F>(
    request: &NativeAgentRequest,
    executor: Arc<dyn RuntimeToolExecutor>,
    cancellation: &CancellationToken,
    on_progress: &mut F,
) -> Result<NativeAgentCompletion, NativeAgentError>
where
    F: FnMut(NativeAgentProgress),
{
    if cancellation.is_cancelled() {
        return Err(NativeAgentError::Cancelled);
    }

    match request.llm_request().provider_kind {
        ProviderKind::OpenAiCompatible => {
            let model = openai_compatible_model_for_request(request.llm_request())?;
            run_rig_agent(model, request, executor, cancellation, on_progress).await
        }
        ProviderKind::OpenAi => {
            let client = openai_completions_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Anthropic => {
            let client = anthropic_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Gemini => {
            let client = gemini_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::DeepSeek => {
            let client = deepseek_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Together => {
            let client = together_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Groq => {
            let client = groq_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Xai => {
            let client = xai_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Ollama => {
            let client = ollama_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Cohere => {
            let client = cohere_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Zai => {
            let client = zai_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Mimo => {
            let client = xiaomi_mimo_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::GithubCopilot => {
            let client = copilot_client_for_request(request.llm_request())?;
            run_rig_agent(
                client.completion_model(request.llm_request().model_id.clone()),
                request,
                executor,
                cancellation,
                on_progress,
            )
            .await
        }
        ProviderKind::Fireworks
        | ProviderKind::OllamaCloud
        | ProviderKind::BigModel
        | ProviderKind::Aliyun
        | ProviderKind::Nebius
        | ProviderKind::Vertex => Err(NativeLlmError::UnsupportedProvider {
            provider_id: request.llm_request().provider_id.clone(),
            provider_kind: request.llm_request().provider_kind,
        }
        .into()),
    }
}
