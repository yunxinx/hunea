use std::time::Instant;

use crate::{
    Model, ModelOptions, StartupBannerOptions, StyleMode, frame_time::FrameRenderContext,
    theme::default_palette,
};

use super::large_composer_draft_fixture;

/// `TailLayoutSummary` 收敛 stream animation frame 下的 tail 布局特征。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TailLayoutSummary {
    pub line_count: usize,
    pub plain_text_len: usize,
    pub composer_line_count: usize,
    pub cursor_x: u16,
    pub cursor_y: usize,
}

/// `StreamActivityTailBench` 驱动真实 stream deadline 下的大草稿 tail 重建。
#[derive(Debug)]
pub struct StreamActivityTailBench {
    model: Model,
    frame_time: Instant,
}

impl StreamActivityTailBench {
    /// 构造并预热 active stream activity 下的大 composer tail。
    pub fn new(draft_bytes: usize, width: u16, height: u16) -> Self {
        assert!(width > 0, "stream tail benchmark width must be non-zero");
        assert!(height > 0, "stream tail benchmark height must be non-zero");
        let mut model = Model::new_with_options(
            StartupBannerOptions {
                app_name: Some("hunea".to_string()),
                version: Some("benchmark".to_string()),
                model_name: Some("benchmark-model".to_string()),
                work_dir: Some("/workspace/hunea".to_string()),
                width: 0,
            },
            ModelOptions {
                style_mode: StyleMode::Cx,
                ..ModelOptions::default()
            },
        );
        model.transcript_mut().clear();
        model.sync_transcript_render();
        model.set_window(width, height);
        model.set_palette(default_palette(), true);
        model
            .composer_mut()
            .reset_text_and_move_to_end(large_composer_draft_fixture(draft_bytes));
        model.sync_composer_height();
        model.show_stream_activity_with_header("Working");

        let frame_time = Instant::now();
        let _ = model.build_document_tail_layout(FrameRenderContext::new(frame_time));

        Self { model, frame_time }
    }

    /// 返回最近一次已物化的 animation frame time。
    #[cfg(test)]
    pub(super) fn frame_time(&self) -> Instant {
        self.frame_time
    }

    /// 跨到真实下一 stream deadline 并重建 tail。
    pub fn rebuild_next_activity_frame(&mut self) -> TailLayoutSummary {
        self.frame_time = self
            .model
            .stream_activity_next_frame_deadline_at(self.frame_time)
            .expect("active stream benchmark should expose a next frame deadline");
        let tail = self
            .model
            .build_document_tail_layout(FrameRenderContext::new(self.frame_time));

        TailLayoutSummary {
            line_count: tail.line_count(),
            plain_text_len: tail.plain_text_len_for_range(0, tail.line_count()),
            composer_line_count: tail.composer_slot.content_line_count,
            cursor_x: tail.cursor_x,
            cursor_y: tail.cursor_y,
        }
    }
}
