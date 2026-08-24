//! 可由多个进程外协议复用的严格 Content-Length JSON framing。

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;

/// 默认允许的最大 frame body 大小。
pub const DEFAULT_MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 32 * 1024;

/// 同步 Content-Length frame codec；不拥有 request ordering 或 process lifecycle。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameCodec {
    max_frame_bytes: usize,
}

impl Default for FrameCodec {
    fn default() -> Self {
        Self {
            max_frame_bytes: DEFAULT_MAX_FRAME_BYTES,
        }
    }
}

impl FrameCodec {
    /// 使用非零 max frame body 大小创建 codec。
    pub fn new(max_frame_bytes: usize) -> Result<Self, FrameError> {
        if max_frame_bytes == 0 {
            return Err(FrameError::InvalidMaxFrame);
        }
        Ok(Self { max_frame_bytes })
    }

    /// 返回允许的最大 frame body 大小。
    pub const fn max_frame_bytes(self) -> usize {
        self.max_frame_bytes
    }

    /// 读取一个完整 frame body，并保留 reader 中的后续 bytes。
    pub fn read_frame<R: std::io::Read>(&self, reader: &mut R) -> Result<Vec<u8>, FrameError> {
        let mut content_length = None;
        let mut header_bytes: usize = 0;
        loop {
            let line = read_header_line(reader)?;
            header_bytes = header_bytes
                .checked_add(line.len() + 2)
                .ok_or(FrameError::HeadersTooLarge)?;
            if header_bytes > MAX_HEADER_BYTES {
                return Err(FrameError::HeadersTooLarge);
            }
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').ok_or(FrameError::InvalidHeader)?;
            if name.eq_ignore_ascii_case("content-length") {
                if content_length.is_some() {
                    return Err(FrameError::DuplicateContentLength);
                }
                let value = value.trim_matches([' ', '\t']);
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(FrameError::InvalidContentLength);
                }
                let length = value
                    .parse::<usize>()
                    .map_err(|_| FrameError::InvalidContentLength)?;
                if length > self.max_frame_bytes {
                    return Err(FrameError::FrameTooLarge {
                        length,
                        max: self.max_frame_bytes,
                    });
                }
                content_length = Some(length);
            }
        }
        let length = content_length.ok_or(FrameError::MissingContentLength)?;
        let mut body = vec![0; length];
        read_exact_with_count(reader, &mut body)?;
        Ok(body)
    }

    /// 读取并解码一个 UTF-8 JSON frame。
    pub fn read_json<R: std::io::Read, T: DeserializeOwned>(
        &self,
        reader: &mut R,
    ) -> Result<T, FrameError> {
        let body = self.read_frame(reader)?;
        let text = std::str::from_utf8(&body).map_err(|_| FrameError::InvalidUtf8)?;
        serde_json::from_str(text).map_err(|_| FrameError::InvalidJson)
    }

    /// 编码 JSON 并写入一个完整 frame。
    pub fn write_json<W: std::io::Write, T: Serialize>(
        &self,
        writer: &mut W,
        value: &T,
    ) -> Result<(), FrameError> {
        let body = serde_json::to_vec(value).map_err(|_| FrameError::Encode)?;
        self.write_frame(writer, &body)
    }

    /// 写入 header/body，随后 flush 且不追加 newline。
    pub fn write_frame<W: std::io::Write>(
        &self,
        writer: &mut W,
        body: &[u8],
    ) -> Result<(), FrameError> {
        if body.len() > self.max_frame_bytes {
            return Err(FrameError::FrameTooLarge {
                length: body.len(),
                max: self.max_frame_bytes,
            });
        }
        write!(writer, "Content-Length: {}\r\n\r\n", body.len()).map_err(|_| FrameError::Io)?;
        writer.write_all(body).map_err(|_| FrameError::Io)?;
        writer.flush().map_err(|_| FrameError::Io)
    }
}

/// Content-Length framing 或 JSON codec 的安全错误投影。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("frame maximum must be greater than zero")]
    InvalidMaxFrame,
    #[error("frame header is truncated")]
    TruncatedHeader,
    #[error("frame header is invalid")]
    InvalidHeader,
    #[error("frame headers exceed the maximum size")]
    HeadersTooLarge,
    #[error("frame has duplicate Content-Length headers")]
    DuplicateContentLength,
    #[error("frame has no Content-Length header")]
    MissingContentLength,
    #[error("Content-Length is invalid")]
    InvalidContentLength,
    #[error("frame body is truncated")]
    TruncatedBody,
    #[error("frame length {length} exceeds maximum {max}")]
    FrameTooLarge { length: usize, max: usize },
    #[error("frame is not valid UTF-8")]
    InvalidUtf8,
    #[error("frame is not valid JSON")]
    InvalidJson,
    #[error("protocol value could not be encoded")]
    Encode,
    #[error("frame I/O failed")]
    Io,
}

fn read_header_line<R: std::io::Read>(reader: &mut R) -> Result<String, FrameError> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0; 1];
        let count = reader.read(&mut byte).map_err(|_| FrameError::Io)?;
        if count == 0 {
            return Err(FrameError::TruncatedHeader);
        }
        match byte[0] {
            b'\r' => {
                let mut line_end = [0; 1];
                let count = reader.read(&mut line_end).map_err(|_| FrameError::Io)?;
                if count == 0 {
                    return Err(FrameError::TruncatedHeader);
                }
                if line_end[0] != b'\n' {
                    return Err(FrameError::InvalidHeader);
                }
                return String::from_utf8(bytes).map_err(|_| FrameError::InvalidHeader);
            }
            b'\n' => return Err(FrameError::InvalidHeader),
            byte if byte == b'\t' || byte == b' ' || byte.is_ascii_graphic() => bytes.push(byte),
            _ => return Err(FrameError::InvalidHeader),
        }
        if bytes.len() > MAX_HEADER_LINE_BYTES {
            return Err(FrameError::InvalidHeader);
        }
    }
}

