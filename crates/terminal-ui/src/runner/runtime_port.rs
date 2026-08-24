use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent},
};

pub use runtime_domain::runtime_wake::RuntimeWake;

/// 事件 port 只暴露 wake 绑定与 runtime fact drain。
pub trait RuntimeEventPort {
    /// 绑定当前 TUI event loop 的 wake callback。
    fn bind_runtime_wake(&mut self, wake: RuntimeWake) -> Result<(), String>;

    /// 取出 runtime 已发布的事实事件。
    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent>;
}

/// command port 只负责派发用户意图，不暴露 runtime 事实或 capability state。
pub trait RuntimeCommandPort {
    /// 向 runtime 派发控制命令。
    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String>;
}

/// model port 只负责模型选择持久化与 provider catalog refresh。
pub trait ModelRuntimePort {
    /// 取出 model provider refresh 事实事件。
    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent>;

    /// 持久化当前模型选择。
    fn persist_selected_model(&mut self, selection: &ModelSelection) -> Result<(), String>;

    /// 请求刷新指定 provider 的模型目录。
    fn refresh_model_provider(&mut self, request: ProviderSyncRequest) -> Result<(), String>;
}

/// prompt port 只负责 `/prompt` working-copy 的生命周期。
pub trait PromptRuntimePort {
    /// 进入 `/prompt` overlay 时加载 working copy。
    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String>;

    /// 在 prompt working copy 上同步应用 mutation。
    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String>;

    /// 退出 `/prompt` overlay 时提交 working copy。
    fn commit_prompt_assembly_edit(&mut self) -> Result<(), String>;
}

/// `NoopUiRuntimePort` 让纯 TUI 构建可以独立运行到模型更新层。
#[derive(Debug, Default)]
pub struct NoopUiRuntimePort;

impl RuntimeEventPort for NoopUiRuntimePort {
    fn bind_runtime_wake(&mut self, _wake: RuntimeWake) -> Result<(), String> {
        Ok(())
    }

    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        Vec::new()
    }
}

impl RuntimeCommandPort for NoopUiRuntimePort {
    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        match command {
            RuntimeCommand::LoadMessageHistoryStartupCache
            | RuntimeCommand::RecordMessageHistory { .. } => Ok(RuntimeCommandReceipt::Accepted),
            _ => Err(match command.target() {
                Some(target) => format!("Runtime is not available: {}", target.display_label()),
                None => "Runtime is not available".to_string(),
            }),
        }
    }
}

impl ModelRuntimePort for NoopUiRuntimePort {
    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        Vec::new()
    }

    fn persist_selected_model(&mut self, _selection: &ModelSelection) -> Result<(), String> {
        Ok(())
    }

    fn refresh_model_provider(&mut self, _request: ProviderSyncRequest) -> Result<(), String> {
        Err("Model refresh runtime is not available".to_string())
    }
}

impl PromptRuntimePort for NoopUiRuntimePort {
    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        _mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    fn commit_prompt_assembly_edit(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ModelRuntimePort, NoopUiRuntimePort, PromptRuntimePort, RuntimeCommandPort,
        RuntimeEventPort,
    };
    use runtime_domain::{
        model_catalog::ModelSelection,
        session::{RuntimeCommand, RuntimeCommandReceipt},
    };

    fn assert_event_port<T: RuntimeEventPort>() {}
    fn assert_command_port<T: RuntimeCommandPort>() {}
    fn assert_model_port<T: ModelRuntimePort>() {}
    fn assert_prompt_port<T: PromptRuntimePort>() {}

    #[test]
    fn noop_port_implements_each_narrow_view() {
        let mut port = NoopUiRuntimePort;
        assert_event_port::<NoopUiRuntimePort>();
        assert_command_port::<NoopUiRuntimePort>();
        assert_model_port::<NoopUiRuntimePort>();
        assert_prompt_port::<NoopUiRuntimePort>();
        assert!(RuntimeEventPort::drain_runtime_events(&mut port).is_empty());
        assert!(ModelRuntimePort::drain_model_provider_refresh_events(&mut port).is_empty());
        assert!(PromptRuntimePort::commit_prompt_assembly_edit(&mut port).is_ok());
        assert_eq!(
            RuntimeCommandPort::dispatch_runtime_command(
                &mut port,
                RuntimeCommand::LoadMessageHistoryStartupCache,
            ),
            Ok(RuntimeCommandReceipt::Accepted)
        );
        assert!(
            ModelRuntimePort::persist_selected_model(
                &mut port,
                &ModelSelection::new("provider", "model"),
            )
            .is_ok()
        );
    }
}
