use std::{fmt, time::Duration};

const MAX_HOOK_ID_LEN: usize = 64;
const DEFAULT_CANCELLATION_GRACE: Duration = Duration::from_millis(100);

/// `HookOwnerId` 是 diagnostics-safe 的 hook owner identity。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookOwnerId(String);

impl HookOwnerId {
    /// 验证并创建 owner identity；错误不会保留原始输入。
    pub fn try_new(value: impl Into<String>) -> Result<Self, HookOwnerIdError> {
        validate_id(value.into())
            .map(Self)
            .map_err(HookOwnerIdError::from)
    }

    /// 返回已经验证的 owner identity。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HookOwnerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("HookOwnerId").field(&self.0).finish()
    }
}

impl fmt::Display for HookOwnerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `HookOwnerId` 的封闭校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HookOwnerIdError {
    #[error("empty_hook_owner_id")]
    Empty,
    #[error("hook_owner_id_too_long")]
    TooLong,
    #[error("invalid_hook_owner_id_format")]
    InvalidFormat,
}

impl From<IdValidationError> for HookOwnerIdError {
    fn from(error: IdValidationError) -> Self {
        match error {
            IdValidationError::Empty => Self::Empty,
            IdValidationError::TooLong => Self::TooLong,
            IdValidationError::InvalidFormat => Self::InvalidFormat,
        }
    }
}

/// `HookId` 标识同一 owner 在一个 phase 内的稳定 registration slot。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HookId(String);

impl HookId {
    /// 验证并创建 hook identity；错误不会保留原始输入。
    pub fn try_new(value: impl Into<String>) -> Result<Self, HookIdError> {
        validate_id(value.into())
            .map(Self)
            .map_err(HookIdError::from)
    }

    /// 返回已经验证的 hook identity。
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for HookId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("HookId").field(&self.0).finish()
    }
}

impl fmt::Display for HookId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `HookId` 的封闭校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HookIdError {
    #[error("empty_hook_id")]
    Empty,
    #[error("hook_id_too_long")]
    TooLong,
    #[error("invalid_hook_id_format")]
    InvalidFormat,
}

impl From<IdValidationError> for HookIdError {
    fn from(error: IdValidationError) -> Self {
        match error {
            IdValidationError::Empty => Self::Empty,
            IdValidationError::TooLong => Self::TooLong,
            IdValidationError::InvalidFormat => Self::InvalidFormat,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdValidationError {
    Empty,
    TooLong,
    InvalidFormat,
}

fn validate_id(value: String) -> Result<String, IdValidationError> {
    if value.is_empty() {
        return Err(IdValidationError::Empty);
    }
    if value.len() > MAX_HOOK_ID_LEN {
        return Err(IdValidationError::TooLong);
    }
    let bytes = value.as_bytes();
    if !bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        || !bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        || bytes
            .iter()
            .any(|byte| !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-')
        || bytes.windows(2).any(|pair| pair == b"--")
    {
        return Err(IdValidationError::InvalidFormat);
    }
    Ok(value)
}

/// `HookPriority` 越小越先执行。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct HookPriority(i32);

impl HookPriority {
    /// 创建一个稳定的 signed priority。
    pub const fn new(value: i32) -> Self {
        Self(value)
    }

    /// 返回原始 priority 值。
    pub const fn get(self) -> i32 {
        self.0
    }
}

/// 每个 hook registration 的确定性调度选项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookRegistrationOptions {
    priority: HookPriority,
    timeout: Duration,
    cancellation_grace: Duration,
}

impl HookRegistrationOptions {
    /// 创建选项；zero timeout 会在 registration 前被拒绝。
    pub fn try_new(
        priority: HookPriority,
        timeout: Duration,
    ) -> Result<Self, HookRegistrationOptionsError> {
        if timeout.is_zero() {
            return Err(HookRegistrationOptionsError::ZeroTimeout);
        }
        Ok(Self {
            priority,
            timeout,
            cancellation_grace: DEFAULT_CANCELLATION_GRACE,
        })
    }

    /// 创建显式 cancellation grace 的选项；external adapter 用它绑定 host policy。
    pub fn try_new_with_cancellation_grace(
        priority: HookPriority,
        timeout: Duration,
        cancellation_grace: Duration,
    ) -> Result<Self, HookRegistrationOptionsError> {
        if timeout.is_zero() {
            return Err(HookRegistrationOptionsError::ZeroTimeout);
        }
        if cancellation_grace.is_zero() {
            return Err(HookRegistrationOptionsError::ZeroCancellationGrace);
        }
        Ok(Self {
            priority,
            timeout,
            cancellation_grace,
        })
    }

    /// 返回稳定 priority。
    pub const fn priority(self) -> HookPriority {
        self.priority
    }

    /// 返回单次 invocation timeout。
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// 返回 cancellation 后继续 poll hook cleanup 的最大时间。
    pub const fn cancellation_grace(self) -> Duration {
        self.cancellation_grace
    }
}

/// Hook registration options 的封闭校验错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HookRegistrationOptionsError {
    #[error("hook_timeout_must_be_non_zero")]
    ZeroTimeout,
    #[error("hook_cancellation_grace_must_be_non_zero")]
    ZeroCancellationGrace,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_reject_unbounded_or_unsafe_diagnostic_values() {
        assert_eq!(
            HookOwnerId::try_new("/secret/path"),
            Err(HookOwnerIdError::InvalidFormat)
        );
        assert_eq!(
            HookId::try_new("UPPERCASE"),
            Err(HookIdError::InvalidFormat)
        );
        assert_eq!(
            HookId::try_new("a".repeat(MAX_HOOK_ID_LEN + 1)),
            Err(HookIdError::TooLong)
        );
        assert_eq!(
            HookRegistrationOptions::try_new(HookPriority::default(), Duration::ZERO),
            Err(HookRegistrationOptionsError::ZeroTimeout)
        );
        assert_eq!(
            HookRegistrationOptions::try_new_with_cancellation_grace(
                HookPriority::default(),
                Duration::from_secs(1),
                Duration::ZERO,
            ),
            Err(HookRegistrationOptionsError::ZeroCancellationGrace)
        );
    }

    #[test]
    fn default_and_explicit_cancellation_grace_are_stable() {
        let default =
            HookRegistrationOptions::try_new(HookPriority::default(), Duration::from_secs(1))
                .unwrap();
        assert_eq!(default.cancellation_grace(), Duration::from_millis(100));

        let explicit = HookRegistrationOptions::try_new_with_cancellation_grace(
            HookPriority::default(),
            Duration::from_secs(1),
            Duration::from_millis(25),
        )
        .unwrap();
        assert_eq!(explicit.cancellation_grace(), Duration::from_millis(25));
    }
}
