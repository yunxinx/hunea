use std::time::{Duration, Instant};

use crate::Model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopWaitPlan {
    Block,
    Wait {
        duration: Duration,
        render_on_timeout: bool,
    },
}

pub(super) fn loop_wait_plan(model: &Model, now: Instant) -> LoopWaitPlan {
    let deadline = next_pipeline_deadline(model, now);
    match deadline {
        Some(deadline) => LoopWaitPlan::Wait {
            duration: deadline.saturating_duration_since(now),
            render_on_timeout: render_on_timeout(model, now, deadline),
        },
        None => LoopWaitPlan::Block,
    }
}

impl LoopWaitPlan {
    pub(super) const fn timeout(self) -> Option<Duration> {
        match self {
            Self::Block => None,
            Self::Wait { duration, .. } => Some(duration),
        }
    }

    pub(super) const fn render_on_timeout(self) -> bool {
        match self {
            Self::Block => false,
            Self::Wait {
                render_on_timeout, ..
            } => render_on_timeout,
        }
    }
}

fn next_pipeline_deadline(model: &Model, now: Instant) -> Option<Instant> {
    let mut next_deadline: Option<Instant> = None;

    if let Some(model_deadline) = model.next_timeout_deadline() {
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(model_deadline),
            None => model_deadline,
        });
    }

    if let Some(activity_deadline) = next_animation_deadline(model, now) {
        next_deadline = Some(match next_deadline {
            Some(deadline) => deadline.min(activity_deadline),
            None => activity_deadline,
        });
    }

    next_deadline
}

fn next_animation_deadline(model: &Model, now: Instant) -> Option<Instant> {
    [
        model.stream_activity_next_frame_deadline_at(now),
        model.startup_banner_entrance_next_frame_deadline_at(now),
        model.toast_next_frame_deadline_at(now),
        model.tool_activity_next_frame_deadline_at(now),
    ]
    .into_iter()
    .flatten()
    .min()
}

fn render_on_timeout(model: &Model, now: Instant, deadline: Instant) -> bool {
    if model
        .next_timeout_deadline()
        .is_some_and(|model_deadline| model_deadline == deadline)
    {
        return false;
    }

    next_animation_deadline(model, now)
        .is_some_and(|animation_deadline| deadline == animation_deadline)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{
        Model, StartupBannerOptions, tool_result::TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL,
    };
    use runtime_domain::session::{
        RuntimeToolActivity, RuntimeToolActivityStatus, RuntimeToolKind,
    };

    #[test]
    fn static_model_blocks_without_periodic_polling() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.update(crate::AppEvent::StartupReadyTimeout);
        let now = Instant::now();

        assert_eq!(loop_wait_plan(&model, now), LoopWaitPlan::Block);
    }

    #[test]
    fn startup_banner_entrance_deadline_requests_render_on_timeout() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.update(crate::AppEvent::StartupReadyTimeout);
        let now = Instant::now();
        model.start_startup_banner_entrance_for_test(now);

        assert_eq!(
            loop_wait_plan(&model, now),
            LoopWaitPlan::Wait {
                duration: Duration::from_millis(16),
                render_on_timeout: true,
            }
        );
    }

    #[test]
    fn toast_animation_deadline_requests_render_on_timeout() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.update(crate::AppEvent::StartupReadyTimeout);
        model.show_toast(crate::toast::ToastSeverity::Info, "Saved");
        let now = Instant::now();
        model.advance_toast_at(now);

        assert_eq!(
            loop_wait_plan(&model, now),
            LoopWaitPlan::Wait {
                duration: crate::toast::TOAST_FRAME_INTERVAL,
                render_on_timeout: true,
            }
        );
    }

    #[test]
    fn stream_activity_deadline_requests_render_on_timeout() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.update(crate::AppEvent::StartupReadyTimeout);
        model.show_stream_activity("working");
        let now = Instant::now();
        let duration = model
            .stream_activity_next_frame_deadline_at(now)
            .expect("stream activity should schedule a frame")
            .saturating_duration_since(now);

        assert_eq!(
            loop_wait_plan(&model, now),
            LoopWaitPlan::Wait {
                duration,
                render_on_timeout: true,
            }
        );
    }

    #[test]
    fn reduced_motion_stream_waits_for_elapsed_second_boundary() {
        let mut model = Model::new_with_options(
            StartupBannerOptions::default(),
            crate::ModelOptions {
                motion_mode: crate::MotionMode::Reduced,
                ..crate::ModelOptions::default()
            },
        );
        model.update(crate::AppEvent::StartupReadyTimeout);
        model.show_stream_activity("working");
        let now = Instant::now();
        let duration = model
            .stream_activity_next_frame_deadline_at(now)
            .expect("reduced motion elapsed should schedule a semantic tick")
            .saturating_duration_since(now);
        assert_eq!(
            loop_wait_plan(&model, now),
            LoopWaitPlan::Wait {
                duration,
                render_on_timeout: true,
            }
        );

        model.show_toast(crate::toast::ToastSeverity::Info, "Saved");
        let plan = loop_wait_plan(&model, now);
        assert!(matches!(
            plan,
            LoopWaitPlan::Wait {
                render_on_timeout: true,
                ..
            }
        ));
    }

    #[test]
    fn active_tool_activity_deadline_requests_render_on_timeout() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.update(crate::AppEvent::StartupReadyTimeout);
        model
            .transcript_mut()
            .append_runtime_tool_activity(RuntimeToolActivity {
                activity_id: "tool-1".to_string(),
                title: "WriteFile: TEMP.md".to_string(),
                kind: RuntimeToolKind::Other,
                status: RuntimeToolActivityStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
                raw_output: None,
            });
        model.sync_transcript_render();
        let now = model
            .transcript_mut()
            .active_tool_activity_started_at()
            .expect("active tool activity should have a start time");

        assert_eq!(
            loop_wait_plan(&model, now),
            LoopWaitPlan::Wait {
                duration: TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL,
                render_on_timeout: true,
            }
        );
    }

    #[test]
    fn active_tool_activity_uses_absolute_next_frame_deadline() {
        let mut model = Model::new(StartupBannerOptions::default());
        model.update(crate::AppEvent::StartupReadyTimeout);
        model
            .transcript_mut()
            .append_runtime_tool_activity(RuntimeToolActivity {
                activity_id: "tool-1".to_string(),
                title: "WriteFile: TEMP.md".to_string(),
                kind: RuntimeToolKind::Other,
                status: RuntimeToolActivityStatus::InProgress,
                content: Vec::new(),
                locations: Vec::new(),
                raw_input: Some(r##"{"path":"TEMP.md","content":"body"}"##.into()),
                raw_output: None,
            });
        model.sync_transcript_render();
        let started_at = model
            .transcript_mut()
            .active_tool_activity_started_at()
            .expect("active tool activity should have a start time");
        let now =
            started_at + TOOL_ACTIVITY_ACTIVE_MARKER_BLINK_INTERVAL - Duration::from_millis(10);

        assert_eq!(
            loop_wait_plan(&model, now),
            LoopWaitPlan::Wait {
                duration: Duration::from_millis(10),
                render_on_timeout: true,
            }
        );
    }
}
