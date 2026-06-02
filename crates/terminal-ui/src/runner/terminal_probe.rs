//! 启动期终端探测。
//!
//! 这里参考 `codex-rs` 的 startup probe 思路：终端查询只在 TUI 启动期发起，
//! 并在 crossterm 正常输入循环接管前用短超时直接读取响应。probe 期间读到的
//! 字节会被消费；不完整响应不会在 `Event` 层尝试补救。

use std::time::Duration;

use crate::theme::TerminalBackgroundColor;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TerminalBackgroundProbeResult {
    pub(super) background: Option<TerminalBackgroundColor>,
}

impl TerminalBackgroundProbeResult {
    pub(super) const fn unavailable() -> Self {
        Self { background: None }
    }

    const fn detected(background: TerminalBackgroundColor) -> Self {
        Self {
            background: Some(background),
        }
    }

    #[cfg(test)]
    pub(super) const fn timed_out() -> Self {
        Self::unavailable()
    }
}

#[cfg(unix)]
mod imp {
    use std::{
        fs::{File, OpenOptions},
        io::{self, Write},
        os::fd::{AsRawFd, FromRawFd},
        time::{Duration, Instant},
    };

    use super::TerminalBackgroundProbeResult;

    struct TerminalProbeTty {
        reader: File,
        writer: File,
        original_flags: libc::c_int,
    }

    impl TerminalProbeTty {
        fn open() -> io::Result<Self> {
            let stdio_reader = duplicate_fd(libc::STDIN_FILENO);
            let stdio_writer = duplicate_fd(libc::STDOUT_FILENO);
            match (stdio_reader, stdio_writer) {
                (Ok(reader), Ok(writer)) => Self::new(reader, writer),
                (reader, writer) => {
                    let stdio_error = match (reader.err(), writer.err()) {
                        (Some(reader_error), Some(writer_error)) => {
                            format!("reader: {reader_error}; writer: {writer_error}")
                        }
                        (Some(reader_error), None) => format!("reader: {reader_error}"),
                        (None, Some(writer_error)) => format!("writer: {writer_error}"),
                        (None, None) => "unknown stdio duplicate error".to_string(),
                    };
                    let reader =
                        OpenOptions::new()
                            .read(true)
                            .open("/dev/tty")
                            .map_err(|fallback_error| {
                                io::Error::new(
                                    fallback_error.kind(),
                                    format!(
                                        "failed to duplicate stdio ({stdio_error}) or open /dev/tty reader ({fallback_error})"
                                    ),
                                )
                            })?;
                    let writer = OpenOptions::new().write(true).open("/dev/tty").map_err(
                        |fallback_error| {
                            io::Error::new(
                                fallback_error.kind(),
                                format!(
                                    "failed to duplicate stdio ({stdio_error}) or open /dev/tty writer ({fallback_error})"
                                ),
                            )
                        },
                    )?;
                    Self::new(reader, writer)
                }
            }
        }

