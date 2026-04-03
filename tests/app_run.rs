use std::io;

use lumos::app::run_with_writer;

#[test]
fn run_with_writer_adds_context_when_banner_output_fails() {
    let error = run_with_writer(&mut BrokenWriter).expect_err("run should bubble writer failures");
    let message = format!("{error:?}");

    assert!(message.contains("failed to write startup banner"));
}

struct BrokenWriter;

impl io::Write for BrokenWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::other("synthetic writer failure"))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
