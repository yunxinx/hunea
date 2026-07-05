use std::{
    future::Future,
    sync::{Mutex, MutexGuard, OnceLock},
};

use color_eyre::eyre::{Result, WrapErr, eyre};
use tokio::runtime::Runtime;

static SESSION_STORE_RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();

/// `block_on_session_store` 为同步 app 边界复用同一个 Tokio runtime。
///
/// `SessionStore` 是 async trait，但 TUI app 的启动与命令处理仍在同步栈上。集中这个
/// bridge 可以避免各调用点反复创建 runtime，也让后续迁移到 worker/handle 注入时有
/// 单一入口。
pub(crate) fn block_on_session_store<T>(
    future: impl Future<Output = T>,
    runtime_context: &'static str,
) -> Result<T> {
    let runtime = session_store_runtime(runtime_context)?;
    Ok(runtime.block_on(future))
}

fn session_store_runtime(runtime_context: &'static str) -> Result<MutexGuard<'static, Runtime>> {
    if SESSION_STORE_RUNTIME.get().is_none() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .wrap_err(runtime_context)?;
        let _ = SESSION_STORE_RUNTIME.set(Mutex::new(runtime));
    }

    let Some(runtime) = SESSION_STORE_RUNTIME.get() else {
        return Err(eyre!("session store runtime was not initialized"));
    };
    Ok(match runtime.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    })
}
