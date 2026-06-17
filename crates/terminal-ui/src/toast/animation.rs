use super::*;

impl ToastAnimation {
    pub(super) fn enter(notice: ToastNotice) -> Self {
        Self {
            notice,
            kind: ToastAnimationKind::Enter,
            started_at: None,
        }
    }

    pub(super) fn exit(notice: ToastNotice) -> Self {
        Self {
            notice,
            kind: ToastAnimationKind::Exit,
            started_at: None,
        }
    }

    pub(super) fn frame_at(&self, now: Instant, width: u16) -> ToastAnimationFrame {
        let progress = self.progress_at(now);
        let swept_columns = swept_columns(width, progress);
        match self.kind {
            ToastAnimationKind::Enter => ToastAnimationFrame {
                erased_columns: swept_columns,
                visible_columns: if progress >= 1.0 {
                    width
                } else {
                    swept_columns.saturating_sub(TOAST_ERASE_EDGE_WIDTH)
                },
                is_complete: progress >= 1.0,
            },
            ToastAnimationKind::Exit => ToastAnimationFrame {
                erased_columns: swept_columns,
                visible_columns: width.saturating_sub(swept_columns),
                is_complete: progress >= 1.0,
            },
        }
    }

    pub(super) fn advance_at(&mut self, now: Instant) -> bool {
        if self.started_at.is_none() {
            self.started_at = Some(now);
        }
        self.progress_at(now) >= 1.0
    }

    fn progress_at(&self, now: Instant) -> f64 {
        let started_at = self.started_at.unwrap_or(now);
        let elapsed = now.saturating_duration_since(started_at);
        let duration = match self.kind {
            ToastAnimationKind::Enter => TOAST_ENTER_DURATION,
            ToastAnimationKind::Exit => TOAST_EXIT_DURATION,
        };
        if duration.is_zero() {
            return 1.0;
        }
        let raw_progress = (elapsed.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0);
        smoothstep(raw_progress)
    }
}

fn smoothstep(progress: f64) -> f64 {
    progress * progress * (3.0 - 2.0 * progress)
}

fn swept_columns(width: u16, progress: f64) -> u16 {
    if width == 0 {
        return 0;
    }
    if progress >= 1.0 {
        return width;
    }
    let progress = progress.clamp(0.0, 1.0);
    let columns = (progress * f64::from(width)).ceil().max(1.0);
    // progress 已限制在 [0, 1]，因此列数不会超过 width。
    (columns as u16).min(width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swept_columns_uses_ceil_with_one_column_while_animating() {
        assert_eq!(swept_columns(0, 0.5), 0);
        assert_eq!(swept_columns(12, 0.0), 1);
        assert_eq!(swept_columns(12, 0.01), 1);
        assert_eq!(swept_columns(12, 0.5), 6);
        assert_eq!(swept_columns(12, 0.51), 7);
        assert_eq!(swept_columns(12, 1.0), 12);
    }
}
