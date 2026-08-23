use std::sync::Arc;

use runtime_domain::{
    model_catalog::{ModelProviderRefreshEvent, ModelSelection, ProviderSyncRequest},
    session::{RuntimeCommand, RuntimeCommandReceipt, RuntimeEvent},
};

type RuntimeWakeCallback = dyn Fn() + Send + Sync + 'static;

/// `RuntimeWake` 是 runtime 通知 TUI 重新 drain 事件的通用回调。
///
/// 该类型隐藏 terminal event pump，避免 runtime adapter 依赖 TUI 的具体 wake 实现。
#[derive(Clone)]
pub struct RuntimeWake {
    callback: Arc<RuntimeWakeCallback>,
}

impl RuntimeWake {
    /// `new` 创建一个可跨线程调用的 runtime wake 回调。
    pub fn new(callback: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            callback: Arc::new(callback),
        }
    }

    /// `wake` 通知 TUI runner 重新观察 runtime 事件。
    pub fn wake(&self) {
        (self.callback)();
    }
}

/// `UiRuntimePort` 是 TUI 消费 runtime 能力的稳定 seam。
///
/// command 是控制输入，event 是事实输出；adapter 不得把 instruction/control metadata
/// 拼入用户可见的 delivery content。
pub trait UiRuntimePort {
    /// 绑定当前 TUI event loop 的 wake callback。
    fn bind_runtime_wake(&mut self, _wake: RuntimeWake) -> Result<(), String> {
        Ok(())
    }

    /// 取出 runtime 已发布的事实事件。
    fn drain_runtime_events(&mut self) -> Vec<RuntimeEvent> {
        Vec::new()
    }

    /// 取出 model provider refresh 事件。
    fn drain_model_provider_refresh_events(&mut self) -> Vec<ModelProviderRefreshEvent> {
        Vec::new()
    }

    /// 向 runtime 派发控制命令。
    fn dispatch_runtime_command(
        &mut self,
        command: RuntimeCommand,
    ) -> Result<RuntimeCommandReceipt, String> {
        Err(match command.target() {
            Some(target) => format!("Runtime is not available: {}", target.display_label()),
            None => "Runtime is not available".to_string(),
        })
    }

    /// 持久化当前模型选择。
    fn persist_selected_model(&mut self, _selection: &ModelSelection) -> Result<(), String> {
        Ok(())
    }

    /// 请求刷新指定 provider 的模型目录。
    fn refresh_model_provider(&mut self, _request: ProviderSyncRequest) -> Result<(), String> {
        Err("Model refresh runtime is not available".to_string())
    }

    /// 进入 `/prompt` overlay 时加载 working copy。
    fn begin_prompt_assembly_edit(
        &mut self,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    /// 在 prompt working copy 上同步应用 mutation。
    fn apply_prompt_assembly_edit_mutation(
        &mut self,
        _mutation: runtime_domain::prompt_assembly::PromptAssemblyMutation,
    ) -> Result<runtime_domain::prompt_assembly::PromptAssemblyManagerSnapshot, String> {
        Err("Prompt assembly editing is not available".to_string())
    }

    /// 退出 `/prompt` overlay 时提交 working copy。
    fn commit_prompt_assembly_edit(&mut self) -> Result<(), String> {
        Ok(())
    }
}

/// `NoopUiRuntimePort` 让纯 TUI 构建可以独立运行到模型更新层。
#[derive(Debug, Default)]
pub struct NoopUiRuntimePort;

impl UiRuntimePort for NoopUiRuntimePort {
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
