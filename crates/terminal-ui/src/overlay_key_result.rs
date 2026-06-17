use crate::AppEffect;

/// `OverlayKeyResult` 明确表达覆盖层按键是否接管，以及是否产生副作用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayKeyResult {
    Ignored,
    Handled,
    Effect(AppEffect),
}

impl OverlayKeyResult {
    pub(crate) fn from_effect(effect: Option<AppEffect>) -> Self {
        match effect {
            Some(effect) => Self::Effect(effect),
            None => Self::Handled,
        }
    }

    pub(crate) fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }

    pub(crate) fn into_effect(self) -> Option<AppEffect> {
        match self {
            Self::Ignored | Self::Handled => None,
            Self::Effect(effect) => Some(effect),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppEffect;

    #[test]
    fn overlay_key_result_names_key_dispatch_states() {
        let effect = AppEffect::OpenCopyPicker;

        assert_eq!(
            OverlayKeyResult::from_effect(None),
            OverlayKeyResult::Handled
        );
        assert_eq!(
            OverlayKeyResult::from_effect(Some(effect.clone())),
            OverlayKeyResult::Effect(effect.clone())
        );

        assert!(OverlayKeyResult::Ignored.is_ignored());
        assert!(!OverlayKeyResult::Handled.is_ignored());
        assert!(!OverlayKeyResult::Effect(effect.clone()).is_ignored());

        assert_eq!(OverlayKeyResult::Ignored.into_effect(), None);
        assert_eq!(OverlayKeyResult::Handled.into_effect(), None);
        assert_eq!(
            OverlayKeyResult::Effect(effect.clone()).into_effect(),
            Some(effect)
        );
    }
}
