use crate::AppEffect;

/// `OverlayInputResult` 明确表达覆盖层输入是否接管，以及是否产生副作用。
#[must_use = "overlay input results decide whether input was consumed"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OverlayInputResult {
    Ignored,
    Handled,
    Effect(AppEffect),
}

impl OverlayInputResult {
    pub(crate) fn from_effect(effect: Option<AppEffect>) -> Self {
        match effect {
            Some(effect) => Self::Effect(effect),
            None => Self::Handled,
        }
    }

    #[must_use]
    pub(crate) fn is_ignored(&self) -> bool {
        matches!(self, Self::Ignored)
    }

    #[must_use = "the extracted effect must be applied or deliberately ignored"]
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
    fn overlay_input_result_names_key_dispatch_states() {
        let effect = AppEffect::OpenCopyPicker;

        assert_eq!(
            OverlayInputResult::from_effect(None),
            OverlayInputResult::Handled
        );
        assert_eq!(
            OverlayInputResult::from_effect(Some(effect.clone())),
            OverlayInputResult::Effect(effect.clone())
        );

        assert!(OverlayInputResult::Ignored.is_ignored());
        assert!(!OverlayInputResult::Handled.is_ignored());
        assert!(!OverlayInputResult::Effect(effect.clone()).is_ignored());

        assert_eq!(OverlayInputResult::Ignored.into_effect(), None);
        assert_eq!(OverlayInputResult::Handled.into_effect(), None);
        assert_eq!(
            OverlayInputResult::Effect(effect.clone()).into_effect(),
            Some(effect)
        );
    }
}