        fn new(reader: File, writer: File) -> io::Result<Self> {
            let fd = reader.as_raw_fd();
            // SAFETY: `fd` 来自存活的 `File`，这里只读取该文件描述符的状态标志。
            let original_flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if original_flags == -1 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: `fd` 仍由 `reader` 持有；这里只为启动期 probe 临时增加 O_NONBLOCK。
            if unsafe { libc::fcntl(fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK) } == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                reader,
                writer,
                original_flags,
            })
        }

        fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.writer.write_all(bytes)?;
            self.writer.flush()
        }

        fn read_available(&mut self, buffer: &mut Vec<u8>) -> io::Result<()> {
            let mut chunk = [0_u8; 256];
            loop {
                // SAFETY: `chunk` 是有效可写缓冲区，`reader` 在整个调用期间持有有效 fd。
                let count = unsafe {
                    libc::read(
                        self.reader.as_raw_fd(),
                        chunk.as_mut_ptr().cast::<libc::c_void>(),
                        chunk.len(),
                    )
                };
                if count > 0 {
                    buffer.extend_from_slice(&chunk[..count as usize]);
                    continue;
                }
                if count == 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) {
                    return Ok(());
                }
                return Err(error);
            }
        }

        fn poll_readable(&self, timeout: Duration) -> io::Result<bool> {
            let mut fd = libc::pollfd {
                fd: self.reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let deadline = Instant::now() + timeout;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    return Ok(false);
                }
                let timeout_ms = deadline
                    .saturating_duration_since(now)
                    .as_millis()
                    .min(libc::c_int::MAX as u128) as libc::c_int;
                // SAFETY: `fd` 指向栈上单个 `pollfd`，长度参数为 1，调用期间指针有效。
                let result = unsafe { libc::poll(&mut fd, 1, timeout_ms) };
                if result > 0 {
                    return Ok((fd.revents & libc::POLLIN) != 0);
                }
                if result == 0 {
                    return Ok(false);
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
    }

    impl Drop for TerminalProbeTty {
        fn drop(&mut self) {
            // SAFETY: `reader` 仍持有该 fd；Drop 阶段只恢复 probe 前保存的状态标志。
            let _ =
                unsafe { libc::fcntl(self.reader.as_raw_fd(), libc::F_SETFL, self.original_flags) };
        }
    }

    fn duplicate_fd(fd: libc::c_int) -> io::Result<File> {
        // SAFETY: `dup` 复制进程 stdio fd；成功返回的新 fd 交给 `File` 独占管理。
        let duplicated = unsafe { libc::dup(fd) };
        if duplicated == -1 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `duplicated` 是 `dup` 新建且尚未被托管的 fd。
        Ok(unsafe { File::from_raw_fd(duplicated) })
    }

    pub(super) fn query_background(timeout: Duration) -> io::Result<TerminalBackgroundProbeResult> {
        let mut tty = TerminalProbeTty::open()?;
        tty.write_all(b"\x1B]11;?\x1B\\")?;
        match read_until(&mut tty, timeout, super::parse_background_probe_completion) {
            Ok(Some(result)) => Ok(result),
            Ok(None) | Err(_) => Ok(TerminalBackgroundProbeResult::unavailable()),
        }
    }

    fn read_until<T>(
        tty: &mut TerminalProbeTty,
        timeout: Duration,
        mut parse: impl FnMut(&[u8]) -> Option<T>,
    ) -> io::Result<Option<T>> {
        let deadline = Instant::now() + timeout;
        let mut buffer = Vec::new();
        loop {
            tty.read_available(&mut buffer)?;
            if let Some(value) = parse(&buffer) {
                return Ok(Some(value));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            if !tty.poll_readable(deadline.saturating_duration_since(now))? {
                return Ok(None);
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use std::{io, time::Duration};

    use super::TerminalBackgroundProbeResult;

    pub(super) fn query_background(
        _timeout: Duration,
    ) -> io::Result<TerminalBackgroundProbeResult> {
        Ok(TerminalBackgroundProbeResult::unavailable())
    }
}

pub(super) fn query_background(timeout: Duration) -> TerminalBackgroundProbeResult {
    imp::query_background(timeout)
        .ok()
        .unwrap_or_else(TerminalBackgroundProbeResult::unavailable)
}

fn parse_background_response(buffer: &[u8]) -> Option<TerminalBackgroundColor> {
    parse_osc_color(buffer, 11)
}

fn parse_background_probe_completion(buffer: &[u8]) -> Option<TerminalBackgroundProbeResult> {
    parse_background_response(buffer).map(TerminalBackgroundProbeResult::detected)
}

fn parse_osc_color(buffer: &[u8], slot: u8) -> Option<TerminalBackgroundColor> {
    let prefix = format!("\x1B]{slot};");
    let start = find_subslice(buffer, prefix.as_bytes())?;
    let payload_start = start + prefix.len();
    let rest = &buffer[payload_start..];
    let (payload_end, _terminator_len) = osc_payload_end(rest)?;
    let payload = std::str::from_utf8(&rest[..payload_end]).ok()?;
    parse_osc_rgb(payload)
}

fn osc_payload_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let mut idx = 0;
    while idx < buffer.len() {
        match buffer[idx] {
            0x07 => return Some((idx, 1)),
            0x1B if buffer.get(idx + 1) == Some(&b'\\') => return Some((idx, 2)),
            _ => idx += 1,
        }
    }
    None
}

fn parse_osc_rgb(payload: &str) -> Option<TerminalBackgroundColor> {
    let (prefix, values) = payload.trim().split_once(':')?;
    if !prefix.eq_ignore_ascii_case("rgb") && !prefix.eq_ignore_ascii_case("rgba") {
        return None;
    }

    let mut parts = values.split('/');
    let red = parse_osc_component(parts.next()?)?;
    let green = parse_osc_component(parts.next()?)?;
    let blue = parse_osc_component(parts.next()?)?;
    if prefix.eq_ignore_ascii_case("rgba") {
        parse_osc_component(parts.next()?)?;
    }
    parts
        .next()
        .is_none()
        .then_some(TerminalBackgroundColor::from_rgb(red, green, blue))
}

fn parse_osc_component(component: &str) -> Option<u8> {
    match component.len() {
        2 => u8::from_str_radix(component, 16).ok(),
        4 => u16::from_str_radix(component, 16)
            .ok()
            .map(|value| (value / 257) as u8),
        _ => None,
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_background_response_with_bel_and_st() {
        assert_eq!(
            parse_background_response(b"\x1B]11;rgb:2828/2c2c/3434\x07"),
            Some(TerminalBackgroundColor::from_rgb(40, 44, 52))
        );
        assert_eq!(
            parse_background_response(b"\x1B]11;rgb:28/2c/34\x1B\\"),
            Some(TerminalBackgroundColor::from_rgb(40, 44, 52))
        );
    }

    #[test]
    fn completes_detected_background_without_status_report_fence() {
        assert_eq!(
            parse_background_probe_completion(b"\x1B]11;rgb:2828/2c2c/3434\x1B\\"),
            Some(TerminalBackgroundProbeResult::detected(
                TerminalBackgroundColor::from_rgb(40, 44, 52)
            ))
        );
    }

    #[test]
    fn ignores_status_report_that_arrives_before_background_response() {
        assert_eq!(parse_background_probe_completion(b"\x1B[0n"), None);
        assert_eq!(
            parse_background_probe_completion(b"\x1B[0n\x1B]11;rgb:2828/2c2c/3434\x1B\\"),
            Some(TerminalBackgroundProbeResult::detected(
                TerminalBackgroundColor::from_rgb(40, 44, 52)
            ))
        );
    }

    #[test]
    fn ignores_incomplete_status_report_fence() {
        assert_eq!(parse_background_probe_completion(b"\x1B[0"), None);
    }

    #[test]
    fn ignores_incomplete_background_response() {
        assert_eq!(
            parse_background_response(b"\x1B]11;rgb:2828/2c2c/3434"),
            None
        );
    }

    #[test]
    fn parses_rgba_background_response() {
        assert_eq!(
            parse_background_response(b"\x1B]11;rgba:ffff/8000/0000/ffff\x1B\\"),
            Some(TerminalBackgroundColor::from_rgb(255, 127, 0))
        );
    }
}
