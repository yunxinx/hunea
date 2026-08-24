//! 提供确定性调度与可逆 registration 的进程内 typed extension hooks。

mod error;
mod identity;
mod payload;
mod registry;

pub use error::{
    HookDispatchError, HookDispatchErrorKind, HookFailureKind, HookPhase, HookRejectionKind,
};
pub use identity::{
    HookId, HookIdError, HookOwnerId, HookOwnerIdError, HookPriority, HookRegistrationOptions,
    HookRegistrationOptionsError,
};
pub use payload::{
    AfterToolResultDecision, AfterToolResultHook, AfterToolResultPayload,
    AfterToolResultPayloadError, BeforeToolExecuteDecision, BeforeToolExecuteHook,
    BeforeToolExecutePayload, BeforeTurnDecision, BeforeTurnHook, BeforeTurnPayload,
    BeforeTurnPayloadError, HookFuture,
};
pub use registry::{
    ExtensionHookRegistry, HookRegistration, HookRegistrationError, HookRegistrationSnapshot,
};
