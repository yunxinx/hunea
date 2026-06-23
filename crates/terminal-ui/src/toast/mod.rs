//! 上层 toast notice 的状态机与渲染。
//!
//! Toast 用于需要醒目反馈的结果性事件，例如复制成功或失败；不会打断阅读节奏的导航与
//! 确认类短提示仍使用底部状态行 notice。

use std::time::{Duration, Instant};

use ratatui::{
    buffer::{Buffer, Cell},
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, BorderType, Clear, Paragraph, Widget},
};

use crate::{
    Model,
    display_width::display_width,
    render_frame::RenderFrame,
    status_line::truncate_display_width_with_ellipsis,
    theme::{TerminalPalette, primary_text_style},
};

pub(crate) const TOAST_FRAME_INTERVAL: Duration = Duration::from_millis(16);

const TOAST_HEIGHT: u16 = 3;
const TOAST_HORIZONTAL_FRAME_WIDTH: usize = 2;
const TOAST_MIN_WIDTH: u16 = 2;
const TOAST_ENTER_DURATION: Duration = Duration::from_millis(210);
const TOAST_EXIT_DURATION: Duration = Duration::from_millis(220);
const TOAST_ERASE_EDGE_WIDTH: u16 = 1;

mod animation;
mod model;
mod render;
mod state;

#[cfg(test)]
mod tests;

use render::{ToastUnderlaySnapshot, render_toast_notice, render_toast_transition, toast_rect};
use state::{ToastAnimation, ToastAnimationFrame, ToastAnimationKind, ToastNotice};
pub(crate) use state::{ToastSeverity, ToastState};
