use std::cmp::Ordering;
use std::time::Instant;

use crate::Model;

use super::{
    DocumentLayout, ViewportState, bottom_follow_viewport_line_indices,
    manual_scroll::crossed_manual_document_scroll_restore_target, offset_viewport_line_indices,
};

const DOCUMENT_MOUSE_WHEEL_DELTA: isize = 3;

impl Model {
    pub(crate) fn document_mouse_wheel_delta() -> isize {
        DOCUMENT_MOUSE_WHEEL_DELTA
    }

    /// 用户当前是否贴底跟随：仅在 follow_bottom 且未处于手动滚动时成立。
    /// 自动滚动到底部的行为只允许在该谓词成立时发生，避免打断用户回看历史。
    pub(crate) fn document_pinned_to_bottom(&self) -> bool {
        self.document_runtime.follow_bottom && !self.document_runtime.manual_scroll
    }

    pub(crate) fn preserved_viewport_state_for_transcript_refresh(
        &mut self,
    ) -> Option<ViewportState> {
        self.document_runtime
            .manual_scroll
            .then(|| self.current_document_viewport_state())
    }

    pub(crate) fn current_document_viewport_state(&mut self) -> ViewportState {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        self.capture_viewport_state_with_layout(
            &layout,
            self.document_runtime.viewport_y,
            self.document_runtime.follow_bottom,
            self.document_runtime.manual_scroll,
        )
    }

    /// 滚轮平滑滚动是否生效：档位非 `Off` 且 motion 模式允许动画。
    /// `MotionMode::Reduced` 下无论档位取值均为瞬时滚动，尊重 reduce-motion 语义。
    pub(crate) fn smooth_scroll_enabled(&self) -> bool {
        self.scroll_animation != crate::ScrollAnimationMode::Off
            && self.motion_mode.allows_animation()
    }

