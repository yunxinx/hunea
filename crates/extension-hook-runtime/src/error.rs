use std::fmt;

use crate::{HookId, HookOwnerId};

/// `HookPhase` 是 registry 支持的封闭 dispatch 集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HookPhase {
    BeforeTurn,
    BeforeToolExecute,
    AfterToolResult,
}

impl HookPhase {
    /// 返回稳定、安全的 phase code。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeTurn => "before_turn",
            Self::BeforeToolExecute => "before_tool_execute",
            Self::AfterToolResult => "after_tool_result",
        }
    }
}

impl fmt::Display for HookPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Hook implementation 可以报告的封闭 failure kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookFailureKind {
    Unavailable,
    InvalidInput,
    Internal,
}

impl HookFailureKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "hook_unavailable",
            Self::InvalidInput => "hook_invalid_input",
            Self::Internal => "hook_internal_failure",
        }
    }
}

/// Hook gate 可以报告的封闭 rejection kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookRejectionKind {
    PolicyDenied,
    UnsupportedOperation,
}

impl HookRejectionKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PolicyDenied => "hook_policy_denied",
            Self::UnsupportedOperation => "hook_unsupported_operation",
        }
    }
}

/// Registry dispatch 的封闭 error kind。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookDispatchErrorKind {
    Rejected(HookRejectionKind),
    Failed(HookFailureKind),
    TimedOut,
    CallerCancelled,
    RegistrationDisposed,
    InvalidOutput,
}

impl HookDispatchErrorKind {
    /// 返回稳定且不包含 hook payload 的 error code。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rejected(reason) => reason.as_str(),
            Self::Failed(reason) => reason.as_str(),
            Self::TimedOut => "hook_timed_out",
            Self::CallerCancelled => "hook_caller_cancelled",
            Self::RegistrationDisposed => "hook_registration_disposed",
            Self::InvalidOutput => "hook_invalid_output",
        }
    }
}

/// `HookDispatchError` 只保存 phase、closed kind 与 validated registration metadata。
#[derive(Clone, PartialEq, Eq)]
pub struct HookDispatchError {
    phase: HookPhase,
    kind: HookDispatchErrorKind,
    registration: Option<(HookOwnerId, HookId)>,
}

impl HookDispatchError {
    pub(crate) fn for_registration(
        phase: HookPhase,
        kind: HookDispatchErrorKind,
        owner: HookOwnerId,
        hook_id: HookId,
    ) -> Self {
        Self {
            phase,
            kind,
            registration: Some((owner, hook_id)),
        }
    }

    /// 返回发生错误的 phase。
    pub const fn phase(&self) -> HookPhase {
        self.phase
    }

    /// 返回封闭 error kind。
    pub const fn kind(&self) -> HookDispatchErrorKind {
        self.kind
    }

    /// 返回安全 owner metadata；caller cancellation 可能没有 registration。
    pub fn owner(&self) -> Option<&HookOwnerId> {
        self.registration.as_ref().map(|(owner, _)| owner)
    }

    /// 返回安全 hook metadata；caller cancellation 可能没有 registration。
    pub fn hook_id(&self) -> Option<&HookId> {
        self.registration.as_ref().map(|(_, hook_id)| hook_id)
    }
}

impl fmt::Display for HookDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "extension hook dispatch failed: phase={} kind={}",
            self.phase,
            self.kind.as_str()
        )?;
        if let Some((owner, hook_id)) = &self.registration {
            write!(formatter, " owner={owner} hook={hook_id}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for HookDispatchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HookDispatchError")
            .field("phase", &self.phase)
            .field("kind", &self.kind)
            .field("registration", &self.registration)
            .finish()
    }
}

impl std::error::Error for HookDispatchError {}