fn read_exact_with_count<R: std::io::Read>(
    reader: &mut R,
    body: &mut [u8],
) -> Result<(), FrameError> {
    let mut offset = 0;
    while offset < body.len() {
        let count = reader
            .read(&mut body[offset..])
            .map_err(|_| FrameError::Io)?;
        if count == 0 {
            return Err(FrameError::TruncatedBody);
        }
        offset += count;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{self, Cursor, Write};

    use super::*;

    #[test]
    fn reads_exact_frame_and_flushes_writes() {
        let codec = FrameCodec::new(128).expect("codec should construct");
        let mut output = FlushTrackingWriter::default();
        codec
            .write_json(&mut output, &serde_json::json!({"method": "tools.list"}))
            .expect("frame should write");
        assert!(output.flushed);
        assert_eq!(
            output.bytes,
            b"Content-Length: 23\r\n\r\n{\"method\":\"tools.list\"}".to_vec()
        );

        let mut reader = Cursor::new(output.bytes);
        let decoded: serde_json::Value = codec.read_json(&mut reader).expect("frame should read");
        assert_eq!(decoded["method"], "tools.list");

        let mut two_frames =
            Cursor::new(b"Content-Length: 2\r\n\r\n{}Content-Length: 2\r\n\r\n[]".to_vec());
        assert_eq!(
            codec.read_frame(&mut two_frames).expect("first frame"),
            b"{}"
        );
        assert_eq!(
            codec.read_frame(&mut two_frames).expect("second frame"),
            b"[]"
        );
    }

    #[test]
    fn rejects_malformed_and_oversized_frames() {
        let codec = FrameCodec::new(4).expect("codec should construct");
        let cases: Vec<(Vec<u8>, FrameError)> = vec![
            (b"\r\n{}".to_vec(), FrameError::MissingContentLength),
            (
                b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".to_vec(),
                FrameError::DuplicateContentLength,
            ),
            (
                b"Content-Length: 3\r\n\r\n{}".to_vec(),
                FrameError::TruncatedBody,
            ),
            (
                b"Content-Length: 2\n\n{}".to_vec(),
                FrameError::InvalidHeader,
            ),
            (
                b"Content-Length: 2\rX\n{}".to_vec(),
                FrameError::InvalidHeader,
            ),
            (
                b"Content-Length: +2\r\n\r\n{}".to_vec(),
                FrameError::InvalidContentLength,
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(codec.read_frame(&mut Cursor::new(bytes)), Err(expected));
        }
        assert!(matches!(
            codec.read_frame(&mut Cursor::new(b"Content-Length: 5\r\n\r\n12345")),
            Err(FrameError::FrameTooLarge { .. })
        ));

        let mut ascii_ows = Cursor::new(b"Content-Length:\t2 \t\r\n\r\n{}".to_vec());
        assert_eq!(codec.read_frame(&mut ascii_ows).expect("valid OWS"), b"{}");
        let mut non_ascii = Cursor::new("Content-Length: \u{a0}2\r\n\r\n{}".as_bytes().to_vec());
        assert_eq!(
            codec.read_frame(&mut non_ascii),
            Err(FrameError::InvalidHeader)
        );

        let oversized_line = format!("X-Header: {}\r\n\r\n", "x".repeat(MAX_HEADER_LINE_BYTES));
        assert_eq!(
            FrameCodec::default().read_frame(&mut Cursor::new(oversized_line)),
            Err(FrameError::InvalidHeader)
        );
        let mut headers = String::new();
        for _ in 0..5 {
            headers.push_str("X-Header: ");
            headers.push_str(&"x".repeat(8_000));
            headers.push_str("\r\n");
        }
        headers.push_str("\r\n");
        assert_eq!(
            FrameCodec::default().read_frame(&mut Cursor::new(headers)),
            Err(FrameError::HeadersTooLarge)
        );
    }

    #[test]
    fn rejects_invalid_json_encoding_and_io_without_retaining_sources() {
        assert_eq!(
            FrameCodec::default().write_json(&mut Vec::new(), &Unencodable),
            Err(FrameError::Encode)
        );
        assert_eq!(
            FrameCodec::new(32)
                .expect("codec")
                .read_json::<_, serde_json::Value>(&mut Cursor::new(
                    b"Content-Length: 1\r\n\r\n\xff"
                )),
            Err(FrameError::InvalidUtf8)
        );
        assert_eq!(
            FrameCodec::new(32)
                .expect("codec")
                .read_json::<_, serde_json::Value>(&mut Cursor::new(b"Content-Length: 1\r\n\r\n{")),
            Err(FrameError::InvalidJson)
        );

        let error = FrameCodec::default()
            .read_frame(&mut FailingReader)
            .expect_err("read should fail");
        assert!(std::error::Error::source(&error).is_none());
        assert!(!format!("{error:?}").contains("secret"));
    }

    #[derive(Default)]
    struct FlushTrackingWriter {
        bytes: Vec<u8>,
        flushed: bool,
    }

    impl Write for FlushTrackingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushed = true;
            Ok(())
        }
    }

    struct Unencodable;

    impl Serialize for Unencodable {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: serde::Serializer,
        {
            Err(serde::ser::Error::custom("sensitive serializer detail"))
        }
    }

    struct FailingReader;

    impl io::Read for FailingReader {
        fn read(&mut self, _bytes: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("/workspace/secret.txt"))
        }
    }
}