    /// 当前生效的平滑滚动调参：仅在平滑路径生效时返回档位 tuning。
    /// 调参按调用取表传入状态模块，不驻留可被会话重置的运行时状态。
    fn active_smooth_scroll_tuning(&self) -> Option<&'static super::SmoothScrollTuning> {
        if !self.smooth_scroll_enabled() {
            return None;
        }
        self.scroll_animation.tuning()
    }

    /// 主文档滚轮入口。平滑路径只把增量累加进 pending，位移与滚动反馈
    /// 延迟到渲染帧 drain 统一发生；禁用路径保持既有瞬时滚动。
    ///
    /// `now` 由事件入口采样一次传入（与 `frame_time` 同风格），状态模块
    /// 内部不取时钟，便于测试注入固定时刻。
    pub(crate) fn document_mouse_wheel_at(&mut self, delta_lines: isize, now: Instant) {
        let Some(tuning) = self.active_smooth_scroll_tuning() else {
            // 「关闭 = 精确现状」：加速度倍率在连滚时会爬升（并非恒 1.0），
            // 因此禁用路径不经 accumulate_wheel 放大，直接用原始增量瞬时滚动。
            self.document_runtime.smooth_scroll.clear_pending();
            self.scroll_document_by_wheel(delta_lines);
            return;
        };
        self.document_runtime
            .smooth_scroll
            .accumulate_wheel(delta_lines, now, tuning);
    }

    /// 滚轮驱动的文档滚动：位移后统一执行滚动反馈副作用
    /// （历史滚动指示器、pending composer click 清理）。
    ///
    /// 这些副作用依赖位移前后的状态对比，因此必须与实际位移绑定在同一处：
    /// 瞬时路径与平滑 drain 路径共用本方法，两条路径行为一致且对比逻辑不重复。
    fn scroll_document_by_wheel(&mut self, lines: isize) {
        let before_document_viewport_y = self.document_runtime.viewport_y;
        let before_composer_viewport_y = self.composer.viewport_offset();
        let before_follow_bottom = self.document_runtime.follow_bottom;
        let before_manual_scroll = self.document_runtime.manual_scroll;
        let had_pending_click = self.pending_composer_cursor_click.active;

        self.scroll_document_by(lines);

        let viewport_moved = self.document_runtime.viewport_y != before_document_viewport_y
            || self.composer.viewport_offset() != before_composer_viewport_y
            || self.document_runtime.follow_bottom != before_follow_bottom
            || self.document_runtime.manual_scroll != before_manual_scroll;
        if viewport_moved {
            self.clear_pending_composer_cursor_click();
            if had_pending_click {
                self.reset_selection_click();
            }
            self.show_history_scroll_indicator();
        }
    }

    /// 渲染帧前推进平滑滚动 drain：取出本帧步长并实际位移。
    /// 与 `advance_toast_at` 同模式，`frame_time` 由 runner 采样传入。
    ///
    /// drain 按 8ms 时钟节流（非渲染频率）：streaming、连续按键等高频渲染
    /// 期间，未到帧间隔则本次不推进，恒速动画节奏不被事件流量加速。
    pub(crate) fn advance_smooth_scroll_at(&mut self, frame_time: Instant) {
        if !self.document_runtime.smooth_scroll.is_settling() {
            return;
        }
        // 全屏模态层遮挡主文档时冻结动画：残余 drain 若继续，用户看不见的
        // 主视口会移动，关闭覆盖层后位置与离开时不同。清空定格在当前位置，
        // 是最不意外的行为。
        if self.top_modal_layer().is_some() {
            self.document_runtime.smooth_scroll.clear_pending();
            return;
        }
        if !self
            .document_runtime
            .smooth_scroll
            .ready_to_drain_at(frame_time)
        {
            return;
        }
        let Some(tuning) = self.active_smooth_scroll_tuning() else {
            // 档位与 motion 在运行期不可变，pending 存续时不应走到这里；
            // 防御性清空维持「非平滑路径无 pending」的不变量。
            self.document_runtime.smooth_scroll.clear_pending();
            return;
        };
        let step = self
            .document_runtime
            .smooth_scroll
            .drain_step(frame_time, tuning);
        if step != 0 {
            self.scroll_document_by_wheel(step);
        }
    }

    /// 平滑滚动的下一动画帧 deadline；与其他 `*_next_frame_deadline_at`
    /// 同形，供事件泵聚合为统一等待计划。
    pub(crate) fn smooth_scroll_next_frame_deadline_at(&self, now: Instant) -> Option<Instant> {
        self.document_runtime
            .smooth_scroll
            .next_frame_deadline_at(now)
    }

    /// 测试辅助：把 pending 平滑滚动一次性 drain 至收敛。
    /// 供沿用「滚轮事件 → 立即断言位移」形状的既有测试使用。
    #[cfg(test)]
    pub(crate) fn settle_smooth_scroll_for_test(&mut self) {
        let mut now = Instant::now();
        while self.document_runtime.smooth_scroll.is_settling() {
            now += std::time::Duration::from_millis(8);
            self.advance_smooth_scroll_at(now);
        }
    }

    pub(crate) fn scroll_document_by(&mut self, lines: isize) {
        if lines == 0 {
            return;
        }

        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if layout.line_count() == 0 {
            self.apply_document_viewport_position(&layout, 0, 0, true, false);
            self.clear_manual_document_scroll_restore_target();
            // 空文档没有可滚动区域，残余 pending 无意义，一并清空。
            self.document_runtime.smooth_scroll.clear_pending();
            return;
        }

        let current_offset = self
            .clamp_document_viewport_offset(self.document_runtime.viewport_y, layout.line_count());
        let next_offset =
            self.clamp_document_viewport_offset_signed(current_offset, lines, layout.line_count());
        if next_offset == current_offset {
            // 触顶/触底：clamp 后无位移。此时必须清空平滑滚动累加器，防止
            // 自由滚轮在边界外持续积压 delta——否则用户反向滚动要先抵消
            // 积压才开始移动（反向粘滞）。
            self.document_runtime.smooth_scroll.clear_pending();
            return;
        }

        self.start_manual_document_scroll_if_needed();
        let (restore_offset, restore_composer_offset, restore_follow_bottom) =
            self.manual_document_scroll_restore_offsets(&layout);

        if crossed_manual_document_scroll_restore_target(
            current_offset,
            next_offset,
            restore_offset,
        ) {
            self.apply_document_viewport_position(
                &layout,
                restore_offset,
                restore_composer_offset,
                restore_follow_bottom,
                false,
            );
            self.clear_manual_document_scroll_restore_target();
            // 吸附是本次滚动手势的语义终点：残余 pending 若继续 drain，
            // 会把视口再次拖离恢复位置，因此在此一并清空。
            self.document_runtime.smooth_scroll.clear_pending();
            return;
        }

        // 手动滚动中向下触底 = 「回到最新」，恢复贴底跟随（聊天客户端直觉）。
        // 向上滚动因 clamp 约束不可能落在底部 offset，方向守卫仍显式保留语义。
        // 排除 selection 拖拽自动滚动：拖选触底属于「扩大选区」而非「回到最新」，
        // 若在此贴底，streaming 时视口会随新内容移动、鼠标下方内容逐帧变化导致
        // 选区跳变。滚轮与键盘翻页（非拖拽）不受影响。
        // pill 清除与审批面板可见性联动由 sync_document_viewport_to_bottom 内的
        // apply_document_viewport_position 汇聚点统一处理，不在此处旁路。
        if lines > 0
            && next_offset == self.document_bottom_offset(layout.line_count())
            && !self.selection_runtime.selection.is_dragging()
        {
            self.sync_document_viewport_to_bottom();
            self.document_runtime.smooth_scroll.clear_pending();
            return;
        }

        let composer_offset = self.current_composer_viewport_offset(&layout, next_offset);
        self.apply_document_viewport_position(&layout, next_offset, composer_offset, false, true);
    }

    /// 主界面 PageUp/PageDown 的文档翻页；返回是否已消费按键。
    ///
    /// 行为矩阵（快慢分治：滚轮平滑、翻页瞬时）：
    /// - 手动滚动中：双向翻文档页；
    /// - 贴底 + PageUp + composer 单屏可容纳：向上翻文档页并进入手动滚动；
    /// - 其余（贴底 PageDown、composer 超一屏、composer 光标模式）：
    ///   返回 false，交还既有 composer 翻页，不破坏长文本编辑。
    pub(crate) fn handle_document_page_key(&mut self, direction: isize) -> bool {
        let pages_document = if self.document_runtime.manual_scroll {
            true
        } else if direction < 0 && self.document_pinned_to_bottom() {
            let layout =
                self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
            self.composer_fits_single_viewport(&layout)
        } else {
            false
        };
        if !pages_document {
            return false;
        }

        // 翻页是瞬时 snap：先清空平滑滚动残余，避免 drain 与整页跳变叠加。
        self.document_runtime.smooth_scroll.clear_pending();
        let page_lines = self.document_page_scroll_lines() as isize;
        // scroll_document_by 自带手动滚动进入、restore-target 吸附与
        // 向下触底恢复贴底；PageDown 翻到底自动回到贴底是期望行为。
        self.scroll_document_by(direction.signum() * page_lines);
        true
    }

    /// 文档翻页步长：视口高度减 1（zellij 约定，保留一行重叠上下文），
    /// 高度为 0/1 时钳制为 1，防止零步长或 overflow。
    fn document_page_scroll_lines(&self) -> usize {
        self.document_viewport_height().saturating_sub(1).max(1)
    }

    /// composer 内容是否单屏可容纳（不需要 composer 自身翻页）。
    /// 与 `sync_document_viewport_for_composer_page` 共用同一判据，
    /// 防止两处判据各自漂移。
    pub(crate) fn composer_fits_single_viewport(&self, layout: &DocumentLayout) -> bool {
        layout.composer_line_count <= self.composer.viewport_height().max(1)
    }

    pub(crate) fn sync_document_viewport_to_bottom(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        let (document_offset, composer_offset) = self.bottom_follow_viewport_offsets(&layout);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            true,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_for_composer_cursor(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        let mut current_offset = self
            .clamp_document_viewport_offset(self.document_runtime.viewport_y, layout.line_count());
        let viewport_height = self.document_viewport_height();
        if viewport_height == 0 {
            self.apply_document_viewport_position(&layout, 0, 0, false, false);
            return;
        }

        match layout.cursor_y.cmp(&current_offset) {
            Ordering::Less => current_offset = layout.cursor_y,
            Ordering::Greater if layout.cursor_y >= current_offset + viewport_height => {
                current_offset = layout.cursor_y - viewport_height + 1;
            }
            _ => {}
        }

        let document_offset =
            self.clamp_document_viewport_offset(current_offset, layout.line_count());
        let composer_offset = self.current_composer_viewport_offset(&layout, document_offset);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            false,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_preserving_position(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if layout.line_count() == 0 {
            self.apply_document_viewport_position(
                &layout,
                0,
                0,
                false,
                self.document_runtime.manual_scroll,
            );
            return;
        }

        let document_offset = self
            .clamp_document_viewport_offset(self.document_runtime.viewport_y, layout.line_count());
        let composer_offset = self.current_composer_viewport_offset(&layout, document_offset);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            self.document_runtime.follow_bottom,
            self.document_runtime.manual_scroll,
        );
    }

    pub(crate) fn sync_document_viewport_for_viewport_state(&mut self, state: &ViewportState) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        if layout.line_count() == 0 {
            self.apply_document_viewport_position(
                &layout,
                0,
                0,
                state.follow_bottom(),
                state.manual_scroll(),
            );
            return;
        }

        if state.follow_bottom() && !state.manual_scroll() {
            let (document_offset, composer_offset) = self.bottom_follow_viewport_offsets(&layout);
            self.apply_document_viewport_position(
                &layout,
                document_offset,
                composer_offset,
                true,
                false,
            );
            return;
        }

        let document_offset = state.resolve_offset(&layout, self.document_viewport_height());
        let composer_offset = self.current_composer_viewport_offset(&layout, document_offset);
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            composer_offset,
            state.follow_bottom(),
            state.manual_scroll(),
        );
    }

    pub(crate) fn sync_document_viewport_for_composer_page(&mut self) {
        let layout = self.build_document_layout(crate::frame_time::FrameRenderContext::capture());
        let max_offset = layout
            .composer_line_count
            .saturating_sub(self.composer.viewport_height().max(1));
        if self.composer.viewport_offset() > max_offset {
            self.composer.set_viewport_offset(max_offset);
        }

        if self.composer_fits_single_viewport(&layout) {
            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        let document_offset = self.clamp_document_viewport_offset(
            layout.composer_start_line + self.composer.viewport_offset(),
            layout.line_count(),
        );
        self.apply_document_viewport_position(
            &layout,
            document_offset,
            self.composer.viewport_offset(),
            false,
            false,
        );
        self.clear_manual_document_scroll_restore_target();
    }

    pub(crate) fn sync_document_viewport_after_composer_interaction(
        &mut self,
        old_value: &str,
        old_line: usize,
        old_column: usize,
    ) {
        if self.composer.value() != old_value {
            if self.selection_runtime.selection.is_active() {
                self.invalidate_selection_for_reflow();
            }
            if self.document_runtime.manual_scroll {
                self.restore_from_manual_document_scroll();
                return;
            }

            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.composer.line() != old_line || self.composer.column() != old_column {
            self.document_runtime.follow_bottom = self.composer_at_bottom_follow_anchor();
            if self.document_runtime.follow_bottom {
                self.sync_document_viewport_to_bottom();
                return;
            }

            self.sync_document_viewport_for_composer_cursor();
            return;
        }

        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if self.document_runtime.manual_scroll {
            self.sync_document_viewport_preserving_position();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn sync_document_viewport_after_transcript_refresh(
        &mut self,
        preserved_viewport_state: Option<ViewportState>,
    ) {
        if self.document_runtime.follow_bottom {
            self.sync_document_viewport_to_bottom();
            return;
        }

        if let Some(state) = preserved_viewport_state.as_ref() {
            self.sync_document_viewport_for_viewport_state(state);
            if self.document_runtime.manual_scroll {
                self.complete_manual_document_scroll_if_restored();
            }
            return;
        }

        if self.document_runtime.manual_scroll {
            self.sync_document_viewport_preserving_position();
            self.complete_manual_document_scroll_if_restored();
            return;
        }

        self.sync_document_viewport_for_composer_cursor();
    }

    pub(crate) fn composer_at_bottom_follow_anchor(&self) -> bool {
        if self.composer.value().is_empty() {
            return true;
        }

        let lines = self.composer.value().split('\n').collect::<Vec<_>>();
        let Some(last_line) = lines.last() else {
            return true;
        };

        self.composer.line() == lines.len().saturating_sub(1)
            && self.composer.column() == last_line.chars().count()
    }

    pub(crate) fn bottom_follow_viewport_offsets(&self, layout: &DocumentLayout) -> (usize, usize) {
        if self.composer.value().is_empty() {
            let viewport_height = self.document_viewport_height();
            if viewport_height == 0 {
                return (0, 0);
            }

            let document_offset = self.clamp_document_viewport_offset(
                layout.cursor_y.saturating_sub(viewport_height - 1),
                layout.line_count(),
            );
            return (document_offset, 0);
        }

        (
            self.document_bottom_offset(layout.line_count()),
            self.composer.bottom_viewport_offset(),
        )
    }

    pub(crate) fn capture_viewport_state_with_layout(
        &self,
        layout: &DocumentLayout,
        document_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
    ) -> ViewportState {
        let resolved_offset =
            self.clamp_document_viewport_offset(document_offset, layout.line_count());
        let line_indices = self.document_viewport_line_indices_for_mode(
            layout,
            resolved_offset,
            follow_bottom,
            manual_scroll,
        );
        ViewportState::capture(
            layout,
            &line_indices,
            resolved_offset,
            follow_bottom,
            manual_scroll,
            self.document_viewport_height(),
            self.width,
        )
    }

    pub(crate) fn apply_document_viewport_position(
        &mut self,
        layout: &DocumentLayout,
        document_offset: usize,
        composer_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
    ) {
        let was_pinned_to_bottom = self.document_pinned_to_bottom();
        let document_offset =
            self.clamp_document_viewport_offset(document_offset, layout.line_count());
        self.document_runtime.viewport_y = document_offset;
        self.composer.set_viewport_offset(composer_offset);
        self.document_runtime.follow_bottom = follow_bottom;
        self.document_runtime.manual_scroll = manual_scroll;
        self.document_runtime.viewport_state = self.capture_viewport_state_with_layout(
            layout,
            document_offset,
            follow_bottom,
            manual_scroll,
        );
        // 贴底状态变化的汇聚点：回到贴底且无遮挡时，新消息与审批面板均已可见。
        self.clear_new_message_pill_if_pinned();
        self.sync_tool_approval_attention_visibility();
        if !was_pinned_to_bottom && self.document_pinned_to_bottom() {
            // 贴底恢复：让非贴底期间被抑制的审批预览 fullscreen 升级延迟生效。
            // sync_tool_approval_preview_mode 只改面板自身状态，不回调视口方法，无重入；
            // 升级改变后续 layout，调用方在 apply 之后不得再消费本次传入的 layout。
            self.sync_tool_approval_preview_mode();
        }
    }

    pub(crate) fn document_viewport_line_indices_for_mode(
        &self,
        layout: &DocumentLayout,
        document_offset: usize,
        follow_bottom: bool,
        manual_scroll: bool,
    ) -> Vec<usize> {
        if follow_bottom && !manual_scroll {
            return bottom_follow_viewport_line_indices(
                layout,
                self.document_viewport_height(),
                self.bottom_follow_presentation(layout),
            );
        }

        offset_viewport_line_indices(layout, document_offset, self.document_viewport_height())
    }
}
