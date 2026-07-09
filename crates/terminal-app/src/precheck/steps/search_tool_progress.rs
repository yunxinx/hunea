//! 搜索工具下载进度的文本格式化（进度条 / 字节 / 速度 / ETA）。
//!
//! 不用 ratatui Gauge：precheck 全是 Paragraph lines，字符 bar 与现有布局一致。

use std::time::Instant;

use tool_runtime::builtin::ManagedToolProgress;

pub(super) fn stage_label(progress: &ManagedToolProgress) -> &'static str {
    match progress {
        ManagedToolProgress::Downloading { .. } => "Downloading",
        ManagedToolProgress::Verifying => "Verifying checksum",
        ManagedToolProgress::Extracting => "Extracting archive",
        ManagedToolProgress::Installing => "Installing",
        ManagedToolProgress::Ready { .. } => "Ready",
        ManagedToolProgress::Failed { .. } => "Failed",
    }
}

/// 字符进度条：`[████░░░░] 50%`；无 total 时用 indeterminate 滚动块。
pub(super) fn format_progress_bar(
    bytes_received: u64,
    bytes_total: Option<u64>,
    width: usize,
) -> String {
    let width = width.max(1);
    match bytes_total {
        Some(total) if total > 0 => {
            let filled = ((bytes_received as u128 * width as u128) / total as u128) as usize;
            let filled = filled.min(width);
            let empty = width - filled;
            let pct = ((bytes_received as u128 * 100) / total as u128).min(100) as u64;
            format!("[{}{}] {pct}%", "█".repeat(filled), "░".repeat(empty))
        }
        _ => {
            // Starting（0 字节）走 format_empty_progress_bar，不要进这里。
            let block = 3.min(width);
            let travel = width.saturating_sub(block).max(1);
            let pos = ((bytes_received / 64) as usize) % (travel + 1);
            let mut cells = vec!['░'; width];
            for cell in cells.iter_mut().skip(pos).take(block) {
                *cell = '█';
            }
            format!("[{}]", cells.into_iter().collect::<String>())
        }
    }
}

/// Starting：全空条 + 0%，避免 indeterminate 起始块看起来像已有进度。
pub(super) fn format_empty_progress_bar(width: usize) -> String {
    let width = width.max(1);
    format!("[{}] 0%", "░".repeat(width))
}

/// Verifying / Extracting / Installing：满条 100%。
pub(super) fn format_full_progress_bar(width: usize) -> String {
    let width = width.max(1);
    format!("[{}] 100%", "█".repeat(width))
}

/// 字节 + 平均速度 + ETA。平均速率实现简单、抖动小；不引入滑动窗口。
pub(super) fn format_transfer_stats(
    bytes_received: u64,
    bytes_total: Option<u64>,
    started_at: Option<Instant>,
) -> String {
    let bytes = format_byte_progress(bytes_received, bytes_total);
    let Some(started_at) = started_at else {
        return bytes;
    };
    // 不足 100ms 或尚无字节时不报速度（样本太噪）。
    let elapsed = started_at.elapsed().as_secs_f64();
    if elapsed < 0.1 || bytes_received == 0 {
        return bytes;
    }
    let bps = bytes_received as f64 / elapsed;
    let speed = format_bytes_rate(bps);
    match bytes_total {
        Some(total) if total > bytes_received && bps > 0.0 => {
            let eta_secs = (total - bytes_received) as f64 / bps;
            format!("{bytes}  ·  {speed}  ·  ETA {}", format_eta(eta_secs))
        }
        _ => format!("{bytes}  ·  {speed}"),
    }
}

fn format_byte_progress(bytes_received: u64, bytes_total: Option<u64>) -> String {
    let received = format_bytes(bytes_received);
    match bytes_total {
        Some(total) => format!("{received} / {}", format_bytes(total)),
        None => format!("{received} / unknown"),
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn format_bytes_rate(bytes_per_sec: f64) -> String {
    let rate = if bytes_per_sec.is_finite() && bytes_per_sec > 0.0 {
        bytes_per_sec
    } else {
        0.0
    };
    format!("{}/s", format_bytes(rate as u64))
}

fn format_eta(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "—".to_string();
    }
    let total = secs.ceil() as u64;
    if total < 60 {
        format!("{total}s")
    } else if total < 3600 {
        let m = total / 60;
        let s = total % 60;
        if s == 0 {
            format!("{m}m")
        } else {
            format!("{m}m {s}s")
        }
    } else {
        let h = total / 3600;
        let m = (total % 3600) / 60;
        if m == 0 {
            format!("{h}h")
        } else {
            format!("{h}h {m}m")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_formats_correctly() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn format_progress_bar_determinate_fills_and_reports_percent() {
        assert_eq!(format_progress_bar(0, Some(100), 10), "[░░░░░░░░░░] 0%");
        assert_eq!(format_progress_bar(50, Some(100), 10), "[█████░░░░░] 50%");
        assert_eq!(format_progress_bar(100, Some(100), 10), "[██████████] 100%");
        assert_eq!(format_progress_bar(150, Some(100), 10), "[██████████] 100%");
    }

    #[test]
    fn format_empty_progress_bar_is_fully_empty_with_zero_percent() {
        assert_eq!(format_empty_progress_bar(10), "[░░░░░░░░░░] 0%");
        assert_ne!(
            format_empty_progress_bar(10),
            format_progress_bar(0, None, 10)
        );
    }

    #[test]
    fn format_full_progress_bar_is_filled() {
        assert_eq!(format_full_progress_bar(10), "[██████████] 100%");
    }

    #[test]
    fn format_progress_bar_indeterminate_has_brackets_without_percent() {
        let bar = format_progress_bar(0, None, 10);
        assert!(bar.starts_with('['), "{bar}");
        assert!(bar.ends_with(']'), "{bar}");
        assert!(!bar.contains('%'), "{bar}");
    }

    #[test]
    fn format_byte_progress_handles_known_and_unknown_total() {
        assert_eq!(format_byte_progress(1024, Some(2048)), "1.0 KB / 2.0 KB");
        assert_eq!(format_byte_progress(512, None), "512 B / unknown");
    }

    #[test]
    fn format_eta_formats_seconds_minutes_hours() {
        assert_eq!(format_eta(0.2), "1s");
        assert_eq!(format_eta(45.0), "45s");
        assert_eq!(format_eta(60.0), "1m");
        assert_eq!(format_eta(90.0), "1m 30s");
        assert_eq!(format_eta(3600.0), "1h");
        assert_eq!(format_eta(3660.0), "1h 1m");
    }

    #[test]
    fn format_transfer_stats_without_start_is_bytes_only() {
        assert_eq!(
            format_transfer_stats(1024, Some(2048), None),
            "1.0 KB / 2.0 KB"
        );
    }

    #[test]
    fn format_transfer_stats_with_zero_bytes_skips_speed() {
        let started = Instant::now();
        assert_eq!(
            format_transfer_stats(0, Some(2048), Some(started)),
            "0 B / 2.0 KB"
        );
        assert_eq!(format_bytes_rate(1024.0 * 1024.0), "1.0 MB/s");
        assert_eq!(format_bytes_rate(512.0), "512 B/s");
    }
}
