use std::time::{Duration, Instant};

use crate::frontend::tui::Model;

/// 后台 runtime 仍使用非唤醒式 receiver 时，主循环需要低频醒来 drain 一次。
pub(super) const BACKGROUND_EVENT_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalWaitPlan {
    Block,
    Poll {
        duration: Duration,
        render_on_timeout: bool,
    },
}

pub(super) fn terminal_wait_plan(
    model: &Model,
    startup_deadline: Instant,
    now: Instant,
    has_background_runtime: bool,
) -> TerminalWaitPlan {
    let deadline = next_pipeline_deadline(model, startup_deadline, now, has_background_runtime);
    match deadline {
        Some(deadline) => TerminalWaitPlan::Poll {
            duration: deadline.saturating_duration_since(now),
            render_on_timeout: render_on_timeout(model, startup_deadline, now, deadline),
        },
        None => TerminalWaitPlan::Block,
    }
}

impl TerminalWaitPlan {
    pub(super) const fn render_on_timeout(self) -> bool {
        match self {
            Self::Block => false,
            Self::Poll {
                render_on_timeout, ..
            } => render_on_timeout,
        }
    }
}

fn next_pipeline_deadline(
    model: &Model,
    startup_deadline: Instant,
    now: Instant,
    has_background_runtime: bool,
) -> Option<Instant> {
    let mut next_deadline = if model.has_palette() {
        None
    } else {
        Some(startup_deadline)
    };

    if let Some(model_deadline) = model.next_timeout_deadline() {
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(model_deadline),
            None => model_deadline,
        });
    }

    if let Some(activity_interval) = model.stream_activity_frame_interval_at(now) {
        let activity_deadline = now + activity_interval;
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(activity_deadline),
            None => activity_deadline,
        });
    }

    if has_background_runtime {
        let background_deadline = now + BACKGROUND_EVENT_POLL_INTERVAL;
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(background_deadline),
            None => background_deadline,
        });
    }

    next_deadline
}

fn render_on_timeout(
    model: &Model,
    startup_deadline: Instant,
    now: Instant,
    deadline: Instant,
) -> bool {
    if !model.has_palette() && deadline == startup_deadline {
        return false;
    }

    if model
        .next_timeout_deadline()
        .is_some_and(|model_deadline| model_deadline == deadline)
    {
        return false;
    }

    model
        .stream_activity_frame_interval_at(now)
        .is_some_and(|activity_interval| now + activity_interval == deadline)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::frontend::tui::{HeroOptions, Model};

    #[test]
    fn static_model_blocks_without_periodic_polling() {
        let mut model = Model::new(HeroOptions::default());
        model.update(crate::frontend::tui::AppEvent::StartupReadyTimeout);
        let now = Instant::now();

        assert_eq!(
            terminal_wait_plan(&model, now + Duration::from_secs(10), now, false),
            TerminalWaitPlan::Block
        );
    }

    #[test]
    fn background_runtime_keeps_low_frequency_poll_deadline() {
        let mut model = Model::new(HeroOptions::default());
        model.update(crate::frontend::tui::AppEvent::StartupReadyTimeout);
        let now = Instant::now();

        assert_eq!(
            terminal_wait_plan(&model, now + Duration::from_secs(10), now, true),
            TerminalWaitPlan::Poll {
                duration: BACKGROUND_EVENT_POLL_INTERVAL,
                render_on_timeout: false,
            }
        );
    }

    #[test]
    fn startup_deadline_wins_over_background_poll() {
        let model = Model::new(HeroOptions::default());
        let now = Instant::now();

        assert_eq!(
            terminal_wait_plan(&model, now + Duration::from_millis(10), now, true),
            TerminalWaitPlan::Poll {
                duration: Duration::from_millis(10),
                render_on_timeout: false,
            }
        );
    }

    #[test]
    fn stream_activity_deadline_requests_render_on_timeout() {
        let mut model = Model::new(HeroOptions::default());
        model.update(crate::frontend::tui::AppEvent::StartupReadyTimeout);
        model.show_stream_activity("working");
        let now = Instant::now();

        assert_eq!(
            terminal_wait_plan(&model, now + Duration::from_secs(10), now, false),
            TerminalWaitPlan::Poll {
                duration: Duration::from_millis(80),
                render_on_timeout: true,
            }
        );
    }

    #[test]
    fn background_poll_deadline_does_not_request_render_without_events() {
        let mut model = Model::new(HeroOptions::default());
        model.update(crate::frontend::tui::AppEvent::StartupReadyTimeout);
        let now = Instant::now();

        assert_eq!(
            terminal_wait_plan(&model, now + Duration::from_secs(10), now, true),
            TerminalWaitPlan::Poll {
                duration: BACKGROUND_EVENT_POLL_INTERVAL,
                render_on_timeout: false,
            }
        );
    }
}
